//! The pipeline: feed frames in, one deterministic flight state out. Core (pure, B1): a function
//! of the frames it is given. No I/O and no clock; replay time is each frame's own
//! `recv_unix_ns` (B4), so a recording replays bit for bit (B6).
//!
//! A frame goes through four steps, each counted and none skipped silently:
//!
//! 1. the replay clock moves to the frame's time under the step rules of [`crate::clock`] (a
//!    frame earlier than one already seen of its feed is counted as a time regression and the
//!    clock stays; a time more than 2 minutes from the clock and from its feed's last frame is
//!    held until three frames in a row confirm it, and a confirmed step marks every row stale
//!    until it is heard again), the tables' eviction cadences run ([`advance_time`]), and a checkpoint is recorded
//!    if one is due;
//! 2. only MESSAGE frames carry feed data; REJECT and STATUS frames are counted;
//! 3. the payload is parsed with oa-xml and decoded with oa-fixm (SFDPS), oa-tbfm (TBFM),
//!    oa-tfms (TFMS) or oa-notam (FNS), or decoded with `oa_adsb::decode_payload_lenient` (ADS-B:
//!    refused whole when its header or length is wrong; a record that cannot be read is dropped
//!    and counted, the rest is kept); every failure is counted under its [`ErrKind`]. A document with more
//!    rows or route text than the row tables hold is decoded as far as it fits and the rest is
//!    counted (SFDPS, TBFM and TFMS; FNS decoding is as it was);
//! 4. each decoded row is applied to the flight table ([`apply_sfdps`], [`apply_tbfm`]) or, for
//!    ADS-B, TFMS and FNS, to the table of its own ([`apply_adsb`], [`apply_tfms_flight`],
//!    [`apply_tfms_flow`], [`apply_fns`]).
//!
//! The ADS-B, TFMS and FNS tables are part of the state hash only once a frame of their feed has
//! been fed ([`replay_hash`]).
//!
//! Checkpoints fall on multiples of `checkpoint_ns` of replay time: the first frame at or after
//! a boundary records the state as it stood before that frame (after the table's own sweep at
//! that time), stamped with that frame's replay time. A long gap with no frames records one
//! checkpoint, since the state did not change inside it.
//!
//! Memory: the parser, decoders, event and row tables, the flight table and the checkpoint ring
//! are all allocated in [`new_pipeline`]; nothing grows with the length of the run (C5). The
//! ring holds `checkpoint_ring` checkpoints for the consumer to take with
//! [`pipeline_drain_checkpoints`]; when it is full the oldest undrained one is dropped and
//! counted ([`CheckpointRing::dropped`]).

use oa_adsb::{decode_payload_lenient, reports_clear, reports_len, PayloadError, Reports};
use oa_feed::Kind;
use oa_fixm::{clear_events, new_events, Decoder, Events, FixmError, Limits, REJECT_COUNT};
use oa_notam::{
    clear_notams, new_decoder, new_stats, notams_len, DocError as FnsDocError,
    Limits as NotamLimits, Notams,
};
use oa_sha256::{Digest, Sha256};
use oa_tbfm::{clear_rows, decode_doc_prefix, new_rows, rows_len, DocError, Rows, MSG_KIND_COUNT};
use oa_tfms::{
    clear_rows as clear_tfms_rows, decode_doc_prefix as decode_tfms_doc, flights_len, flows_len,
    new_rows as new_tfms_rows, DocError as TfmsDocError, Rows as TfmsRows, DEFAULT_FLIGHT_CAPACITY,
    DEFAULT_FLOW_CAPACITY, DEFAULT_STRING_BYTES,
};
use oa_xml::Parser;

use crate::adsb::{
    adsb_advance, adsb_hash_into, adsb_shift_times, adsb_stale_count, apply_adsb, new_adsb_table,
    AdsbApplied, AdsbTable, ADSB_CAPACITY,
};
use crate::apply::{apply_sfdps, apply_tbfm, Applied};
use crate::clock::{clock_observe, Clock, Tick};
use crate::errors::{count_error, sfdps_reject_kind, ErrKind, Errors, ERR_COUNT};
use crate::fns::{
    apply_fns, fns_hash_into, fns_shift_times, fns_stale_count, fns_sweep, new_fns_table,
    FnsApplied, FnsTable, FNS_CAPACITY,
};
use crate::hash::state_hash;
use crate::table::{
    advance_time, new_table, shift_times, stale_count, ConfigError, FlightTable, TableConfig,
    NS_PER_S,
};
use crate::tfms::{
    apply_tfms_flight, apply_tfms_flow, flow_hash_into, flow_shift_times, flow_stale_count,
    flow_sweep, new_flow_table, new_tfms_table, tfms_due, tfms_hash_into, tfms_shift_times,
    tfms_stale_count, tfms_sweep, FlowApplied, FlowTable, TfmsApplied, TfmsTable, FLOW_CAPACITY,
    TFMS_CAPACITY,
};

/// Which feed a frame came from; chosen by the caller, never guessed from the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FeedKind {
    /// SWIM SFDPS flight messages.
    Sfdps = 0,
    /// SWIM TBFM metering publication.
    Tbfm = 1,
    /// SWIM TFMS flight and flow data (`oa-tfms`).
    Tfms = 2,
    /// SWIM FNS notices (`oa-notam`).
    Fns = 3,
    /// ADS-B position reports (`docs/ADSB_PAYLOAD.md`). Sorts after every SWIM feed in the merge
    /// tie-break (OA-RPL-021): at an equal receive time, SWIM frames come first.
    Adsb = 4,
}

/// Number of [`FeedKind`] values.
pub const FEED_COUNT: usize = 5;

/// Display names, indexed by `FeedKind as usize`.
pub const FEED_NAMES: [&str; FEED_COUNT] = ["sfdps", "tbfm", "tfms", "fns", "adsb"];

/// Default spacing of checkpoints: 10 s of replay time.
pub const DEFAULT_CHECKPOINT_NS: i64 = 10 * NS_PER_S;
/// Smallest checkpoint spacing: 1 s. Each checkpoint hashes every table, so a spacing under the
/// gap between frames (the SFDPS feed's frames come at most 0.5 s apart) would hash the whole
/// state on every frame; a second is below any spacing the measurements want and keeps a run's
/// cost bounded by its replay time.
pub const MIN_CHECKPOINT_NS: i64 = NS_PER_S;
/// Default checkpoints the ring holds: 24 hours at the default spacing.
pub const DEFAULT_CHECKPOINT_RING: usize = 8_640;
/// Rows the SFDPS event table and the TBFM row table hold for one document. The measured feeds
/// carry at most 100 messages per SFDPS document. A document with more is decoded as far as it
/// fits, never refused whole, and the messages or rows past that point are counted
/// ([`ErrKind::SfdpsRowCapacity`], [`ErrKind::SfdpsStringCapacity`], [`ErrKind::TbfmCapacity`],
/// [`ErrKind::TfmsCapacity`]).
pub const DOC_ROW_CAPACITY: usize = 4096;
/// Elements, and text nodes, of one document. A frame carries at most `oa_feed::MAX_PAYLOAD`
/// (4 MiB), so the parser accepts any payload a frame can hold, and the largest document the
/// measured feeds have shown (3,000 elements) is a hundredth of this.
pub const DOC_MAX_ELEMENTS: usize = 262_144;
/// Attributes of one document.
pub const DOC_MAX_ATTRIBUTES: usize = 524_288;
/// Bytes of route text one SFDPS document may add.
pub const DOC_STRING_BYTES: usize = 1 << 20;

/// The XML parser limits of one feed document. The one definition of what a document may hold:
/// the pipeline and the benchmark's legacy extraction both build their parser from it, so the
/// two accept the same files.
pub fn doc_xml_limits() -> oa_xml::Limits {
    oa_xml::Limits {
        max_input_bytes: oa_feed::MAX_PAYLOAD,
        max_elements: DOC_MAX_ELEMENTS,
        max_attributes: DOC_MAX_ATTRIBUTES,
        max_text_nodes: DOC_MAX_ELEMENTS,
        ..oa_xml::Limits::default()
    }
}

/// The SFDPS decoder limits of one document (see [`doc_xml_limits`]).
pub fn doc_fixm_limits() -> Limits {
    Limits {
        max_elements: DOC_MAX_ELEMENTS,
        ..Limits::default()
    }
}

/// The SFDPS event table of one document (see [`doc_xml_limits`]). Documents are decoded with
/// `decode_prefix`: as far as this table fits, the rest counted.
pub fn new_doc_events() -> Events {
    new_events(DOC_ROW_CAPACITY, DOC_STRING_BYTES)
}

/// The TBFM row table of one document.
pub fn new_doc_tbfm_rows() -> Rows {
    new_rows(DOC_ROW_CAPACITY)
}

/// The TFMS row tables of one document.
pub fn new_doc_tfms_rows() -> TfmsRows {
    new_tfms_rows(
        DEFAULT_FLIGHT_CAPACITY,
        DEFAULT_FLOW_CAPACITY,
        DEFAULT_STRING_BYTES,
    )
}

/// Pipeline settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineConfig {
    /// The flight table's settings.
    pub table: TableConfig,
    /// Replay-time spacing of checkpoints, ns.
    pub checkpoint_ns: i64,
    /// Checkpoints the ring holds before the oldest undrained one is dropped; at least 1.
    pub checkpoint_ring: usize,
}

impl Default for PipelineConfig {
    fn default() -> PipelineConfig {
        PipelineConfig {
            table: TableConfig::default(),
            checkpoint_ns: DEFAULT_CHECKPOINT_NS,
            checkpoint_ring: DEFAULT_CHECKPOINT_RING,
        }
    }
}

/// A recorded point of the run. Plain data (A1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    /// Replay time, ns since the Unix epoch.
    pub replay_ns: i64,
    /// Frames fed so far, all kinds, before this point.
    pub frames: u64,
    /// Rows applied so far (both feeds).
    pub rows_applied: u64,
    /// Flights in the table.
    pub flights: u64,
    /// SHA-256 of the canonical table state.
    pub hash: Digest,
}

/// A fixed ring of checkpoints, allocated once. Pushing onto a full ring drops the oldest
/// undrained checkpoint and counts it. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct CheckpointRing {
    buf: Vec<Checkpoint>,
    head: usize,
    len: usize,
    dropped: u64,
    recorded: u64,
}

fn empty_checkpoint() -> Checkpoint {
    Checkpoint {
        replay_ns: 0,
        frames: 0,
        rows_applied: 0,
        flights: 0,
        hash: [0; 32],
    }
}

impl CheckpointRing {
    fn new(capacity: usize) -> CheckpointRing {
        CheckpointRing {
            buf: vec![empty_checkpoint(); capacity],
            head: 0,
            len: 0,
            dropped: 0,
            recorded: 0,
        }
    }

    /// Checkpoints held now, not yet drained.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no checkpoint is held.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Checkpoints the ring can hold; fixed at construction.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Checkpoints dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Checkpoints recorded in all, drained, dropped or held.
    pub fn recorded(&self) -> u64 {
        self.recorded
    }

    /// The `i`th held checkpoint, oldest first.
    pub fn get(&self, i: usize) -> Option<&Checkpoint> {
        if i >= self.len || self.buf.is_empty() {
            return None;
        }
        self.buf.get((self.head + i) % self.buf.len())
    }

    /// The held checkpoints, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Checkpoint> + '_ {
        (0..self.len).filter_map(|i| self.get(i))
    }

    /// A copy of the held checkpoints, oldest first (allocates: for tests and reports).
    pub fn to_vec(&self) -> Vec<Checkpoint> {
        self.iter().copied().collect()
    }

    fn push(&mut self, c: Checkpoint) {
        let cap = self.buf.len();
        if cap == 0 {
            self.dropped += 1;
            self.recorded += 1;
            return;
        }
        if self.len == cap {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
            self.dropped += 1;
        }
        let at = (self.head + self.len) % cap;
        if let Some(slot) = self.buf.get_mut(at) {
            *slot = c;
            self.len += 1;
        }
        self.recorded += 1;
    }

    fn pop_oldest(&mut self) {
        if self.len > 0 && !self.buf.is_empty() {
            self.head = (self.head + 1) % self.buf.len();
            self.len -= 1;
        }
    }
}

/// Why a pipeline could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineError {
    /// The table settings were refused.
    Config(ConfigError),
    /// `checkpoint_ns` is below [`MIN_CHECKPOINT_NS`].
    Checkpoint(i64),
    /// `checkpoint_ring` is 0.
    CheckpointRing(usize),
    /// The XML parser could not be built.
    Parser,
}

/// The pipeline and everything it counts. A subsystem under D5: it owns the parser, decoders and
/// table.
pub struct Pipeline {
    /// Settings.
    pub cfg: PipelineConfig,
    /// The flight state.
    pub table: FlightTable,
    /// The ADS-B aircraft table (empty, and left out of the state hash, until an ADS-B frame is
    /// fed).
    pub adsb: AdsbTable,
    /// The TFMS flight table (empty, and left out of the state hash, until a TFMS frame is fed).
    pub tfms: TfmsTable,
    /// The TFMS programme table (same rule as `tfms`).
    pub flows: FlowTable,
    /// The NOTAMs in force (empty, and left out of the state hash, until an FNS frame is fed).
    pub fns: FnsTable,
    parser: Parser,
    fixm: Decoder,
    events: Events,
    rows: Rows,
    reports: Reports,
    tfms_rows: TfmsRows,
    notam_dec: oa_notam::Decoder,
    notam_stats: oa_notam::Stats,
    notams: Notams,
    /// Replay time now, ns; `i64::MIN` before the first frame.
    pub replay_ns: i64,
    /// The clock's step rules: frames held and steps confirmed ([`clock_observe`]).
    pub clock: Clock,
    /// Rows marked stale by confirmed steps of the clock, all tables, cumulative: each was last
    /// updated before a step and its age was shifted ([`pipeline_stale_count`] is how many still
    /// are).
    pub stale_marked: u64,
    next_checkpoint_ns: i64,
    /// Checkpoints recorded and not yet drained ([`pipeline_drain_checkpoints`]).
    pub checkpoints: CheckpointRing,
    /// Frames fed per feed (every kind of frame).
    pub frames: [u64; FEED_COUNT],
    /// MESSAGE frames per feed.
    pub messages: [u64; FEED_COUNT],
    /// REJECT and STATUS frames per feed (carry no feed data).
    pub other_frames: [u64; FEED_COUNT],
    /// MESSAGE frames of feeds with no decoder: always 0 now that every feed has one; kept so the
    /// summary line keeps its shape.
    pub not_applied: [u64; FEED_COUNT],
    /// Rows decoded per feed.
    pub rows_seen: [u64; FEED_COUNT],
    /// Rows applied per feed.
    pub rows_applied: [u64; FEED_COUNT],
    /// Rows counted out of order per feed: TBFM rows refused whole, SFDPS rows whose position was
    /// dropped as older (those are also in `rows_applied`).
    pub rows_out_of_order: [u64; FEED_COUNT],
    /// Rows refused because the table was full, per feed.
    pub rows_refused_full: [u64; FEED_COUNT],
    /// TBFM rows with no usable key, TFMS flight rows with no flight reference, and FNS
    /// cancellations that name no NOTAM held.
    pub rows_unlinked: u64,
    /// TBFM messages by `oa_tbfm::MsgKind`.
    pub tbfm_kinds: [u64; MSG_KIND_COUNT],
    /// Failures by kind.
    pub errors: Errors,
}

/// Builds a pipeline. Everything is allocated here.
pub fn new_pipeline(cfg: PipelineConfig) -> Result<Pipeline, PipelineError> {
    if cfg.checkpoint_ns < MIN_CHECKPOINT_NS {
        return Err(PipelineError::Checkpoint(cfg.checkpoint_ns));
    }
    if cfg.checkpoint_ring == 0 {
        return Err(PipelineError::CheckpointRing(cfg.checkpoint_ring));
    }
    let table = new_table(cfg.table).map_err(PipelineError::Config)?;
    let parser = Parser::new(doc_xml_limits()).map_err(|_| PipelineError::Parser)?;
    Ok(Pipeline {
        cfg,
        table,
        adsb: new_adsb_table(ADSB_CAPACITY),
        tfms: new_tfms_table(TFMS_CAPACITY),
        flows: new_flow_table(FLOW_CAPACITY),
        fns: new_fns_table(FNS_CAPACITY),
        parser,
        fixm: Decoder::new(doc_fixm_limits()),
        events: new_doc_events(),
        rows: new_doc_tbfm_rows(),
        reports: Reports::default(),
        tfms_rows: new_doc_tfms_rows(),
        notam_dec: new_decoder(NotamLimits::default()),
        notam_stats: new_stats(0),
        notams: default_notams_small(),
        replay_ns: i64::MIN,
        clock: Clock::default(),
        stale_marked: 0,
        next_checkpoint_ns: i64::MIN,
        checkpoints: CheckpointRing::new(cfg.checkpoint_ring),
        frames: [0; FEED_COUNT],
        messages: [0; FEED_COUNT],
        other_frames: [0; FEED_COUNT],
        not_applied: [0; FEED_COUNT],
        rows_seen: [0; FEED_COUNT],
        rows_applied: [0; FEED_COUNT],
        rows_out_of_order: [0; FEED_COUNT],
        rows_refused_full: [0; FEED_COUNT],
        rows_unlinked: 0,
        tbfm_kinds: [0; MSG_KIND_COUNT],
        errors: [0; ERR_COUNT],
    })
}

/// The NOTAM row buffer: one message yields one row, so a few rows suffice; the string and
/// vertex tables hold the largest message the decoder accepts by default.
fn default_notams_small() -> Notams {
    oa_notam::new_notams(4, 64 * 1024, 64, 16 * 1024)
}

/// Total frames fed, all feeds.
pub fn frames_total(p: &Pipeline) -> u64 {
    p.frames.iter().sum()
}

/// Total rows applied, both feeds.
pub fn rows_applied_total(p: &Pipeline) -> u64 {
    p.rows_applied.iter().sum()
}

/// The pipeline's state as a checkpoint at the current replay time (used for the final one).
pub fn snapshot(p: &Pipeline) -> Checkpoint {
    Checkpoint {
        replay_ns: p.replay_ns,
        frames: frames_total(p),
        rows_applied: rows_applied_total(p),
        flights: p.table.live as u64,
        hash: replay_hash(p),
    }
}

/// Hands every held checkpoint, oldest first, to `f` and removes it from the ring.
pub fn pipeline_drain_checkpoints(p: &mut Pipeline, mut f: impl FnMut(&Checkpoint)) {
    let _ = pipeline_try_drain_checkpoints(p, |c| {
        f(c);
        Ok::<(), ()>(())
    });
}

/// [`pipeline_drain_checkpoints`] with a consumer that can fail: a checkpoint is removed only
/// after `f` returned `Ok` for it, so the first error stops the drain with that checkpoint
/// still held.
pub fn pipeline_try_drain_checkpoints<E>(
    p: &mut Pipeline,
    mut f: impl FnMut(&Checkpoint) -> Result<(), E>,
) -> Result<(), E> {
    while let Some(c) = p.checkpoints.get(0) {
        f(c)?;
        p.checkpoints.pop_oldest();
    }
    Ok(())
}

/// The hash of the table contents: the flight table and, once a frame of their feed has been fed,
/// the ADS-B, TFMS and FNS tables. It does not cover the replay clock ([`replay_state_digest`]
/// does), so two pipelines with equal contents hash equal whatever their clocks hold. Until an ADS-B, TFMS or FNS frame has been fed this is
/// exactly [`state_hash`] of the flight table, so replays of SFDPS and TBFM alone hash as they
/// always did. With ADS-B frames only it is SHA-256 over the tag `OATSA`, the flight-table hash
/// and the ADS-B table's serialisation ([`adsb_hash_into`]), as before TFMS and FNS were applied.
/// With TFMS or FNS frames it is SHA-256 over the tag `OATSX`, the flight-table hash, one byte
/// with bit 0 set for ADS-B, bit 1 for TFMS and bit 2 for FNS, and then the serialisation of each
/// table whose bit is set, in that order ([`adsb_hash_into`], [`tfms_hash_into`] with
/// [`flow_hash_into`], [`fns_hash_into`]).
pub fn replay_hash(p: &Pipeline) -> Digest {
    let flights = state_hash(&p.table);
    let fed = |f: FeedKind| p.frames.get(f as usize).copied().unwrap_or(0) > 0;
    let (adsb, tfms, fns) = (fed(FeedKind::Adsb), fed(FeedKind::Tfms), fed(FeedKind::Fns));
    if !adsb && !tfms && !fns {
        return flights;
    }
    let mut h = Sha256::new();
    if !tfms && !fns {
        let _ = h.update(b"OATSA");
        let _ = h.update(&flights);
        adsb_hash_into(&mut h, &p.adsb);
        return h.finalize();
    }
    let _ = h.update(b"OATSX");
    let _ = h.update(&flights);
    let _ = h.update(&[u8::from(adsb) | u8::from(tfms) << 1 | u8::from(fns) << 2]);
    if adsb {
        adsb_hash_into(&mut h, &p.adsb);
    }
    if tfms {
        tfms_hash_into(&mut h, &p.tfms);
        flow_hash_into(&mut h, &p.flows);
    }
    if fns {
        fns_hash_into(&mut h, &p.fns);
    }
    h.finalize()
}

/// The digest of the whole replay state: SHA-256 over the tag `OATSD`, the content hash
/// ([`replay_hash`]), the replay time, and then every field of the clock (each feed's `last_ns`,
/// `seen`, `ever`, `cand_last_ns`, `cand_n` and `cand_back`, then `held`, `forward_steps`,
/// `backward_steps` and `steps_outvoted`) and the cadence state (the next checkpoint time and
/// the next sweep time of the flight, ADS-B and TFMS tables), integers as eight little-endian
/// bytes and flags as one byte. Two pipelines with equal digests behave alike on any further
/// input; equal [`replay_hash`] alone does not say so, as a held candidate step changes what the
/// next frames do without changing a table.
pub fn replay_state_digest(p: &Pipeline) -> Digest {
    let mut h = Sha256::new();
    let _ = h.update(b"OATSD");
    let _ = h.update(&replay_hash(p));
    let i = |h: &mut Sha256, v: i64| {
        let _ = h.update(&v.to_le_bytes());
    };
    let u = |h: &mut Sha256, v: u64| {
        let _ = h.update(&v.to_le_bytes());
    };
    i(&mut h, p.replay_ns);
    for f in &p.clock.feeds {
        i(&mut h, f.last_ns);
        i(&mut h, f.cand_last_ns);
        u(&mut h, u64::from(f.cand_n));
        let _ = h.update(&[u8::from(f.seen), u8::from(f.ever), u8::from(f.cand_back)]);
    }
    u(&mut h, p.clock.held);
    u(&mut h, p.clock.forward_steps);
    u(&mut h, p.clock.backward_steps);
    u(&mut h, p.clock.steps_outvoted);
    i(&mut h, p.next_checkpoint_ns);
    i(&mut h, p.table.next_sweep_ns);
    i(&mut h, p.adsb.next_sweep_ns);
    i(&mut h, p.tfms.next_sweep_ns);
    h.finalize()
}

/// The rows, in every table, that were last updated before a confirmed step of the clock and have
/// not been updated since: what they show was current before the step, whatever their age says.
pub fn pipeline_stale_count(p: &Pipeline) -> usize {
    stale_count(&p.table)
        + tfms_stale_count(&p.tfms)
        + flow_stale_count(&p.flows)
        + fns_stale_count(&p.fns)
        + adsb_stale_count(&p.adsb)
}

/// Adds `n` to an error count (for failures found before the pipeline, such as frame errors).
pub fn add_errors(p: &mut Pipeline, k: ErrKind, n: u64) {
    count_error(&mut p.errors, k, n);
}

fn next_boundary(now: i64, step: i64) -> i64 {
    now.div_euclid(step).saturating_add(1).saturating_mul(step)
}

/// Feeds one frame. `feed` says how to decode a MESSAGE payload; `recv_unix_ns` is the replay
/// time.
pub fn process_frame(
    p: &mut Pipeline,
    feed: FeedKind,
    kind: Kind,
    recv_unix_ns: i64,
    payload: &[u8],
) {
    let f = feed as usize;
    // 1. Clock, eviction cadence, checkpoint.
    let clock_before = p.clock;
    match clock_observe(&mut p.clock, f, p.replay_ns, recv_unix_ns) {
        Tick::Advance => p.replay_ns = recv_unix_ns,
        Tick::Keep => {}
        Tick::Regress => count_error(&mut p.errors, ErrKind::TimeRegression, 1),
        Tick::Held { back } => {
            if back {
                count_error(&mut p.errors, ErrKind::TimeRegression, 1);
            }
        }
        Tick::Step { .. } => {
            // The step is applied only if its size is exact: the ages are shifted by it, and a
            // saturated size would leave them wrong against the clock that takes the new time.
            match recv_unix_ns.checked_sub(p.replay_ns) {
                Some(delta) => step_clock(p, recv_unix_ns, delta),
                None => {
                    p.clock = clock_before;
                    count_error(&mut p.errors, ErrKind::ClockStepOverflow, 1);
                }
            }
        }
    }
    let now = p.replay_ns;
    advance_time(&mut p.table, now);
    adsb_advance(&mut p.adsb, now);
    if tfms_due(&mut p.tfms.next_sweep_ns, now) {
        tfms_sweep(&mut p.tfms, now);
        flow_sweep(&mut p.flows, now);
        fns_sweep(&mut p.fns, now);
    }
    if p.next_checkpoint_ns == i64::MIN {
        p.next_checkpoint_ns = next_boundary(now, p.cfg.checkpoint_ns);
    } else if now >= p.next_checkpoint_ns {
        let cp = snapshot(p);
        p.checkpoints.push(cp);
        p.next_checkpoint_ns = next_boundary(now, p.cfg.checkpoint_ns);
    }
    if let Some(c) = p.frames.get_mut(f) {
        *c += 1;
    }
    // 2. Only MESSAGE frames carry feed data.
    if kind != Kind::Message {
        if let Some(c) = p.other_frames.get_mut(f) {
            *c += 1;
        }
        return;
    }
    if let Some(c) = p.messages.get_mut(f) {
        *c += 1;
    }
    match feed {
        FeedKind::Sfdps => process_sfdps(p, payload, now),
        FeedKind::Tbfm => process_tbfm(p, payload, now),
        FeedKind::Adsb => process_adsb(p, payload, now),
        FeedKind::Tfms => process_tfms(p, payload, now),
        FeedKind::Fns => process_fns(p, payload, now),
    }
}

/// A confirmed step of the clock: take the new time, shift the ages the tables hold (every table, ADS-B included) by the same
/// amount so nothing is evicted or stranded by it (and mark every row as stale until it is heard
/// again, since its age is no longer the time since it was heard), and restart the sweep and checkpoint cadences
/// from the new time.
fn step_clock(p: &mut Pipeline, to_ns: i64, delta: i64) {
    p.replay_ns = to_ns;
    let marked = shift_times(&mut p.table, delta)
        + tfms_shift_times(&mut p.tfms, delta)
        + flow_shift_times(&mut p.flows, delta)
        + fns_shift_times(&mut p.fns, delta)
        + adsb_shift_times(&mut p.adsb, delta);
    p.stale_marked += marked as u64;
    // Each cadence is due now: a sweep and a checkpoint at the new time, as after any long gap.
    p.table.next_sweep_ns = to_ns;
    p.adsb.next_sweep_ns = to_ns;
    p.tfms.next_sweep_ns = to_ns;
    p.next_checkpoint_ns = to_ns;
}

fn bump(a: &mut [u64; FEED_COUNT], feed: FeedKind, n: u64) {
    if let Some(c) = a.get_mut(feed as usize) {
        *c += n;
    }
}

fn note(p: &mut Pipeline, feed: FeedKind, a: Applied) {
    match a {
        Applied::Applied => bump(&mut p.rows_applied, feed, 1),
        Applied::AppliedStalePosition => {
            bump(&mut p.rows_applied, feed, 1);
            bump(&mut p.rows_out_of_order, feed, 1);
        }
        Applied::OutOfOrder => bump(&mut p.rows_out_of_order, feed, 1),
        Applied::TableFull => bump(&mut p.rows_refused_full, feed, 1),
        Applied::Unlinked => p.rows_unlinked += 1,
        Applied::BadRow => count_error(&mut p.errors, ErrKind::BadRow, 1),
    }
}

fn process_sfdps(p: &mut Pipeline, payload: &[u8], now: i64) {
    clear_events(&mut p.events);
    let doc = match p.parser.parse(payload) {
        Ok(d) => d,
        Err(_) => {
            count_error(&mut p.errors, ErrKind::SfdpsXml, 1);
            return;
        }
    };
    let before_rejected = p.fixm.stats().rejected;
    let result = p.fixm.decode_prefix(&doc, payload, &mut p.events);
    let summary = match result {
        Ok(s) => s,
        Err(e) => {
            let k = match e {
                FixmError::UnsupportedRoot => ErrKind::SfdpsUnsupportedRoot,
                FixmError::TooManyElements { .. } => ErrKind::SfdpsTooManyElements,
                FixmError::RowCapacity { .. } => ErrKind::SfdpsRowCapacity,
                FixmError::StringCapacity { .. } => ErrKind::SfdpsStringCapacity,
                FixmError::InputMismatch => ErrKind::SfdpsInternal,
            };
            count_error(&mut p.errors, k, 1);
            return;
        }
    };
    let after = p.fixm.stats().rejected;
    for r in 0..REJECT_COUNT {
        let d = after
            .get(r)
            .copied()
            .unwrap_or(0)
            .saturating_sub(before_rejected.get(r).copied().unwrap_or(0));
        count_error(&mut p.errors, sfdps_reject_kind(r), d);
    }
    count_error(
        &mut p.errors,
        ErrKind::SfdpsField,
        u64::from(summary.field_errors),
    );
    // Messages past the point where the event table filled up (rows or route text).
    let full = if p.events.len >= p.events.capacity {
        ErrKind::SfdpsRowCapacity
    } else {
        ErrKind::SfdpsStringCapacity
    };
    count_error(&mut p.errors, full, u64::from(summary.not_decoded));
    for i in 0..p.events.len {
        let a = apply_sfdps(&mut p.table, &p.events, i, now);
        bump(&mut p.rows_seen, FeedKind::Sfdps, 1);
        note(p, FeedKind::Sfdps, a);
    }
}

fn process_adsb(p: &mut Pipeline, payload: &[u8], now: i64) {
    reports_clear(&mut p.reports);
    match decode_payload_lenient(payload, &mut p.reports) {
        // A record that cannot be read loses only itself, and is counted.
        Ok(c) => count_error(&mut p.errors, ErrKind::AdsbBadRecord, c.dropped as u64),
        Err(e) => {
            let k = match e {
                PayloadError::Length => ErrKind::AdsbLength,
                PayloadError::Magic => ErrKind::AdsbMagic,
                PayloadError::Layout => ErrKind::AdsbLayout,
                PayloadError::TooManyRows => ErrKind::AdsbTooManyRows,
                PayloadError::BadRecord(_) => ErrKind::AdsbBadRecord,
                PayloadError::Range => ErrKind::AdsbInternal,
            };
            count_error(&mut p.errors, k, 1);
            return;
        }
    }
    for i in 0..reports_len(&p.reports) {
        let a = apply_adsb(&mut p.adsb, &p.reports, i, now);
        bump(&mut p.rows_seen, FeedKind::Adsb, 1);
        match a {
            AdsbApplied::New | AdsbApplied::Updated => bump(&mut p.rows_applied, FeedKind::Adsb, 1),
            AdsbApplied::OutOfOrder => bump(&mut p.rows_out_of_order, FeedKind::Adsb, 1),
            AdsbApplied::TableFull => bump(&mut p.rows_refused_full, FeedKind::Adsb, 1),
            // Counted in the table's own `future_refused`.
            AdsbApplied::TooFarAhead => {}
            AdsbApplied::BadRow => count_error(&mut p.errors, ErrKind::BadRow, 1),
        }
    }
}

fn process_tbfm(p: &mut Pipeline, payload: &[u8], now: i64) {
    clear_rows(&mut p.rows);
    let doc = match p.parser.parse(payload) {
        Ok(d) => d,
        Err(_) => {
            count_error(&mut p.errors, ErrKind::TbfmXml, 1);
            return;
        }
    };
    match decode_doc_prefix(&doc, &mut p.rows) {
        Ok(out) => {
            count_error(&mut p.errors, ErrKind::TbfmCapacity, u64::from(out.dropped));
            // Flights refused for a field are dropped alone; the rest of the message applies.
            count_error(&mut p.errors, ErrKind::TbfmField, u64::from(out.rejected));
            let k = out.kind as usize;
            if let Some(c) = p.tbfm_kinds.get_mut(k) {
                *c += 1;
            }
        }
        Err(e) => {
            let k = match e {
                DocError::Xml(_) => ErrKind::TbfmXml,
                DocError::NotTbfm { .. } => ErrKind::TbfmNotTbfm,
                DocError::Field(_) => ErrKind::TbfmField,
                DocError::Capacity => ErrKind::TbfmCapacity,
            };
            count_error(&mut p.errors, k, 1);
            return;
        }
    }
    for i in 0..rows_len(&p.rows) {
        let a = apply_tbfm(&mut p.table, &p.rows, i, now);
        bump(&mut p.rows_seen, FeedKind::Tbfm, 1);
        note(p, FeedKind::Tbfm, a);
    }
}

fn process_tfms(p: &mut Pipeline, payload: &[u8], now: i64) {
    clear_tfms_rows(&mut p.tfms_rows);
    let doc = match p.parser.parse(payload) {
        Ok(d) => d,
        Err(_) => {
            count_error(&mut p.errors, ErrKind::TfmsXml, 1);
            return;
        }
    };
    match decode_tfms_doc(&doc, &mut p.tfms_rows) {
        Ok(out) => {
            count_error(&mut p.errors, ErrKind::TfmsCapacity, u64::from(out.dropped));
            // Messages refused for a field are dropped alone; their siblings apply.
            count_error(&mut p.errors, ErrKind::TfmsField, u64::from(out.rejected));
        }
        Err(e) => {
            let k = match e {
                TfmsDocError::Xml(_) => ErrKind::TfmsXml,
                TfmsDocError::NotTfms { .. } => ErrKind::TfmsNotTfms,
                TfmsDocError::Field(_) => ErrKind::TfmsField,
                TfmsDocError::Capacity(_) => ErrKind::TfmsCapacity,
            };
            count_error(&mut p.errors, k, 1);
            return;
        }
    }
    for i in 0..flights_len(&p.tfms_rows.flights) {
        let a = apply_tfms_flight(&mut p.tfms, &p.table, &p.tfms_rows, i, now);
        bump(&mut p.rows_seen, FeedKind::Tfms, 1);
        match a {
            TfmsApplied::New | TfmsApplied::Updated => bump(&mut p.rows_applied, FeedKind::Tfms, 1),
            TfmsApplied::OutOfOrder => bump(&mut p.rows_out_of_order, FeedKind::Tfms, 1),
            TfmsApplied::TableFull => bump(&mut p.rows_refused_full, FeedKind::Tfms, 1),
            TfmsApplied::NoRef => p.rows_unlinked += 1,
            TfmsApplied::BadRow => count_error(&mut p.errors, ErrKind::BadRow, 1),
        }
    }
    for i in 0..flows_len(&p.tfms_rows.flows) {
        let a = apply_tfms_flow(&mut p.flows, &p.tfms_rows, i, now);
        bump(&mut p.rows_seen, FeedKind::Tfms, 1);
        match a {
            FlowApplied::New | FlowApplied::Updated => bump(&mut p.rows_applied, FeedKind::Tfms, 1),
            FlowApplied::OutOfOrder => bump(&mut p.rows_out_of_order, FeedKind::Tfms, 1),
            FlowApplied::TableFull => bump(&mut p.rows_refused_full, FeedKind::Tfms, 1),
            FlowApplied::BadRow => count_error(&mut p.errors, ErrKind::BadRow, 1),
        }
    }
}

fn process_fns(p: &mut Pipeline, payload: &[u8], now: i64) {
    clear_notams(&mut p.notams);
    let doc = match p.parser.parse(payload) {
        Ok(d) => d,
        Err(_) => {
            count_error(&mut p.errors, ErrKind::FnsXml, 1);
            return;
        }
    };
    match oa_notam::decode_doc(&mut p.notam_dec, &doc, &mut p.notams, &mut p.notam_stats) {
        Ok(o) => count_error(&mut p.errors, ErrKind::FnsField, u64::from(o.field_errors)),
        Err(e) => {
            let k = match e {
                FnsDocError::Xml(_) => ErrKind::FnsXml,
                FnsDocError::Rejected(_) => ErrKind::FnsRejected,
                FnsDocError::TooManyElements { .. } => ErrKind::FnsTooManyElements,
                FnsDocError::Capacity(_) => ErrKind::FnsCapacity,
            };
            count_error(&mut p.errors, k, 1);
            return;
        }
    }
    for i in 0..notams_len(&p.notams) {
        let a = apply_fns(&mut p.fns, &p.notams, i, now);
        bump(&mut p.rows_seen, FeedKind::Fns, 1);
        match a {
            FnsApplied::New
            | FnsApplied::Reissued
            | FnsApplied::Replaced
            | FnsApplied::Cancelled => bump(&mut p.rows_applied, FeedKind::Fns, 1),
            FnsApplied::CancelUnmatched => p.rows_unlinked += 1,
            FnsApplied::OutOfOrder => bump(&mut p.rows_out_of_order, FeedKind::Fns, 1),
            FnsApplied::TableFull => bump(&mut p.rows_refused_full, FeedKind::Fns, 1),
            FnsApplied::NoKey => count_error(&mut p.errors, ErrKind::FnsNoKey, 1),
            FnsApplied::BadRow => count_error(&mut p.errors, ErrKind::BadRow, 1),
        }
    }
}
