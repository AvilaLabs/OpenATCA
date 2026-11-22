//! The frame chain (**core**, B1): map, traffic and optional safety indications into one display
//! list, then pixels. Every
//! public function here is core. Every table is allocated once in [`replay_new`]; a frame
//! ([`replay_build`], [`replay_raster`]) allocates nothing: its text is written into a stack
//! buffer. The time text of the frame is an input (the shell formats it).

use crate::core::raw::{
    raw_targets_at_with_source_stale, RawInputError, RawObservationColumns, RawStats,
};
use crate::core::rows::{
    convert_check, targets_at, Callsigns, ConvertConfig, ConvertStats, RowStore, RowsError,
};
use oa_airspace::core::model::Model;
use oa_display::core::font::{text_width_units, CAP_HEIGHT};
use oa_display::core::list::{
    list_clear, list_new, push_rect, push_text, DisplayError, DisplayList, ListCaps, ALIGN_CENTRE,
    ALIGN_RIGHT,
};
use oa_display::core::ownership::{
    build_ownership_status, ownership_caps, ownership_status_fits, OwnershipStatus,
};
use oa_display::core::palette;
use oa_display::core::raster::{frame_len, rasterize, FrameSpec, RenderError};
use oa_display::core::safety::{
    build_safety, safety_caps, FeedStatus, SafetyScene, LAYER_BANNER, LAYER_BANNER_BG,
};
use oa_display::core::scene::{
    build_scene, scene_caps, scene_index, scratch_for, view_new, SceneIndex, SceneStats, Scratch,
    View,
};
use oa_display::core::text::{text_clipped, text_new, text_push_dec, text_push_str, text_str};
use oa_display::core::traffic::{
    build_traffic, caps_add, targets_clear, targets_malformed, targets_new, targets_refused,
    traffic_caps, TargetSet, TrafficStats, TrafficStyle, LAYER_BLOCK, MAX_TARGET_SYMBOL_RADIUS_PX,
};
use oa_safety::inhibit::InhibitTable;
use oa_safety::net::StateTable;
use std::fmt;

/// The frame's own text: the time and the target count, bottom right.
const STAMP_SIZE: u8 = 12;
const STAMP_RIGHT_PX: i32 = 8;
const STAMP_FROM_BOTTOM_PX: i32 = 22;
const STAMP_BYTES: usize = 96;
const RAW_BANNER: &str = "RAW SURVEILLANCE ONLY";
const RAW_BANNER_SIZE: u8 = 16;
const RAW_BANNER_NARROW_SIZE: u8 = STAMP_SIZE;
const RAW_BANNER_TOP_PX: i32 = 10;
const RAW_BANNER_PADDING_PX: i32 = 4;

/// Why a frame could not be made.
// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    Display(DisplayError),
    Render(RenderError),
    /// The conversion configuration is outside its ranges.
    Config(RowsError),
    /// The raw fallback input was not a bounded, ordered set of columns.
    RawInput(RawInputError),
    /// The mandatory raw-only banner cannot fit the viewport clear of map target symbols.
    RawBannerDoesNotFit {
        width: u32,
        height: u32,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Display(e) => write!(f, "display list: {e}"),
            ReplayError::Render(e) => write!(f, "render: {e}"),
            ReplayError::Config(e) => write!(f, "configuration: {e}"),
            ReplayError::RawInput(e) => write!(f, "raw surveillance input: {e}"),
            ReplayError::RawBannerDoesNotFit { width, height } => write!(
                f,
                "raw surveillance banner cannot fit safely in {width}x{height} viewport"
            ),
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

impl From<RawInputError> for ReplayError {
    fn from(e: RawInputError) -> Self {
        ReplayError::RawInput(e)
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

/// Health telemetry supplied by the scope producer as of the frame time. The historical replay
/// command uses [`replay_build`] because its clip provides traffic observations only.
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
    /// Frame status text did not fit its buffer (stamp, or the raw-only banner).
    pub stamp_clipped: bool,
    /// A supplied ownership line was valid but did not fit this projection, so it was omitted.
    pub ownership_clipped: bool,
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
        rects: 1,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts: 2,
        text_bytes: STAMP_BYTES + RAW_BANNER.len(),
    };
    let caps = caps_add(
        &caps_add(
            &scene_caps(&model)?,
            &caps_add(
                &traffic_caps(cfg.max_targets, &cfg.style)?,
                &caps_add(&safety_caps(), &ownership_caps())?,
            )?,
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
    replay_build_with_overlays(r, store, calls, t_ns, stamp, None, None)
}

/// Builds a frame directly from caller-owned raw observation columns, without using tracker
/// output. The caller must supply only observations available by as-of time: the columns encode
/// observation time, not receipt time. Only observations at or before as-of and no older than the
/// finite cutoff in the raw reducer can appear. No ownership, callsign, coast, trail, or
/// extrapolated position is inferred. The banner makes this step down visible to the controller.
/// On any error, the previous list is cleared and rasterization is refused.
pub fn replay_build_raw(
    r: &mut Replay,
    observations: &RawObservationColumns<'_>,
    as_of_ns: i64,
    stamp: &str,
) -> Result<(FrameInfo, RawStats), ReplayError> {
    replay_build_raw_with_source_stale(r, observations, None, as_of_ns, stamp)
}

/// Raw replay for a shell that has source-stale metadata alongside the stable public columns.
pub(crate) fn replay_build_raw_with_source_stale(
    r: &mut Replay,
    observations: &RawObservationColumns<'_>,
    source_stale: Option<&[bool]>,
    as_of_ns: i64,
    stamp: &str,
) -> Result<(FrameInfo, RawStats), ReplayError> {
    list_clear(&mut r.list);
    targets_clear(&mut r.set);
    let result = (|| {
        let (width, height) = (r.spec.width, r.spec.height);
        let half_span_at_size = |size: u8| {
            let banner_width = i64::from(text_width_units(RAW_BANNER.chars().count()))
                .saturating_mul(i64::from(size))
                .saturating_add(i64::from(CAP_HEIGHT) - 1)
                / i64::from(CAP_HEIGHT);
            banner_width.saturating_add(1) / 2 + i64::from(RAW_BANNER_PADDING_PX)
        };
        let full_span = half_span_at_size(RAW_BANNER_SIZE);
        let (banner_size, half_span) = if full_span * 2 <= i64::from(width) {
            (RAW_BANNER_SIZE, full_span)
        } else {
            (
                RAW_BANNER_NARROW_SIZE,
                half_span_at_size(RAW_BANNER_NARROW_SIZE),
            )
        };
        let banner_bottom = RAW_BANNER_TOP_PX + i32::from(banner_size) + RAW_BANNER_PADDING_PX;
        let marker_clearance_bottom =
            i64::from(banner_bottom).saturating_add(i64::from(MAX_TARGET_SYMBOL_RADIUS_PX));
        if half_span * 2 > i64::from(width)
            || i64::from(banner_bottom) > i64::from(height)
            || marker_clearance_bottom >= i64::from(r.view.proj.top_inset)
        {
            return Err(ReplayError::RawBannerDoesNotFit { width, height });
        }
        let raw = raw_targets_at_with_source_stale(
            observations,
            source_stale,
            as_of_ns,
            &r.view.proj,
            &mut r.set,
        )?;
        let scene = build_scene(&mut r.list, &r.model, &r.index, &r.view, &mut r.scratch)?;
        let traffic = build_traffic(&mut r.list, &r.view.proj, &r.ell, &r.set, &r.cfg.style)?;

        let mut status = text_new();
        text_push_str(&mut status, stamp);
        text_push_str(&mut status, "  TARGETS ");
        text_push_dec(&mut status, u64::from(traffic.drawn), 0);
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
            text_str(&status),
        )?;

        let banner_x = i32::try_from(width / 2).unwrap_or(i32::MAX);
        let half_span = i32::try_from(half_span).unwrap_or(i32::MAX);
        // The legend is opaque and sits above traffic. Use the safety overlay layers for the
        // raw banner so a successfully drawn scope always identifies its source.
        push_rect(
            &mut r.list,
            (
                banner_x.saturating_sub(half_span).saturating_mul(16),
                (RAW_BANNER_TOP_PX - RAW_BANNER_PADDING_PX) * 16,
            ),
            (
                banner_x.saturating_add(half_span).saturating_mul(16),
                banner_bottom * 16,
            ),
            palette::BACKGROUND,
            LAYER_BANNER_BG,
        )?;
        let mut banner = text_new();
        text_push_str(&mut banner, RAW_BANNER);
        push_text(
            &mut r.list,
            (banner_x.saturating_mul(16), RAW_BANNER_TOP_PX * 16),
            banner_size,
            ALIGN_CENTRE,
            palette::ALERT,
            LAYER_BANNER,
            text_str(&banner),
        )?;
        Ok((
            FrameInfo {
                scene,
                traffic,
                convert: ConvertStats::default(),
                refused: targets_refused(&r.set),
                malformed: targets_malformed(&r.set),
                stamp_clipped: text_clipped(&status) || text_clipped(&banner),
                ownership_clipped: false,
            },
            raw,
        ))
    })();
    if result.is_err() {
        list_clear(&mut r.list);
        r.list.faulted = true;
        targets_clear(&mut r.set);
    }
    result
}

/// Builds the scope frame with independent feed, track-freshness and safety-net telemetry.
/// If safety rendering returns an error after drawing a fallback banner, the caller can still
/// render that banner. [`replay_raster`] continues to reject a faulted list.
pub fn replay_build_with_safety(
    r: &mut Replay,
    store: &RowStore,
    calls: &Callsigns,
    t_ns: i64,
    stamp: &str,
    safety: &ReplaySafety<'_>,
) -> Result<FrameInfo, ReplayError> {
    replay_build_with_overlays(r, store, calls, t_ns, stamp, Some(safety), None)
}

/// Builds a scope frame with optional safety telemetry and caller-supplied ownership evidence.
/// Ownership is presentation input only; this function does not infer it from NAM acknowledgments.
/// A valid line that cannot fit is omitted and reported by `FrameInfo`.
pub fn replay_build_with_overlays(
    r: &mut Replay,
    store: &RowStore,
    calls: &Callsigns,
    t_ns: i64,
    stamp: &str,
    safety: Option<&ReplaySafety<'_>>,
    ownership: Option<&OwnershipStatus>,
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
    let ownership_clipped = if let Some(status) = ownership {
        if ownership_status_fits(&r.view.proj, status)? {
            build_ownership_status(&mut r.list, &r.view.proj, status)?;
            false
        } else {
            true
        }
    } else {
        false
    };
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
        ownership_clipped,
    })
}

/// Draws the list into `r.buf`.
pub fn replay_raster(r: &mut Replay) -> Result<(), ReplayError> {
    rasterize(&r.list, &r.spec, &mut r.buf)?;
    Ok(())
}
