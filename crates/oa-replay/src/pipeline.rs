//! The pipeline: feed frames in, one deterministic flight state out. Core (pure, B1): a function
//! of the frames it is given. No I/O and no clock; replay time is each frame's own
//! `recv_unix_ns` (B4), so a recording replays bit for bit (B6).
//!
//! A frame goes through four steps, each counted and none skipped silently:
//!
//! 1. the replay clock moves to the frame's time (never backwards; a frame earlier than one
//!    already seen is counted as a time regression and the clock stays), the table's eviction
//!    cadence runs ([`advance_time`]), and a checkpoint is recorded if one is due;
//! 2. only MESSAGE frames carry feed data; REJECT and STATUS frames are counted;
//! 3. the payload is parsed with oa-xml and decoded with oa-fixm (SFDPS) or oa-tbfm (TBFM);
//!    every failure is counted under its [`ErrKind`];
//! 4. each decoded row is applied to the flight table ([`apply_sfdps`], [`apply_tbfm`]).
//!
//! Checkpoints fall on multiples of `checkpoint_ns` of replay time: the first frame at or after
//! a boundary records the state as it stood before that frame (after the table's own sweep at
//! that time), stamped with that frame's replay time. A long gap with no frames records one
//! checkpoint, since the state did not change inside it.
//!
//! Memory: the parser, decoders, event and row tables and the flight table are all allocated in
//! [`new_pipeline`]. The checkpoint list grows with the length of the run (one entry per
//! interval of replay time), the stated exception for an offline harness (C6).

use oa_feed::Kind;
use oa_fixm::{clear_events, new_events, Decoder, Events, FixmError, Limits, REJECT_COUNT};
use oa_sha256::Digest;
use oa_tbfm::{clear_rows, decode_doc, new_rows, rows_len, DocError, Rows, MSG_KIND_COUNT};
use oa_xml::Parser;

use crate::apply::{apply_sfdps, apply_tbfm, Applied};
use crate::errors::{count_error, sfdps_reject_kind, ErrKind, Errors, ERR_COUNT};
use crate::hash::state_hash;
use crate::table::{advance_time, new_table, ConfigError, FlightTable, TableConfig, NS_PER_S};

/// Which feed a frame came from; chosen by the caller, never guessed from the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FeedKind {
    /// SWIM SFDPS flight messages.
    Sfdps = 0,
    /// SWIM TBFM metering publication.
    Tbfm = 1,
    /// SWIM TFMS flow data. Not decoded yet: frames are counted in `not_applied`.
    Tfms = 2,
    /// SWIM FNS notices. Not decoded yet: frames are counted in `not_applied`.
    Fns = 3,
}

/// Number of [`FeedKind`] values.
pub const FEED_COUNT: usize = 4;

/// Display names, indexed by `FeedKind as usize`.
pub const FEED_NAMES: [&str; FEED_COUNT] = ["sfdps", "tbfm", "tfms", "fns"];

/// Default spacing of checkpoints: 10 s of replay time.
pub const DEFAULT_CHECKPOINT_NS: i64 = 10 * NS_PER_S;
/// Rows the SFDPS event table and the TBFM row table hold for one document. The measured feeds
/// carry at most 100 messages per SFDPS document; a document with more is refused whole and
/// counted ([`ErrKind::SfdpsRowCapacity`], [`ErrKind::TbfmCapacity`]).
pub const DOC_ROW_CAPACITY: usize = 4096;
/// Bytes of route text one SFDPS document may add.
pub const DOC_STRING_BYTES: usize = 1 << 20;

/// Pipeline settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineConfig {
    /// The flight table's settings.
    pub table: TableConfig,
    /// Replay-time spacing of checkpoints, ns.
    pub checkpoint_ns: i64,
}

impl Default for PipelineConfig {
    fn default() -> PipelineConfig {
        PipelineConfig {
            table: TableConfig::default(),
            checkpoint_ns: DEFAULT_CHECKPOINT_NS,
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

/// Why a pipeline could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineError {
    /// The table settings were refused.
    Config(ConfigError),
    /// `checkpoint_ns` is not positive.
    Checkpoint(i64),
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
    parser: Parser,
    fixm: Decoder,
    events: Events,
    rows: Rows,
    /// Replay time now, ns; `i64::MIN` before the first frame.
    pub replay_ns: i64,
    next_checkpoint_ns: i64,
    /// Checkpoints recorded.
    pub checkpoints: Vec<Checkpoint>,
    /// Frames fed per feed (every kind of frame).
    pub frames: [u64; FEED_COUNT],
    /// MESSAGE frames per feed.
    pub messages: [u64; FEED_COUNT],
    /// REJECT and STATUS frames per feed (carry no feed data).
    pub other_frames: [u64; FEED_COUNT],
    /// MESSAGE frames of feeds with no decoder yet (TFMS, FNS): counted, not applied.
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
    /// TBFM rows with no usable key.
    pub rows_unlinked: u64,
    /// TBFM messages by `oa_tbfm::MsgKind`.
    pub tbfm_kinds: [u64; MSG_KIND_COUNT],
    /// Failures by kind.
    pub errors: Errors,
}

/// Builds a pipeline. Everything is allocated here.
pub fn new_pipeline(cfg: PipelineConfig) -> Result<Pipeline, PipelineError> {
    if cfg.checkpoint_ns <= 0 {
        return Err(PipelineError::Checkpoint(cfg.checkpoint_ns));
    }
    let table = new_table(cfg.table).map_err(PipelineError::Config)?;
    let parser = Parser::new(oa_xml::Limits::default()).map_err(|_| PipelineError::Parser)?;
    Ok(Pipeline {
        cfg,
        table,
        parser,
        fixm: Decoder::new(Limits::default()),
        events: new_events(DOC_ROW_CAPACITY, DOC_STRING_BYTES),
        rows: new_rows(DOC_ROW_CAPACITY),
        replay_ns: i64::MIN,
        next_checkpoint_ns: i64::MIN,
        checkpoints: Vec::new(),
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
        hash: state_hash(&p.table),
    }
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
    if p.replay_ns == i64::MIN {
        p.replay_ns = recv_unix_ns;
    } else if recv_unix_ns < p.replay_ns {
        count_error(&mut p.errors, ErrKind::TimeRegression, 1);
    } else {
        p.replay_ns = recv_unix_ns;
    }
    let now = p.replay_ns;
    advance_time(&mut p.table, now);
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
        FeedKind::Tfms | FeedKind::Fns => bump(&mut p.not_applied, feed, 1),
    }
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
    let result = p.fixm.decode(&doc, payload, &mut p.events);
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
    for i in 0..p.events.len {
        let a = apply_sfdps(&mut p.table, &p.events, i, now);
        bump(&mut p.rows_seen, FeedKind::Sfdps, 1);
        note(p, FeedKind::Sfdps, a);
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
    match decode_doc(&doc, &mut p.rows) {
        Ok(out) => {
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
