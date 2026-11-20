//! Replaying a recorded SFDPS stream into the flight record. Shell (RULESET B3): it reads the
//! recordings and drives the core table with the operations the adapter makes. Replay time is each
//! frame's receive time; the lag is applied by the table (`oa-flight`), so a question asked "as of" a
//! time sees only what had arrived and aged by the lag floor.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use oa_feed::Kind;
use oa_fixm::{Decoder, FIELD_COUNT};
use oa_flight::{
    flights_apply, flights_new, flights_op_avail, flights_sweep, FlightConfig, FlightTable, Op,
    Outcome, C_COUNT, OP_COUNT, REM_COUNT, R_COUNT, ST_COUNT,
};
use oa_replay::{
    doc_fixm_limits, doc_xml_limits, list_recordings, new_doc_events, spec_from_paths,
    try_merge_streams, FeedKind, MergeReport,
};
use oa_xml::Parser;

use crate::adapter::{row_ops, AdapterStats};

/// Lag histogram bounds: one-second buckets from `LAG_MIN_S` up to (excluding) `LAG_MAX_S`.
pub const LAG_MIN_S: i64 = -10;
pub const LAG_MAX_S: i64 = 300;

/// The lag of a message: receive time minus the message's own timestamp (UK-004). A7.2: cold
/// struct, size guard waived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LagHist {
    pub rows: u64,
    pub min_ms: i64,
    pub max_ms: i64,
    pub under: u64,
    pub over: u64,
    /// Rows by whole second of lag, `LAG_MIN_S` upward.
    pub buckets: Vec<u64>,
}

fn lag_new() -> LagHist {
    LagHist {
        rows: 0,
        min_ms: i64::MAX,
        max_ms: i64::MIN,
        under: 0,
        over: 0,
        buckets: vec![0; (LAG_MAX_S - LAG_MIN_S) as usize],
    }
}

/// The lag in seconds at or under which `num / den` of the rows fall (whole-second resolution,
/// rounded up to the bucket edge); `None` when there are no rows or the quantile is out of range.
pub fn lag_quantile_s(h: &LagHist, num: u64, den: u64) -> Option<i64> {
    if h.rows == 0 || den == 0 {
        return None;
    }
    let want = (u128::from(h.rows) * u128::from(num)).div_ceil(u128::from(den)) as u64;
    let mut seen = h.under;
    if seen >= want {
        return None;
    }
    for (k, c) in h.buckets.iter().enumerate() {
        seen += c;
        if seen >= want {
            return Some(LAG_MIN_S + k as i64 + 1);
        }
    }
    None
}

/// What a replay is asked to do. A7.2: cold struct, size guard waived.
#[derive(Clone, Copy, Debug)]
pub struct ReplayConfig {
    pub flight: FlightConfig,
    /// Frames received before this are skipped without being decoded; frames after `to_ns` end
    /// the run (Unix ns).
    pub from_ns: i64,
    pub to_ns: i64,
    /// How often, in replay time, the table is swept, and the retention and silence limits.
    pub sweep_every_ms: i64,
    pub retain_ms: i64,
    pub silence_ms: i64,
}

/// What a replay found. A7.2: cold struct, size guard waived.
pub struct ReplayReport {
    pub frames: u64,
    pub frames_skipped_before: u64,
    pub frames_after_end: u64,
    /// Documents the XML parser refused, and documents the decoder refused whole.
    pub docs_unparsed: u64,
    pub docs_refused: u64,
    /// Refused documents by reason: unsupported root, too many elements, input mismatch, row
    /// capacity, string capacity.
    pub refused_by: [u64; 5],
    pub first_recv_ns: i64,
    pub last_recv_ns: i64,
    pub fixm: oa_fixm::Stats,
    pub adapter: AdapterStats,
    pub lag: LagHist,
    /// Rows that have each decoder field, and the forms of the altitudes and handoff events.
    pub present: [u64; FIELD_COUNT],
    pub assigned_forms: [u64; 6],
    pub requested_forms: [u64; 6],
    pub handoff_events: [u64; 7],
    pub interim_nil: u64,
    pub interim_value: u64,
    /// Refusals by (operation, reason, state of the plan view when refused).
    pub refusals: BTreeMap<(u8, u8, u8), u64>,
    /// Plan view states at the end of the run, over every view of every flight.
    pub view_states: [u64; ST_COUNT],
    pub removed_by_reason: [u64; REM_COUNT],
    pub merge: Option<MergeReport>,
    pub table: FlightTable,
}

/// Every recording (`.oaf`, `.oaf.gz`) of a directory, sorted by name.
pub fn recordings_in(dir: &std::path::Path) -> io::Result<Vec<PathBuf>> {
    list_recordings(dir)
}

const WINDOW_END: &str = "end of the replay window";

/// Replays the SFDPS recordings `paths` into a new table. Before each operation is applied,
/// `before_op(table, avail_ms)` is called with the time the operation will become available, so
/// the caller can answer questions as of any earlier time; a last call with `i64::MAX` follows the
/// final operation.
pub fn replay_sfdps(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, i64),
) -> io::Result<ReplayReport> {
    let mut table = flights_new(&cfg.flight).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut parser =
        Parser::new(doc_xml_limits()).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut decoder = Decoder::new(doc_fixm_limits());
    let mut events = new_doc_events();
    let mut rep = ReplayReport {
        frames: 0,
        frames_skipped_before: 0,
        frames_after_end: 0,
        docs_unparsed: 0,
        docs_refused: 0,
        refused_by: [0; 5],
        first_recv_ns: i64::MAX,
        last_recv_ns: i64::MIN,
        fixm: decoder.stats().clone(),
        adapter: AdapterStats::default(),
        lag: lag_new(),
        present: [0; FIELD_COUNT],
        assigned_forms: [0; 6],
        requested_forms: [0; 6],
        handoff_events: [0; 7],
        interim_nil: 0,
        interim_value: 0,
        refusals: BTreeMap::new(),
        view_states: [0; ST_COUNT],
        removed_by_reason: [0; REM_COUNT],
        merge: None,
        table: flights_new(&FlightConfig {
            capacity: 1,
            ..cfg.flight
        })
        .map_err(|e| io::Error::other(format!("{e:?}")))?,
    };
    let mut ops: Vec<Op> = Vec::with_capacity(crate::adapter::MAX_OPS_PER_MESSAGE);
    let mut next_sweep = i64::MIN;
    let spec = spec_from_paths(FeedKind::Sfdps, paths);
    let mut ended = false;
    let res = try_merge_streams(vec![spec], 1 << 16, &mut |m| {
        if m.feed != FeedKind::Sfdps || m.kind != Kind::Message {
            return Ok(());
        }
        if m.recv_ns < cfg.from_ns {
            rep.frames_skipped_before += 1;
            return Ok(());
        }
        if m.recv_ns > cfg.to_ns {
            rep.frames_after_end += 1;
            ended = true;
            return Err(io::Error::other(WINDOW_END));
        }
        rep.frames += 1;
        rep.first_recv_ns = rep.first_recv_ns.min(m.recv_ns);
        rep.last_recv_ns = rep.last_recv_ns.max(m.recv_ns);
        let seen_ms = m.recv_ns.div_euclid(1_000_000);
        let doc = match parser.parse(m.payload) {
            Ok(d) => d,
            Err(_) => {
                rep.docs_unparsed += 1;
                return Ok(());
            }
        };
        oa_fixm::clear_events(&mut events);
        if let Err(e) = decoder.decode_prefix(&doc, m.payload, &mut events) {
            rep.docs_refused += 1;
            let k = match e {
                oa_fixm::FixmError::UnsupportedRoot => 0,
                oa_fixm::FixmError::TooManyElements { .. } => 1,
                oa_fixm::FixmError::InputMismatch => 2,
                oa_fixm::FixmError::RowCapacity { .. } => 3,
                oa_fixm::FixmError::StringCapacity { .. } => 4,
            };
            if let Some(c) = rep.refused_by.get_mut(k) {
                *c += 1;
            }
            return Ok(());
        }
        for row in 0..events.len {
            count_row(&mut rep, &events, row, seen_ms);
            ops.clear();
            let msg = m.seq << 16 | (row as u64 & 0xFFFF);
            row_ops(&events, row, seen_ms, msg, &mut rep.adapter, &mut ops);
            for o in &ops {
                let avail = flights_op_avail(&table, o);
                before_op(&table, avail);
                let out = flights_apply(&mut table, o);
                debug_assert!(matches!(
                    out,
                    Outcome::Applied | Outcome::Unchanged | Outcome::Refused(_)
                ));
            }
        }
        drain_refusals(&mut table, &mut rep);
        if table.clock_ms >= next_sweep {
            if next_sweep != i64::MIN {
                let now = table.clock_ms;
                flights_sweep(&mut table, now, cfg.retain_ms, cfg.silence_ms);
                drain_removals(&mut table, &mut rep);
            }
            next_sweep = table.clock_ms.saturating_add(cfg.sweep_every_ms);
        }
        Ok(())
    });
    match res {
        Ok(r) => rep.merge = Some(r),
        Err(e) if ended && e.to_string() == WINDOW_END => {}
        Err(e) => return Err(e),
    }
    before_op(&table, i64::MAX);
    drain_refusals(&mut table, &mut rep);
    drain_removals(&mut table, &mut rep);
    for f in table
        .flights
        .iter()
        .filter(|f| f.key.kind != oa_flight::KEY_FREE)
    {
        for v in f.views.iter().take(usize::from(f.n_views)) {
            if let Some(c) = rep.view_states.get_mut(usize::from(v.state)) {
                *c += 1;
            }
        }
    }
    rep.fixm = decoder.stats().clone();
    rep.table = table;
    Ok(rep)
}

fn count_row(rep: &mut ReplayReport, ev: &oa_fixm::Events, row: usize, seen_ms: i64) {
    use oa_fixm::{has_field, Field, ALL_FIELDS};
    for (k, f) in ALL_FIELDS.iter().enumerate() {
        if has_field(ev, row, *f) {
            if let Some(c) = rep.present.get_mut(k) {
                *c += 1;
            }
        }
    }
    let bump = |a: &mut [u64], i: u8| {
        if let Some(c) = a.get_mut(usize::from(i)) {
            *c += 1;
        }
    };
    if has_field(ev, row, Field::AssignedForm) {
        bump(
            &mut rep.assigned_forms,
            ev.assigned_form.get(row).copied().unwrap_or(0),
        );
    }
    if has_field(ev, row, Field::RequestedForm) {
        bump(
            &mut rep.requested_forms,
            ev.requested_form.get(row).copied().unwrap_or(0),
        );
    }
    if has_field(ev, row, Field::HandoffEvent) {
        bump(
            &mut rep.handoff_events,
            ev.handoff_event.get(row).copied().unwrap_or(0),
        );
    }
    if has_field(ev, row, Field::InterimAlt) {
        if ev
            .flags
            .get(row)
            .is_some_and(|f| f & oa_fixm::FLAG_INTERIM_NIL != 0)
        {
            rep.interim_nil += 1;
        } else {
            rep.interim_value += 1;
        }
    }
    if let Some(&ts) = ev.ts_ms.get(row) {
        let lag = seen_ms - ts;
        let h = &mut rep.lag;
        h.rows += 1;
        h.min_ms = h.min_ms.min(lag);
        h.max_ms = h.max_ms.max(lag);
        let s = lag.div_euclid(1000);
        if s < LAG_MIN_S {
            h.under += 1;
        } else if s >= LAG_MAX_S {
            h.over += 1;
        } else if let Some(c) = h.buckets.get_mut((s - LAG_MIN_S) as usize) {
            *c += 1;
        }
    }
}

fn drain_refusals(t: &mut FlightTable, rep: &mut ReplayReport) {
    for r in &t.refusals {
        *rep.refusals.entry((r.op, r.reason, r.state)).or_default() += 1;
    }
    t.refusals.clear();
    t.conflicts.clear();
}

fn drain_removals(t: &mut FlightTable, rep: &mut ReplayReport) {
    for r in &t.removals {
        if let Some(c) = rep.removed_by_reason.get_mut(usize::from(r.reason)) {
            *c += 1;
        }
    }
    t.removals.clear();
}

/// The number of refusal reasons, for tables the report prints.
pub const REASONS: usize = R_COUNT;
/// The number of operation codes.
pub const OPS: usize = OP_COUNT;
/// The number of conflict kinds.
pub const CONFLICTS: usize = C_COUNT;
