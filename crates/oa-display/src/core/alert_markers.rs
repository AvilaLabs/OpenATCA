//! Current MSAW alert markers (**core**, B1, A3): bounded plain observations to display-list
//! primitives. This presenter does not decide alert state or move a marker to an alert origin.
//!
//! At most [`MAX_ALERT_MARKERS`] whole 16-byte records (2,048 bytes) are read per call; the
//! regional bridge currently supplies at most 32. Records must be ordered by unique positive
//! aircraft address. Validation, view-width checks and list-capacity checks finish before any
//! display-list mutation. Renderer-owned budgets use the same typed headings and font metrics
//! as presentation. When an input list exceeds its marker allocation, a dedicated overflow row
//! identifies the family and omitted total.

use crate::core::font::{text_width_units, CAP_HEIGHT};
use crate::core::list::{
    push_circle, push_text, DisplayError, DisplayList, ALIGN_CENTRE, ALIGN_LEFT,
};
use crate::core::palette;
use crate::core::project::{project, Projection};
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
/// Top strip required when an alert family can need a dedicated overflow row.
pub const ALERT_OVERFLOW_STATUS_INSET_PX: u32 = 252;

/// Sentinel used when `position_known` is zero.
pub const NO_POSITION_E7: i32 = i32::MIN;
const ALERT_LAYER: u8 = 13;
const ALERT_TEXT_SIZE: u8 = 10;
const ALERT_HEADER_X_PX: i32 = 8;
const ALERT_HEADER_Y_PX: i32 = 112;
const ALERT_RESERVED_HEADER_INSET_PX: u32 = 176;
const ALERT_RESERVED_MSAW_HEADER_Y_PX: i32 = 119;
const ALERT_RESERVED_FINAL_HEADER_Y_PX: i32 = 135;
const ALERT_RESERVED_MSAW_OVERFLOW_Y_PX: i32 = 151;
const ALERT_RESERVED_FINAL_OVERFLOW_Y_PX: i32 = 165;
const ALERT_OUTER_RADIUS_PX: i32 = 16;
const ALERT_INNER_RADIUS_PX: i32 = 12;
const MIN_ALERT_VIEW_WIDTH: u32 = 500;
const MIN_ALERT_TOP_INSET: u32 = 128;

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

/// Counts for one bounded presentation call. State counts include alerts off the frame or without
/// a known position; `drawn` counts only alerts with a visible current point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct MarkerStats {
    pub examined: u16,
    pub drawn: u16,
    pub off_map: u16,
    pub unlocated: u16,
    pub caution: u16,
    pub warning: u16,
    pub protection_lost: u16,
    pub omitted: u16,
}
const _: () = assert!(core::mem::size_of::<MarkerStats>() == 16);

type ValidatedMarkerSet = (MarkerStats, [Option<(i32, i32)>; MAX_ALERT_MARKERS]);

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

fn increment(counter: &mut u16) {
    *counter = counter.saturating_add(1);
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
    text_push_dec(&mut text, u64::from(stats.warning), 0);
    text_push_str(&mut text, " CAUTION ");
    text_push_dec(&mut text, u64::from(stats.caution), 0);
    match source {
        AlertHeaderSource::Msaw => text_push_str(&mut text, " LOST "),
        AlertHeaderSource::FinalMonitor { .. } => text_push_str(&mut text, " PROTECTION LOST "),
    }
    text_push_dec(&mut text, u64::from(stats.protection_lost), 0);
    text_push_str(&mut text, " OFF ");
    text_push_dec(&mut text, u64::from(stats.off_map), 0);
    text_push_str(&mut text, " UNLOCATED ");
    text_push_dec(&mut text, u64::from(stats.unlocated), 0);
    text
}

fn required_text_width_px(text: &crate::core::text::TextBuf) -> Result<u32, DisplayError> {
    if crate::core::text::text_clipped(text) {
        return Err(bad_alert("MSAW count header exceeds its fixed text buffer"));
    }
    let subpixel = i64::from(crate::core::project::SUBPIXEL);
    let width16 = i64::try_from(text_width_units(text_str(text).chars().count()))
        .ok()
        .and_then(|width| width.checked_mul(i64::from(ALERT_TEXT_SIZE)))
        .and_then(|width| width.checked_mul(subpixel))
        .and_then(|width| width.checked_div(i64::from(CAP_HEIGHT)))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW header width",
            requested: u128::MAX,
            limit: u128::from(u32::MAX) * 16,
        })?;
    let right_edge16 = i64::from(ALERT_HEADER_X_PX)
        .checked_mul(subpixel)
        .and_then(|origin| origin.checked_add(width16))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW header width",
            requested: u128::MAX,
            limit: u128::from(u32::MAX) * 16,
        })?;
    let minimum_width = right_edge16
        .checked_add(subpixel - 1)
        .and_then(|extent| extent.checked_div(subpixel))
        .and_then(|pixels| u32::try_from(pixels).ok())
        .ok_or(DisplayError::TooLarge {
            what: "MSAW header width",
            requested: u128::MAX,
            limit: u128::from(u32::MAX) * 16,
        })?;
    Ok(minimum_width)
}

fn overflow_text(source: AlertHeaderSource, omitted: usize) -> crate::core::text::TextBuf {
    let mut text = text_new();
    match source {
        AlertHeaderSource::Msaw => text_push_str(&mut text, "MSAW +"),
        AlertHeaderSource::FinalMonitor { .. } => text_push_str(&mut text, "FINAL +"),
    }
    text_push_dec(&mut text, u64::try_from(omitted).unwrap_or(u64::MAX), 0);
    text_push_str(&mut text, " NOT SHOWN");
    text
}

fn maximum_header_stats(source: AlertHeaderSource, total: usize, omitted: usize) -> MarkerStats {
    let mut category_best = MarkerStats {
        omitted: u16::try_from(omitted).unwrap_or(u16::MAX),
        ..MarkerStats::default()
    };
    let mut category_len = 0;
    for warning in 0..=total {
        for caution in 0..=total - warning {
            let lost = total - warning - caution;
            let candidate = MarkerStats {
                warning: u16::try_from(warning).unwrap_or(u16::MAX),
                caution: u16::try_from(caution).unwrap_or(u16::MAX),
                protection_lost: u16::try_from(lost).unwrap_or(u16::MAX),
                omitted: u16::try_from(omitted).unwrap_or(u16::MAX),
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
            candidate.off_map = u16::try_from(off_map).unwrap_or(u16::MAX);
            candidate.unlocated = u16::try_from(unlocated).unwrap_or(u16::MAX);
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
/// `max_input_rows` bounds producer records and `marker_budget` bounds the rows drawn. When the
/// former is larger, the budget includes the family's explicit overflow line and its geometry.
pub fn alert_render_budget(
    kind: AlertRenderKind,
    max_input_rows: usize,
    marker_budget: usize,
) -> Result<AlertRenderBudget, DisplayError> {
    if max_input_rows > MAX_ALERT_MARKERS || marker_budget > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "alert rendering budget",
            requested: max_input_rows.max(marker_budget) as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    let source = AlertHeaderSource::from(kind);
    let draws = max_input_rows.min(marker_budget);
    let may_overflow = max_input_rows > marker_budget;
    let mut minimum_width_px = 0;
    let mut header_bytes = 0;
    let mut overflow_bytes = 0;
    for total in 0..=max_input_rows {
        let omitted = total.saturating_sub(marker_budget);
        let heading = header(maximum_header_stats(source, total, omitted), source);
        minimum_width_px = minimum_width_px.max(required_text_width_px(&heading)?);
        header_bytes = header_bytes.max(text_str(&heading).len());
        if omitted > 0 {
            let overflow = overflow_text(source, omitted);
            minimum_width_px = minimum_width_px.max(required_text_width_px(&overflow)?);
            overflow_bytes = overflow_bytes.max(text_str(&overflow).len());
        }
    }
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
        .and_then(|count| count.checked_add(if may_overflow { 1 } else { 0 }))
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
        minimum_top_inset_px: if may_overflow {
            ALERT_OVERFLOW_STATUS_INSET_PX
        } else {
            ALERT_RESERVED_HEADER_INSET_PX
        },
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
/// The view must provide a 128 px top strip and at least 500 px of width. The header occupies
/// rows 112..124 below the standard 104 px legend. Exact header width
/// and all worst-case list capacity requirements are checked before writing anything.
pub fn append_alert_markers(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
) -> Result<MarkerStats, DisplayError> {
    append_alert_markers_with_budget(list, projection, markers, MAX_ALERT_MARKERS)
}

/// Append MSAW markers within an allocated row budget, reporting any omitted rows visibly.
pub fn append_alert_markers_with_budget(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
    marker_budget: usize,
) -> Result<MarkerStats, DisplayError> {
    append_alert_markers_titled(
        list,
        projection,
        markers,
        AlertHeaderSource::Msaw,
        marker_budget,
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
    append_final_monitor_frame_with_budget(list, projection, markers, degraded, MAX_ALERT_MARKERS)
}

/// Append final-monitor markers within an allocated row budget and report any omitted rows.
pub fn append_final_monitor_frame_with_budget(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[FinalMonitorMarker],
    degraded: bool,
    marker_budget: usize,
) -> Result<MarkerStats, DisplayError> {
    if markers.len() > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "final monitor marker records",
            requested: markers.len() as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    let mut converted = [AlertMarker {
        address: 0,
        lat_e7: NO_POSITION_E7,
        lon_e7: NO_POSITION_E7,
        state: MarkerState::Caution,
        position_known: 0,
        reserved: [0; 2],
    }; MAX_ALERT_MARKERS];
    for (i, marker) in markers.iter().enumerate() {
        let severity = final_monitor_severity(marker.severity)?;
        *converted
            .get_mut(i)
            .ok_or_else(|| bad_alert("final monitor conversion index"))? = AlertMarker {
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
        };
    }
    let converted = converted
        .get(..markers.len())
        .ok_or_else(|| bad_alert("final monitor converted packet extent"))?;
    append_alert_markers_titled(
        list,
        projection,
        converted,
        AlertHeaderSource::FinalMonitor { degraded },
        marker_budget,
    )
}

fn validate_and_project_markers(
    projection: &Projection,
    markers: &[AlertMarker],
    marker_budget: usize,
) -> Result<ValidatedMarkerSet, DisplayError> {
    let examined = u16::try_from(markers.len()).map_err(|_| DisplayError::TooLarge {
        what: "MSAW alert marker count",
        requested: markers.len() as u128,
        limit: MAX_ALERT_MARKERS as u128,
    })?;
    let mut stats = MarkerStats {
        examined,
        omitted: u16::try_from(markers.len().saturating_sub(marker_budget)).map_err(|_| {
            DisplayError::TooLarge {
                what: "omitted alert marker count",
                requested: markers.len().saturating_sub(marker_budget) as u128,
                limit: u16::MAX as u128,
            }
        })?,
        ..MarkerStats::default()
    };
    let mut projected = [None; MAX_ALERT_MARKERS];
    let mut previous_address = 0u32;
    for (index, marker) in markers.iter().enumerate() {
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
            MarkerState::Caution => increment(&mut stats.caution),
            MarkerState::Warning => increment(&mut stats.warning),
            MarkerState::ProtectionLost => increment(&mut stats.protection_lost),
        }
        match marker.position_known {
            0 => {
                if marker.lat_e7 != NO_POSITION_E7 || marker.lon_e7 != NO_POSITION_E7 {
                    return Err(bad_alert("unknown MSAW position must use both sentinels"));
                }
                increment(&mut stats.unlocated);
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
                    Some(point) if position_on_map(projection, point) => {
                        if index < marker_budget {
                            *projected
                                .get_mut(index)
                                .ok_or_else(|| bad_alert("MSAW projection index"))? = Some(point);
                            increment(&mut stats.drawn);
                        }
                    }
                    _ => increment(&mut stats.off_map),
                }
            }
            _ => return Err(bad_alert("MSAW position-known flag must be zero or one")),
        }
    }
    Ok((stats, projected))
}

fn checked_header(
    projection: &Projection,
    stats: MarkerStats,
    source: AlertHeaderSource,
) -> Result<crate::core::text::TextBuf, DisplayError> {
    let text = header(stats, source);
    if projection.width < required_text_width_px(&text)? {
        return Err(bad_alert("MSAW count header does not fit the view width"));
    }
    Ok(text)
}

fn header_y(projection: &Projection, source: AlertHeaderSource) -> i32 {
    if projection.top_inset < ALERT_RESERVED_HEADER_INSET_PX {
        return ALERT_HEADER_Y_PX;
    }
    match source {
        AlertHeaderSource::Msaw => ALERT_RESERVED_MSAW_HEADER_Y_PX,
        AlertHeaderSource::FinalMonitor { .. } => ALERT_RESERVED_FINAL_HEADER_Y_PX,
    }
}

fn overflow_y(source: AlertHeaderSource) -> i32 {
    match source {
        AlertHeaderSource::Msaw => ALERT_RESERVED_MSAW_OVERFLOW_Y_PX,
        AlertHeaderSource::FinalMonitor { .. } => ALERT_RESERVED_FINAL_OVERFLOW_Y_PX,
    }
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
    markers: &[AlertMarker],
    projected: &[Option<(i32, i32)>; MAX_ALERT_MARKERS],
) -> Result<(), DisplayError> {
    for (index, marker) in markers.iter().enumerate() {
        let Some(point) = projected.get(index).copied().flatten() else {
            continue;
        };
        match marker.state {
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
        let label = match marker.state {
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
) -> Result<MarkerStats, DisplayError> {
    if projection.width < MIN_ALERT_VIEW_WIDTH
        || projection.top_inset < MIN_ALERT_TOP_INSET
        || projection.height <= projection.top_inset
    {
        return Err(bad_alert(
            "MSAW markers require width 500 and a 128 px top strip",
        ));
    }
    if list.faulted {
        return Err(bad_alert("MSAW markers cannot append to a faulted list"));
    }
    if markers.len() > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "MSAW alert marker records",
            requested: markers.len() as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    if marker_budget > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "alert marker draw budget",
            requested: marker_budget as u128,
            limit: MAX_ALERT_MARKERS as u128,
        });
    }
    let (stats, projected) = validate_and_project_markers(projection, markers, marker_budget)?;
    let header_text = checked_header(projection, stats, header_source)?;
    let overflow =
        (stats.omitted > 0).then(|| overflow_text(header_source, usize::from(stats.omitted)));
    let overflow_bytes = overflow.as_ref().map_or(0, |text| text_str(text).len());
    if let Some(text) = overflow.as_ref() {
        if crate::core::text::text_clipped(text)
            || projection.width < required_text_width_px(text)?
            || projection.top_inset < ALERT_OVERFLOW_STATUS_INSET_PX
        {
            return Err(bad_alert(
                "alert overflow indication does not fit the reserved view",
            ));
        }
    }
    let drawn_markers = markers
        .get(..markers.len().min(marker_budget))
        .ok_or_else(|| bad_alert("alert marker draw extent"))?;
    reserve_marker_output(
        list,
        drawn_markers.len(),
        header_source,
        text_str(&header_text).len(),
        overflow_bytes,
    )?;

    // All input, header and list budgets are checked before the first display-list write.
    append_marker_geometry(list, drawn_markers, &projected)?;
    push_text(
        list,
        (
            px(ALERT_HEADER_X_PX),
            px(header_y(projection, header_source)),
        ),
        ALERT_TEXT_SIZE,
        ALIGN_LEFT,
        palette::ALERT,
        15,
        text_str(&header_text),
    )?;
    if let Some(text) = overflow.as_ref() {
        push_text(
            list,
            (px(ALERT_HEADER_X_PX), px(overflow_y(header_source))),
            ALERT_TEXT_SIZE,
            ALIGN_LEFT,
            palette::ALERT,
            15,
            text_str(text),
        )?;
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
