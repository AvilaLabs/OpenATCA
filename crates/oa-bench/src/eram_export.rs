//! The ERAM export: the part of a recorded SFDPS day that the P-002 scorer reads, as a plain-text
//! file a third party can check and score from without the recording (OA-REP-001 and following).
//!
//! The scorer reads, of everything the SFDPS ingest builds, only these ([`crate::legacy`]):
//!
//! * per flight, in flight order: the aircraft address (`aircraftAddress`), the address hint
//!   (`ADSB_02M_*`) and the callsign, which [`crate::assoc::associate`] joins to the ADS-B
//!   reference, and the number of track samples;
//! * per track sample of a flight that has at least one of those keys, in ingest order: the
//!   position time, the position, the reported altitude, the track velocity and the coast flag.
//!
//! A flight with no key (no address, no hint, no callsign once normalised) is never paired; the
//! scorer counts its samples as an exclusion and nothing else, so only their number is written.
//! Everything else the ingest keeps (GUFI, transponder code, flight plan identifier, departure,
//! arrival, receive time, handoffs, routes, TBFM and TFMS) is not read and not written. The
//! callsign is written as [`crate::assoc::norm_callsign`] gives it, the only form the scorer
//! compares.
//!
//! The file is UTF-8 text, one record per line, every integer in canonical decimal:
//!
//! ```text
//! oa-eram-export 1
//! source-dir <text>            where the recording was read from, as the scorer prints it
//! source-input <text>          (any number) the input line the scorer prints for the recording
//! source-sha256 <text>         the manifest hash of the recording
//! replay-hash <text>
//! sealed-dropped <n>
//! note <text>                  (any number) loader notes, printed as the scorer prints them
//! count <text>                 (any number) the ingest counts, printed as the scorer prints them
//! flights <n>
//! rows <n>                     samples over all flights
//! F <addr> <hint> <callsign> <n>   addr and hint: six lower-case hex digits or `-`; callsign:
//!                              printable ASCII without space, `x:` and hex bytes when it is not,
//!                              or `-` for none; n: the flight's samples
//! T <pos_ms> <lat_e7> <lon_e7> <alt_ft|-> <vx_dkt|-> <vy_dkt|-> <coast 0|1>   (n lines after F,
//!                              none when the flight has no key)
//! end <flights> <rows> <sha256 of every byte before this line>
//! ```
//!
//! The reader refuses anything else, a count that disagrees, a digest that does not match, and a
//! sample inside a sealed window ([`oa_sealed::in_sealed_window`]); the writer refuses the last too.
//! Shell (B7), like the rest of the crate: no file is opened here, the caller gives and takes bytes.

use std::collections::BTreeSet;
use std::fmt::Write as _;

// Development export only. These bounds keep a single export, its parse work and its decoded
// in-memory census finite on a workstation; they are format/resource limits, not observed-data
// thresholds or product capacities.
const MAX_EXPORT_BYTES: usize = 512 * 1024 * 1024;
const MAX_EXPORT_LINE_BYTES: usize = 16 * 1024;
const MAX_EXPORT_ROWS: u64 = 8_000_000;
const MAX_EXPORT_FLIGHTS: usize = 100_000;

use oa_sha256::{sha256, to_hex};

use crate::assoc::norm_callsign;
use crate::legacy::{flight_ix, new_legacy, push_th, Legacy, NONE, NO_ALT, NO_TIME, NO_VEL};

/// The first line of every export file.
pub const MAGIC: &str = "oa-eram-export 1";

/// The format version of [`MAGIC`].
pub const FORMAT_VERSION: u32 = 1;

/// Where the recording was read from and what its reader counted, as the scorer prints it; carried
/// so that a scored export prints what the scored recording printed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub source_dir: String,
    pub source_inputs: Vec<String>,
    pub source_sha256: String,
    pub replay_hash: String,
    pub sealed_dropped: u64,
    pub notes: Vec<String>,
    pub counts: Vec<String>,
}

/// One ERAM track sample.
// A7.2: cold struct (one per exported sample, held for the length of an export only)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub pos_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// [`NO_ALT`] when absent.
    pub alt_ft: i32,
    /// [`NO_VEL`] when absent.
    pub vx_dkt: i16,
    pub vy_dkt: i16,
    pub coast: bool,
}

/// One flight: its keys and samples.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flight {
    /// 24-bit aircraft address, [`NONE`] when absent.
    pub addr: u32,
    /// 24-bit address hint, [`NONE`] when absent.
    pub hint: u32,
    /// Normalised ([`norm_callsign`]), zero padded.
    pub callsign: [u8; 8],
    /// Samples the flight had.
    pub n_rows: u32,
    /// The samples, in ingest order; empty for a flight with no key ([`flight_keyless`]).
    pub rows: Vec<Row>,
}

/// Whether the flight has no address, no hint and no callsign: the scorer never pairs it.
/// A1: Flight is a plain record; its key predicate lives in this free function.
pub fn flight_keyless(flight: &Flight) -> bool {
    flight.addr == NONE && flight.hint == NONE && flight.callsign.iter().all(|&b| b == 0)
}

/// An export: the recording's printed description and the flights.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Export {
    pub meta: Meta,
    pub flights: Vec<Flight>,
}

/// What a drop list removed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropStats {
    /// Addresses on the list.
    pub listed: usize,
    pub flights: u64,
    pub rows: u64,
}

/// Counts of an export for the package description.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub flights: u64,
    /// Flights with an aircraft address / only an address hint / only a callsign / no key.
    pub with_addr: u64,
    pub hint_only: u64,
    pub callsign_only: u64,
    pub keyless: u64,
    /// Samples written in full / counted only (flights with no key) / carrying the coast flag.
    pub rows_listed: u64,
    pub rows_counted_only: u64,
    pub rows_coast: u64,
    /// Distinct aircraft addresses (address, or hint where there is no address) over listed flights.
    pub distinct_addresses: u64,
}

/// The export of a loaded recording: every flight in order, keys normalised, samples of keyless
/// flights reduced to their number. Legacy table shape and export caps are checked before an
/// `Export` or its rows are allocated; malformed public legacy tables are refused rather than
/// silently dropping rows.
pub fn from_legacy(leg: &Legacy, meta: Meta) -> Result<Export, String> {
    let n = leg.flights.gufi.len();
    if n > MAX_EXPORT_FLIGHTS {
        return Err("legacy flight count exceeds export limit 100,000".to_string());
    }
    let th_n = leg.th.flight.len();
    if th_n > MAX_EXPORT_ROWS as usize {
        return Err("legacy TH row count exceeds export limit 8,000,000".to_string());
    }
    if [
        leg.flights.callsign.len(),
        leg.flights.addr.len(),
        leg.flights.addr_hint.len(),
        leg.flights.beacon.len(),
        leg.flights.plan_id.len(),
        leg.flights.dep.len(),
        leg.flights.arr.len(),
    ]
    .into_iter()
    .any(|len| len != n)
    {
        return Err("legacy flight table columns have inconsistent lengths".to_string());
    }
    if [
        leg.th.pos_ms.len(),
        leg.th.recv_ms.len(),
        leg.th.lat_e7.len(),
        leg.th.lon_e7.len(),
        leg.th.alt_ft.len(),
        leg.th.vx_dkt.len(),
        leg.th.vy_dkt.len(),
        leg.th.coast.len(),
    ]
    .into_iter()
    .any(|len| len != th_n)
    {
        return Err("legacy TH table columns have inconsistent lengths".to_string());
    }

    // Count and validate every source row before allocating any Export values.
    let mut rows_per_flight = Vec::new();
    rows_per_flight
        .try_reserve_exact(n)
        .map_err(|_| "cannot allocate bounded legacy row counts".to_string())?;
    rows_per_flight.resize(n, 0u32);
    for (i, &flight) in leg.th.flight.iter().enumerate() {
        let Some(count) = rows_per_flight.get_mut(flight as usize) else {
            return Err(format!(
                "legacy TH row {i} references nonexistent flight {flight}"
            ));
        };
        *count = count
            .checked_add(1)
            .ok_or_else(|| "legacy per-flight row count overflow".to_string())?;
    }

    let mut flights = Vec::new();
    flights
        .try_reserve_exact(n)
        .map_err(|_| "cannot allocate bounded export flight table".to_string())?;
    for f in 0..n {
        let callsign = leg
            .flights
            .callsign
            .get(f)
            .ok_or_else(|| "legacy callsign column changed after validation".to_string())?;
        let addr = leg
            .flights
            .addr
            .get(f)
            .copied()
            .ok_or_else(|| "legacy address column changed after validation".to_string())?;
        let hint = leg
            .flights
            .addr_hint
            .get(f)
            .copied()
            .ok_or_else(|| "legacy address-hint column changed after validation".to_string())?;
        let n_rows = rows_per_flight
            .get(f)
            .copied()
            .ok_or_else(|| "legacy row counts changed after validation".to_string())?;
        flights.push(Flight {
            addr,
            hint,
            callsign: norm_callsign(callsign),
            n_rows,
            rows: Vec::new(),
        });
    }
    for (f, flight) in flights.iter_mut().enumerate() {
        if !flight_keyless(flight) {
            let row_capacity = rows_per_flight
                .get(f)
                .copied()
                .ok_or_else(|| "legacy row counts changed after validation".to_string())?
                as usize;
            flight
                .rows
                .try_reserve_exact(row_capacity)
                .map_err(|_| "cannot allocate bounded export track rows".to_string())?;
        }
    }
    for i in 0..th_n {
        let f = leg
            .th
            .flight
            .get(i)
            .copied()
            .ok_or_else(|| "legacy TH flight column changed after validation".to_string())?
            as usize;
        let flight = flights
            .get_mut(f)
            .ok_or_else(|| format!("legacy TH row {i} references nonexistent flight {f}"))?;
        if flight_keyless(flight) {
            continue;
        }
        flight.rows.push(Row {
            pos_ms: leg.th.pos_ms.get(i).copied().ok_or_else(|| {
                "legacy TH position-time column changed after validation".to_string()
            })?,
            lat_e7: leg
                .th
                .lat_e7
                .get(i)
                .copied()
                .ok_or_else(|| "legacy TH latitude column changed after validation".to_string())?,
            lon_e7: leg
                .th
                .lon_e7
                .get(i)
                .copied()
                .ok_or_else(|| "legacy TH longitude column changed after validation".to_string())?,
            alt_ft: leg
                .th
                .alt_ft
                .get(i)
                .copied()
                .ok_or_else(|| "legacy TH altitude column changed after validation".to_string())?,
            vx_dkt: leg.th.vx_dkt.get(i).copied().ok_or_else(|| {
                "legacy TH east-velocity column changed after validation".to_string()
            })?,
            vy_dkt: leg.th.vy_dkt.get(i).copied().ok_or_else(|| {
                "legacy TH north-velocity column changed after validation".to_string()
            })?,
            coast: leg
                .th
                .coast
                .get(i)
                .copied()
                .ok_or_else(|| "legacy TH coast column changed after validation".to_string())?
                != 0,
        });
    }
    Ok(Export { meta, flights })
}

/// The legacy tables the scorer pairs, from an export: flights in order, the samples of a flight
/// with a key in file order, and for a flight with no key as many placeholder samples (no position
/// time, no altitude, no velocity) as it had, which the scorer only counts. The GUFI of a flight is
/// its number; nothing else of the tables is filled, and the ingest counts stay empty (the scorer
/// prints the export's own).
pub fn to_legacy(e: &Export) -> Result<Legacy, String> {
    let mut leg = new_legacy();
    for (i, f) in e.flights.iter().enumerate() {
        let Some(ix) = flight_ix(&mut leg, i as u128) else {
            return Err("the export holds more flights than the legacy tables take".to_string());
        };
        let k = ix as usize;
        if let Some(s) = leg.flights.callsign.get_mut(k) {
            *s = f.callsign;
        }
        if let Some(s) = leg.flights.addr.get_mut(k) {
            *s = f.addr;
        }
        if let Some(s) = leg.flights.addr_hint.get_mut(k) {
            *s = f.hint;
        }
        if flight_keyless(f) {
            for _ in 0..f.n_rows {
                push_th(&mut leg, ix, NO_TIME, NO_TIME, 0, 0, NO_ALT, None, false);
            }
        }
        for r in &f.rows {
            let vel = (r.vx_dkt != NO_VEL && r.vy_dkt != NO_VEL).then_some((r.vx_dkt, r.vy_dkt));
            push_th(
                &mut leg, ix, r.pos_ms, NO_TIME, r.lat_e7, r.lon_e7, r.alt_ft, vel, r.coast,
            );
        }
    }
    if leg.counts.overflow_th > 0 {
        return Err("the export holds more samples than the legacy tables take".to_string());
    }
    Ok(leg)
}

/// The counts of an export.
pub fn summary(e: &Export) -> Summary {
    let mut s = Summary::default();
    let mut addrs: BTreeSet<u32> = BTreeSet::new();
    for f in &e.flights {
        s.flights += 1;
        if flight_keyless(f) {
            s.keyless += 1;
            s.rows_counted_only += u64::from(f.n_rows);
            continue;
        }
        if f.addr != NONE {
            s.with_addr += 1;
            addrs.insert(f.addr);
        } else if f.hint != NONE {
            s.hint_only += 1;
            addrs.insert(f.hint);
        } else {
            s.callsign_only += 1;
        }
        s.rows_listed += f.rows.len() as u64;
        s.rows_coast += f.rows.iter().filter(|r| r.coast).count() as u64;
    }
    s.distinct_addresses = addrs.len() as u64;
    s
}

/// The addresses of a drop list: one per line, six hex digits (24 bits, either case); blank lines
/// and lines starting with `#` are skipped; anything else is an error naming the line.
pub fn parse_drop_list(text: &str) -> Result<BTreeSet<u32>, String> {
    let mut set = BTreeSet::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.len() != 6 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "drop list line {}: expected six hex digits (a 24-bit aircraft address)",
                i + 1
            ));
        }
        let a = u32::from_str_radix(t, 16)
            .map_err(|_| format!("drop list line {}: not a hex number", i + 1))?;
        set.insert(a);
    }
    Ok(set)
}

/// Removes every flight whose address or address hint is on the list, with its samples.
pub fn drop_addresses(e: &mut Export, list: &BTreeSet<u32>) -> DropStats {
    let mut st = DropStats {
        listed: list.len(),
        ..DropStats::default()
    };
    e.flights.retain(|f| {
        let hit = (f.addr != NONE && list.contains(&f.addr))
            || (f.hint != NONE && list.contains(&f.hint));
        if hit {
            st.flights += 1;
            st.rows += u64::from(f.n_rows);
        }
        !hit
    });
    st
}

/// The note a scored run prints for a drop list that removed something.
pub fn drop_note(st: &DropStats) -> String {
    format!(
        "ERAM flights dropped by an address drop list of {} addresses: {} flights, {} track samples",
        st.listed, st.flights, st.rows
    )
}

/// Refuses an export holding a sample inside a sealed window (OA-SEAL-014): the count is named,
/// nothing is said of the rows.
pub fn check_unsealed(e: &Export) -> Result<(), String> {
    let bad = e
        .flights
        .iter()
        .flat_map(|f| f.rows.iter())
        .filter(|r| oa_sealed::in_sealed_window(r.pos_ms.saturating_mul(1_000_000)))
        .count();
    if bad > 0 {
        return Err(format!(
            "the export holds {bad} track samples inside the sealed hold-out window; it is never written or read (OA-SEAL-014)"
        ));
    }
    Ok(())
}

fn clean_line(label: &str, s: &str) -> Result<(), String> {
    if s.is_empty() || s != s.trim() || s.chars().any(char::is_control) {
        return Err(format!(
            "{label}: a header text must be one non-empty line with no control characters and no outer white space"
        ));
    }
    Ok(())
}

fn hex_addr(a: u32) -> String {
    if a == NONE {
        "-".to_string()
    } else {
        format!("{a:06x}")
    }
}

fn callsign_text(c: &[u8; 8]) -> String {
    let n = c.iter().position(|&b| b == 0).unwrap_or(c.len());
    let (head, tail) = c.split_at(n);
    let plain = !head.is_empty()
        && tail.iter().all(|&b| b == 0)
        && head.iter().all(|&b| (0x21..=0x7e).contains(&b))
        && head != b"-"
        && !head.starts_with(b"x:");
    if head.is_empty() && tail.iter().all(|&b| b == 0) {
        "-".to_string()
    } else if plain {
        head.iter().map(|&b| b as char).collect()
    } else {
        let mut s = String::from("x:");
        for b in c {
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}

fn opt<T: std::fmt::Display + PartialEq>(v: T, none: T) -> String {
    if v == none {
        "-".to_string()
    } else {
        v.to_string()
    }
}

/// The file text of an export, trailer included. Refuses a header text that is not one clean line,
/// and a sample in a sealed window.
pub fn write(e: &Export) -> Result<String, String> {
    check_unsealed(e)?;
    let metadata_lines = e.meta.source_inputs.len() + e.meta.notes.len() + e.meta.counts.len();
    if metadata_lines > 20_000
        || [
            &e.meta.source_dir,
            &e.meta.source_sha256,
            &e.meta.replay_hash,
        ]
        .iter()
        .any(|s| s.len() > MAX_EXPORT_LINE_BYTES)
        || e.meta
            .source_inputs
            .iter()
            .chain(&e.meta.notes)
            .chain(&e.meta.counts)
            .any(|s| s.len() > MAX_EXPORT_LINE_BYTES)
    {
        return Err("export metadata exceeds the line/count bounds".to_string());
    }
    if e.flights.len() > MAX_EXPORT_FLIGHTS {
        return Err("export exceeds 100,000 flights".to_string());
    }
    let m = &e.meta;
    let mut s = String::new();
    macro_rules! add_line {
        ($($arg:tt)*) => {{
            let line = format!($($arg)*);
            if line.len() > MAX_EXPORT_LINE_BYTES
                || s.len().checked_add(line.len() + 1).filter(|&n| n <= MAX_EXPORT_BYTES).is_none()
            {
                return Err("export exceeds its 512 MiB or 16 KiB line bound".to_string());
            }
            s.push_str(&line);
            s.push('\n');
        }};
    }
    add_line!("{MAGIC}");
    clean_line("source-dir", &m.source_dir)?;
    add_line!("source-dir {}", m.source_dir);
    for l in &m.source_inputs {
        clean_line("source-input", l)?;
        add_line!("source-input {l}");
    }
    clean_line("source-sha256", &m.source_sha256)?;
    add_line!("source-sha256 {}", m.source_sha256);
    clean_line("replay-hash", &m.replay_hash)?;
    add_line!("replay-hash {}", m.replay_hash);
    add_line!("sealed-dropped {}", m.sealed_dropped);
    for l in &m.notes {
        clean_line("note", l)?;
        add_line!("note {l}");
    }
    for l in &m.counts {
        clean_line("count", l)?;
        add_line!("count {l}");
    }
    let rows: u64 = e.flights.iter().map(|f| u64::from(f.n_rows)).sum();
    if rows > MAX_EXPORT_ROWS {
        return Err("export exceeds 8,000,000 rows".to_string());
    }
    add_line!("flights {}", e.flights.len());
    add_line!("rows {rows}");
    for f in &e.flights {
        let listed = if flight_keyless(f) {
            0
        } else {
            f.n_rows as usize
        };
        if f.rows.len() != listed {
            return Err("a flight's samples are not the number it states".to_string());
        }
        add_line!(
            "F {} {} {} {}",
            hex_addr(f.addr),
            hex_addr(f.hint),
            callsign_text(&f.callsign),
            f.n_rows
        );
        for r in &f.rows {
            add_line!(
                "T {} {} {} {} {} {} {}",
                r.pos_ms,
                r.lat_e7,
                r.lon_e7,
                opt(r.alt_ft, NO_ALT),
                opt(r.vx_dkt, NO_VEL),
                opt(r.vy_dkt, NO_VEL),
                u8::from(r.coast)
            );
        }
    }
    let digest = to_hex(&sha256(s.as_bytes()));
    add_line!("end {} {rows} {digest}", e.flights.len());
    if s.len() > MAX_EXPORT_BYTES {
        return Err("export exceeds 512 MiB".to_string());
    }
    if s.lines().any(|line| line.len() > MAX_EXPORT_LINE_BYTES) {
        return Err("export contains a line longer than 16 KiB".to_string());
    }
    Ok(s)
}

/// The SHA-256 of an export file's bytes (the digest a manifest line carries).
pub fn file_sha256(bytes: &[u8]) -> String {
    to_hex(&sha256(bytes))
}

/// A manifest line for a file, in `sha256sum` form: the digest, two spaces, the name.
pub fn manifest_line(name: &str, bytes: &[u8]) -> Result<String, String> {
    if name.is_empty() || name.chars().any(|c| c.is_control() || c == ' ' || c == '/') {
        return Err(format!("{name:?} is not a plain file name"));
    }
    Ok(format!("{}  {name}\n", file_sha256(bytes)))
}

/// Checks `bytes` against the line of `manifest` that names `name`: the manifest must hold exactly
/// one such line, of the `sha256sum` form, and the digest must match.
pub fn check_manifest(manifest: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let mut found: Option<&str> = None;
    for line in manifest.lines().filter(|l| !l.trim().is_empty()) {
        let Some((h, n)) = line.split_once("  ") else {
            return Err(format!(
                "manifest line not of the form `<sha256>  <name>`: {line:?}"
            ));
        };
        if h.len() != 64 || !h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!("manifest line has no SHA-256: {line:?}"));
        }
        if n == name {
            if found.is_some() {
                return Err(format!("the manifest names {name} twice"));
            }
            found = Some(h);
        }
    }
    let Some(want) = found else {
        return Err(format!("the manifest has no line for {name}"));
    };
    let got = file_sha256(bytes);
    if got != want {
        return Err(format!(
            "{name}: SHA-256 {got} does not match the manifest's {want}"
        ));
    }
    Ok(())
}

fn canon<T: std::str::FromStr + std::fmt::Display>(tok: &str) -> Option<T> {
    let v: T = tok.parse().ok()?;
    (v.to_string() == tok).then_some(v)
}

fn parse_addr(tok: &str) -> Option<u32> {
    if tok == "-" {
        return Some(NONE);
    }
    if tok.len() != 6 || !tok.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u32::from_str_radix(tok, 16).ok()
}

fn parse_callsign(tok: &str) -> Option<[u8; 8]> {
    let mut out = [0u8; 8];
    if tok == "-" {
        return Some(out);
    }
    if let Some(h) = tok.strip_prefix("x:") {
        if h.len() != 16 || !h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        for (o, i) in out.iter_mut().zip((0..16).step_by(2)) {
            *o = u8::from_str_radix(h.get(i..i + 2)?, 16).ok()?;
        }
    } else {
        let b = tok.as_bytes();
        if b.len() > 8 || !b.iter().all(|c| (0x21..=0x7e).contains(c)) {
            return None;
        }
        for (o, c) in out.iter_mut().zip(b) {
            *o = *c;
        }
    }
    // Only the normalised form is written, so only it is read.
    (norm_callsign(&out) == out && out.iter().any(|&b| b != 0)).then_some(out)
}

fn opt_num<T: std::str::FromStr + std::fmt::Display>(tok: &str, none: T) -> Option<T> {
    if tok == "-" {
        Some(none)
    } else {
        canon(tok)
    }
}

/// Reads an export. The trailer digest must match every byte before it, the counts must agree,
/// every record must be in the written form, and no sample may be in a sealed window.
pub fn parse(bytes: &[u8]) -> Result<Export, String> {
    if bytes.len() > MAX_EXPORT_BYTES {
        return Err("export exceeds 512 MiB".to_string());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the export is not UTF-8".to_string())?;
    if text.lines().any(|line| line.len() > MAX_EXPORT_LINE_BYTES) {
        return Err("export contains a line longer than 16 KiB".to_string());
    }
    let body_end = text
        .strip_suffix('\n')
        .and_then(|t| t.rfind('\n'))
        .map(|i| i + 1)
        .ok_or_else(|| "the export has no trailer".to_string())?;
    let (body, trailer) = text.split_at(body_end);
    let mut t = trailer.trim_end_matches('\n').split(' ');
    let (Some("end"), Some(nf), Some(nr), Some(digest), None) =
        (t.next(), t.next(), t.next(), t.next(), t.next())
    else {
        return Err("the export's last line is not `end <flights> <rows> <sha256>`".to_string());
    };
    let got = to_hex(&sha256(body.as_bytes()));
    if got != digest {
        return Err(format!(
            "the export's own digest does not match its contents (file says {digest}, contents give {got})"
        ));
    }
    let bad = |n: usize, why: &str| format!("export line {n}: {why}");
    let mut lines = body.lines().enumerate().map(|(i, l)| (i + 1, l));
    if lines.next().map(|(_, l)| l) != Some(MAGIC) {
        return Err(format!("the export is not a `{MAGIC}` file"));
    }
    let mut e = Export::default();
    let (mut src_dir, mut src_sha, mut replay, mut sealed) = (None, None, None, None);
    let (mut n_flights, mut n_rows): (Option<u64>, Option<u64>) = (None, None);
    let mut pending: Option<(usize, usize)> = None; // (line of the F, samples still to read)
    let mut rows_seen = 0u64;
    for (n, line) in lines {
        if let Some(left) = pending.as_mut().map(|p| &mut p.1).filter(|l| **l > 0) {
            let f = e.flights.last_mut().ok_or_else(|| bad(n, "no flight"))?;
            let mut p = line.split(' ');
            let (Some("T"), Some(a), Some(b), Some(c), Some(d), Some(g), Some(h), Some(i), None) = (
                p.next(),
                p.next(),
                p.next(),
                p.next(),
                p.next(),
                p.next(),
                p.next(),
                p.next(),
                p.next(),
            ) else {
                return Err(bad(n, "expected a T line of seven fields"));
            };
            let row = (|| {
                Some(Row {
                    pos_ms: canon(a)?,
                    lat_e7: canon(b)?,
                    lon_e7: canon(c)?,
                    alt_ft: opt_num(d, NO_ALT)?,
                    vx_dkt: opt_num(g, NO_VEL)?,
                    vy_dkt: opt_num(h, NO_VEL)?,
                    coast: match i {
                        "0" => false,
                        "1" => true,
                        _ => return None,
                    },
                })
            })()
            .ok_or_else(|| bad(n, "a T field is not in the written form"))?;
            f.rows.push(row);
            *left -= 1;
            rows_seen = rows_seen
                .checked_add(1)
                .filter(|&total| total <= MAX_EXPORT_ROWS)
                .ok_or_else(|| bad(n, "row count exceeds 8,000,000"))?;
            continue;
        }
        let (key, val) = line.split_once(' ').unwrap_or((line, ""));
        let text_val = |v: &str| -> Result<String, String> {
            clean_line(key, v).map_err(|_| bad(n, "the text is empty or not one clean line"))?;
            Ok(v.to_string())
        };
        match key {
            "source-dir" if src_dir.is_none() && e.flights.is_empty() => {
                src_dir = Some(text_val(val)?);
            }
            "source-input" if e.flights.is_empty() => {
                e.meta.source_inputs.push(text_val(val)?);
            }
            "source-sha256" if src_sha.is_none() => src_sha = Some(text_val(val)?),
            "replay-hash" if replay.is_none() => replay = Some(text_val(val)?),
            "sealed-dropped" if sealed.is_none() => {
                sealed = Some(canon::<u64>(val).ok_or_else(|| bad(n, "not a number"))?);
            }
            "note" if e.flights.is_empty() => e.meta.notes.push(text_val(val)?),
            "count" if e.flights.is_empty() => e.meta.counts.push(text_val(val)?),
            "flights" if n_flights.is_none() => {
                let count = canon::<u64>(val).ok_or_else(|| bad(n, "not a number"))?;
                if count > MAX_EXPORT_FLIGHTS as u64 {
                    return Err(bad(n, "flight count exceeds 100,000"));
                }
                n_flights = Some(count);
            }
            "rows" if n_rows.is_none() => {
                let count = canon::<u64>(val).ok_or_else(|| bad(n, "not a number"))?;
                if count > MAX_EXPORT_ROWS {
                    return Err(bad(n, "row count exceeds 8,000,000"));
                }
                n_rows = Some(count);
            }
            "F" => {
                if e.flights.len() >= MAX_EXPORT_FLIGHTS {
                    return Err(bad(n, "flight count exceeds 100,000"));
                }
                let mut p = val.split(' ');
                let (Some(a), Some(h), Some(c), Some(k), None) =
                    (p.next(), p.next(), p.next(), p.next(), p.next())
                else {
                    return Err(bad(n, "expected an F line of four fields"));
                };
                let fl = (|| {
                    Some(Flight {
                        addr: parse_addr(a)?,
                        hint: parse_addr(h)?,
                        callsign: parse_callsign(c)?,
                        n_rows: canon(k)?,
                        rows: Vec::new(),
                    })
                })()
                .ok_or_else(|| bad(n, "an F field is not in the written form"))?;
                if fl.n_rows as u64 > MAX_EXPORT_ROWS {
                    return Err(bad(n, "flight row count exceeds 8,000,000"));
                }
                pending = Some((
                    n,
                    if flight_keyless(&fl) {
                        0
                    } else {
                        fl.n_rows as usize
                    },
                ));
                rows_seen = rows_seen
                    .checked_add(if flight_keyless(&fl) {
                        u64::from(fl.n_rows)
                    } else {
                        0
                    })
                    .filter(|&total| total <= MAX_EXPORT_ROWS)
                    .ok_or_else(|| bad(n, "row count exceeds 8,000,000"))?;
                e.flights.push(fl);
            }
            _ => {
                return Err(bad(
                    n,
                    "a record that is not part of the format, or out of place",
                ))
            }
        }
        if e.meta.source_inputs.len() + e.meta.notes.len() + e.meta.counts.len() > 20_000 {
            return Err(bad(n, "metadata record count exceeds 20,000"));
        }
    }
    if pending.is_some_and(|(_, left)| left > 0) {
        return Err("the export ends inside a flight".to_string());
    }
    let (Some(sd), Some(ss), Some(rh), Some(sl), Some(nf0), Some(nr0)) =
        (src_dir, src_sha, replay, sealed, n_flights, n_rows)
    else {
        return Err("the export's header is incomplete".to_string());
    };
    e.meta.source_dir = sd;
    e.meta.source_sha256 = ss;
    e.meta.replay_hash = rh;
    e.meta.sealed_dropped = sl;
    let (tf, tr) = (
        canon::<u64>(nf).ok_or("the trailer's flight count is not a number")?,
        canon::<u64>(nr).ok_or("the trailer's sample count is not a number")?,
    );
    if nf0 != e.flights.len() as u64 || tf != nf0 || nr0 != rows_seen || tr != nr0 {
        return Err(format!(
            "the counts disagree: header {nf0} flights, {nr0} samples; trailer {tf}, {tr}; read {}, {rows_seen}",
            e.flights.len()
        ));
    }
    check_unsealed(&e)?;
    Ok(e)
}
