//! Bounded decoder for one adsb.lol `/v2/*` snapshot. A position time is estimated as the
//! server's `now` minus the reported `seen_pos` age; callers must retain the exact response and
//! report that estimate and its source precision, not treat it as archive-time truth.

use std::fmt;

use oa_json::{Doc, Kind, NodeIx};

use crate::{
    is_reserved_address, reports_push, Filter, Reports, Row, NONE_I32, NONE_U16, NONE_U8, NON_ICAO,
    ROW_EMPTY, SIL_PER_HOUR, SIL_PER_SAMPLE, SIL_UNKNOWN, SOURCE_NAMES, SOURCE_UNKNOWN,
    TIME_MAX_NS, TIME_MIN_NS,
};

/// Maximum body size accepted by the snapshot decoder (1 MiB).
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
/// Maximum `ac` entries accepted in one response.
pub const MAX_SNAPSHOT_ROWS: usize = 10_000;
/// Maximum JSON nodes in one response; leaves headroom over the 10,000-row envelope without
/// inheriting the much larger historical-trace parser allowance.
pub const MAX_SNAPSHOT_NODES: usize = 600_000;
/// The decoder accepts no more than this age cap, even if the caller asks for more.
pub const MAX_POSITION_AGE_MS: u32 = 30_000;

/// Why a complete snapshot was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    // A7.2: cold error record; size guard waived.
    /// Raw JSON exceeds the input body limit.
    TooLarge(usize),
    /// JSON syntax or parser limits failed.
    Json(oa_json::JsonError),
    /// A required envelope field is absent, malformed, or inconsistent.
    Envelope(&'static str),
    /// The reported aircraft count exceeds the decoder limit.
    TooManyRows(u64),
    /// API source time/cache time is inconsistent with local receipt.
    ReceiptTime,
    /// Caller requested an unsupported source-position-age limit.
    AgeLimit,
    /// A duplicate key would make field interpretation ambiguous.
    DuplicateKey,
    /// JSON document exceeded the bounded structural walk.
    StructureLimit,
    /// One or more snapshot timestamps fall in a compiled sealed interval.
    SealedTime,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "snapshot is {n} bytes; limit is {MAX_SNAPSHOT_BYTES}"),
            Self::Json(e) => write!(f, "snapshot JSON: {e}"),
            Self::Envelope(s) => write!(f, "snapshot envelope: {s}"),
            Self::TooManyRows(n) => {
                write!(f, "snapshot has {n} aircraft; limit is {MAX_SNAPSHOT_ROWS}")
            }
            Self::ReceiptTime => {
                f.write_str("snapshot server and local receipt times are inconsistent")
            }
            Self::AgeLimit => f.write_str("position-age limit must be 1 to 30000 ms"),
            Self::DuplicateKey => f.write_str("snapshot contains a duplicate object key"),
            Self::StructureLimit => f.write_str("snapshot JSON structure exceeds decoder limits"),
            Self::SealedTime => f.write_str("snapshot envelope time intersects a sealed interval"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Auditable denominators and source identity counts for a single snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapshotStats {
    // A7.2: cold report record; size guard waived.
    /// Exact response byte length.
    pub bytes: u64,
    /// API envelope `now`, Unix milliseconds.
    pub server_now_ms: i64,
    /// API envelope `ctime`, Unix milliseconds.
    pub server_cache_ms: i64,
    /// Envelope `total`, validated against the `ac` length.
    pub aircraft_reported: u64,
    /// Aircraft entries visited in the `ac` array.
    pub aircraft_seen: u64,
    /// Position rows appended to `Reports`.
    pub accepted: u64,
    /// Entries without a current coordinate pair.
    pub no_position: u64,
    /// Entries lacking current coordinates that included a `lastPosition` object (not used).
    pub last_position_only: u64,
    /// Entries with absent or malformed addresses.
    pub invalid_address: u64,
    /// Entries with absent, malformed, or impossible `seen_pos` ages.
    pub invalid_age: u64,
    /// Entries over the caller's maximum position age.
    pub too_old: u64,
    /// Entries with out-of-range/malformed coordinates.
    pub invalid_position: u64,
    /// Valid rows outside the supplied geographic filter.
    pub outside_box: u64,
    /// Reserved transponder addresses.
    pub reserved_address: u64,
    /// Valid report times within compiled sealed windows.
    pub sealed_window_dropped: u64,
    /// Accepted positions whose source type is not one of the direct ADS-B types.
    pub non_adsb_position: u64,
    /// Accepted aircraft entries with nonempty `mlat` field lists.
    pub with_mlat_fields: u64,
    /// Accepted aircraft entries with nonempty `tisb` field lists.
    pub with_tisb_fields: u64,
    /// Position rows whose direct ADS-B type conflicts with MLAT/TIS-B-derived fields.
    pub source_conflict: u64,
    /// Position rows with missing or malformed `mlat`/`tisb` source identity arrays.
    pub invalid_source_identity: u64,
    /// Optional fields with malformed or out-of-range values.
    pub bad_optional_fields: u64,
    /// Counts for each entry in [`SOURCE_NAMES`].
    pub source_types: [u64; 14],
    /// Smallest accepted `seen_pos` age rounded to milliseconds (0 when none were accepted).
    pub min_seen_pos_ms: u32,
    /// Largest accepted `seen_pos` age rounded to milliseconds.
    pub max_seen_pos_ms: u32,
}

fn kind(doc: &Doc<'_>, n: Option<NodeIx>) -> Option<Kind> {
    doc.kind(n?)
}

fn field<'d>(doc: &Doc<'d>, obj: NodeIx, name: &str) -> Option<NodeIx> {
    doc.get(obj, name)
}

fn number(doc: &Doc<'_>, obj: NodeIx, name: &str) -> Option<f64> {
    doc.as_f64(field(doc, obj, name)?)
}

fn int(doc: &Doc<'_>, obj: NodeIx, name: &str) -> Option<i64> {
    doc.as_i64(field(doc, obj, name)?)
}

fn str_field<'a>(doc: &'a Doc<'_>, obj: NodeIx, name: &str) -> Option<&'a str> {
    doc.str(field(doc, obj, name)?)
}

fn optional_number(doc: &Doc<'_>, obj: NodeIx, name: &str) -> Option<f64> {
    let n = field(doc, obj, name)?;
    match doc.kind(n) {
        Some(Kind::Null) => None,
        Some(Kind::Number) => doc.as_f64(n),
        _ => None,
    }
}

fn parse_address(s: &str) -> Option<u32> {
    let (non, digits) = match s.strip_prefix('~') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(digits, 16).ok()?;
    Some(if non { n | NON_ICAO } else { n })
}

fn unique_object(doc: &Doc<'_>, n: NodeIx) -> Result<(), SnapshotError> {
    if doc.len(n) > crate::MAX_OBJECT_MEMBERS {
        return Err(SnapshotError::StructureLimit);
    }
    let mut names: Vec<&str> = Vec::with_capacity(doc.len(n));
    for (k, _) in doc.members(n) {
        let Some(key) = doc.str(k) else {
            return Err(SnapshotError::StructureLimit);
        };
        if names.contains(&key) {
            return Err(SnapshotError::DuplicateKey);
        }
        names.push(key);
    }
    Ok(())
}

fn validate_tree(
    doc: &Doc<'_>,
    n: NodeIx,
    depth: u8,
    budget: &mut usize,
) -> Result<(), SnapshotError> {
    if depth > 64 {
        return Err(SnapshotError::StructureLimit);
    }
    *budget = budget.checked_sub(1).ok_or(SnapshotError::StructureLimit)?;
    match doc.kind(n) {
        Some(Kind::Object) => {
            unique_object(doc, n)?;
            for (_, v) in doc.members(n) {
                *budget = budget.checked_sub(1).ok_or(SnapshotError::StructureLimit)?;
                validate_tree(doc, v, depth + 1, budget)?;
            }
            Ok(())
        }
        Some(Kind::Array) => {
            for v in doc.elements(n) {
                validate_tree(doc, v, depth + 1, budget)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Decodes one V2 snapshot using the body's source timestamp and position-age values.
/// `receipt_unix_ms` is sampled by the caller at response receipt. The clip frame separately
/// carries this actual local receipt in OAF2's receive and monotonic fields.
///
/// The output report timestamp is the estimate `now - seen_pos`; the raw body must be retained
/// by the shell so its exact age text and source headers remain available. The filter's
/// geographic bounds are applied; its time bounds are deliberately not applied, so snapshot
/// truth around a planned cohort's boundaries is not clipped. Compiled sealed windows always
/// apply.
pub fn decode_snapshot(
    json: &[u8],
    receipt_unix_ms: i64,
    max_age_ms: u32,
    filter: &Filter,
    out: &mut Reports,
) -> Result<SnapshotStats, SnapshotError> {
    if json.len() > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::TooLarge(json.len()));
    }
    if max_age_ms == 0 || max_age_ms > MAX_POSITION_AGE_MS {
        return Err(SnapshotError::AgeLimit);
    }
    let limits = oa_json::Limits {
        max_input_bytes: MAX_SNAPSHOT_BYTES,
        max_nodes: MAX_SNAPSHOT_NODES,
        ..oa_json::DEFAULT_LIMITS
    };
    let doc = oa_json::parse(json, &limits).map_err(SnapshotError::Json)?;
    let root = NodeIx::ROOT;
    if doc.kind(root) != Some(Kind::Object) {
        return Err(SnapshotError::Envelope("top level is not an object"));
    }
    unique_object(&doc, root)?;
    let ac = field(&doc, root, "ac")
        .filter(|n| doc.kind(*n) == Some(Kind::Array))
        .ok_or(SnapshotError::Envelope("missing ac array"))?;
    let now = int(&doc, root, "now")
        .filter(|x| *x >= 946_684_800_000 && *x < 4_102_444_800_000)
        .ok_or(SnapshotError::Envelope(
            "now must be Unix milliseconds within 2000–2100",
        ))?;
    let ctime = int(&doc, root, "ctime")
        .filter(|x| *x >= 946_684_800_000 && *x < 4_102_444_800_000)
        .ok_or(SnapshotError::Envelope(
            "ctime must be Unix milliseconds within 2000–2100",
        ))?;
    if int(&doc, root, "ptime").is_none_or(|x| x < 0) {
        return Err(SnapshotError::Envelope(
            "ptime must be a nonnegative integer",
        ));
    }
    let total_raw = int(&doc, root, "total")
        .filter(|x| *x >= 0)
        .ok_or(SnapshotError::Envelope(
            "total must be a nonnegative integer",
        ))? as u64;
    if doc.get(root, "msg").and_then(|n| doc.str(n)) != Some("No error") {
        return Err(SnapshotError::Envelope("msg is not No error"));
    }
    if total_raw > MAX_SNAPSHOT_ROWS as u64 {
        return Err(SnapshotError::TooManyRows(total_raw));
    }
    let total = usize::try_from(total_raw).map_err(|_| SnapshotError::TooManyRows(total_raw))?;
    if total != doc.len(ac) {
        return Err(SnapshotError::Envelope("total does not match ac length"));
    }
    // The API documents both as Unix ms. Allow limited network/cache delay, never silently clamp.
    if now > receipt_unix_ms.saturating_add(10_000)
        || receipt_unix_ms.saturating_sub(now) > 60_000
        || ctime > now.saturating_add(10_000)
        || now.saturating_sub(ctime) > 60_000
    {
        return Err(SnapshotError::ReceiptTime);
    }
    for ms in [receipt_unix_ms, now, ctime] {
        let ns = ms
            .checked_mul(1_000_000)
            .ok_or(SnapshotError::ReceiptTime)?;
        if oa_sealed::in_sealed_window(ns) {
            return Err(SnapshotError::SealedTime);
        }
    }
    let mut structure_budget = MAX_SNAPSHOT_NODES;
    validate_tree(&doc, root, 0, &mut structure_budget)?;
    let mut st = SnapshotStats {
        bytes: json.len() as u64,
        server_now_ms: now,
        server_cache_ms: ctime,
        aircraft_reported: total as u64,
        min_seen_pos_ms: u32::MAX,
        ..SnapshotStats::default()
    };
    let mut appended = Vec::new();
    for acn in doc.elements(ac) {
        st.aircraft_seen += 1;
        if doc.kind(acn) != Some(Kind::Object) {
            return Err(SnapshotError::Envelope("ac entry is not an object"));
        }
        if kind(&doc, field(&doc, acn, "lat")).is_none_or(|k| k == Kind::Null)
            || kind(&doc, field(&doc, acn, "lon")).is_none_or(|k| k == Kind::Null)
        {
            if field(&doc, acn, "lastPosition").is_some_and(|n| doc.kind(n) == Some(Kind::Object)) {
                st.last_position_only += 1;
            } else {
                st.no_position += 1;
            }
            continue;
        }
        let Some(hex) = str_field(&doc, acn, "hex").and_then(parse_address) else {
            st.invalid_address += 1;
            continue;
        };
        let Some(age) = number(&doc, acn, "seen_pos")
            .filter(|v| v.is_finite() && *v >= 0.0 && *v * 1000.0 <= f64::from(u32::MAX))
        else {
            st.invalid_age += 1;
            continue;
        };
        if age * 1000.0 > f64::from(max_age_ms) {
            st.too_old += 1;
            continue;
        }
        // Rust f64::round is nearest integer, ties away from zero; retain that published rule.
        let age_ms = (age * 1000.0).round() as u32;
        st.min_seen_pos_ms = st.min_seen_pos_ms.min(age_ms);
        st.max_seen_pos_ms = st.max_seen_pos_ms.max(age_ms);
        let Some(lat) = number(&doc, acn, "lat").filter(|v| (-90.0..=90.0).contains(v)) else {
            st.invalid_position += 1;
            continue;
        };
        let Some(lon) = number(&doc, acn, "lon").filter(|v| (-180.0..=180.0).contains(v)) else {
            st.invalid_position += 1;
            continue;
        };
        let time_ns = now
            .saturating_mul(1_000_000)
            .saturating_sub((age * 1_000_000_000.0).round() as i64);
        if !(TIME_MIN_NS..TIME_MAX_NS).contains(&time_ns) {
            st.invalid_age += 1;
            continue;
        }
        let lat_e7 = (lat * 1e7).round() as i32;
        let lon_e7 = (lon * 1e7).round() as i32;
        if lat_e7 < filter.lat_min_e7
            || lat_e7 > filter.lat_max_e7
            || lon_e7 < filter.lon_min_e7
            || lon_e7 > filter.lon_max_e7
        {
            st.outside_box += 1;
            continue;
        }
        if is_reserved_address(hex) {
            st.reserved_address += 1;
            continue;
        }
        if oa_sealed::in_sealed_window(time_ns) {
            st.sealed_window_dropped += 1;
            oa_sealed::note_window_drops(1);
            continue;
        }
        let mut row: Row = ROW_EMPTY;
        row.icao = hex;
        row.time_ns = time_ns;
        row.lat_e7 = lat_e7;
        row.lon_e7 = lon_e7;
        let source_name = str_field(&doc, acn, "type").ok_or(SnapshotError::Envelope(
            "aircraft type missing or malformed",
        ))?;
        row.source = SOURCE_NAMES
            .iter()
            .position(|&s| s == source_name)
            .map_or(SOURCE_UNKNOWN, |i| i as u8);
        let Some(mlat) = field(&doc, acn, "mlat").filter(|&n| string_array(&doc, n)) else {
            st.invalid_source_identity += 1;
            continue;
        };
        let Some(tisb) = field(&doc, acn, "tisb").filter(|&n| string_array(&doc, n)) else {
            st.invalid_source_identity += 1;
            continue;
        };
        let has_mlat = doc.len(mlat) > 0;
        let has_tisb = doc.len(tisb) > 0;
        if (row.source == 0 || row.source == 1) && (has_mlat || has_tisb) {
            st.source_conflict += 1;
            continue;
        }
        if let Some(n) = st.source_types.get_mut(usize::from(row.source)) {
            *n += 1;
        }
        if row.source != 0 && row.source != 1 {
            st.non_adsb_position += 1;
        }
        if has_mlat {
            st.with_mlat_fields += 1;
        }
        if has_tisb {
            st.with_tisb_fields += 1;
        }
        if let Some(n) = field(&doc, acn, "alt_baro") {
            match doc.kind(n) {
                Some(Kind::String | Kind::StringPooled) if doc.str(n) == Some("ground") => {
                    row.flags |= crate::F_GROUND
                }
                Some(Kind::Number) => {
                    match doc.as_i64(n).filter(|v| (-204_800..=204_800).contains(v)) {
                        Some(v) => row.baro_alt_ft = v as i32,
                        None => st.bad_optional_fields += 1,
                    }
                }
                Some(Kind::Null) => (),
                _ => st.bad_optional_fields += 1,
            }
        }
        set_i32(
            &doc,
            acn,
            "alt_geom",
            (-204_800, 204_800),
            &mut row.geom_alt_ft,
            &mut st.bad_optional_fields,
        );
        match field(&doc, acn, "geom_rate") {
            Some(n) if doc.kind(n) == Some(Kind::Number) => {
                set_i32(
                    &doc,
                    acn,
                    "geom_rate",
                    (-262_144, 262_144),
                    &mut row.vrate_fpm,
                    &mut st.bad_optional_fields,
                );
                if row.vrate_fpm != NONE_I32 {
                    row.flags |= crate::F_RATE_GEOM;
                }
            }
            Some(n) if doc.kind(n) != Some(Kind::Null) => st.bad_optional_fields += 1,
            _ => set_i32(
                &doc,
                acn,
                "baro_rate",
                (-262_144, 262_144),
                &mut row.vrate_fpm,
                &mut st.bad_optional_fields,
            ),
        }
        set_scaled_u16(
            &doc,
            acn,
            "gs",
            (0.0, 6000.0),
            10.0,
            &mut row.gs_dkt,
            &mut st.bad_optional_fields,
        );
        set_scaled_u16(
            &doc,
            acn,
            "track",
            (0.0, 360.0),
            10.0,
            &mut row.track_ddeg,
            &mut st.bad_optional_fields,
        );
        row.track_ddeg = if row.track_ddeg == NONE_U16 {
            NONE_U16
        } else {
            row.track_ddeg % 3600
        };
        set_u8(
            &doc,
            acn,
            "nic",
            11,
            &mut row.nic,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "nac_p",
            15,
            &mut row.nac_p,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "nac_v",
            7,
            &mut row.nac_v,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "sil",
            3,
            &mut row.sil,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "version",
            7,
            &mut row.version,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "gva",
            3,
            &mut row.gva,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "sda",
            3,
            &mut row.sda,
            &mut st.bad_optional_fields,
        );
        set_u8(
            &doc,
            acn,
            "nic_baro",
            1,
            &mut row.nic_baro,
            &mut st.bad_optional_fields,
        );
        row.sil_type = match field(&doc, acn, "sil_type") {
            None => NONE_U8,
            Some(n) if doc.kind(n) == Some(Kind::Null) => NONE_U8,
            Some(n) => match doc.str(n) {
                Some("unknown") => SIL_UNKNOWN,
                Some("perhour") => SIL_PER_HOUR,
                Some("persample") => SIL_PER_SAMPLE,
                _ => {
                    st.bad_optional_fields += 1;
                    SIL_UNKNOWN
                }
            },
        };
        if let Some(n) = field(&doc, acn, "squawk") {
            if doc.kind(n) != Some(Kind::Null) {
                match doc.str(n) {
                    Some(sq) if sq.len() == 4 && sq.bytes().all(|b| (b'0'..=b'7').contains(&b)) => {
                        row.squawk = sq.parse().unwrap_or(NONE_U16)
                    }
                    _ => st.bad_optional_fields += 1,
                }
            }
        }
        if let Some(n) = field(&doc, acn, "flight") {
            if doc.kind(n) != Some(Kind::Null) {
                match doc.str(n) {
                    Some(flight)
                        if flight.len() <= 8
                            && flight.bytes().all(|b| (0x20..=0x7e).contains(&b)) =>
                    {
                        for (dst, src) in row.callsign.iter_mut().zip(flight.trim_end().bytes()) {
                            *dst = src;
                        }
                    }
                    _ => st.bad_optional_fields += 1,
                }
            }
        }
        if let Some(n) = field(&doc, acn, "category") {
            if doc.kind(n) != Some(Kind::Null) {
                match doc.str(n).and_then(|cat| {
                    (cat.len() == 2)
                        .then(|| u8::from_str_radix(cat, 16).ok())
                        .flatten()
                }) {
                    Some(v) => row.category = v,
                    None => st.bad_optional_fields += 1,
                }
            }
        }
        st.accepted += 1;
        reports_push(out, &row);
        appended.push((time_ns, hex));
    }
    if st.min_seen_pos_ms == u32::MAX {
        st.min_seen_pos_ms = 0;
    }
    // Keep one-snapshot report rows deterministic for the encoder.
    if appended
        .windows(2)
        .any(|w| matches!((w.first(), w.get(1)), (Some(a), Some(b)) if a > b))
    {
        sort_new_rows(out, st.accepted as usize);
    }
    Ok(st)
}

fn string_array(doc: &Doc<'_>, n: NodeIx) -> bool {
    doc.kind(n) == Some(Kind::Array)
        && doc
            .elements(n)
            .all(|v| matches!(doc.kind(v), Some(Kind::String | Kind::StringPooled)))
}

fn set_i32(
    doc: &Doc<'_>,
    obj: NodeIx,
    key: &str,
    bounds: (i64, i64),
    dst: &mut i32,
    bad: &mut u64,
) {
    match (field(doc, obj, key), int(doc, obj, key)) {
        (None, _) => (),
        (Some(n), _) if doc.kind(n) == Some(Kind::Null) => (),
        (_, Some(v)) if (bounds.0..=bounds.1).contains(&v) => *dst = v as i32,
        (Some(_), _) => *bad += 1,
    }
}
fn set_u8(doc: &Doc<'_>, obj: NodeIx, key: &str, hi: i64, dst: &mut u8, bad: &mut u64) {
    match (field(doc, obj, key), int(doc, obj, key)) {
        (None, _) => (),
        (Some(n), _) if doc.kind(n) == Some(Kind::Null) => (),
        (_, Some(v)) if (0..=hi).contains(&v) => *dst = v as u8,
        (Some(_), _) => *bad += 1,
    }
}
fn set_scaled_u16(
    doc: &Doc<'_>,
    obj: NodeIx,
    key: &str,
    bounds: (f64, f64),
    scale: f64,
    dst: &mut u16,
    bad: &mut u64,
) {
    match (field(doc, obj, key), optional_number(doc, obj, key)) {
        (None, _) => (),
        (Some(n), _) if doc.kind(n) == Some(Kind::Null) => (),
        (_, Some(v)) if v.is_finite() && (bounds.0..=bounds.1).contains(&v) => {
            *dst = (v * scale).round() as u16
        }
        (Some(_), _) => *bad += 1,
    }
}
fn sort_new_rows(out: &mut Reports, count: usize) {
    let start = out.time_ns.len().saturating_sub(count);
    let mut ix: Vec<usize> = (start..out.time_ns.len()).collect();
    ix.sort_by_key(|&i| {
        (
            out.time_ns.get(i).copied().unwrap_or(0),
            out.icao.get(i).copied().unwrap_or(0),
        )
    });
    let rows: Vec<Row> = ix
        .into_iter()
        .filter_map(|i| crate::reports_row(out, i))
        .collect();
    out.time_ns.truncate(start);
    out.icao.truncate(start);
    out.lat_e7.truncate(start);
    out.lon_e7.truncate(start);
    out.baro_alt_ft.truncate(start);
    out.geom_alt_ft.truncate(start);
    out.gs_dkt.truncate(start);
    out.track_ddeg.truncate(start);
    out.vrate_fpm.truncate(start);
    out.flags.truncate(start);
    out.source.truncate(start);
    out.callsign.truncate(start);
    out.squawk.truncate(start);
    out.category.truncate(start);
    out.nic.truncate(start);
    out.nac_p.truncate(start);
    out.nac_v.truncate(start);
    out.sil.truncate(start);
    out.sil_type.truncate(start);
    out.version.truncate(start);
    out.gva.truncate(start);
    out.sda.truncate(start);
    out.nic_baro.truncate(start);
    out.ws_kt.truncate(start);
    out.wd_deg.truncate(start);
    out.tas_kt.truncate(start);
    out.ias_kt.truncate(start);
    out.mach_milli.truncate(start);
    out.mag_heading_cdeg.truncate(start);
    out.true_heading_cdeg.truncate(start);
    out.oat_c.truncate(start);
    out.tat_c.truncate(start);
    out.roll_cdeg.truncate(start);
    out.track_rate_cdps.truncate(start);
    for r in rows {
        reports_push(out, &r);
    }
}
