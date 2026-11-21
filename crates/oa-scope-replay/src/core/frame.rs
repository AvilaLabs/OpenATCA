//! The frame chain (**core**, B1): map and traffic into one display list, then pixels. Every
//! public function here is core. Every table is allocated once in [`replay_new`]; a frame
//! ([`replay_build`], [`replay_raster`]) allocates nothing: its text is written into a stack
//! buffer. The time text of the frame is an input (the shell formats it).

use crate::core::rows::{
    convert_check, targets_at, Callsigns, ConvertConfig, ConvertStats, RowStore, RowsError,
};
use oa_airspace::core::model::Model;
use oa_display::core::list::{
    list_new, push_text, DisplayError, DisplayList, ListCaps, ALIGN_RIGHT,
};
use oa_display::core::palette;
use oa_display::core::raster::{frame_len, rasterize, FrameSpec, RenderError};
use oa_display::core::safety::{build_safety, safety_caps, FeedStatus, SafetyScene};
use oa_display::core::scene::{
    build_scene, scene_caps, scene_index, scratch_for, view_new, SceneIndex, SceneStats, Scratch,
    View,
};
use oa_display::core::text::{text_clipped, text_new, text_push_dec, text_push_str, text_str};
use oa_display::core::traffic::{
    build_traffic, caps_add, targets_clear, targets_malformed, targets_new, targets_refused,
    traffic_caps, TargetSet, TrafficStats, TrafficStyle, LAYER_BLOCK,
};
use oa_safety::inhibit::InhibitTable;
use oa_safety::net::StateTable;
use std::fmt;

/// The frame's own text: the time and the target count, bottom right.
const STAMP_SIZE: u8 = 12;
const STAMP_RIGHT_PX: i32 = 8;
const STAMP_FROM_BOTTOM_PX: i32 = 22;
const STAMP_BYTES: usize = 96;

/// Why a frame could not be made.
// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    Display(DisplayError),
    Render(RenderError),
    /// The conversion configuration is outside its ranges.
    Config(RowsError),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Display(e) => write!(f, "display list: {e}"),
            ReplayError::Render(e) => write!(f, "render: {e}"),
            ReplayError::Config(e) => write!(f, "configuration: {e}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<DisplayError> for ReplayError {
    fn from(e: DisplayError) -> Self {
        ReplayError::Display(e)
    }
}

impl From<RenderError> for ReplayError {
    fn from(e: RenderError) -> Self {
        ReplayError::Render(e)
    }
}

/// What to draw. Plain data; A7.2: cold struct (one per run).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplayConfig {
    pub centre_lat_e7: i32,
    pub centre_lon_e7: i32,
    pub range_nm: f64,
    pub width: u32,
    pub height: u32,
    /// Targets one frame can hold; more are refused, counted and said on the frame.
    pub max_targets: usize,
    pub style: TrafficStyle,
    pub convert: ConvertConfig,
}

/// The state of a replay: the model, the view, every table. A7.2: cold struct (one per run).
pub struct Replay {
    pub cfg: ReplayConfig,
    pub spec: FrameSpec,
    /// The pixels of the last [`replay_raster`].
    pub buf: Vec<u8>,
    pub list: DisplayList,
    pub set: TargetSet,
    pub view: View,
    model: Model,
    index: SceneIndex,
    scratch: Scratch,
    ell: oa_geo::Ellipsoid,
}

/// Live health telemetry supplied by the scope producer for one frame. The replay command has no
/// such telemetry and must not infer it from its historical clip.
#[derive(Clone, Copy, Debug)]
pub struct ReplaySafety<'a> {
    pub states: &'a StateTable,
    pub inhibits: &'a InhibitTable,
    pub feeds: &'a [FeedStatus],
    /// Time of the newest accepted track update, not merely the newest emitted row.
    pub newest_track_ns: Option<i64>,
    pub frozen_limit_ns: i64,
}

/// What one frame build drew and left out. A7.2: cold struct (one per frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameInfo {
    pub scene: SceneStats,
    pub traffic: TrafficStats,
    pub convert: ConvertStats,
    /// Targets the set refused at capacity.
    pub refused: u32,
    /// Targets the set refused as not valid.
    pub malformed: u32,
    /// The frame's own stamp text did not fit its buffer (LOSS: the stamp on the frame is short).
    pub stamp_clipped: bool,
}

/// Allocates every table for the model and the configuration.
pub fn replay_new(model: Model, cfg: &ReplayConfig) -> Result<Replay, ReplayError> {
    convert_check(&cfg.convert).map_err(ReplayError::Config)?;
    // The conversion makes `convert.trail_n` history positions and the drawing shows
    // `style.trail_n`; two numbers for one setting would silently draw the smaller.
    if cfg.convert.trail_n != cfg.style.trail_n {
        return Err(ReplayError::Config(RowsError::Config(
            "the conversion and the style disagree on the trail length",
        )));
    }
    let view = view_new(
        cfg.centre_lat_e7,
        cfg.centre_lon_e7,
        cfg.range_nm,
        cfg.width,
        cfg.height,
    )?;
    let spec = FrameSpec {
        width: cfg.width,
        height: cfg.height,
        stride: cfg.width.saturating_mul(4),
    };
    let stamp = ListCaps {
        lines: 0,
        rects: 0,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts: 1,
        text_bytes: STAMP_BYTES,
    };
    let caps = caps_add(
        &caps_add(
            &scene_caps(&model)?,
            &caps_add(&traffic_caps(cfg.max_targets, &cfg.style)?, &safety_caps())?,
        )?,
        &stamp,
    )?;
    let index = scene_index(&model);
    let scratch = scratch_for(&model, &view)?;
    Ok(Replay {
        cfg: *cfg,
        buf: vec![0u8; frame_len(&spec)],
        spec,
        list: list_new(caps)?,
        set: targets_new(cfg.max_targets)?,
        view,
        model,
        index,
        scratch,
        ell: oa_geo::wgs84(),
    })
}

/// Builds the display list of the frame at `t_ns`: the map, then the traffic, then `stamp` and the
/// target count. On an error the list is faulted and [`replay_raster`] refuses it.
pub fn replay_build(
    r: &mut Replay,
    store: &RowStore,
    calls: &Callsigns,
    t_ns: i64,
    stamp: &str,
) -> Result<FrameInfo, ReplayError> {
    replay_build_inner(r, store, calls, t_ns, stamp, None)
}

/// Builds the scope frame with independent feed, track-freshness and safety-net telemetry.
pub fn replay_build_with_safety(
    r: &mut Replay,
    store: &RowStore,
    calls: &Callsigns,
    t_ns: i64,
    stamp: &str,
    safety: &ReplaySafety<'_>,
) -> Result<FrameInfo, ReplayError> {
    replay_build_inner(r, store, calls, t_ns, stamp, Some(safety))
}

fn replay_build_inner(
    r: &mut Replay,
    store: &RowStore,
    calls: &Callsigns,
    t_ns: i64,
    stamp: &str,
    safety: Option<&ReplaySafety<'_>>,
) -> Result<FrameInfo, ReplayError> {
    targets_clear(&mut r.set);
    let convert = targets_at(store, calls, t_ns, &r.cfg.convert, &r.view.proj, &mut r.set);
    let scene = build_scene(&mut r.list, &r.model, &r.index, &r.view, &mut r.scratch)?;
    let traffic = build_traffic(&mut r.list, &r.view.proj, &r.ell, &r.set, &r.cfg.style)?;
    if let Some(s) = safety {
        let scene = SafetyScene {
            states: s.states,
            inhibits: s.inhibits,
            now_ns: t_ns,
            feeds: s.feeds,
            newest_track_ns: s.newest_track_ns,
            frozen_limit_ns: s.frozen_limit_ns,
            targets: &r.set,
        };
        build_safety(&mut r.list, &r.view.proj, &scene)?;
    }
    let mut tb = text_new();
    text_push_str(&mut tb, stamp);
    text_push_str(&mut tb, "  TARGETS ");
    text_push_dec(&mut tb, u64::from(traffic.drawn), 0);
    let y = i32::try_from(r.cfg.height)
        .unwrap_or(i32::MAX)
        .saturating_sub(STAMP_FROM_BOTTOM_PX)
        .saturating_mul(16);
    let x = i32::try_from(r.cfg.width)
        .unwrap_or(i32::MAX)
        .saturating_sub(STAMP_RIGHT_PX)
        .saturating_mul(16);
    push_text(
        &mut r.list,
        (x, y),
        STAMP_SIZE,
        ALIGN_RIGHT,
        palette::TEXT,
        LAYER_BLOCK,
        text_str(&tb),
    )?;
    Ok(FrameInfo {
        scene,
        traffic,
        convert,
        refused: targets_refused(&r.set),
        malformed: targets_malformed(&r.set),
        stamp_clipped: text_clipped(&tb),
    })
}

/// Draws the list into `r.buf`.
pub fn replay_raster(r: &mut Replay) -> Result<(), ReplayError> {
    rasterize(&r.list, &r.spec, &mut r.buf)?;
    Ok(())
}
