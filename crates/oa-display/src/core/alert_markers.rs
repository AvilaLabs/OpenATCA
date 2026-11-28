//! Current MSAW alert markers (**core**, B1, A3): bounded plain observations to display-list
//! primitives. This presenter does not decide alert state or move a marker to an alert origin.
//!
//! At most [`MAX_ALERT_MARKERS`] whole 16-byte records (2,048 bytes) are drawn per call; the
//! budgeted APIs validate the complete borrowed input and report any undisplayed tail. Records
//! must be ordered by unique positive aircraft address. Validation, view-width checks and
//! list-capacity checks finish before any display-list mutation. Renderer-owned budgets use the
//! same typed headings and font metrics as presentation, including a dedicated overflow row.

use crate::core::list::{push_circle, push_text, DisplayError, DisplayList, ALIGN_CENTRE};
use crate::core::palette;
use crate::core::project::{project, Projection};
use crate::core::status_strip::{
    regional_status_plan, RegionalStatusPlan, StatusStripPlacement, StatusStripRow,
};
use crate::core::text::{text_new, text_push_dec, text_push_str, text_str};

/// Fixed records read by one presentation call. The regional bridge uses a smaller limit of 32.
pub const MAX_ALERT_MARKERS: usize = 128;
/// Worst case circle table reservation for one marker (outer and inner rings).
pub const ALERT_CIRCLES_PER_MARKER: usize = 2;
/// Each marker reserves room for one short state label, whether or not it is visible.
pub const ALERT_TEXTS_PER_MARKER: usize = 1;
/// Header text rows reserved independently of their current decimal values.
pub const ALERT_HEADER_TEXTS: usize = 1;
/// MSAW header reservation at the 128-marker count bound.
pub const MSAW_HEADER_TEXT_BYTES: usize = 64;
/// Maximum final-degraded header size at the 128-marker count bounds.
pub const FINAL_MONITOR_HEADER_TEXT_BYTES: usize = 77;
/// Largest typed alert header reservation, retained for callers reserving either mode.
pub const ALERT_HEADER_TEXT_BYTES: usize = FINAL_MONITOR_HEADER_TEXT_BYTES;
pub const ALERT_TEXT_BYTES_PER_MARKER: usize = 1;
/// Sentinel used when `position_known` is zero.
pub const NO_POSITION_E7: i32 = i32::MIN;
const ALERT_LAYER: u8 = 13;
const ALERT_TEXT_SIZE: u8 = 10;
const ALERT_OUTER_RADIUS_PX: i32 = 16;
const ALERT_INNER_RADIUS_PX: i32 = 12;
const MIN_ALERT_VIEW_WIDTH: u32 = 500;

#[derive(Clone, Copy)]
enum AlertHeaderSource {
    Msaw,
    FinalMonitor { degraded: bool },
}

/// Typed heading and overflow identity used by admission and frame presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertRenderKind {
    Msaw,
    FinalMonitor { degraded: bool },
}

impl From<AlertRenderKind> for AlertHeaderSource {
    fn from(kind: AlertRenderKind) -> Self {
        match kind {
            AlertRenderKind::Msaw => Self::Msaw,
            AlertRenderKind::FinalMonitor { degraded } => Self::FinalMonitor { degraded },
        }
    }
}

/// Fixed display-list and geometry needs for one alert family at an admitted input/allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertRenderBudget {
    pub circles: usize,
    pub texts: usize,
    pub text_bytes: usize,
    pub minimum_width_px: u32,
    pub minimum_top_inset_px: u32,
}

const fn px(value: i32) -> i32 {
    value * 16
}

/// MSAW state, shown with shape and a one-letter label as well as alert colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MarkerState {
    Caution = 0,
    Warning = 1,
    ProtectionLost = 2,
}
const _: () = assert!(core::mem::size_of::<MarkerState>() == 1);

/// One current MSAW alert and its current observed position. Unknown position never becomes a
/// guessed point; a known position is projected at the observation coordinates supplied here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AlertMarker {
    pub address: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub state: MarkerState,
    pub position_known: u8,
    pub reserved: [u8; 2],
}
const _: () = assert!(core::mem::size_of::<AlertMarker>() == 16);

/// Counts for one presentation call. `examined` counts supplied retained rows; aggregate severity
/// counts include all validated supplied rows; `omitted` also includes checked producer-reported
/// rows that could not be retained. `drawn` counts selected visible points within the budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct MarkerStats {
    pub examined: u64,
    pub drawn: u64,
    pub off_map: u64,
    pub unlocated: u64,
    pub caution: u64,
    pub warning: u64,
    pub protection_lost: u64,
    pub omitted: u64,
}
const _: () = assert!(core::mem::size_of::<MarkerStats>() == 64);

// A7.2: cold struct, size guard waived (one bounded SoA selection bundle per presentation call).
struct ValidatedMarkerSet {
    stats: MarkerStats,
    states: [MarkerState; MAX_ALERT_MARKERS],
    projected: [Option<(i32, i32)>; MAX_ALERT_MARKERS],
    visible_count: usize,
    visible_high_severity: usize,
}

#[derive(Clone, Copy)]
struct AlertPresentationMode {
    header_source: AlertHeaderSource,
    overflow_reserved: bool,
}

fn bad_alert(what: &'static str) -> DisplayError {
    DisplayError::BadView(what)
}

fn final_monitor_severity(value: u8) -> Result<FinalMonitorSeverity, DisplayError> {
    match value {
        0 => Ok(FinalMonitorSeverity::Caution),
        1 => Ok(FinalMonitorSeverity::Warning),
        2 => Ok(FinalMonitorSeverity::ProtectionLost),
        _ => Err(bad_alert(
            "final monitor severity must be zero, one, or two",
        )),
    }
}

fn ensure_room(
    table: &'static str,
    used: usize,
    needed: usize,
    capacity: usize,
) -> Result<(), DisplayError> {
    if used
        .checked_add(needed)
        .is_some_and(|requested| requested <= capacity)
    {
        Ok(())
    } else {
        Err(DisplayError::Full { table, capacity })
    }
}

fn position_on_map(p: &Projection, (x, y): (i32, i32)) -> bool {
    let x = i64::from(x);
    let y = i64::from(y);
    // Classification follows the supplied position, not whether the full warning halo fits.
    // The rasterizer clips a marker centered inside the map at the frame edge.
    x >= 0
        && x < i64::from(p.width) * 16
        && y >= i64::from(p.top_inset) * 16
        && y < i64::from(p.height) * 16
}

fn increment(counter: &mut u64) -> Result<(), DisplayError> {
    *counter = counter.checked_add(1).ok_or(DisplayError::TooLarge {
        what: "alert marker aggregate count",
        requested: u128::MAX,
        limit: u64::MAX as u128,
    })?;
    Ok(())
}

fn header(stats: MarkerStats, source: AlertHeaderSource) -> crate::core::text::TextBuf {
    let mut text = text_new();
    match source {
        AlertHeaderSource::Msaw => text_push_str(&mut text, "MSAW"),
        AlertHeaderSource::FinalMonitor { degraded: false } => text_push_str(&mut text, "FINAL"),
        AlertHeaderSource::FinalMonitor { degraded: true } => {
            text_push_str(&mut text, "FINAL DEGRADED")
        }
    }
    text_push_str(&mut text, " WARNING ");
    text_push_dec(&mut text, stats.warning, 0);
    text_push_str(&mut text, " CAUTION ");
    text_push_dec(&mut text, stats.caution, 0);
    match source {
        AlertHeaderSource::Msaw => text_push_str(&mut text, " LOST "),
        AlertHeaderSource::FinalMonitor { .. } => text_push_str(&mut text, " PROTECTION LOST "),
    }
    text_push_dec(&mut text, stats.protection_lost, 0);
    text_push_str(&mut text, " OFF ");
    text_push_dec(&mut text, stats.off_map, 0);
    text_push_str(&mut text, " UNLOCATED ");
    text_push_dec(&mut text, stats.unlocated, 0);
    text
}

fn partial_header(
    source: AlertHeaderSource,
    has_protection_lost: bool,
) -> crate::core::text::TextBuf {
    let mut text = text_new();
    match source {
        AlertHeaderSource::Msaw if has_protection_lost => {
            text_push_str(&mut text, "MSAW PROTECTION LOST PARTIAL")
        }
        AlertHeaderSource::Msaw => text_push_str(&mut text, "MSAW PARTIAL"),
        AlertHeaderSource::FinalMonitor { degraded: false } => {
            if has_protection_lost {
                text_push_str(&mut text, "FINAL PROTECTION LOST PARTIAL")
            } else {
                text_push_str(&mut text, "FINAL PARTIAL")
            }
        }
        AlertHeaderSource::FinalMonitor { degraded: true } => {
            text_push_str(&mut text, "FINAL DEGRADED PROTECTION LOST PARTIAL");
        }
    }
    text
}

fn required_row_width_px(
    placement: StatusStripPlacement,
    text: &crate::core::text::TextBuf,
) -> Result<u32, DisplayError> {
    if crate::core::text::text_clipped(text) {
        return Err(bad_alert("status row exceeds its fixed text buffer"));
    }
    let bounds = placement.bounds_for(text_str(text))?;
    u32::try_from(bounds.x1).map_err(|_| DisplayError::TooLarge {
        what: "status row width",
        requested: u128::from(bounds.x1.unsigned_abs()),
        limit: u32::MAX as u128,
    })
}

fn overflow_text(source: AlertHeaderSource, omitted: u64) -> crate::core::text::TextBuf {
    let mut text = text_new();
    match source {
        AlertHeaderSource::Msaw => text_push_str(&mut text, "MSAW +"),
        AlertHeaderSource::FinalMonitor { .. } => text_push_str(&mut text, "FINAL +"),
    }
    text_push_dec(&mut text, omitted, 0);
    text_push_str(&mut text, " NOT SHOWN");
    text
}

fn maximum_header_stats(source: AlertHeaderSource, total: usize) -> MarkerStats {
    let mut category_best = MarkerStats::default();
    let mut category_len = 0;
    for warning in 0..=total {
        for caution in 0..=total - warning {
            let lost = total - warning - caution;
            let candidate = MarkerStats {
                warning: u64::try_from(warning).unwrap_or(u64::MAX),
                caution: u64::try_from(caution).unwrap_or(u64::MAX),
                protection_lost: u64::try_from(lost).unwrap_or(u64::MAX),
                ..MarkerStats::default()
            };
            let candidate_len = text_str(&header(candidate, source)).len();
            if candidate_len > category_len {
                category_best = candidate;
                category_len = candidate_len;
            }
        }
    }
    let mut best = category_best;
    let mut best_len = 0;
    for off_map in 0..=total {
        for unlocated in 0..=total - off_map {
            let mut candidate = category_best;
            candidate.off_map = u64::try_from(off_map).unwrap_or(u64::MAX);
            candidate.unlocated = u64::try_from(unlocated).unwrap_or(u64::MAX);
            let candidate_len = text_str(&header(candidate, source)).len();
            if candidate_len > best_len {
                best = candidate;
                best_len = candidate_len;
            }
        }
    }
    best
}

/// Derive the family's display-list needs and minimum view geometry from its real headings.
///
/// `marker_budget` bounds drawn rows. The visible overflow row is always reserved, including for
/// unexpected input beyond the producer's declared bound.
pub fn alert_render_budget(
    kind: AlertRenderKind,
    marker_budget: usize,
) -> Result<AlertRenderBudget, DisplayError> {
    if marker_budget > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "alert rendering budget",
            requested: marker_budget as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    let source = AlertHeaderSource::from(kind);
    let plan = regional_status_plan()?;
    let (heading_row, overflow_row) = alert_status_rows(&plan, source)?;
    let draws = marker_budget;
    let mut minimum_width_px = 0;
    let mut header_bytes = 0;
    for total in 0..=draws {
        let heading = header(maximum_header_stats(source, total), source);
        minimum_width_px = minimum_width_px.max(required_row_width_px(heading_row, &heading)?);
        header_bytes = header_bytes.max(text_str(&heading).len());
    }
    for has_protection_lost in [false, true] {
        let partial = partial_header(source, has_protection_lost);
        minimum_width_px = minimum_width_px.max(required_row_width_px(heading_row, &partial)?);
        header_bytes = header_bytes.max(text_str(&partial).len());
    }
    // The displayed omitted count is exact for every supported target's representable slice size.
    let overflow = overflow_text(source, u64::MAX);
    minimum_width_px = minimum_width_px.max(required_row_width_px(overflow_row, &overflow)?);
    let overflow_bytes = text_str(&overflow).len();
    let circles = draws
        .checked_mul(ALERT_CIRCLES_PER_MARKER)
        .ok_or(DisplayError::TooLarge {
            what: "alert circle budget",
            requested: u128::MAX,
            limit: u128::from(u32::MAX),
        })?;
    let texts = draws
        .checked_mul(ALERT_TEXTS_PER_MARKER)
        .and_then(|count| count.checked_add(ALERT_HEADER_TEXTS))
        .and_then(|count| count.checked_add(1))
        .ok_or(DisplayError::TooLarge {
            what: "alert text budget",
            requested: u128::MAX,
            limit: u128::from(u32::MAX),
        })?;
    let reserved_header_bytes = match source {
        AlertHeaderSource::Msaw => MSAW_HEADER_TEXT_BYTES,
        AlertHeaderSource::FinalMonitor { .. } => FINAL_MONITOR_HEADER_TEXT_BYTES,
    };
    let text_bytes = draws
        .checked_mul(ALERT_TEXT_BYTES_PER_MARKER)
        .and_then(|count| count.checked_add(header_bytes.max(reserved_header_bytes)))
        .and_then(|count| count.checked_add(overflow_bytes))
        .ok_or(DisplayError::TooLarge {
            what: "alert text byte budget",
            requested: u128::MAX,
            limit: u128::from(u32::MAX),
        })?;
    Ok(AlertRenderBudget {
        circles,
        texts,
        text_bytes,
        minimum_width_px,
        minimum_top_inset_px: plan.top_inset_px(),
    })
}

/// Final-monitor severity in the presentation packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FinalMonitorSeverity {
    Caution = 0,
    Warning = 1,
    ProtectionLost = 2,
}
const _: () = assert!(core::mem::size_of::<FinalMonitorSeverity>() == 1);

/// Plain final-monitor packet. Unknown positions use the shared E7 sentinels; this presentation
/// adapter has no dependency on oa-runway or its decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FinalMonitorMarker {
    pub address: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Encoded `FinalMonitorSeverity` discriminant; unknown values are refused.
    pub severity: u8,
    pub position_known: u8,
    pub reserved: [u8; 2],
}
const _: () = assert!(core::mem::size_of::<FinalMonitorMarker>() == 16);

/// Append current alert markers and their bounded count header to an existing display list.
///
/// The view must provide the shared regional status-strip inset and at least 500 px of width.
/// Exact row width and list capacity requirements are checked before writing anything.
pub fn append_alert_markers(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
) -> Result<MarkerStats, DisplayError> {
    if markers.len() > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "MSAW alert marker records",
            requested: markers.len() as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    append_alert_markers_titled(
        list,
        projection,
        markers,
        AlertHeaderSource::Msaw,
        MAX_ALERT_MARKERS,
        false,
        0,
    )
}

/// Validate every MSAW marker, draw the allocated prefix, and report any omitted rows visibly.
/// The borrowed producer slice may exceed [`MAX_ALERT_MARKERS`]; only rendered scratch is capped.
pub fn append_alert_markers_with_budget(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
    marker_budget: usize,
) -> Result<MarkerStats, DisplayError> {
    append_alert_markers_with_budget_and_producer_omissions(
        list,
        projection,
        markers,
        marker_budget,
        0,
    )
}

/// Like [`append_alert_markers_with_budget`], adding the provider's count of new violations that
/// its retained open table refused. This count affects only `omitted`; it does not fabricate
/// examined rows, severity, or positions. Callers must exclude ordinary closed episodes and log
/// loss to avoid double-counting. The sum is checked before any display-list mutation.
pub fn append_alert_markers_with_budget_and_producer_omissions(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
    marker_budget: usize,
    producer_omitted: u64,
) -> Result<MarkerStats, DisplayError> {
    append_alert_markers_titled(
        list,
        projection,
        markers,
        AlertHeaderSource::Msaw,
        marker_budget,
        true,
        producer_omitted,
    )
}

/// Append final-monitor packets using the existing alert marker palette, symbols, and bounded
/// layout. The packet remains a plain shared value and its closed severity type is mapped before
/// drawing.
pub fn append_final_monitor_markers(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[FinalMonitorMarker],
) -> Result<MarkerStats, DisplayError> {
    append_final_monitor_frame(list, projection, markers, false)
}

/// Append the final-monitor alert packets and expose a stale-feed degraded state in its header.
pub fn append_final_monitor_frame(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[FinalMonitorMarker],
    degraded: bool,
) -> Result<MarkerStats, DisplayError> {
    if markers.len() > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "final monitor marker records",
            requested: markers.len() as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    append_final_monitor_frame_impl(
        list,
        projection,
        markers,
        degraded,
        MAX_ALERT_MARKERS,
        false,
    )
}

/// Validate every final-monitor marker, draw the allocated prefix, and report omitted rows.
/// The borrowed producer slice may exceed [`MAX_ALERT_MARKERS`]; only rendered scratch is capped.
pub fn append_final_monitor_frame_with_budget(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[FinalMonitorMarker],
    degraded: bool,
    marker_budget: usize,
) -> Result<MarkerStats, DisplayError> {
    append_final_monitor_frame_impl(list, projection, markers, degraded, marker_budget, true)
}

fn append_final_monitor_frame_impl(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[FinalMonitorMarker],
    degraded: bool,
    marker_budget: usize,
    overflow_reserved: bool,
) -> Result<MarkerStats, DisplayError> {
    let plan = regional_status_plan()?;
    if (!overflow_reserved && projection.width < MIN_ALERT_VIEW_WIDTH)
        || projection.top_inset < plan.top_inset_px()
        || projection.height <= projection.top_inset
    {
        return Err(bad_alert(
            "final monitor markers do not fit the reserved view",
        ));
    }
    if marker_budget > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "final monitor draw budget",
            requested: marker_budget as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    let source = AlertHeaderSource::FinalMonitor { degraded };
    let validated = validate_and_project_markers(
        projection,
        markers.len(),
        markers.iter().map(final_marker_as_alert),
        marker_budget,
        0,
    )?;
    let visible = validated
        .states
        .get(..validated.visible_count)
        .ok_or_else(|| bad_alert("final monitor visible state extent"))?;
    append_validated_markers(
        list,
        projection,
        visible,
        validated.stats,
        &validated.projected,
        AlertPresentationMode {
            header_source: source,
            overflow_reserved,
        },
    )
}

fn final_marker_as_alert(marker: &FinalMonitorMarker) -> Result<AlertMarker, DisplayError> {
    let severity = final_monitor_severity(marker.severity)?;
    Ok(AlertMarker {
        address: marker.address,
        lat_e7: marker.lat_e7,
        lon_e7: marker.lon_e7,
        state: match severity {
            FinalMonitorSeverity::Caution => MarkerState::Caution,
            FinalMonitorSeverity::Warning => MarkerState::Warning,
            FinalMonitorSeverity::ProtectionLost => MarkerState::ProtectionLost,
        },
        position_known: marker.position_known,
        reserved: marker.reserved,
    })
}

fn validate_marker_position(
    projection: &Projection,
    marker: AlertMarker,
    stats: &mut MarkerStats,
) -> Result<Option<(i32, i32)>, DisplayError> {
    match marker.position_known {
        0 => {
            if marker.lat_e7 != NO_POSITION_E7 || marker.lon_e7 != NO_POSITION_E7 {
                return Err(bad_alert("unknown MSAW position must use both sentinels"));
            }
            increment(&mut stats.unlocated)?;
            Ok(None)
        }
        1 => {
            if !(-900_000_000..=900_000_000).contains(&marker.lat_e7)
                || !(-1_800_000_000..=1_800_000_000).contains(&marker.lon_e7)
            {
                return Err(bad_alert(
                    "known MSAW position is outside coordinate bounds",
                ));
            }
            match project(projection, marker.lat_e7, marker.lon_e7) {
                Some(point) if position_on_map(projection, point) => Ok(Some(point)),
                _ => {
                    increment(&mut stats.off_map)?;
                    Ok(None)
                }
            }
        }
        _ => Err(bad_alert("MSAW position-known flag must be zero or one")),
    }
}

fn select_visible_marker(
    selected: &mut ValidatedMarkerSet,
    state: MarkerState,
    point: Option<(i32, i32)>,
    marker_budget: usize,
    prioritize: bool,
) -> Result<(), DisplayError> {
    let high_severity = matches!(state, MarkerState::Warning | MarkerState::ProtectionLost);
    let selected_index = if prioritize && high_severity {
        if selected.visible_high_severity >= marker_budget {
            None
        } else {
            let insertion = selected.visible_high_severity;
            let next_count = selected.visible_count.saturating_add(1).min(marker_budget);
            if selected.visible_count == marker_budget
                && selected
                    .projected
                    .get(selected.visible_count.saturating_sub(1))
                    .copied()
                    .flatten()
                    .is_some()
            {
                selected.stats.drawn = selected
                    .stats
                    .drawn
                    .checked_sub(1)
                    .ok_or_else(|| bad_alert("selected marker count underflow"))?;
            }
            for destination in (insertion.saturating_add(1)..next_count).rev() {
                let source = destination
                    .checked_sub(1)
                    .ok_or_else(|| bad_alert("marker selection source underflow"))?;
                let prior_state = *selected
                    .states
                    .get(source)
                    .ok_or_else(|| bad_alert("selected source state index"))?;
                let prior_point = *selected
                    .projected
                    .get(source)
                    .ok_or_else(|| bad_alert("selected source projection index"))?;
                *selected
                    .states
                    .get_mut(destination)
                    .ok_or_else(|| bad_alert("selected destination state index"))? = prior_state;
                *selected
                    .projected
                    .get_mut(destination)
                    .ok_or_else(|| bad_alert("selected destination projection index"))? =
                    prior_point;
            }
            selected.visible_count = next_count;
            selected.visible_high_severity += 1;
            Some(insertion)
        }
    } else if !prioritize || !high_severity {
        if selected.visible_count < marker_budget {
            let index = selected.visible_count;
            selected.visible_count += 1;
            Some(index)
        } else {
            None
        }
    } else {
        None
    };
    if let Some(index) = selected_index {
        *selected
            .states
            .get_mut(index)
            .ok_or_else(|| bad_alert("alert visible state index"))? = state;
        *selected
            .projected
            .get_mut(index)
            .ok_or_else(|| bad_alert("alert projection index"))? = point;
        if point.is_some() {
            increment(&mut selected.stats.drawn)?;
        }
    }
    Ok(())
}

fn validate_and_project_markers<I>(
    projection: &Projection,
    input_count: usize,
    markers: I,
    marker_budget: usize,
    producer_omitted: u64,
) -> Result<ValidatedMarkerSet, DisplayError>
where
    I: IntoIterator<Item = Result<AlertMarker, DisplayError>>,
{
    let renderer_omitted = input_count.saturating_sub(marker_budget);
    let renderer_omitted_u64 =
        u64::try_from(renderer_omitted).map_err(|_| DisplayError::TooLarge {
            what: "omitted alert marker count",
            requested: renderer_omitted as u128,
            limit: u64::MAX as u128,
        })?;
    let requested_omitted = u128::from(renderer_omitted_u64) + u128::from(producer_omitted);
    let omitted =
        renderer_omitted_u64
            .checked_add(producer_omitted)
            .ok_or(DisplayError::TooLarge {
                what: "combined omitted alert marker count",
                requested: requested_omitted,
                limit: u64::MAX as u128,
            })?;
    let stats = MarkerStats {
        omitted,
        ..MarkerStats::default()
    };
    let states = [MarkerState::Caution; MAX_ALERT_MARKERS];
    let projected = [None; MAX_ALERT_MARKERS];
    let mut selected = ValidatedMarkerSet {
        stats,
        states,
        projected,
        visible_count: 0,
        visible_high_severity: 0,
    };
    let prioritize = input_count > marker_budget;
    let mut seen = 0usize;
    let mut previous_address = 0u32;
    for (index, item) in markers.into_iter().enumerate() {
        if index >= input_count {
            return Err(bad_alert(
                "alert input iterator exceeded its declared length",
            ));
        }
        let marker = item?;
        seen = index + 1;
        increment(&mut selected.stats.examined)?;
        if marker.address == 0 || marker.address <= previous_address {
            return Err(bad_alert(
                "MSAW addresses must be positive, unique and increasing",
            ));
        }
        previous_address = marker.address;
        if marker.reserved != [0; 2] {
            return Err(bad_alert("MSAW marker reserved bytes are nonzero"));
        }
        match marker.state {
            MarkerState::Caution => increment(&mut selected.stats.caution)?,
            MarkerState::Warning => increment(&mut selected.stats.warning)?,
            MarkerState::ProtectionLost => increment(&mut selected.stats.protection_lost)?,
        }
        let point = validate_marker_position(projection, marker, &mut selected.stats)?;
        select_visible_marker(
            &mut selected,
            marker.state,
            point,
            marker_budget,
            prioritize,
        )?;
    }
    if seen != input_count {
        return Err(bad_alert("alert input iterator length changed"));
    }
    Ok(selected)
}

fn checked_header(
    projection: &Projection,
    stats: MarkerStats,
    source: AlertHeaderSource,
    plan: &RegionalStatusPlan,
) -> Result<(crate::core::text::TextBuf, StatusStripPlacement), DisplayError> {
    let text = if stats.omitted > 0 {
        partial_header(source, stats.protection_lost > 0)
    } else {
        header(stats, source)
    };
    let (placement, _) = alert_status_rows(plan, source)?;
    if projection.width < required_row_width_px(placement, &text)? {
        return Err(bad_alert("alert count heading does not fit the view width"));
    }
    Ok((text, placement))
}

fn alert_status_rows(
    plan: &RegionalStatusPlan,
    source: AlertHeaderSource,
) -> Result<(StatusStripPlacement, StatusStripPlacement), DisplayError> {
    let (heading, overflow) = match source {
        AlertHeaderSource::Msaw => (StatusStripRow::MsawHeading, StatusStripRow::MsawOverflow),
        AlertHeaderSource::FinalMonitor { .. } => {
            (StatusStripRow::FinalHeading, StatusStripRow::FinalOverflow)
        }
    };
    let heading = plan
        .row(heading)
        .ok_or_else(|| bad_alert("alert heading row is unavailable"))?;
    let overflow = plan
        .row(overflow)
        .ok_or_else(|| bad_alert("alert overflow row is unavailable"))?;
    Ok((heading, overflow))
}

fn append_status_text(
    list: &mut DisplayList,
    placement: StatusStripPlacement,
    text: &str,
) -> Result<(), DisplayError> {
    placement.bounds_for(text)?;
    push_text(
        list,
        (placement.x_px() * 16, placement.y_px() * 16),
        placement.size(),
        placement.align(),
        placement.colour(),
        placement.layer(),
        text,
    )
}

fn reserve_marker_output(
    list: &DisplayList,
    marker_count: usize,
    source: AlertHeaderSource,
    header_bytes: usize,
    overflow_bytes: usize,
) -> Result<(), DisplayError> {
    let reserved_header_bytes = match source {
        AlertHeaderSource::Msaw => MSAW_HEADER_TEXT_BYTES,
        AlertHeaderSource::FinalMonitor { .. } => FINAL_MONITOR_HEADER_TEXT_BYTES,
    };
    let header_bytes = header_bytes.max(reserved_header_bytes);
    let needed_circles =
        marker_count
            .checked_mul(ALERT_CIRCLES_PER_MARKER)
            .ok_or(DisplayError::TooLarge {
                what: "MSAW circle reservation",
                requested: u128::MAX,
                limit: list.caps.circles as u128,
            })?;
    let needed_texts = marker_count
        .checked_mul(ALERT_TEXTS_PER_MARKER)
        .and_then(|count| count.checked_add(ALERT_HEADER_TEXTS))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW text reservation",
            requested: u128::MAX,
            limit: list.caps.texts as u128,
        })?;
    let needed_text_bytes = marker_count
        .checked_mul(ALERT_TEXT_BYTES_PER_MARKER)
        .and_then(|count| count.checked_add(header_bytes))
        .and_then(|count| count.checked_add(overflow_bytes))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW text byte reservation",
            requested: u128::MAX,
            limit: list.caps.text_bytes as u128,
        })?;
    ensure_room(
        "circles",
        list.circles.x.len(),
        needed_circles,
        list.caps.circles,
    )?;
    let needed_texts = needed_texts
        .checked_add(if overflow_bytes > 0 { 1 } else { 0 })
        .ok_or(DisplayError::TooLarge {
            what: "MSAW overflow text reservation",
            requested: u128::MAX,
            limit: list.caps.texts as u128,
        })?;
    ensure_room("texts", list.texts.x.len(), needed_texts, list.caps.texts)?;
    ensure_room(
        "text bytes",
        list.text.len(),
        needed_text_bytes,
        list.caps.text_bytes,
    )
}

fn append_marker_geometry(
    list: &mut DisplayList,
    states: &[MarkerState],
    projected: &[Option<(i32, i32)>; MAX_ALERT_MARKERS],
) -> Result<(), DisplayError> {
    for (index, state) in states.iter().enumerate() {
        let Some(point) = projected.get(index).copied().flatten() else {
            continue;
        };
        match state {
            MarkerState::Caution => push_circle(
                list,
                point,
                px(ALERT_INNER_RADIUS_PX),
                palette::ALERT,
                ALERT_LAYER,
            )?,
            MarkerState::Warning | MarkerState::ProtectionLost => {
                push_circle(
                    list,
                    point,
                    px(ALERT_OUTER_RADIUS_PX),
                    palette::ALERT,
                    ALERT_LAYER,
                )?;
                push_circle(
                    list,
                    point,
                    px(ALERT_INNER_RADIUS_PX),
                    palette::ALERT,
                    ALERT_LAYER,
                )?;
            }
        }
        let label = match state {
            MarkerState::Caution => "C",
            MarkerState::Warning => "W",
            MarkerState::ProtectionLost => "L",
        };
        push_text(
            list,
            (point.0, point.1 - px(i32::from(ALERT_TEXT_SIZE) / 2)),
            ALERT_TEXT_SIZE,
            ALIGN_CENTRE,
            palette::ALERT,
            ALERT_LAYER,
            label,
        )?;
    }
    Ok(())
}

fn append_alert_markers_titled(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
    header_source: AlertHeaderSource,
    marker_budget: usize,
    overflow_reserved: bool,
    producer_omitted: u64,
) -> Result<MarkerStats, DisplayError> {
    let plan = regional_status_plan()?;
    if (!overflow_reserved && projection.width < MIN_ALERT_VIEW_WIDTH)
        || projection.top_inset < plan.top_inset_px()
        || projection.height <= projection.top_inset
    {
        return Err(bad_alert(if overflow_reserved {
            "budgeted alert markers require the reserved overflow strip"
        } else {
            "MSAW markers require width 500 and the shared status strip"
        }));
    }
    if list.faulted {
        return Err(bad_alert("MSAW markers cannot append to a faulted list"));
    }
    if marker_budget > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "alert marker draw budget",
            requested: marker_budget as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    let validated = validate_and_project_markers(
        projection,
        markers.len(),
        markers.iter().copied().map(Ok),
        marker_budget,
        producer_omitted,
    )?;
    let drawn_states = validated
        .states
        .get(..validated.visible_count)
        .ok_or_else(|| bad_alert("alert state draw extent"))?;
    append_validated_markers(
        list,
        projection,
        drawn_states,
        validated.stats,
        &validated.projected,
        AlertPresentationMode {
            header_source,
            overflow_reserved,
        },
    )
}

fn append_validated_markers(
    list: &mut DisplayList,
    projection: &Projection,
    drawn_states: &[MarkerState],
    stats: MarkerStats,
    projected: &[Option<(i32, i32)>; MAX_ALERT_MARKERS],
    mode: AlertPresentationMode,
) -> Result<MarkerStats, DisplayError> {
    if list.faulted {
        return Err(bad_alert("alert markers cannot append to a faulted list"));
    }
    let plan = regional_status_plan()?;
    let (header_text, heading_row) = checked_header(projection, stats, mode.header_source, &plan)?;
    let (_, overflow_row) = alert_status_rows(&plan, mode.header_source)?;
    let overflow = (stats.omitted > 0).then(|| overflow_text(mode.header_source, stats.omitted));
    let overflow_bytes = overflow.as_ref().map_or(0, |text| text_str(text).len());
    if let Some(text) = overflow.as_ref() {
        if !mode.overflow_reserved
            || crate::core::text::text_clipped(text)
            || projection.width < required_row_width_px(overflow_row, text)?
        {
            return Err(bad_alert(
                "alert overflow indication does not fit the reserved view",
            ));
        }
    }
    reserve_marker_output(
        list,
        drawn_states.len(),
        mode.header_source,
        text_str(&header_text).len(),
        overflow_bytes,
    )?;

    // All input, header and list budgets are checked before the first display-list write.
    append_marker_geometry(list, drawn_states, projected)?;
    append_status_text(list, heading_row, text_str(&header_text))?;
    if let Some(text) = overflow.as_ref() {
        append_status_text(list, overflow_row, text_str(text))?;
    }
    Ok(stats)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod position_boundary_tests {
    use super::*;
    use crate::core::project::projection_new;

    // Mutation killed: use closed right or bottom edges, or change the 1/16-pixel map bounds.
    // REQ: OA-DSP-077
    #[test]
    fn marker_centres_use_half_open_map_bounds_in_fixed_point_pixels() {
        let projection = projection_new(0, 0, 20.0, 800, 600, 160).unwrap();
        let left = 0;
        let top = 160 * 16;
        let right = 800 * 16;
        let bottom = 600 * 16;

        assert!(position_on_map(&projection, (left, top)));
        assert!(position_on_map(&projection, (right - 1, bottom - 1)));
        assert!(!position_on_map(&projection, (left - 1, top)));
        assert!(!position_on_map(&projection, (right, top)));
        assert!(!position_on_map(&projection, (left, top - 1)));
        assert!(!position_on_map(&projection, (left, bottom)));
    }
}
