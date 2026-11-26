//! The caller-owned output: NOTAM event rows as a struct-of-arrays with stated capacities, the
//! staged row, and the free functions that read them. Plain data and free functions (RULESET
//! A1, A3). Core (pure); the only allocation is in [`new_notams`].

use crate::fields::{Field, FIELD_COUNT};

/// Rows a [`Notams`] holds by default: the 2026-11-14 sample of the feed is 112 messages, so
/// this is 73 times that.
pub const DEFAULT_ROW_CAPACITY: usize = 8192;
/// Bytes of text in the default string table: 1 KiB per row, against a longest surveyed NOTAM
/// text of 341 bytes and a schedule of 21.
pub const DEFAULT_STRING_BYTES: usize = 8192 * 1024;
/// Polygons in the default table: four per row; the survey shows at most two in a message.
pub const DEFAULT_POLYGONS: usize = 8192 * 4;
/// Polygon vertices in the default table: 128 per row, against 83 per message with geometry in
/// the survey (903 vertices in 11 messages, the largest polygon 588 vertices).
pub const DEFAULT_VERTICES: usize = 8192 * 128;
/// Default feature-capacity reservation per row. This is a memory-sizing default, not a model
/// or AIXM occurrence limit; callers may provide a larger total via
/// [`new_notams_with_feature_capacity`]. The 16-per-row budget uses 48 bytes per feature column
/// set (about 768 bytes per row, 6 MiB at the default row capacity).
pub const DEFAULT_FEATURES_PER_ROW: usize = 16;
pub const FEATURE_TIME_INSTANT: u8 = 1;
pub const FEATURE_TIME_PERIOD: u8 = 2;
pub const FEATURE_META_SEQUENCE: u8 = 1;
pub const FEATURE_META_CORRECTION: u8 = 2;
pub const FEATURE_ERROR_TIME: u8 = 1;
pub const FEATURE_ERROR_INTERPRETATION: u8 = 2;
pub const FEATURE_ERROR_SEQUENCE: u8 = 4;
pub const FEATURE_ERROR_CORRECTION: u8 = 8;
pub const FEATURE_ERROR_DESIGNATOR: u8 = 16;
pub const FEATURE_TIME_BEGIN_PRESENT: u8 = 1;
pub const FEATURE_TIME_END_PRESENT: u8 = 2;
pub const FEATURE_TIME_INSTANT_PRESENT: u8 = 4;

/// One AIXM feature time slice retained for encoder round trips. `Notams` stores each member in
/// its own column; this is the staged read type returned by [`feature_at`]. Its padded C layout
/// is 56 bytes; the corresponding struct-of-arrays columns occupy 48 bytes per feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FeatureSlice {
    pub kind: u8,
    /// `1` is GML TimeInstant, `2` is GML TimePeriod.
    pub time_kind: u8,
    pub interpretation: u8,
    /// Presence bits for optional sequence and correction numbers.
    pub meta: u8,
    /// Presence bits for the instant position or period begin/end values.
    pub time_meta: u8,
    /// Error bits for malformed or duplicate feature time-slice fields.
    pub errors: u8,
    pub reserved: [u8; 2],
    pub begin_ms: i64,
    pub end_ms: i64,
    pub sequence_number: u32,
    pub correction_number: u32,
    /// Exact per-object designator retained from supported feature members.
    pub designator: [u8; 12],
    pub designator_len: u8,
    /// Airport ICAO indicator, kept distinct from each airport's designator.
    pub icao: [u8; 4],
    pub icao_len: u8,
}
const _: () = assert!(core::mem::size_of::<FeatureSlice>() == 56);

/// A row being built, in model units. Fields are meaningful only where `present` has the
/// field's bit.
// A7.1: 272 bytes per row, up 24 from the guarded 248-byte row to retain the feature span and
// event-slice metadata. Per-feature data is in separately bounded columns (48 bytes/feature).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Row {
    pub present: u64,
    pub errors: u64,
    pub issued_ms: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub canceled_ms: i64,
    pub flags: u32,
    pub number: u32,
    pub ref_number: u32,
    pub sched_off: u32,
    pub sched_len: u32,
    pub text_off: u32,
    pub text_len: u32,
    pub radius_m: u32,
    pub poly_first: u32,
    pub poly_count: u32,
    /// Span into the NOTAM feature time-slice columns.
    pub feature_first: u32,
    pub feature_count: u32,
    pub feat_mask: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub floor_ft: i32,
    pub ceil_ft: i32,
    pub year: u16,
    pub ref_year: u16,
    pub scenario: u16,
    pub ntype: u8,
    pub class: u8,
    pub traffic: u8,
    pub purpose: u8,
    pub scope: u8,
    pub floor_ref: u8,
    pub ceil_ref: u8,
    pub feat_kind: u8,
    /// AIXM event `interpretation` code; zero means missing or not understood.
    pub interpretation: u8,
    /// Presence bits for optional event `sequenceNumber` and `correctionNumber`.
    pub slice_meta: u8,
    /// Error bits for malformed or duplicate event time-slice metadata.
    pub slice_errors: u8,
    pub sequence_number: u32,
    pub correction_number: u32,
    pub event_id: [u8; 16],
    pub series: [u8; 12],
    pub ref_series: [u8; 12],
    pub xover: [u8; 16],
    pub feat_desig: [u8; 12],
    pub rwy: [u8; 12],
    pub apt: [u8; 8],
    pub navaid: [u8; 8],
    pub unit: [u8; 8],
    pub selection: [u8; 5],
    pub location: [u8; 4],
    pub fir: [u8; 4],
    pub icao_location: [u8; 4],
    pub apt_icao: [u8; 4],
}
const _: () = assert!(core::mem::size_of::<Row>() == 272);

/// A row with every field absent.
pub const BLANK_ROW: Row = Row {
    present: 0,
    errors: 0,
    issued_ms: 0,
    start_ms: 0,
    end_ms: 0,
    canceled_ms: 0,
    flags: 0,
    number: 0,
    ref_number: 0,
    sched_off: 0,
    sched_len: 0,
    text_off: 0,
    text_len: 0,
    radius_m: 0,
    poly_first: 0,
    poly_count: 0,
    feature_first: 0,
    feature_count: 0,
    feat_mask: 0,
    lat_e7: 0,
    lon_e7: 0,
    floor_ft: 0,
    ceil_ft: 0,
    year: 0,
    ref_year: 0,
    scenario: 0,
    ntype: 0,
    class: 0,
    traffic: 0,
    purpose: 0,
    scope: 0,
    floor_ref: 0,
    ceil_ref: 0,
    feat_kind: 0,
    interpretation: 0,
    slice_meta: 0,
    slice_errors: 0,
    sequence_number: 0,
    correction_number: 0,
    event_id: [0; 16],
    series: [0; 12],
    ref_series: [0; 12],
    xover: [0; 16],
    feat_desig: [0; 12],
    rwy: [0; 12],
    apt: [0; 8],
    navaid: [0; 8],
    unit: [0; 8],
    selection: [0; 5],
    location: [0; 4],
    fir: [0; 4],
    icao_location: [0; 4],
    apt_icao: [0; 4],
};

/// NOTAM event rows, struct-of-arrays: element `i` of every row column is row `i`. Every
/// column is reserved at its capacity once and never grows. A column value is meaningful only
/// where the row's `present` word has the field's bit (bit `Field as u32`); `errors` has the
/// bit of every field that was present but malformed, duplicated or inconsistent.
///
/// Geometry is shared: row `r` owns polygons `poly_first[r] .. poly_first[r] + poly_count[r]`,
/// with `poly_feature` preserving each polygon's owning feature slice;
/// polygon `p` owns vertices `poly_vstart[p] .. poly_vstart[p] + poly_vlen[p]` of the
/// `vtx_lat` and `vtx_lon` columns (1e-7 degree, the closing vertex not repeated). The
/// circle is `lat_e7`, `lon_e7` (field `Centre`) and `radius_m` (field `Radius`). Short text is
/// zero-padded in fixed arrays; read it with [`code_str`]. The long text and the schedule are
/// in `strings`, read with [`text_of`] and [`schedule_of`].
// A3: one Vec per column.
#[derive(Clone, Debug)]
pub struct Notams {
    pub capacity: usize,
    pub len: usize,
    pub present: Vec<u64>,
    pub errors: Vec<u64>,
    pub flags: Vec<u32>,
    /// The event's `gml:identifier`, the 16 bytes of its UUID.
    pub event_id: Vec<[u8; 16]>,
    pub series: Vec<[u8; 12]>,
    pub number: Vec<u32>,
    pub year: Vec<u16>,
    /// [`crate::TYPE_NEW`] and its neighbours.
    pub ntype: Vec<u8>,
    pub ref_series: Vec<[u8; 12]>,
    pub ref_number: Vec<u32>,
    pub ref_year: Vec<u16>,
    pub location: Vec<[u8; 4]>,
    pub fir: Vec<[u8; 4]>,
    pub icao_location: Vec<[u8; 4]>,
    /// Unix milliseconds.
    pub issued_ms: Vec<i64>,
    pub start_ms: Vec<i64>,
    pub end_ms: Vec<i64>,
    pub canceled_ms: Vec<i64>,
    pub sched_off: Vec<u32>,
    pub sched_len: Vec<u32>,
    pub text_off: Vec<u32>,
    pub text_len: Vec<u32>,
    /// The five-letter Q code (`QMXLC`).
    pub selection: Vec<[u8; 5]>,
    pub scenario: Vec<u16>,
    pub class: Vec<u8>,
    pub traffic: Vec<u8>,
    pub purpose: Vec<u8>,
    pub scope: Vec<u8>,
    pub xover: Vec<[u8; 16]>,
    /// Circle centre, 1e-7 degree, and radius, metres.
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    pub radius_m: Vec<u32>,
    /// Altitude limits in feet ([`crate::ALT_UNLIMITED`] for `UNL`) and their references.
    pub floor_ft: Vec<i32>,
    pub ceil_ft: Vec<i32>,
    pub floor_ref: Vec<u8>,
    pub ceil_ref: Vec<u8>,
    pub poly_first: Vec<u32>,
    pub poly_count: Vec<u32>,
    pub feature_first: Vec<u32>,
    pub feature_count: Vec<u32>,
    /// Bounded total capacity of retained feature time slices. The default constructor reserves
    /// `capacity * DEFAULT_FEATURES_PER_ROW`; this explicit capacity may be larger than that.
    pub feature_capacity: usize,
    pub feature_kind: Vec<u8>,
    pub feature_time_kind: Vec<u8>,
    pub feature_interpretation: Vec<u8>,
    pub feature_meta: Vec<u8>,
    pub feature_time_meta: Vec<u8>,
    pub feature_errors: Vec<u8>,
    pub feature_begin_ms: Vec<i64>,
    pub feature_end_ms: Vec<i64>,
    pub feature_sequence_number: Vec<u32>,
    pub feature_correction_number: Vec<u32>,
    pub feature_designator: Vec<[u8; 12]>,
    pub feature_designator_len: Vec<u8>,
    pub feature_icao: Vec<[u8; 4]>,
    pub feature_icao_len: Vec<u8>,
    /// Bit `k` is set when a feature of kind `k` ([`crate::FEAT_AIRPORT`] ...) is in the message.
    pub feat_mask: Vec<u32>,
    /// The most specific feature kind present (see `docs/FNS_SURVEY.md`), 0 for none.
    pub feat_kind: Vec<u8>,
    /// AIXM event time-slice interpretation code.
    pub interpretation: Vec<u8>,
    /// Presence bits for optional event time-slice sequence/correction values.
    pub slice_meta: Vec<u8>,
    /// Error bits for invalid or duplicate event time-slice values.
    pub slice_errors: Vec<u8>,
    pub sequence_number: Vec<u32>,
    pub correction_number: Vec<u32>,
    /// That feature's designator, empty when its kind has none.
    pub feat_desig: Vec<[u8; 12]>,
    pub apt: Vec<[u8; 8]>,
    pub apt_icao: Vec<[u8; 4]>,
    pub rwy: Vec<[u8; 12]>,
    pub navaid: Vec<[u8; 8]>,
    pub unit: Vec<[u8; 8]>,
    pub poly_capacity: usize,
    pub poly_vstart: Vec<u32>,
    pub poly_vlen: Vec<u32>,
    /// The feature kind the polygon belongs to.
    pub poly_kind: Vec<u8>,
    /// Index of the feature slice owning this polygon, within the feature columns (4 bytes per
    /// polygon, about 128 KiB at the default polygon capacity).
    pub poly_feature: Vec<u32>,
    pub vtx_capacity: usize,
    pub vtx_lat: Vec<i32>,
    pub vtx_lon: Vec<i32>,
    /// The string table: `strings.len()` bytes of capacity, `strings_used` of them in use.
    pub strings: Vec<u8>,
    pub strings_used: usize,
}

/// An empty `Notams` with room for `capacity` rows, `string_bytes` bytes of text, `polygons`
/// polygons and `vertices` polygon vertices.
pub fn new_notams(
    capacity: usize,
    string_bytes: usize,
    polygons: usize,
    vertices: usize,
) -> Notams {
    new_notams_with_feature_capacity(
        capacity,
        string_bytes,
        polygons,
        vertices,
        capacity.saturating_mul(DEFAULT_FEATURES_PER_ROW),
    )
}

/// An empty `Notams` with an explicit total capacity for retained feature time slices.
///
/// Each feature slice uses 48 bytes across its struct-of-arrays columns. Capacity is total across
/// all rows; the default 16-per-row reserve is only a memory budget and does not constrain valid
/// AIXM message multiplicity.
pub fn new_notams_with_feature_capacity(
    capacity: usize,
    string_bytes: usize,
    polygons: usize,
    vertices: usize,
    feature_capacity: usize,
) -> Notams {
    Notams {
        capacity,
        len: 0,
        present: Vec::with_capacity(capacity),
        errors: Vec::with_capacity(capacity),
        flags: Vec::with_capacity(capacity),
        event_id: Vec::with_capacity(capacity),
        series: Vec::with_capacity(capacity),
        number: Vec::with_capacity(capacity),
        year: Vec::with_capacity(capacity),
        ntype: Vec::with_capacity(capacity),
        ref_series: Vec::with_capacity(capacity),
        ref_number: Vec::with_capacity(capacity),
        ref_year: Vec::with_capacity(capacity),
        location: Vec::with_capacity(capacity),
        fir: Vec::with_capacity(capacity),
        icao_location: Vec::with_capacity(capacity),
        issued_ms: Vec::with_capacity(capacity),
        start_ms: Vec::with_capacity(capacity),
        end_ms: Vec::with_capacity(capacity),
        canceled_ms: Vec::with_capacity(capacity),
        sched_off: Vec::with_capacity(capacity),
        sched_len: Vec::with_capacity(capacity),
        text_off: Vec::with_capacity(capacity),
        text_len: Vec::with_capacity(capacity),
        selection: Vec::with_capacity(capacity),
        scenario: Vec::with_capacity(capacity),
        class: Vec::with_capacity(capacity),
        traffic: Vec::with_capacity(capacity),
        purpose: Vec::with_capacity(capacity),
        scope: Vec::with_capacity(capacity),
        xover: Vec::with_capacity(capacity),
        lat_e7: Vec::with_capacity(capacity),
        lon_e7: Vec::with_capacity(capacity),
        radius_m: Vec::with_capacity(capacity),
        floor_ft: Vec::with_capacity(capacity),
        ceil_ft: Vec::with_capacity(capacity),
        floor_ref: Vec::with_capacity(capacity),
        ceil_ref: Vec::with_capacity(capacity),
        poly_first: Vec::with_capacity(capacity),
        poly_count: Vec::with_capacity(capacity),
        feature_first: Vec::with_capacity(capacity),
        feature_count: Vec::with_capacity(capacity),
        feature_capacity,
        feature_kind: Vec::with_capacity(feature_capacity),
        feature_time_kind: Vec::with_capacity(feature_capacity),
        feature_interpretation: Vec::with_capacity(feature_capacity),
        feature_meta: Vec::with_capacity(feature_capacity),
        feature_time_meta: Vec::with_capacity(feature_capacity),
        feature_errors: Vec::with_capacity(feature_capacity),
        feature_begin_ms: Vec::with_capacity(feature_capacity),
        feature_end_ms: Vec::with_capacity(feature_capacity),
        feature_sequence_number: Vec::with_capacity(feature_capacity),
        feature_correction_number: Vec::with_capacity(feature_capacity),
        feature_designator: Vec::with_capacity(feature_capacity),
        feature_designator_len: Vec::with_capacity(feature_capacity),
        feature_icao: Vec::with_capacity(feature_capacity),
        feature_icao_len: Vec::with_capacity(feature_capacity),
        feat_mask: Vec::with_capacity(capacity),
        feat_kind: Vec::with_capacity(capacity),
        interpretation: Vec::with_capacity(capacity),
        slice_meta: Vec::with_capacity(capacity),
        slice_errors: Vec::with_capacity(capacity),
        sequence_number: Vec::with_capacity(capacity),
        correction_number: Vec::with_capacity(capacity),
        feat_desig: Vec::with_capacity(capacity),
        apt: Vec::with_capacity(capacity),
        apt_icao: Vec::with_capacity(capacity),
        rwy: Vec::with_capacity(capacity),
        navaid: Vec::with_capacity(capacity),
        unit: Vec::with_capacity(capacity),
        poly_capacity: polygons,
        poly_vstart: Vec::with_capacity(polygons),
        poly_vlen: Vec::with_capacity(polygons),
        poly_kind: Vec::with_capacity(polygons),
        poly_feature: Vec::with_capacity(polygons),
        vtx_capacity: vertices,
        vtx_lat: Vec::with_capacity(vertices),
        vtx_lon: Vec::with_capacity(vertices),
        strings: vec![0; string_bytes],
        strings_used: 0,
    }
}

/// `new_notams` at the default capacities.
pub fn default_notams() -> Notams {
    new_notams(
        DEFAULT_ROW_CAPACITY,
        DEFAULT_STRING_BYTES,
        DEFAULT_POLYGONS,
        DEFAULT_VERTICES,
    )
}

/// Number of rows.
pub fn notams_len(n: &Notams) -> usize {
    n.len
}

/// Where a [`Notams`] ended at some moment: the lengths [`truncate_notams`] restores.
// Budget: one per document decoded, never in quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub rows: usize,
    pub strings_used: usize,
    pub polygons: usize,
    pub vertices: usize,
    pub features: usize,
}

/// The current lengths of `n`.
pub fn mark(n: &Notams) -> Mark {
    Mark {
        rows: n.len,
        strings_used: n.strings_used,
        polygons: n.poly_vstart.len(),
        vertices: n.vtx_lat.len(),
        features: n.feature_kind.len(),
    }
}

/// Drops everything appended since `m`; capacities are unchanged.
pub fn truncate_notams(n: &mut Notams, m: Mark) {
    n.len = m.rows;
    n.strings_used = m.strings_used;
    n.present.truncate(m.rows);
    n.errors.truncate(m.rows);
    n.flags.truncate(m.rows);
    n.event_id.truncate(m.rows);
    n.series.truncate(m.rows);
    n.number.truncate(m.rows);
    n.year.truncate(m.rows);
    n.ntype.truncate(m.rows);
    n.ref_series.truncate(m.rows);
    n.ref_number.truncate(m.rows);
    n.ref_year.truncate(m.rows);
    n.location.truncate(m.rows);
    n.fir.truncate(m.rows);
    n.icao_location.truncate(m.rows);
    n.issued_ms.truncate(m.rows);
    n.start_ms.truncate(m.rows);
    n.end_ms.truncate(m.rows);
    n.canceled_ms.truncate(m.rows);
    n.sched_off.truncate(m.rows);
    n.sched_len.truncate(m.rows);
    n.text_off.truncate(m.rows);
    n.text_len.truncate(m.rows);
    n.selection.truncate(m.rows);
    n.scenario.truncate(m.rows);
    n.class.truncate(m.rows);
    n.traffic.truncate(m.rows);
    n.purpose.truncate(m.rows);
    n.scope.truncate(m.rows);
    n.xover.truncate(m.rows);
    n.lat_e7.truncate(m.rows);
    n.lon_e7.truncate(m.rows);
    n.radius_m.truncate(m.rows);
    n.floor_ft.truncate(m.rows);
    n.ceil_ft.truncate(m.rows);
    n.floor_ref.truncate(m.rows);
    n.ceil_ref.truncate(m.rows);
    n.poly_first.truncate(m.rows);
    n.poly_count.truncate(m.rows);
    n.feature_first.truncate(m.rows);
    n.feature_count.truncate(m.rows);
    n.feat_mask.truncate(m.rows);
    n.feat_kind.truncate(m.rows);
    n.interpretation.truncate(m.rows);
    n.slice_meta.truncate(m.rows);
    n.slice_errors.truncate(m.rows);
    n.sequence_number.truncate(m.rows);
    n.correction_number.truncate(m.rows);
    n.feat_desig.truncate(m.rows);
    n.apt.truncate(m.rows);
    n.apt_icao.truncate(m.rows);
    n.rwy.truncate(m.rows);
    n.navaid.truncate(m.rows);
    n.unit.truncate(m.rows);
    n.poly_vstart.truncate(m.polygons);
    n.poly_vlen.truncate(m.polygons);
    n.poly_kind.truncate(m.polygons);
    n.poly_feature.truncate(m.polygons);
    n.feature_kind.truncate(m.features);
    n.feature_time_kind.truncate(m.features);
    n.feature_interpretation.truncate(m.features);
    n.feature_meta.truncate(m.features);
    n.feature_time_meta.truncate(m.features);
    n.feature_errors.truncate(m.features);
    n.feature_begin_ms.truncate(m.features);
    n.feature_end_ms.truncate(m.features);
    n.feature_sequence_number.truncate(m.features);
    n.feature_correction_number.truncate(m.features);
    n.feature_designator.truncate(m.features);
    n.feature_designator_len.truncate(m.features);
    n.feature_icao.truncate(m.features);
    n.feature_icao_len.truncate(m.features);
    n.vtx_lat.truncate(m.vertices);
    n.vtx_lon.truncate(m.vertices);
}

/// Drops every row, polygon and the string table contents; capacities are unchanged.
pub fn clear_notams(n: &mut Notams) {
    truncate_notams(
        n,
        Mark {
            rows: 0,
            strings_used: 0,
            polygons: 0,
            vertices: 0,
            features: 0,
        },
    );
}

/// Appends `r` as a row. Returns false, writing nothing, when the row table is full.
pub fn push_row(n: &mut Notams, r: &Row) -> bool {
    if n.len >= n.capacity {
        return false;
    }
    n.present.push(r.present);
    n.errors.push(r.errors);
    n.flags.push(r.flags);
    n.event_id.push(r.event_id);
    n.series.push(r.series);
    n.number.push(r.number);
    n.year.push(r.year);
    n.ntype.push(r.ntype);
    n.ref_series.push(r.ref_series);
    n.ref_number.push(r.ref_number);
    n.ref_year.push(r.ref_year);
    n.location.push(r.location);
    n.fir.push(r.fir);
    n.icao_location.push(r.icao_location);
    n.issued_ms.push(r.issued_ms);
    n.start_ms.push(r.start_ms);
    n.end_ms.push(r.end_ms);
    n.canceled_ms.push(r.canceled_ms);
    n.sched_off.push(r.sched_off);
    n.sched_len.push(r.sched_len);
    n.text_off.push(r.text_off);
    n.text_len.push(r.text_len);
    n.selection.push(r.selection);
    n.scenario.push(r.scenario);
    n.class.push(r.class);
    n.traffic.push(r.traffic);
    n.purpose.push(r.purpose);
    n.scope.push(r.scope);
    n.xover.push(r.xover);
    n.lat_e7.push(r.lat_e7);
    n.lon_e7.push(r.lon_e7);
    n.radius_m.push(r.radius_m);
    n.floor_ft.push(r.floor_ft);
    n.ceil_ft.push(r.ceil_ft);
    n.floor_ref.push(r.floor_ref);
    n.ceil_ref.push(r.ceil_ref);
    n.poly_first.push(r.poly_first);
    n.poly_count.push(r.poly_count);
    n.feature_first.push(r.feature_first);
    n.feature_count.push(r.feature_count);
    n.feat_mask.push(r.feat_mask);
    n.feat_kind.push(r.feat_kind);
    n.interpretation.push(r.interpretation);
    n.slice_meta.push(r.slice_meta);
    n.slice_errors.push(r.slice_errors);
    n.sequence_number.push(r.sequence_number);
    n.correction_number.push(r.correction_number);
    n.feat_desig.push(r.feat_desig);
    n.apt.push(r.apt);
    n.apt_icao.push(r.apt_icao);
    n.rwy.push(r.rwy);
    n.navaid.push(r.navaid);
    n.unit.push(r.unit);
    n.len += 1;
    true
}

/// Row `i` as a staged [`Row`], `None` past the end.
pub fn row_at(n: &Notams, i: usize) -> Option<Row> {
    Some(Row {
        present: *n.present.get(i)?,
        errors: *n.errors.get(i)?,
        issued_ms: *n.issued_ms.get(i)?,
        start_ms: *n.start_ms.get(i)?,
        end_ms: *n.end_ms.get(i)?,
        canceled_ms: *n.canceled_ms.get(i)?,
        flags: *n.flags.get(i)?,
        number: *n.number.get(i)?,
        ref_number: *n.ref_number.get(i)?,
        sched_off: *n.sched_off.get(i)?,
        sched_len: *n.sched_len.get(i)?,
        text_off: *n.text_off.get(i)?,
        text_len: *n.text_len.get(i)?,
        radius_m: *n.radius_m.get(i)?,
        poly_first: *n.poly_first.get(i)?,
        poly_count: *n.poly_count.get(i)?,
        feature_first: *n.feature_first.get(i)?,
        feature_count: *n.feature_count.get(i)?,
        feat_mask: *n.feat_mask.get(i)?,
        lat_e7: *n.lat_e7.get(i)?,
        lon_e7: *n.lon_e7.get(i)?,
        floor_ft: *n.floor_ft.get(i)?,
        ceil_ft: *n.ceil_ft.get(i)?,
        year: *n.year.get(i)?,
        ref_year: *n.ref_year.get(i)?,
        scenario: *n.scenario.get(i)?,
        ntype: *n.ntype.get(i)?,
        class: *n.class.get(i)?,
        traffic: *n.traffic.get(i)?,
        purpose: *n.purpose.get(i)?,
        scope: *n.scope.get(i)?,
        floor_ref: *n.floor_ref.get(i)?,
        ceil_ref: *n.ceil_ref.get(i)?,
        feat_kind: *n.feat_kind.get(i)?,
        interpretation: *n.interpretation.get(i)?,
        slice_meta: *n.slice_meta.get(i)?,
        slice_errors: *n.slice_errors.get(i)?,
        sequence_number: *n.sequence_number.get(i)?,
        correction_number: *n.correction_number.get(i)?,
        event_id: *n.event_id.get(i)?,
        series: *n.series.get(i)?,
        ref_series: *n.ref_series.get(i)?,
        xover: *n.xover.get(i)?,
        feat_desig: *n.feat_desig.get(i)?,
        rwy: *n.rwy.get(i)?,
        apt: *n.apt.get(i)?,
        navaid: *n.navaid.get(i)?,
        unit: *n.unit.get(i)?,
        selection: *n.selection.get(i)?,
        location: *n.location.get(i)?,
        fir: *n.fir.get(i)?,
        icao_location: *n.icao_location.get(i)?,
        apt_icao: *n.apt_icao.get(i)?,
    })
}

/// The number of feature slices retained across all rows.
pub fn feature_slices_len(n: &Notams) -> usize {
    n.feature_kind.len()
}

/// Appends one feature slice if its total capacity is available.
pub fn push_feature_slice(n: &mut Notams, f: FeatureSlice) -> Option<u32> {
    if f.reserved != [0; 2]
        || n.feature_kind.len() >= n.feature_capacity
        || n.feature_time_kind.len() != n.feature_kind.len()
        || n.feature_interpretation.len() != n.feature_kind.len()
        || n.feature_meta.len() != n.feature_kind.len()
        || n.feature_time_meta.len() != n.feature_kind.len()
        || n.feature_errors.len() != n.feature_kind.len()
        || n.feature_begin_ms.len() != n.feature_kind.len()
        || n.feature_end_ms.len() != n.feature_kind.len()
        || n.feature_sequence_number.len() != n.feature_kind.len()
        || n.feature_correction_number.len() != n.feature_kind.len()
        || n.feature_designator.len() != n.feature_kind.len()
        || n.feature_designator_len.len() != n.feature_kind.len()
        || n.feature_icao.len() != n.feature_kind.len()
        || n.feature_icao_len.len() != n.feature_kind.len()
    {
        return None;
    }
    let index = u32::try_from(n.feature_kind.len()).ok()?;
    n.feature_kind.push(f.kind);
    n.feature_time_kind.push(f.time_kind);
    n.feature_interpretation.push(f.interpretation);
    n.feature_meta.push(f.meta);
    n.feature_time_meta.push(f.time_meta);
    n.feature_errors.push(f.errors);
    n.feature_begin_ms.push(f.begin_ms);
    n.feature_end_ms.push(f.end_ms);
    n.feature_sequence_number.push(f.sequence_number);
    n.feature_correction_number.push(f.correction_number);
    n.feature_designator.push(f.designator);
    n.feature_designator_len.push(f.designator_len);
    n.feature_icao.push(f.icao);
    n.feature_icao_len.push(f.icao_len);
    Some(index)
}

/// Replaces one feature slice's staged fields without growing any output table.
pub fn set_feature_slice(n: &mut Notams, index: usize, f: FeatureSlice) -> bool {
    if f.reserved != [0; 2]
        || index >= n.feature_kind.len()
        || index >= n.feature_time_kind.len()
        || index >= n.feature_interpretation.len()
        || index >= n.feature_meta.len()
        || index >= n.feature_time_meta.len()
        || index >= n.feature_errors.len()
        || index >= n.feature_begin_ms.len()
        || index >= n.feature_end_ms.len()
        || index >= n.feature_sequence_number.len()
        || index >= n.feature_correction_number.len()
        || index >= n.feature_designator.len()
        || index >= n.feature_designator_len.len()
        || index >= n.feature_icao.len()
        || index >= n.feature_icao_len.len()
    {
        return false;
    }
    let Some(kind) = n.feature_kind.get_mut(index) else {
        return false;
    };
    *kind = f.kind;
    let Some(value) = n.feature_time_kind.get_mut(index) else {
        return false;
    };
    *value = f.time_kind;
    let Some(value) = n.feature_interpretation.get_mut(index) else {
        return false;
    };
    *value = f.interpretation;
    let Some(value) = n.feature_meta.get_mut(index) else {
        return false;
    };
    *value = f.meta;
    let Some(value) = n.feature_time_meta.get_mut(index) else {
        return false;
    };
    *value = f.time_meta;
    let Some(value) = n.feature_errors.get_mut(index) else {
        return false;
    };
    *value = f.errors;
    let Some(value) = n.feature_begin_ms.get_mut(index) else {
        return false;
    };
    *value = f.begin_ms;
    let Some(value) = n.feature_end_ms.get_mut(index) else {
        return false;
    };
    *value = f.end_ms;
    let Some(value) = n.feature_sequence_number.get_mut(index) else {
        return false;
    };
    *value = f.sequence_number;
    let Some(value) = n.feature_correction_number.get_mut(index) else {
        return false;
    };
    *value = f.correction_number;
    let Some(value) = n.feature_designator.get_mut(index) else {
        return false;
    };
    *value = f.designator;
    let Some(value) = n.feature_designator_len.get_mut(index) else {
        return false;
    };
    *value = f.designator_len;
    let Some(value) = n.feature_icao.get_mut(index) else {
        return false;
    };
    *value = f.icao;
    let Some(value) = n.feature_icao_len.get_mut(index) else {
        return false;
    };
    *value = f.icao_len;
    true
}

/// Feature slice `index` as a staged record.
pub fn feature_at(n: &Notams, index: usize) -> Option<FeatureSlice> {
    Some(FeatureSlice {
        kind: *n.feature_kind.get(index)?,
        time_kind: *n.feature_time_kind.get(index)?,
        interpretation: *n.feature_interpretation.get(index)?,
        meta: *n.feature_meta.get(index)?,
        time_meta: *n.feature_time_meta.get(index)?,
        errors: *n.feature_errors.get(index)?,
        reserved: [0; 2],
        begin_ms: *n.feature_begin_ms.get(index)?,
        end_ms: *n.feature_end_ms.get(index)?,
        sequence_number: *n.feature_sequence_number.get(index)?,
        correction_number: *n.feature_correction_number.get(index)?,
        designator: *n.feature_designator.get(index)?,
        designator_len: *n.feature_designator_len.get(index)?,
        icao: *n.feature_icao.get(index)?,
        icao_len: *n.feature_icao_len.get(index)?,
    })
}

/// The bit of `f` in a `present` or `errors` word.
pub fn field_bit(f: Field) -> u64 {
    1u64 << (f as u32)
}

const _: () = assert!(FIELD_COUNT <= 64);

/// Whether row `row` carries field `f`.
pub fn has_field(n: &Notams, row: usize, f: Field) -> bool {
    n.present.get(row).is_some_and(|w| w & field_bit(f) != 0)
}

/// Whether field `f` of row `row` was present but malformed, duplicated or inconsistent.
pub fn field_error(n: &Notams, row: usize, f: Field) -> bool {
    n.errors.get(row).is_some_and(|w| w & field_bit(f) != 0)
}

/// The text of a zero-padded code array (empty if it is not UTF-8, which parsed codes never
/// are: they are ASCII).
pub fn code_str(code: &[u8]) -> &str {
    let n = code.iter().position(|&b| b == 0).unwrap_or(code.len());
    code.get(..n)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

fn table_text<'a>(n: &'a Notams, off: Option<&u32>, len: Option<&u32>) -> &'a str {
    let (Some(&off), Some(&len)) = (off, len) else {
        return "";
    };
    let a = off as usize;
    n.strings
        .get(a..a.saturating_add(len as usize))
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

/// The NOTAM text of row `row`, empty when the row has none.
pub fn text_of(n: &Notams, row: usize) -> &str {
    if !has_field(n, row, Field::Text) {
        return "";
    }
    table_text(n, n.text_off.get(row), n.text_len.get(row))
}

/// The schedule (item D) of row `row`, empty when the row has none.
pub fn schedule_of(n: &Notams, row: usize) -> &str {
    if !has_field(n, row, Field::Schedule) {
        return "";
    }
    table_text(n, n.sched_off.get(row), n.sched_len.get(row))
}

/// The polygon rows of row `row`: a range into the `poly_*` columns.
pub fn polygons_of(n: &Notams, row: usize) -> core::ops::Range<usize> {
    let first = n.poly_first.get(row).copied().unwrap_or(0) as usize;
    let count = n.poly_count.get(row).copied().unwrap_or(0) as usize;
    first..first.saturating_add(count)
}

/// The vertices of polygon `p` as latitude and longitude slices (1e-7 degree); empty when `p`
/// is out of range.
pub fn polygon_vertices(n: &Notams, p: usize) -> (&[i32], &[i32]) {
    let (Some(&s), Some(&l)) = (n.poly_vstart.get(p), n.poly_vlen.get(p)) else {
        return (&[], &[]);
    };
    let a = s as usize;
    let b = a.saturating_add(l as usize);
    (
        n.vtx_lat.get(a..b).unwrap_or(&[]),
        n.vtx_lon.get(a..b).unwrap_or(&[]),
    )
}

/// Whether row `row` has a circle: a centre and a radius.
pub fn has_circle(n: &Notams, row: usize) -> bool {
    has_field(n, row, Field::Centre) && has_field(n, row, Field::Radius)
}

/// Whether row `row` has any geometry: a circle or at least one polygon.
pub fn has_geometry(n: &Notams, row: usize) -> bool {
    has_circle(n, row) || !polygons_of(n, row).is_empty()
}
