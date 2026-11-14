//! The output of the decoder: one row per metering section, in a caller-owned struct-of-arrays.
//! Plain data (RULESET A1, A3): [`Row`] is one staged record, [`Rows`] the columns. Behaviour is
//! in the free functions below.
//!
//! One row is one section element of one `air` element: a `flt`, `mrp`, `eta`, `sta` or `sch`
//! (the [`section`] code says which). TBFM sends changes, not state: an amendment carries only
//! the sections that changed, so most columns of most rows are absent. Merging rows into a
//! per-flight state is the consumer's job (it needs the message order, which the feed owns).
//!
//! Absent values: a name column is all zero bytes, a time column is [`NO_TIME`], a number column
//! is its `NONE` constant and a code column is 0.

/// A short ASCII name, zero-padded on the right; all zero means absent. Widths come from the
/// longest value in the 12,760-message survey (docs/TBFM_SURVEY.md) plus headroom; a longer
/// value is a field error, never truncated.
pub type Name4 = [u8; 4];
pub type Name8 = [u8; 8];
pub type Name12 = [u8; 12];
pub type Name14 = [u8; 14];
pub type Name16 = [u8; 16];
pub type Name20 = [u8; 20];
pub type Name24 = [u8; 24];

/// Absent time: Unix milliseconds can never be this.
pub const NO_TIME: i64 = i64::MIN;
/// Absent beacon code.
pub const NO_BCN: u16 = u16::MAX;
/// Absent speed or altitude.
pub const NO_NUM: i32 = i32::MIN;

/// Section codes ([`Row::section`]).
pub mod section {
    pub const FLT: u8 = 1;
    pub const MRP: u8 = 2;
    pub const ETA: u8 = 3;
    pub const STA: u8 = 4;
    pub const SCH: u8 = 5;
}

/// Change-type codes ([`Row::air_type`], [`Row::sect_type`]): the wire values `NEW`, `AMD`, `DEL`.
pub mod change {
    pub const NONE: u8 = 0;
    pub const NEW: u8 = 1;
    pub const AMD: u8 = 2;
    pub const DEL: u8 = 3;
}

/// Index into [`Row::eta_ms`] and [`Row::sta_ms`]: the eight places a time is given for.
pub mod pt {
    pub const DFX: usize = 0;
    pub const SFX: usize = 1;
    pub const MFX: usize = 2;
    pub const OMA: usize = 3;
    pub const OOA: usize = 4;
    pub const O3A: usize = 5;
    pub const O4A: usize = 6;
    pub const RWY: usize = 7;
    pub const COUNT: usize = 8;
}

/// Schedule flag positions in [`Row::sch`]: two bits each, 0 absent, 1 `NO`, 2 `YES`.
pub mod sch_flag {
    pub const MAN: u8 = 0;
    pub const RFZ: u8 = 1;
    pub const SFZ: u8 = 2;
    pub const SUS: u8 = 3;
    pub const ABSENT: u8 = 0;
    pub const NO: u8 = 1;
    pub const YES: u8 = 2;
}

/// One metering row. See the module comment; the columns are named for the wire elements and
/// attributes they come from (docs/TBFM_SURVEY.md says what each appears to mean).
// Budget: one per metering section, held in quantity (the SoA columns below store the same
// fields); the exact packed size of its fields. A7.1: the name widths were chosen so that no
// padding is hidden (480 is a multiple of 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Row {
    /// `eta_*` times (Unix ms), indexed by [`pt`].
    pub eta_ms: [i64; pt::COUNT],
    /// `sta_*` times (Unix ms), indexed by [`pt`]; the `MFX` slot is never filled (the feed has
    /// no `sta_mfx`).
    pub sta_ms: [i64; pt::COUNT],
    pub env_ms: i64,
    pub msg_ms: i64,
    pub ctm_ms: i64,
    pub etd_ms: i64,
    pub std_ms: i64,
    /// Bit `f as u8` is set when the element for [`crate::Field`] `f` was sent empty. TBFM sends
    /// changes, so empty and absent differ: an empty element in an amendment appears to clear
    /// the value (unconfirmed, docs/TBFM_SURVEY.md); the value column itself stays absent.
    pub cleared: u64,
    /// Speed in thousandths of the (unconfirmed) unit.
    pub spd_milli: i32,
    pub ina: i32,
    pub msg_id: u32,
    /// Transponder code as the number its four octal digits spell.
    pub bcn: u16,
    pub section: u8,
    pub air_type: u8,
    pub sect_type: u8,
    pub sch: u8,
    /// Centre identifier from `envSrce` (`TMA.<centre>.FAA.GOV`).
    pub center: Name4,
    pub tma_id: Name8,
    pub gufi: Name12,
    pub cid: Name4,
    pub aid: Name8,
    pub apt: Name4,
    pub dap: Name4,
    pub mfx: Name8,
    pub tra: Name4,
    pub dfx: Name8,
    pub sfx: Name8,
    pub oma: Name12,
    pub ooa: Name12,
    pub o3a: Name12,
    pub o4a: Name12,
    pub gat: Name16,
    pub rwy: Name4,
    pub cfg: Name20,
    pub scn: Name20,
    pub cat: Name24,
    pub fps: Name12,
    pub acs: Name12,
    pub typ: Name8,
    pub eng: Name12,
    pub ara: Name8,
    pub trw: Name4,
    pub drw: Name8,
    pub tds: Name4,
    pub cfx: Name14,
}
const _: () = assert!(core::mem::size_of::<Row>() == 480);

/// A row with every value absent.
pub const BLANK_ROW: Row = Row {
    eta_ms: [NO_TIME; pt::COUNT],
    sta_ms: [NO_TIME; pt::COUNT],
    env_ms: NO_TIME,
    msg_ms: NO_TIME,
    ctm_ms: NO_TIME,
    etd_ms: NO_TIME,
    std_ms: NO_TIME,
    cleared: 0,
    spd_milli: NO_NUM,
    ina: NO_NUM,
    msg_id: 0,
    bcn: NO_BCN,
    section: 0,
    air_type: change::NONE,
    sect_type: change::NONE,
    sch: 0,
    center: [0; 4],
    tma_id: [0; 8],
    gufi: [0; 12],
    cid: [0; 4],
    aid: [0; 8],
    apt: [0; 4],
    dap: [0; 4],
    mfx: [0; 8],
    tra: [0; 4],
    dfx: [0; 8],
    sfx: [0; 8],
    oma: [0; 12],
    ooa: [0; 12],
    o3a: [0; 12],
    o4a: [0; 12],
    gat: [0; 16],
    rwy: [0; 4],
    cfg: [0; 20],
    scn: [0; 20],
    cat: [0; 24],
    fps: [0; 12],
    acs: [0; 12],
    typ: [0; 8],
    eng: [0; 12],
    ara: [0; 8],
    trw: [0; 4],
    drw: [0; 8],
    tds: [0; 4],
    cfx: [0; 14],
};

/// The rows, one `Vec` per column sharing an index. Created once by [`new_rows`] with a stated
/// capacity (RULESET C2) and reused after [`clear_rows`]; every column's capacity is reserved
/// up front, so a push within `cap` never allocates (C1). Reaching `cap` is the
/// [`crate::DocError::Capacity`] reject, never a reallocation.
// A7.2: cold struct, size guard waived (one per decoder; the hot record is `Row`)
#[derive(Clone, Debug)]
pub struct Rows {
    pub cap: usize,
    pub eta_ms: [Vec<i64>; pt::COUNT],
    pub sta_ms: [Vec<i64>; pt::COUNT],
    pub env_ms: Vec<i64>,
    pub msg_ms: Vec<i64>,
    pub ctm_ms: Vec<i64>,
    pub etd_ms: Vec<i64>,
    pub std_ms: Vec<i64>,
    pub cleared: Vec<u64>,
    pub spd_milli: Vec<i32>,
    pub ina: Vec<i32>,
    pub msg_id: Vec<u32>,
    pub bcn: Vec<u16>,
    pub section: Vec<u8>,
    pub air_type: Vec<u8>,
    pub sect_type: Vec<u8>,
    pub sch: Vec<u8>,
    pub center: Vec<Name4>,
    pub tma_id: Vec<Name8>,
    pub gufi: Vec<Name12>,
    pub cid: Vec<Name4>,
    pub aid: Vec<Name8>,
    pub apt: Vec<Name4>,
    pub dap: Vec<Name4>,
    pub mfx: Vec<Name8>,
    pub tra: Vec<Name4>,
    pub dfx: Vec<Name8>,
    pub sfx: Vec<Name8>,
    pub oma: Vec<Name12>,
    pub ooa: Vec<Name12>,
    pub o3a: Vec<Name12>,
    pub o4a: Vec<Name12>,
    pub gat: Vec<Name16>,
    pub rwy: Vec<Name4>,
    pub cfg: Vec<Name20>,
    pub scn: Vec<Name20>,
    pub cat: Vec<Name24>,
    pub fps: Vec<Name12>,
    pub acs: Vec<Name12>,
    pub typ: Vec<Name8>,
    pub eng: Vec<Name12>,
    pub ara: Vec<Name8>,
    pub trw: Vec<Name4>,
    pub drw: Vec<Name8>,
    pub tds: Vec<Name4>,
    pub cfx: Vec<Name14>,
}

/// Rows per message the decoder is sized for by default. Analysis: the 12,561 sampled messages
/// carry one `air` each with at most four sections (a flight with up to four meter points); a
/// message is at most 1 MiB (the parser limit) and a section needs at least ten bytes, so 256
/// is 64 times the observed peak and still a small allocation (about 120 KB).
pub const DEFAULT_ROW_CAPACITY: usize = 256;

/// Allocates empty columns able to hold `cap` rows.
pub fn new_rows(cap: usize) -> Rows {
    fn v<T>(cap: usize) -> Vec<T> {
        Vec::with_capacity(cap)
    }
    Rows {
        cap,
        eta_ms: std::array::from_fn(|_| v(cap)),
        sta_ms: std::array::from_fn(|_| v(cap)),
        env_ms: v(cap),
        msg_ms: v(cap),
        ctm_ms: v(cap),
        etd_ms: v(cap),
        std_ms: v(cap),
        cleared: v(cap),
        spd_milli: v(cap),
        ina: v(cap),
        msg_id: v(cap),
        bcn: v(cap),
        section: v(cap),
        air_type: v(cap),
        sect_type: v(cap),
        sch: v(cap),
        center: v(cap),
        tma_id: v(cap),
        gufi: v(cap),
        cid: v(cap),
        aid: v(cap),
        apt: v(cap),
        dap: v(cap),
        mfx: v(cap),
        tra: v(cap),
        dfx: v(cap),
        sfx: v(cap),
        oma: v(cap),
        ooa: v(cap),
        o3a: v(cap),
        o4a: v(cap),
        gat: v(cap),
        rwy: v(cap),
        cfg: v(cap),
        scn: v(cap),
        cat: v(cap),
        fps: v(cap),
        acs: v(cap),
        typ: v(cap),
        eng: v(cap),
        ara: v(cap),
        trw: v(cap),
        drw: v(cap),
        tds: v(cap),
        cfx: v(cap),
    }
}

/// Number of rows held.
pub fn rows_len(r: &Rows) -> usize {
    r.section.len()
}

/// Drops every row, keeping the allocations.
pub fn clear_rows(r: &mut Rows) {
    truncate_rows(r, 0);
}

/// Keeps the first `n` rows.
pub fn truncate_rows(r: &mut Rows, n: usize) {
    for c in r.eta_ms.iter_mut().chain(r.sta_ms.iter_mut()) {
        c.truncate(n);
    }
    r.env_ms.truncate(n);
    r.msg_ms.truncate(n);
    r.ctm_ms.truncate(n);
    r.etd_ms.truncate(n);
    r.std_ms.truncate(n);
    r.cleared.truncate(n);
    r.spd_milli.truncate(n);
    r.ina.truncate(n);
    r.msg_id.truncate(n);
    r.bcn.truncate(n);
    r.section.truncate(n);
    r.air_type.truncate(n);
    r.sect_type.truncate(n);
    r.sch.truncate(n);
    r.center.truncate(n);
    r.tma_id.truncate(n);
    r.gufi.truncate(n);
    r.cid.truncate(n);
    r.aid.truncate(n);
    r.apt.truncate(n);
    r.dap.truncate(n);
    r.mfx.truncate(n);
    r.tra.truncate(n);
    r.dfx.truncate(n);
    r.sfx.truncate(n);
    r.oma.truncate(n);
    r.ooa.truncate(n);
    r.o3a.truncate(n);
    r.o4a.truncate(n);
    r.gat.truncate(n);
    r.rwy.truncate(n);
    r.cfg.truncate(n);
    r.scn.truncate(n);
    r.cat.truncate(n);
    r.fps.truncate(n);
    r.acs.truncate(n);
    r.typ.truncate(n);
    r.eng.truncate(n);
    r.ara.truncate(n);
    r.trw.truncate(n);
    r.drw.truncate(n);
    r.tds.truncate(n);
    r.cfx.truncate(n);
}

/// Appends `row`; false (and nothing written) when the rows are at capacity.
pub fn push_row(r: &mut Rows, row: &Row) -> bool {
    if rows_len(r) >= r.cap {
        return false;
    }
    for (c, v) in r.eta_ms.iter_mut().zip(row.eta_ms) {
        c.push(v);
    }
    for (c, v) in r.sta_ms.iter_mut().zip(row.sta_ms) {
        c.push(v);
    }
    r.env_ms.push(row.env_ms);
    r.msg_ms.push(row.msg_ms);
    r.ctm_ms.push(row.ctm_ms);
    r.etd_ms.push(row.etd_ms);
    r.std_ms.push(row.std_ms);
    r.cleared.push(row.cleared);
    r.spd_milli.push(row.spd_milli);
    r.ina.push(row.ina);
    r.msg_id.push(row.msg_id);
    r.bcn.push(row.bcn);
    r.section.push(row.section);
    r.air_type.push(row.air_type);
    r.sect_type.push(row.sect_type);
    r.sch.push(row.sch);
    r.center.push(row.center);
    r.tma_id.push(row.tma_id);
    r.gufi.push(row.gufi);
    r.cid.push(row.cid);
    r.aid.push(row.aid);
    r.apt.push(row.apt);
    r.dap.push(row.dap);
    r.mfx.push(row.mfx);
    r.tra.push(row.tra);
    r.dfx.push(row.dfx);
    r.sfx.push(row.sfx);
    r.oma.push(row.oma);
    r.ooa.push(row.ooa);
    r.o3a.push(row.o3a);
    r.o4a.push(row.o4a);
    r.gat.push(row.gat);
    r.rwy.push(row.rwy);
    r.cfg.push(row.cfg);
    r.scn.push(row.scn);
    r.cat.push(row.cat);
    r.fps.push(row.fps);
    r.acs.push(row.acs);
    r.typ.push(row.typ);
    r.eng.push(row.eng);
    r.ara.push(row.ara);
    r.trw.push(row.trw);
    r.drw.push(row.drw);
    r.tds.push(row.tds);
    r.cfx.push(row.cfx);
    true
}

/// Row `i` rebuilt from the columns; `None` when out of range.
pub fn row_at(r: &Rows, i: usize) -> Option<Row> {
    let mut o = BLANK_ROW;
    for (p, c) in r.eta_ms.iter().enumerate() {
        *o.eta_ms.get_mut(p)? = *c.get(i)?;
    }
    for (p, c) in r.sta_ms.iter().enumerate() {
        *o.sta_ms.get_mut(p)? = *c.get(i)?;
    }
    o.env_ms = *r.env_ms.get(i)?;
    o.msg_ms = *r.msg_ms.get(i)?;
    o.ctm_ms = *r.ctm_ms.get(i)?;
    o.etd_ms = *r.etd_ms.get(i)?;
    o.std_ms = *r.std_ms.get(i)?;
    o.cleared = *r.cleared.get(i)?;
    o.spd_milli = *r.spd_milli.get(i)?;
    o.ina = *r.ina.get(i)?;
    o.msg_id = *r.msg_id.get(i)?;
    o.bcn = *r.bcn.get(i)?;
    o.section = *r.section.get(i)?;
    o.air_type = *r.air_type.get(i)?;
    o.sect_type = *r.sect_type.get(i)?;
    o.sch = *r.sch.get(i)?;
    o.center = *r.center.get(i)?;
    o.tma_id = *r.tma_id.get(i)?;
    o.gufi = *r.gufi.get(i)?;
    o.cid = *r.cid.get(i)?;
    o.aid = *r.aid.get(i)?;
    o.apt = *r.apt.get(i)?;
    o.dap = *r.dap.get(i)?;
    o.mfx = *r.mfx.get(i)?;
    o.tra = *r.tra.get(i)?;
    o.dfx = *r.dfx.get(i)?;
    o.sfx = *r.sfx.get(i)?;
    o.oma = *r.oma.get(i)?;
    o.ooa = *r.ooa.get(i)?;
    o.o3a = *r.o3a.get(i)?;
    o.o4a = *r.o4a.get(i)?;
    o.gat = *r.gat.get(i)?;
    o.rwy = *r.rwy.get(i)?;
    o.cfg = *r.cfg.get(i)?;
    o.scn = *r.scn.get(i)?;
    o.cat = *r.cat.get(i)?;
    o.fps = *r.fps.get(i)?;
    o.acs = *r.acs.get(i)?;
    o.typ = *r.typ.get(i)?;
    o.eng = *r.eng.get(i)?;
    o.ara = *r.ara.get(i)?;
    o.trw = *r.trw.get(i)?;
    o.drw = *r.drw.get(i)?;
    o.tds = *r.tds.get(i)?;
    o.cfx = *r.cfx.get(i)?;
    Some(o)
}

/// The text of a name column value: the bytes up to the first zero, as `&str` (names are
/// printable ASCII, checked on decode; anything else shows as `?`).
pub fn name_str(n: &[u8]) -> &str {
    let end = n.iter().position(|&b| b == 0).unwrap_or(n.len());
    n.get(..end)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("?")
}

/// The value of schedule flag `flag` ([`sch_flag`]) in `sch`: 0 absent, 1 `NO`, 2 `YES`.
pub fn sch_get(sch: u8, flag: u8) -> u8 {
    (sch >> (flag * 2)) & 3
}

/// Whether the element for `f` was sent empty in a row with the given `cleared` mask.
pub fn is_cleared(cleared: u64, f: crate::Field) -> bool {
    (cleared >> (f as u8)) & 1 == 1
}
