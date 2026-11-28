//! Current MSAW marker presentation and bounded refusal behavior.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::core::alert_markers::*;
use oa_display::core::status_strip::{regional_status_plan, StatusStripRow};
use oa_display::list::{list_new, DisplayError, DisplayList, ListCaps};
use oa_display::palette;
use oa_display::project::{project, projection_new, unproject, Projection};
use oa_display::raster::{frame_len, rasterize, FrameSpec};

const CENTRE: (i32, i32) = (419_786_000, -879_048_000);

fn projection(width: u32, top_inset: u32) -> Projection {
    projection_new(CENTRE.0, CENTRE.1, 20.0, width, 600, top_inset).unwrap()
}

fn regional_projection(width: u32) -> Projection {
    let inset = regional_status_plan().unwrap().top_inset_px();
    projection(width, inset)
}

fn marker(address: u32, state: MarkerState, lat_e7: i32, lon_e7: i32) -> AlertMarker {
    AlertMarker {
        address,
        lat_e7,
        lon_e7,
        state,
        position_known: 1,
        reserved: [0; 2],
    }
}

fn unknown_marker(address: u32, state: MarkerState) -> AlertMarker {
    AlertMarker {
        address,
        lat_e7: NO_POSITION_E7,
        lon_e7: NO_POSITION_E7,
        state,
        position_known: 0,
        reserved: [0; 2],
    }
}

fn final_marker(address: u32, severity: FinalMonitorSeverity) -> FinalMonitorMarker {
    FinalMonitorMarker {
        address,
        lat_e7: CENTRE.0,
        lon_e7: CENTRE.1,
        severity: severity as u8,
        position_known: 1,
        reserved: [0; 2],
    }
}

fn list_for(markers: usize) -> DisplayList {
    list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: markers * ALERT_CIRCLES_PER_MARKER,
        polylines: 0,
        poly_vertices: 0,
        texts: markers * ALERT_TEXTS_PER_MARKER + ALERT_HEADER_TEXTS,
        text_bytes: ALERT_HEADER_TEXT_BYTES + markers * ALERT_TEXT_BYTES_PER_MARKER,
    })
    .unwrap()
}

fn list_for_header_reservation(markers: usize, header_bytes: usize) -> DisplayList {
    list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: markers * ALERT_CIRCLES_PER_MARKER,
        polylines: 0,
        poly_vertices: 0,
        texts: markers * ALERT_TEXTS_PER_MARKER + ALERT_HEADER_TEXTS,
        text_bytes: header_bytes + markers * ALERT_TEXT_BYTES_PER_MARKER,
    })
    .unwrap()
}

fn list_for_render_budget(budget: AlertRenderBudget) -> DisplayList {
    list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: budget.circles,
        polylines: 0,
        poly_vertices: 0,
        texts: budget.texts,
        text_bytes: budget.text_bytes,
    })
    .unwrap()
}

fn maximum_final_markers() -> Vec<FinalMonitorMarker> {
    (0..MAX_ALERT_MARKERS)
        .map(|index| {
            let severity = if index < 100 {
                FinalMonitorSeverity::Caution
            } else if index < 114 {
                FinalMonitorSeverity::Warning
            } else {
                FinalMonitorSeverity::ProtectionLost
            };
            let address = u32::try_from(index + 1).unwrap();
            if index < 100 {
                FinalMonitorMarker {
                    address,
                    lat_e7: 500_000_000,
                    lon_e7: CENTRE.1,
                    severity: severity as u8,
                    position_known: 1,
                    reserved: [0; 2],
                }
            } else {
                FinalMonitorMarker {
                    address,
                    lat_e7: NO_POSITION_E7,
                    lon_e7: NO_POSITION_E7,
                    severity: severity as u8,
                    position_known: 0,
                    reserved: [0; 2],
                }
            }
        })
        .collect()
}

fn texts(list: &DisplayList) -> Vec<String> {
    (0..list.texts.x.len())
        .map(|index| {
            let start = list.texts.start[index] as usize;
            let end = start + list.texts.len[index] as usize;
            list.text.get(start..end).unwrap().to_owned()
        })
        .collect()
}

// REQ: OA-DSP-077
#[test]
fn caution_warning_and_protection_lost_have_distinct_alert_primitives_and_header() {
    let p = regional_projection(800);
    let markers = [
        marker(1, MarkerState::Caution, CENTRE.0, CENTRE.1),
        marker(2, MarkerState::Warning, CENTRE.0, CENTRE.1),
        marker(3, MarkerState::ProtectionLost, CENTRE.0, CENTRE.1),
    ];
    let mut list = list_for(markers.len());
    let stats = append_alert_markers(&mut list, &p, &markers).unwrap();

    assert_eq!(stats.examined, 3);
    assert_eq!(stats.drawn, 3);
    assert_eq!(stats.caution, 1);
    assert_eq!(stats.warning, 1);
    assert_eq!(stats.protection_lost, 1);
    assert_eq!(stats.off_map, 0);
    assert_eq!(stats.unlocated, 0);
    assert_eq!(list.circles.r.as_slice(), &[192, 256, 192, 256, 192]);
    assert!(list
        .circles
        .colour
        .iter()
        .all(|colour| *colour == palette::ALERT));
    assert!(list.circles.layer.iter().all(|layer| *layer == 13));

    let labels = texts(&list);
    assert_eq!(labels.first().map(String::as_str), Some("C"));
    assert_eq!(labels.get(1).map(String::as_str), Some("W"));
    assert_eq!(labels.get(2).map(String::as_str), Some("L"));
    assert_eq!(
        labels.get(3).map(String::as_str),
        Some("MSAW WARNING 1 CAUTION 1 LOST 1 OFF 0 UNLOCATED 0")
    );
    let heading = regional_status_plan()
        .unwrap()
        .row(StatusStripRow::MsawHeading)
        .unwrap();
    assert_eq!(list.texts.x.get(3).copied(), Some(heading.x_px() * 16));
    assert_eq!(list.texts.y.get(3).copied(), Some(heading.y_px() * 16));
    assert_eq!(list.texts.size.get(3), Some(&heading.size()));
    assert_eq!(list.texts.layer.get(3), Some(&heading.layer()));
    let point = project(&p, CENTRE.0, CENTRE.1).unwrap();
    assert_eq!(list.texts.x.first().copied(), Some(point.0));
    assert_eq!(list.texts.y.first().copied(), Some(point.1 - 80));
}

// REQ: OA-DSP-077
#[test]
fn off_map_and_unlocated_alerts_are_counted_without_drawing_guessed_points() {
    let p = regional_projection(800);
    let markers = [
        marker(10, MarkerState::Caution, CENTRE.0, CENTRE.1),
        marker(20, MarkerState::Warning, 500_000_000, CENTRE.1),
        unknown_marker(30, MarkerState::ProtectionLost),
    ];
    let mut list = list_for(markers.len());
    let stats = append_alert_markers(&mut list, &p, &markers).unwrap();

    assert_eq!(stats.examined, 3);
    assert_eq!(stats.drawn, 1);
    assert_eq!(stats.off_map, 1);
    assert_eq!(stats.unlocated, 1);
    assert_eq!(stats.caution, 1);
    assert_eq!(stats.warning, 1);
    assert_eq!(stats.protection_lost, 1);
    assert_eq!(list.circles.x.len(), 1);
    let labels = texts(&list);
    assert_eq!(labels.len(), 2);
    assert_eq!(labels.first().map(String::as_str), Some("C"));
    assert_eq!(
        labels.get(1).map(String::as_str),
        Some("MSAW WARNING 1 CAUTION 1 LOST 1 OFF 1 UNLOCATED 1")
    );
}

// REQ: OA-DSP-077
#[test]
fn invalid_records_and_short_lists_refuse_without_mutating_the_list() {
    let p = regional_projection(800);
    let valid = marker(10, MarkerState::Caution, CENTRE.0, CENTRE.1);
    let mut list = list_for(2);

    let duplicate = [valid, valid];
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &p, &duplicate),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    let mut reserved = valid;
    reserved.reserved[1] = 1;
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &p, &[reserved]),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    let mut malformed_position = valid;
    malformed_position.lat_e7 = 900_000_001;
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &p, &[malformed_position]),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    let unknown_with_coordinates = AlertMarker {
        position_known: 0,
        ..valid
    };
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &p, &[unknown_with_coordinates]),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    let one_missing_coordinate = AlertMarker {
        position_known: 0,
        lat_e7: NO_POSITION_E7,
        lon_e7: CENTRE.1,
        ..valid
    };
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &p, &[one_missing_coordinate]),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    let mut short = list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: 1,
        polylines: 0,
        poly_vertices: 0,
        texts: 2,
        text_bytes: 65,
    })
    .unwrap();
    let before = short.clone();
    assert!(matches!(
        append_alert_markers(&mut short, &p, &[valid]),
        Err(DisplayError::Full {
            table: "circles",
            ..
        })
    ));
    assert_eq!(short, before);

    let mut short_texts = list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: 2,
        polylines: 0,
        poly_vertices: 0,
        texts: 1,
        text_bytes: 65,
    })
    .unwrap();
    let before = short_texts.clone();
    assert!(matches!(
        append_alert_markers(&mut short_texts, &p, &[valid]),
        Err(DisplayError::Full { table: "texts", .. })
    ));
    assert_eq!(short_texts, before);

    let mut short_bytes = list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: 2,
        polylines: 0,
        poly_vertices: 0,
        texts: 2,
        text_bytes: 64,
    })
    .unwrap();
    let before = short_bytes.clone();
    assert!(matches!(
        append_alert_markers(&mut short_bytes, &p, &[valid]),
        Err(DisplayError::Full {
            table: "text bytes",
            ..
        })
    ));
    assert_eq!(short_bytes, before);

    let too_many: Vec<_> = (1..=129)
        .map(|address| unknown_marker(address, MarkerState::Caution))
        .collect();
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &p, &too_many),
        Err(DisplayError::TooLarge { .. })
    ));
    assert_eq!(list, before);
}

// REQ: OA-DSP-077
#[test]
fn view_and_header_width_are_checked_before_writing() {
    let markers = [marker(1, MarkerState::Caution, CENTRE.0, CENTRE.1)];
    let mut list = list_for(markers.len());
    let before = list.clone();
    assert!(matches!(
        append_alert_markers(&mut list, &regional_projection(480), &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    assert!(matches!(
        append_alert_markers(&mut list, &projection(800, 96), &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    // The documented minimum width fits this header. Refusing it would turn the
    // stated minimum into a misleading contract.
    assert!(append_alert_markers(&mut list, &regional_projection(500), &markers).is_ok());

    let mut short_frame = list_for(markers.len());
    let mut invalid_height = regional_projection(800);
    invalid_height.height = invalid_height.top_inset;
    let before_height = short_frame.clone();
    assert!(matches!(
        append_alert_markers(&mut short_frame, &invalid_height, &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(short_frame, before_height);
}

// Mutation killed: undercount the alert header width, or reject its first whole-pixel fit.
// REQ: OA-DSP-077
#[test]
fn maximum_count_header_fits_at_its_first_integer_pixel_width_only() {
    let markers: Vec<_> = (1..=MAX_ALERT_MARKERS as u32)
        .map(|address| unknown_marker(address, MarkerState::Warning))
        .collect();
    // For this exact ASCII heading, the renderer uses a 10 px advance at x=8 with
    // two pixels of right clearance: 53 characters produce a 540 px half-open bound.
    let width = 540;
    let mut exact = list_for(markers.len());
    append_alert_markers(&mut exact, &regional_projection(width), &markers).unwrap();
    assert_eq!(
        texts(&exact).last().map(String::as_str),
        Some("MSAW WARNING 128 CAUTION 0 LOST 0 OFF 0 UNLOCATED 128")
    );

    let mut one_pixel_narrower = list_for(markers.len());
    let before = one_pixel_narrower.clone();
    assert!(matches!(
        append_alert_markers(
            &mut one_pixel_narrower,
            &regional_projection(width - 1),
            &markers,
        ),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(one_pixel_narrower, before);
}

// Mutation killed: choose legacy heading placement at the exact reserved-band boundary.
// REQ: OA-DSP-077
#[test]
fn alert_heading_rows_use_the_shared_status_strip_placements() {
    let plan = regional_status_plan().unwrap();
    let msaw_row = plan.row(StatusStripRow::MsawHeading).unwrap();
    let final_row = plan.row(StatusStripRow::FinalHeading).unwrap();
    let mut msaw = list_for(0);
    append_alert_markers(&mut msaw, &regional_projection(800), &[]).unwrap();
    assert_eq!(msaw.texts.x.first().copied(), Some(msaw_row.x_px() * 16));
    assert_eq!(msaw.texts.y.first().copied(), Some(msaw_row.y_px() * 16));

    let mut final_monitor = list_for(0);
    append_final_monitor_frame(&mut final_monitor, &regional_projection(800), &[], true).unwrap();
    assert_eq!(
        final_monitor.texts.x.first().copied(),
        Some(final_row.x_px() * 16)
    );
    assert_eq!(
        final_monitor.texts.y.first().copied(),
        Some(final_row.y_px() * 16)
    );

    let mut short = list_for(0);
    let before = short.clone();
    assert!(matches!(
        append_alert_markers(&mut short, &projection(800, plan.top_inset_px() - 1), &[],),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(short, before);
}

// REQ: OA-DSP-077
#[test]
fn edge_warning_is_counted_and_rasterized_when_its_halo_is_clipped() {
    let p = regional_projection(800);
    let (lat_e7, lon_e7) = unproject(&p, 8 * 16, 300 * 16).unwrap();
    let point = project(&p, lat_e7, lon_e7).unwrap();
    assert!(point.0 >= 0 && point.0 < 16 * 16);
    assert!(point.1 >= 160 * 16 && point.1 < 600 * 16);

    let markers = [marker(1, MarkerState::Warning, lat_e7, lon_e7)];
    let mut with_warning = list_for(markers.len());
    let stats = append_alert_markers(&mut with_warning, &p, &markers).unwrap();
    assert_eq!(stats.warning, 1);
    assert_eq!(stats.drawn, 1);
    assert_eq!(stats.off_map, 0);
    assert!(texts(&with_warning)
        .last()
        .is_some_and(|header| header.contains("WARNING 1") && header.contains("OFF 0")));

    let mut without_warning = list_for(0);
    append_alert_markers(&mut without_warning, &p, &[]).unwrap();
    let spec = FrameSpec {
        width: 800,
        height: 600,
        stride: 800 * 4,
    };
    let mut warning_pixels = vec![0; frame_len(&spec)];
    let mut empty_pixels = vec![0; frame_len(&spec)];
    rasterize(&with_warning, &spec, &mut warning_pixels).unwrap();
    rasterize(&without_warning, &spec, &mut empty_pixels).unwrap();

    let mut changed_edge_pixels = 0usize;
    for y in (point.1 / 16 - 20).max(160)..=(point.1 / 16 + 20).min(599) {
        for x in 0..24 {
            let offset = (y as usize * spec.stride as usize) + (x as usize * 4);
            if warning_pixels.get(offset..offset + 4) != empty_pixels.get(offset..offset + 4) {
                changed_edge_pixels += 1;
            }
        }
    }
    assert!(changed_edge_pixels > 0);
}

// REQ: OA-DSP-077
#[test]
fn header_width_refusal_is_atomic_and_wider_view_preserves_the_complete_marker_layer() {
    let markers: Vec<_> = (1..=MAX_ALERT_MARKERS as u32)
        .map(|address| marker(address, MarkerState::Warning, CENTRE.0, CENTRE.1))
        .collect();
    let mut narrow_list = list_for(markers.len());
    let before = narrow_list.clone();
    assert!(matches!(
        append_alert_markers(&mut narrow_list, &regional_projection(500), &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(narrow_list, before);

    let mut list = list_for(markers.len());
    let stats = append_alert_markers(&mut list, &regional_projection(800), &markers).unwrap();
    assert_eq!(stats.warning as usize, MAX_ALERT_MARKERS);
    assert_eq!(stats.drawn as usize, MAX_ALERT_MARKERS);
    assert_eq!(
        list.circles.x.len(),
        MAX_ALERT_MARKERS * ALERT_CIRCLES_PER_MARKER
    );
    assert_eq!(list.texts.x.len(), MAX_ALERT_MARKERS + ALERT_HEADER_TEXTS);
    assert!(texts(&list)
        .last()
        .is_some_and(|header| header.contains("WARNING 128") && header.ends_with("UNLOCATED 0")));
}

// REQ: OA-RWY-015
#[test]
fn final_monitor_hook_renders_caution_warning_and_degraded_status() {
    let p = regional_projection(800);
    let markers = [
        final_marker(1, FinalMonitorSeverity::Caution),
        final_marker(2, FinalMonitorSeverity::Warning),
    ];
    let mut list = list_for(markers.len());
    let stats = append_final_monitor_frame(&mut list, &p, &markers, true).unwrap();
    assert_eq!(stats.caution, 1);
    assert_eq!(stats.warning, 1);
    assert_eq!(stats.drawn, 2);
    let labels = texts(&list);
    assert_eq!(labels.first().map(String::as_str), Some("C"));
    assert_eq!(labels.get(1).map(String::as_str), Some("W"));
    assert_eq!(
        labels.get(2).map(String::as_str),
        Some("FINAL DEGRADED WARNING 1 CAUTION 1 PROTECTION LOST 0 OFF 0 UNLOCATED 0")
    );

    let mut fresh = list_for(markers.len());
    append_final_monitor_markers(&mut fresh, &p, &markers).unwrap();
    assert!(texts(&fresh)
        .get(2)
        .is_some_and(|header| header.starts_with("FINAL WARNING")));
}

// REQ: OA-RWY-015
#[test]
fn maximum_final_degraded_header_fits_final_monitor_reservation() {
    let p = regional_projection(800);
    let markers = maximum_final_markers();
    let mut list = list_for_header_reservation(markers.len(), FINAL_MONITOR_HEADER_TEXT_BYTES);
    let stats = append_final_monitor_frame(&mut list, &p, &markers, true).unwrap();
    assert_eq!(stats.caution, 100);
    assert_eq!(stats.warning, 14);
    assert_eq!(stats.protection_lost, 14);
    assert_eq!(stats.off_map, 100);
    assert_eq!(stats.unlocated, 28);
    assert_eq!(list.text.len(), FINAL_MONITOR_HEADER_TEXT_BYTES);
    let rendered_text = texts(&list);
    let header = rendered_text.last().unwrap();
    assert!(header.contains("CAUTION 100"));
    assert!(header.contains("WARNING 14"));
    assert!(header.contains("PROTECTION LOST 14"));
    assert!(header.contains("OFF 100"));
    assert!(header.contains("UNLOCATED 28"));
}

// Mutation killed: admit an initial final-monitor frame narrower than its real loss heading.
// REQ: OA-RWY-015
#[test]
fn renderer_budget_pins_empty_degraded_loss_heading_at_its_first_pixel_width() {
    let budget = alert_render_budget(AlertRenderKind::FinalMonitor { degraded: true }, 1).unwrap();
    assert_eq!(budget.minimum_width_px, 710);
    let mut exact = list_for_render_budget(budget);
    let exact_view = projection(budget.minimum_width_px, budget.minimum_top_inset_px);
    append_final_monitor_frame_with_budget(&mut exact, &exact_view, &[], true, 1).unwrap();
    assert!(texts(&exact)
        .last()
        .is_some_and(|heading| heading.contains("PROTECTION LOST 0")));

    let mut short = list_for_render_budget(budget);
    let before = short.clone();
    let short_view = projection(
        budget.minimum_width_px.saturating_sub(1),
        budget.minimum_top_inset_px,
    );
    assert!(matches!(
        append_final_monitor_frame_with_budget(&mut short, &short_view, &[], true, 1),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(short, before);
}

// Mutation killed: remove either edge of final-monitor view validation, or reject its exact bounds.
// REQ: OA-RWY-015
#[test]
fn final_monitor_legacy_view_accepts_exact_width_inset_and_first_map_row() {
    let plan = regional_status_plan().unwrap();
    let exact_width = projection(620, plan.top_inset_px());
    let mut exact = list_for(0);
    append_final_monitor_frame(&mut exact, &exact_width, &[], false).unwrap();
    assert!(texts(&exact)
        .last()
        .is_some_and(|heading| heading.starts_with("FINAL WARNING 0")));

    let mut narrow = list_for(0);
    let before_narrow = narrow.clone();
    let below_header_width = projection(619, plan.top_inset_px());
    assert!(matches!(
        append_final_monitor_frame(&mut narrow, &below_header_width, &[], false),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(narrow, before_narrow);

    let mut legacy_width = list_for(0);
    let before_legacy_width = legacy_width.clone();
    let below_old_minimum = projection(499, plan.top_inset_px());
    assert!(matches!(
        append_final_monitor_frame(&mut legacy_width, &below_old_minimum, &[], false),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(legacy_width, before_legacy_width);

    let mut shallow_inset = projection(800, plan.top_inset_px().saturating_sub(1));
    let mut refused_inset = list_for(0);
    let before_inset = refused_inset.clone();
    assert!(matches!(
        append_final_monitor_frame(&mut refused_inset, &shallow_inset, &[], false),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(refused_inset, before_inset);

    shallow_inset.top_inset = plan.top_inset_px();
    shallow_inset.height = plan.top_inset_px();
    let mut no_map_row = list_for(0);
    let before_no_map_row = no_map_row.clone();
    assert!(matches!(
        append_final_monitor_frame(&mut no_map_row, &shallow_inset, &[], false),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(no_map_row, before_no_map_row);

    shallow_inset.height = plan.top_inset_px() + 1;
    let mut first_map_row = list_for(0);
    append_final_monitor_frame(&mut first_map_row, &shallow_inset, &[], false).unwrap();
    assert!(texts(&first_map_row)
        .last()
        .is_some_and(|heading| heading.starts_with("FINAL WARNING 0")));
}

// Mutation killed: omit one of the independently specified count terms from the widest heading.
// REQ: OA-RWY-015
#[test]
fn full_budget_worst_header_uses_the_independent_780_pixel_witness() {
    let mut markers = Vec::with_capacity(MAX_ALERT_MARKERS);
    for address in 1..=10 {
        markers.push(final_marker(address, FinalMonitorSeverity::Warning));
        let marker = markers.last_mut().unwrap();
        marker.lat_e7 = 900_000_000;
        marker.lon_e7 = 1_800_000_000;
    }
    for address in 11..=20 {
        let mut marker = final_marker(address, FinalMonitorSeverity::Caution);
        marker.position_known = 0;
        marker.lat_e7 = NO_POSITION_E7;
        marker.lon_e7 = NO_POSITION_E7;
        markers.push(marker);
    }
    for address in 21..=128 {
        let mut marker = final_marker(address, FinalMonitorSeverity::ProtectionLost);
        marker.position_known = 0;
        marker.lat_e7 = NO_POSITION_E7;
        marker.lon_e7 = NO_POSITION_E7;
        markers.push(marker);
    }
    let budget = alert_render_budget(
        AlertRenderKind::FinalMonitor { degraded: true },
        MAX_ALERT_MARKERS,
    )
    .unwrap();
    assert_eq!(budget.minimum_width_px, 780);
    let mut exact = list_for_render_budget(budget);
    let exact_view = projection(780, budget.minimum_top_inset_px);
    let stats = append_final_monitor_frame(&mut exact, &exact_view, &markers, true).unwrap();
    assert_eq!(stats.warning, 10);
    assert_eq!(stats.caution, 10);
    assert_eq!(stats.protection_lost, 108);
    assert_eq!(stats.off_map, 10);
    assert_eq!(stats.unlocated, 118);
    assert!(texts(&exact).iter().any(|text| {
        text == "FINAL DEGRADED WARNING 10 CAUTION 10 PROTECTION LOST 108 OFF 10 UNLOCATED 118"
    }));

    let mut narrow = list_for_render_budget(budget);
    let before = narrow.clone();
    let narrow_view = projection(779, budget.minimum_top_inset_px);
    assert!(matches!(
        append_final_monitor_frame(&mut narrow, &narrow_view, &markers, true),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(narrow, before);
}

// Mutation killed: treat a zero-text-slot heading failure as evidence that notice reservation is atomic.
// REQ: OA-RWY-015
#[test]
fn missing_notice_slot_refuses_before_any_marker_mutation() {
    let budget = alert_render_budget(AlertRenderKind::Msaw, 1).unwrap();
    let mut list = list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: budget.circles,
        polylines: 0,
        poly_vertices: 0,
        texts: ALERT_HEADER_TEXTS + ALERT_TEXTS_PER_MARKER,
        text_bytes: budget.text_bytes,
    })
    .unwrap();
    let before = list.clone();
    let view = regional_projection(800);
    let markers = [
        marker(1, MarkerState::Warning, CENTRE.0, CENTRE.1),
        unknown_marker(2, MarkerState::Caution),
    ];
    assert!(matches!(
        append_alert_markers_with_budget(&mut list, &view, &markers, 1),
        Err(DisplayError::Full { table: "texts", .. })
    ));
    assert_eq!(list, before);

    let mut with_notice_slot = list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: budget.circles,
        polylines: 0,
        poly_vertices: 0,
        texts: budget.texts,
        text_bytes: budget.text_bytes,
    })
    .unwrap();
    let stats =
        append_alert_markers_with_budget(&mut with_notice_slot, &view, &markers, 1).unwrap();
    assert_eq!(stats.drawn, 1);
    assert_eq!(stats.omitted, 1);
    assert!(texts(&with_notice_slot)
        .iter()
        .any(|text| text == "MSAW +1 NOT SHOWN"));
}

// Mutation killed: validate the partial heading but skip the longer overflow notice bound.
// REQ: OA-RWY-015
#[test]
fn overflow_notice_width_accepts_exact_edge_and_refuses_one_pixel_short() {
    let budget = alert_render_budget(AlertRenderKind::Msaw, 0).unwrap();
    let marker = [unknown_marker(1, MarkerState::Caution)];
    let exact_view = projection(180, budget.minimum_top_inset_px);
    let mut exact = list_for_render_budget(budget);
    let stats = append_alert_markers_with_budget(&mut exact, &exact_view, &marker, 0).unwrap();
    assert_eq!(stats.omitted, 1);
    assert!(texts(&exact).iter().any(|text| text == "MSAW PARTIAL"));
    assert!(texts(&exact).iter().any(|text| text == "MSAW +1 NOT SHOWN"));

    let mut short = list_for_render_budget(budget);
    let before = short.clone();
    let short_view = projection(179, budget.minimum_top_inset_px);
    assert!(matches!(
        append_alert_markers_with_budget(&mut short, &short_view, &marker, 0),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(short, before);
}

// Mutation killed: silently truncate over-budget alerts or omit their family-specific visible count.
// REQ: OA-RWY-015
#[test]
fn family_overflow_draws_allocated_rows_and_reports_omissions_in_reserved_strip() {
    let msaw_budget = alert_render_budget(AlertRenderKind::Msaw, 1).unwrap();
    let msaw_view = projection(
        msaw_budget.minimum_width_px,
        msaw_budget.minimum_top_inset_px,
    );
    let msaw_markers = [
        marker(1, MarkerState::Warning, CENTRE.0, CENTRE.1),
        unknown_marker(2, MarkerState::Caution),
    ];
    let mut msaw = list_for_render_budget(msaw_budget);
    let msaw_stats =
        append_alert_markers_with_budget(&mut msaw, &msaw_view, &msaw_markers, 1).unwrap();
    let msaw_text = texts(&msaw);
    assert_eq!(msaw_stats.examined, 2);
    assert_eq!(msaw_stats.drawn, 1);
    assert_eq!(msaw_stats.warning, 1);
    assert_eq!(msaw_stats.caution, 1);
    assert_eq!(msaw_stats.omitted, 1);
    assert!(msaw_text.iter().any(|text| text == "MSAW PARTIAL"));
    let overflow_index = msaw_text
        .iter()
        .position(|text| text == "MSAW +1 NOT SHOWN")
        .unwrap();
    let heading_index = msaw_text
        .iter()
        .position(|text| text == "MSAW PARTIAL")
        .unwrap();
    assert_ne!(
        msaw.texts.y.get(overflow_index),
        msaw.texts.y.get(heading_index)
    );

    let final_budget =
        alert_render_budget(AlertRenderKind::FinalMonitor { degraded: true }, 1).unwrap();
    let final_view = projection(
        final_budget.minimum_width_px,
        final_budget.minimum_top_inset_px,
    );
    let final_markers = [
        final_marker(1, FinalMonitorSeverity::Warning),
        final_marker(2, FinalMonitorSeverity::Caution),
    ];
    let mut final_list = list_for_render_budget(final_budget);
    let final_stats = append_final_monitor_frame_with_budget(
        &mut final_list,
        &final_view,
        &final_markers,
        true,
        1,
    )
    .unwrap();
    let final_text = texts(&final_list);
    assert_eq!(final_stats.drawn, 1);
    assert_eq!(final_stats.omitted, 1);
    assert_eq!(final_stats.protection_lost, 0);
    assert_eq!(final_stats.warning, 1);
    assert_eq!(final_stats.caution, 1);
    let final_heading_index = final_text
        .iter()
        .position(|text| text == "FINAL DEGRADED PROTECTION LOST PARTIAL")
        .unwrap();
    let final_overflow_index = final_text
        .iter()
        .position(|text| text == "FINAL +1 NOT SHOWN")
        .unwrap();
    assert_ne!(
        final_list.texts.y.get(final_heading_index),
        final_list.texts.y.get(final_overflow_index)
    );
}

// Mutation killed: allocate the budget by address order and leave the highest-address severity unseen.
// REQ: OA-RWY-015
#[test]
fn budget_prioritizes_warning_and_protection_lost_at_the_tail() {
    let budget = alert_render_budget(AlertRenderKind::Msaw, MAX_ALERT_MARKERS).unwrap();
    let view = projection(budget.minimum_width_px, budget.minimum_top_inset_px);
    let visible_point = project(&view, CENTRE.0, CENTRE.1).unwrap();
    for (state, label) in [
        (MarkerState::Warning, "W"),
        (MarkerState::ProtectionLost, "L"),
    ] {
        let mut markers: Vec<_> = (1..=u32::try_from(MAX_ALERT_MARKERS + 1).unwrap())
            .map(|address| marker(address, MarkerState::Caution, CENTRE.0, CENTRE.1))
            .collect();
        *markers.last_mut().unwrap() = marker(
            u32::try_from(MAX_ALERT_MARKERS + 1).unwrap(),
            state,
            CENTRE.0,
            CENTRE.1,
        );
        let mut list = list_for_render_budget(budget);
        let stats = append_alert_markers_with_budget(&mut list, &view, &markers, MAX_ALERT_MARKERS)
            .unwrap();
        assert_eq!(
            stats.examined,
            u64::try_from(MAX_ALERT_MARKERS + 1).unwrap()
        );
        assert_eq!(stats.drawn, u64::try_from(MAX_ALERT_MARKERS).unwrap());
        assert_eq!(stats.caution, u64::try_from(MAX_ALERT_MARKERS).unwrap());
        assert_eq!(stats.omitted, 1);
        assert_eq!(stats.warning, u64::from(state == MarkerState::Warning));
        assert_eq!(
            stats.protection_lost,
            u64::from(state == MarkerState::ProtectionLost)
        );
        let rendered = texts(&list);
        let label_index = rendered.iter().position(|text| text == label).unwrap();
        assert_eq!(
            list.texts.x.get(label_index).copied(),
            Some(visible_point.0)
        );
        assert_eq!(
            list.texts.y.get(label_index).copied(),
            Some(visible_point.1 - 80)
        );
        assert!(rendered.iter().any(|text| text == "MSAW +1 NOT SHOWN"));
        assert_eq!(
            list.circles.x.len(),
            MAX_ALERT_MARKERS - 1 + ALERT_CIRCLES_PER_MARKER
        );
    }

    let mut markers: Vec<_> = (1..=u32::try_from(MAX_ALERT_MARKERS + 2).unwrap())
        .map(|address| marker(address, MarkerState::Caution, CENTRE.0, CENTRE.1))
        .collect();
    let warning_address = u32::try_from(MAX_ALERT_MARKERS + 1).unwrap();
    let lost_address = warning_address + 1;
    let warning_index = usize::try_from(warning_address - 1).unwrap();
    let lost_index = usize::try_from(lost_address - 1).unwrap();
    *markers.get_mut(warning_index).unwrap() =
        marker(warning_address, MarkerState::Warning, CENTRE.0, CENTRE.1);
    *markers.get_mut(lost_index).unwrap() = marker(
        lost_address,
        MarkerState::ProtectionLost,
        CENTRE.0,
        CENTRE.1,
    );
    let mut list = list_for_render_budget(budget);
    let stats =
        append_alert_markers_with_budget(&mut list, &view, &markers, MAX_ALERT_MARKERS).unwrap();
    let rendered = texts(&list);
    let warning = rendered.iter().position(|text| text == "W").unwrap();
    let lost = rendered.iter().position(|text| text == "L").unwrap();
    assert!(warning < lost);
    assert_eq!(stats.drawn, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(stats.omitted, 2);
    assert_eq!(stats.caution, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(
        list.circles.x.len(),
        MAX_ALERT_MARKERS - 2 + 2 * ALERT_CIRCLES_PER_MARKER
    );
}

// Mutation killed: let an unlocated or off-map low address consume the sole drawable slot.
// REQ: OA-RWY-015
#[test]
fn budget_selects_drawable_warning_before_unlocated_or_off_map_warning() {
    for marker_budget in [1, 2] {
        let budget = alert_render_budget(AlertRenderKind::Msaw, marker_budget).unwrap();
        let view = projection(800, budget.minimum_top_inset_px);
        for low in [
            unknown_marker(1, MarkerState::Warning),
            marker(1, MarkerState::Warning, -899_000_000, CENTRE.1),
        ] {
            let markers = [low, marker(2, MarkerState::Warning, CENTRE.0, CENTRE.1)];
            let mut list = list_for_render_budget(budget);
            let stats = append_alert_markers_with_budget(&mut list, &view, &markers, marker_budget)
                .unwrap();
            assert_eq!(stats.examined, 2);
            assert_eq!(stats.drawn, 1);
            assert_eq!(stats.warning, 2);
            assert_eq!(stats.omitted, 1);
            assert_eq!(stats.unlocated, u64::from(low.position_known == 0));
            assert_eq!(stats.off_map, u64::from(low.position_known == 1));
            let label = texts(&list).iter().position(|text| text == "W").unwrap();
            assert_eq!(list.texts.x.get(label).copied(), Some(6_400));
            assert_eq!(list.texts.y.get(label).copied(), Some(6_848));
            assert_eq!(list.circles.x.len(), ALERT_CIRCLES_PER_MARKER);
            assert!(texts(&list).iter().any(|text| text == "MSAW +1 NOT SHOWN"));
        }
    }

    for marker_budget in [1, 2] {
        let final_budget = alert_render_budget(
            AlertRenderKind::FinalMonitor { degraded: false },
            marker_budget,
        )
        .unwrap();
        let final_view = projection(800, final_budget.minimum_top_inset_px);
        for (lat_e7, lon_e7, position_known, expected_unlocated, expected_off_map) in [
            (NO_POSITION_E7, NO_POSITION_E7, 0, 1, 0),
            (-899_000_000, CENTRE.1, 1, 0, 1),
        ] {
            let markers = [
                FinalMonitorMarker {
                    address: 1,
                    lat_e7,
                    lon_e7,
                    severity: FinalMonitorSeverity::Warning as u8,
                    position_known,
                    reserved: [0; 2],
                },
                final_marker(2, FinalMonitorSeverity::Warning),
            ];
            let mut list = list_for_render_budget(final_budget);
            let stats = append_final_monitor_frame_with_budget(
                &mut list,
                &final_view,
                &markers,
                false,
                marker_budget,
            )
            .unwrap();
            assert_eq!(stats.drawn, 1);
            assert_eq!(stats.warning, 2);
            assert_eq!(stats.omitted, 1);
            assert_eq!(stats.unlocated, expected_unlocated);
            assert_eq!(stats.off_map, expected_off_map);
            let label = texts(&list).iter().position(|text| text == "W").unwrap();
            assert_eq!(list.texts.x.get(label).copied(), Some(6_400));
            assert_eq!(list.texts.y.get(label).copied(), Some(6_848));
            assert_eq!(list.circles.x.len(), ALERT_CIRCLES_PER_MARKER);
            assert!(texts(&list).iter().any(|text| text == "FINAL +1 NOT SHOWN"));
        }
    }
}

// Mutation killed: drop a producer-reported open-table refusal when the retained list fills its budget.
// REQ: OA-RWY-015
#[test]
fn producer_refused_open_rows_are_checked_and_visible_without_fake_severity() {
    let budget = alert_render_budget(AlertRenderKind::Msaw, MAX_ALERT_MARKERS).unwrap();
    let view = projection(budget.minimum_width_px, budget.minimum_top_inset_px);
    let markers: Vec<_> = (1..=u32::try_from(MAX_ALERT_MARKERS).unwrap())
        .map(|address| marker(address, MarkerState::Caution, CENTRE.0, CENTRE.1))
        .collect();
    let mut list = list_for_render_budget(budget);
    let stats = append_alert_markers_with_budget_and_producer_omissions(
        &mut list,
        &view,
        &markers,
        MAX_ALERT_MARKERS,
        1,
    )
    .unwrap();
    assert_eq!(stats.examined, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(stats.drawn, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(stats.caution, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(stats.warning, 0);
    assert_eq!(stats.protection_lost, 0);
    assert_eq!(stats.omitted, 1);
    let rendered = texts(&list);
    assert!(rendered.iter().any(|text| text == "MSAW PARTIAL"));
    assert!(rendered.iter().any(|text| text == "MSAW +1 NOT SHOWN"));

    let mut next_frame = list_for_render_budget(budget);
    let next_stats = append_alert_markers_with_budget_and_producer_omissions(
        &mut next_frame,
        &view,
        &markers,
        MAX_ALERT_MARKERS,
        1,
    )
    .unwrap();
    assert_eq!(next_stats.omitted, 1);
    assert!(texts(&next_frame)
        .iter()
        .any(|text| text == "MSAW +1 NOT SHOWN"));

    // Mutation killed: drop caution-grade refusals from the checked producer total.
    for (refusals, expected_omitted, expected_notice) in [
        (
            ProducerRefusalCounts {
                caution: 1,
                warning: 0,
            },
            1,
            "MSAW +1 NOT SHOWN",
        ),
        (
            ProducerRefusalCounts {
                caution: 1,
                warning: 2,
            },
            3,
            "MSAW +3 NOT SHOWN (2 WARNING)",
        ),
    ] {
        let mut typed = list_for_render_budget(budget);
        let typed_stats = append_alert_markers_with_budget_and_producer_refusals(
            &mut typed,
            &view,
            &markers,
            MAX_ALERT_MARKERS,
            refusals,
        )
        .unwrap();
        assert_eq!(typed_stats.drawn, u64::try_from(MAX_ALERT_MARKERS).unwrap());
        assert_eq!(
            typed_stats.caution,
            u64::try_from(MAX_ALERT_MARKERS).unwrap()
        );
        assert_eq!(typed_stats.warning, 0);
        assert_eq!(typed_stats.omitted, expected_omitted);
        assert!(texts(&typed).iter().any(|text| text == expected_notice));
    }

    let mut refusing_grade_sum = list_for_render_budget(budget);
    let before_grade_sum = refusing_grade_sum.clone();
    assert!(matches!(
        append_alert_markers_with_budget_and_producer_refusals(
            &mut refusing_grade_sum,
            &view,
            &markers,
            MAX_ALERT_MARKERS,
            ProducerRefusalCounts {
                caution: u64::MAX,
                warning: 1,
            },
        ),
        Err(DisplayError::TooLarge { .. })
    ));
    assert_eq!(refusing_grade_sum, before_grade_sum);

    let too_many = (1..=u32::try_from(MAX_ALERT_MARKERS + 1).unwrap())
        .map(|address| unknown_marker(address, MarkerState::Caution))
        .collect::<Vec<_>>();
    let mut refusing = list_for_render_budget(budget);
    let before = refusing.clone();
    assert!(matches!(
        append_alert_markers_with_budget_and_producer_omissions(
            &mut refusing,
            &view,
            &too_many,
            MAX_ALERT_MARKERS,
            u64::MAX,
        ),
        Err(DisplayError::TooLarge { .. })
    ));
    assert_eq!(refusing, before);
}

// Mutation killed: infer protection loss only from degraded mode or only from drawn rows.
// REQ: OA-RWY-015
#[test]
fn overflow_heading_preserves_actual_loss_from_omitted_rows_without_false_loss() {
    let budget = alert_render_budget(AlertRenderKind::Msaw, 1).unwrap();
    let view = projection(budget.minimum_width_px, budget.minimum_top_inset_px);
    let markers = [
        marker(1, MarkerState::Warning, CENTRE.0, CENTRE.1),
        unknown_marker(2, MarkerState::ProtectionLost),
    ];
    let mut msaw = list_for_render_budget(budget);
    let stats = append_alert_markers_with_budget(&mut msaw, &view, &markers, 1).unwrap();
    let rendered = texts(&msaw);
    assert_eq!(stats.examined, 2);
    assert_eq!(stats.warning, 1);
    assert_eq!(stats.protection_lost, 1);
    assert_eq!(stats.drawn, 1);
    assert_eq!(stats.omitted, 1);
    assert!(rendered
        .iter()
        .any(|text| text == "MSAW PROTECTION LOST PARTIAL"));
    assert!(rendered.iter().any(|text| text == "MSAW +1 NOT SHOWN"));

    let final_budget =
        alert_render_budget(AlertRenderKind::FinalMonitor { degraded: false }, 1).unwrap();
    let final_view = projection(
        final_budget.minimum_width_px,
        final_budget.minimum_top_inset_px,
    );
    let loss_tail = [
        final_marker(1, FinalMonitorSeverity::Warning),
        final_marker(2, FinalMonitorSeverity::ProtectionLost),
    ];
    let mut final_loss = list_for_render_budget(final_budget);
    let loss_stats =
        append_final_monitor_frame_with_budget(&mut final_loss, &final_view, &loss_tail, false, 1)
            .unwrap();
    let loss_text = texts(&final_loss);
    assert_eq!(loss_stats.protection_lost, 1);
    assert_eq!(loss_stats.omitted, 1);
    assert!(loss_text
        .iter()
        .any(|text| text == "FINAL PROTECTION LOST PARTIAL"));
    assert!(loss_text.iter().any(|text| text == "FINAL +1 NOT SHOWN"));

    let no_loss = [
        final_marker(1, FinalMonitorSeverity::Warning),
        final_marker(2, FinalMonitorSeverity::Caution),
    ];
    let mut final_clear = list_for_render_budget(final_budget);
    let clear_stats =
        append_final_monitor_frame_with_budget(&mut final_clear, &final_view, &no_loss, false, 1)
            .unwrap();
    let clear_text = texts(&final_clear);
    assert_eq!(clear_stats.protection_lost, 0);
    assert_eq!(clear_stats.omitted, 1);
    assert!(clear_text.iter().any(|text| text == "FINAL PARTIAL"));
    assert!(clear_text
        .iter()
        .all(|text| !text.contains("PROTECTION LOST")));
    assert!(clear_text.iter().any(|text| text == "FINAL +1 NOT SHOWN"));
}

// Mutation killed: keep the 128-row projection scratch limit as an input cap on budgeted APIs.
// REQ: OA-RWY-015
#[test]
fn budgeted_presenters_validate_and_count_the_129th_input_without_drawing_it() {
    let input_count = MAX_ALERT_MARKERS + 1;
    let msaw_budget = alert_render_budget(AlertRenderKind::Msaw, MAX_ALERT_MARKERS).unwrap();
    let msaw_view = projection(
        msaw_budget.minimum_width_px,
        msaw_budget.minimum_top_inset_px,
    );
    let msaw_markers: Vec<_> = (1..=u32::try_from(input_count).unwrap())
        .map(|address| marker(address, MarkerState::Warning, CENTRE.0, CENTRE.1))
        .collect();
    let msaw_original = msaw_markers.clone();
    let mut msaw = list_for_render_budget(msaw_budget);
    let msaw_stats =
        append_alert_markers_with_budget(&mut msaw, &msaw_view, &msaw_markers, MAX_ALERT_MARKERS)
            .unwrap();
    assert_eq!(msaw_stats.examined, u64::try_from(input_count).unwrap());
    assert_eq!(msaw_stats.warning, u64::try_from(input_count).unwrap());
    assert_eq!(msaw_stats.drawn, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(msaw_stats.omitted, 1);
    assert_eq!(
        msaw.circles.x.len(),
        MAX_ALERT_MARKERS * ALERT_CIRCLES_PER_MARKER
    );
    assert_eq!(msaw_markers, msaw_original);
    assert!(texts(&msaw).iter().any(|text| text == "MSAW +1 NOT SHOWN"));

    let final_budget = alert_render_budget(
        AlertRenderKind::FinalMonitor { degraded: true },
        MAX_ALERT_MARKERS,
    )
    .unwrap();
    let final_view = projection(
        final_budget.minimum_width_px,
        final_budget.minimum_top_inset_px,
    );
    let final_markers: Vec<_> = (1..=u32::try_from(input_count).unwrap())
        .map(|address| final_marker(address, FinalMonitorSeverity::ProtectionLost))
        .collect();
    let mut final_list = list_for_render_budget(final_budget);
    let final_stats = append_final_monitor_frame_with_budget(
        &mut final_list,
        &final_view,
        &final_markers,
        true,
        MAX_ALERT_MARKERS,
    )
    .unwrap();
    assert_eq!(final_stats.examined, u64::try_from(input_count).unwrap());
    assert_eq!(
        final_stats.protection_lost,
        u64::try_from(input_count).unwrap()
    );
    assert_eq!(final_stats.drawn, u64::try_from(MAX_ALERT_MARKERS).unwrap());
    assert_eq!(final_stats.omitted, 1);
    assert_eq!(
        final_list.circles.x.len(),
        MAX_ALERT_MARKERS * ALERT_CIRCLES_PER_MARKER
    );
    assert!(texts(&final_list)
        .iter()
        .any(|text| text == "FINAL +1 NOT SHOWN"));
    assert!(texts(&final_list)
        .iter()
        .any(|text| text.contains("PROTECTION LOST PARTIAL")));
}

// Mutation killed: stop validating producer rows as soon as the fixed drawing prefix is full.
// REQ: OA-RWY-015
#[test]
fn budgeted_presenters_refuse_invalid_tail_records_before_mutating_the_list() {
    let input_count = MAX_ALERT_MARKERS + 1;
    let budget = alert_render_budget(AlertRenderKind::Msaw, MAX_ALERT_MARKERS).unwrap();
    let view = projection(budget.minimum_width_px, budget.minimum_top_inset_px);
    let mut markers: Vec<_> = (1..=u32::try_from(input_count).unwrap())
        .map(|address| unknown_marker(address, MarkerState::Warning))
        .collect();
    let tail = markers.last_mut().unwrap();
    tail.address = 0;
    let mut list = list_for_render_budget(budget);
    let before = list.clone();
    assert!(matches!(
        append_alert_markers_with_budget(&mut list, &view, &markers, MAX_ALERT_MARKERS),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    let mut invalid_coordinate_tail: Vec<_> = (1..=u32::try_from(input_count).unwrap())
        .map(|address| marker(address, MarkerState::Warning, 0, 0))
        .collect();
    let tail = invalid_coordinate_tail.last_mut().unwrap();
    tail.lat_e7 = 900_000_001;
    let mut coordinate_list = list_for_render_budget(budget);
    let before_coordinate = coordinate_list.clone();
    assert!(matches!(
        append_alert_markers_with_budget(
            &mut coordinate_list,
            &view,
            &invalid_coordinate_tail,
            MAX_ALERT_MARKERS,
        ),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(coordinate_list, before_coordinate);

    let final_budget = alert_render_budget(
        AlertRenderKind::FinalMonitor { degraded: true },
        MAX_ALERT_MARKERS,
    )
    .unwrap();
    let final_view = projection(
        final_budget.minimum_width_px,
        final_budget.minimum_top_inset_px,
    );
    let mut final_markers: Vec<_> = (1..=u32::try_from(input_count).unwrap())
        .map(|address| final_marker(address, FinalMonitorSeverity::Warning))
        .collect();
    final_markers.last_mut().unwrap().severity = u8::MAX;
    let mut final_list = list_for_render_budget(final_budget);
    let before_final = final_list.clone();
    assert!(matches!(
        append_final_monitor_frame_with_budget(
            &mut final_list,
            &final_view,
            &final_markers,
            true,
            MAX_ALERT_MARKERS,
        ),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(final_list, before_final);
}

// Mutation killed: charge the longer final-monitor heading to MSAW or accept a one-byte-short list.
// REQ: OA-DSP-077, OA-RWY-015
#[test]
fn typed_header_reservations_accept_exact_capacity_and_refuse_one_byte_short() {
    let p = regional_projection(800);
    let msaw: Vec<_> = (1..=MAX_ALERT_MARKERS as u32)
        .map(|address| unknown_marker(address, MarkerState::Warning))
        .collect();
    let mut exact_msaw = list_for_header_reservation(msaw.len(), MSAW_HEADER_TEXT_BYTES);
    append_alert_markers(&mut exact_msaw, &p, &msaw).unwrap();
    let mut short_msaw =
        list_for_header_reservation(msaw.len(), MSAW_HEADER_TEXT_BYTES.saturating_sub(1));
    let before_msaw = short_msaw.clone();
    assert!(matches!(
        append_alert_markers(&mut short_msaw, &p, &msaw),
        Err(DisplayError::Full { .. })
    ));
    assert_eq!(short_msaw, before_msaw);

    let final_markers = maximum_final_markers();
    let mut exact_final =
        list_for_header_reservation(final_markers.len(), FINAL_MONITOR_HEADER_TEXT_BYTES);
    append_final_monitor_frame(&mut exact_final, &p, &final_markers, true).unwrap();
    let mut short_final = list_for_header_reservation(
        final_markers.len(),
        FINAL_MONITOR_HEADER_TEXT_BYTES.saturating_sub(1),
    );
    let before_final = short_final.clone();
    assert!(matches!(
        append_final_monitor_frame(&mut short_final, &p, &final_markers, true),
        Err(DisplayError::Full { .. })
    ));
    assert_eq!(short_final, before_final);
}

// REQ: OA-RWY-015
#[test]
fn final_monitor_protection_lost_is_typed_counted_and_rasterized_at_the_edge() {
    let p = regional_projection(800);
    let (lat_e7, lon_e7) = unproject(&p, 8 * 16, 300 * 16).unwrap();
    let point = project(&p, lat_e7, lon_e7).unwrap();
    let lost = [FinalMonitorMarker {
        address: 7,
        lat_e7,
        lon_e7,
        severity: FinalMonitorSeverity::ProtectionLost as u8,
        position_known: 1,
        reserved: [0; 2],
    }];
    let mut with_lost = list_for(lost.len());
    let stats = append_final_monitor_frame(&mut with_lost, &p, &lost, false).unwrap();
    assert_eq!(stats.protection_lost, 1);
    assert_eq!(stats.drawn, 1);
    assert_eq!(stats.off_map, 0);
    let rendered_text = texts(&with_lost);
    assert_eq!(rendered_text.first().map(String::as_str), Some("L"));
    assert!(rendered_text
        .last()
        .is_some_and(|header| header.contains("PROTECTION LOST 1")));

    let mut without_lost = list_for(0);
    append_final_monitor_frame(&mut without_lost, &p, &[], false).unwrap();
    let spec = FrameSpec {
        width: 800,
        height: 600,
        stride: 800 * 4,
    };
    let mut lost_pixels = vec![0; frame_len(&spec)];
    let mut clear_pixels = vec![0; frame_len(&spec)];
    rasterize(&with_lost, &spec, &mut lost_pixels).unwrap();
    rasterize(&without_lost, &spec, &mut clear_pixels).unwrap();
    let mut changed_edge_pixels = 0usize;
    for y in (point.1 / 16 - 20).max(160)..=(point.1 / 16 + 20).min(599) {
        for x in 0..24 {
            let offset = (y as usize * spec.stride as usize) + (x as usize * 4);
            if lost_pixels.get(offset..offset + 4) != clear_pixels.get(offset..offset + 4) {
                changed_edge_pixels += 1;
            }
        }
    }
    assert!(changed_edge_pixels > 0);
}

// REQ: OA-RWY-015
#[test]
fn final_monitor_invalid_packets_and_capacity_refuse_atomically() {
    let p = regional_projection(800);
    let valid = final_marker(1, FinalMonitorSeverity::Warning);
    let mut list = list_for(2);
    for invalid in [
        FinalMonitorMarker {
            severity: u8::MAX,
            ..valid
        },
        FinalMonitorMarker {
            reserved: [1, 0],
            ..valid
        },
        FinalMonitorMarker {
            lat_e7: 900_000_001,
            ..valid
        },
        FinalMonitorMarker {
            position_known: 0,
            ..valid
        },
        FinalMonitorMarker {
            position_known: 0,
            lat_e7: NO_POSITION_E7,
            lon_e7: CENTRE.1,
            ..valid
        },
    ] {
        let before = list.clone();
        assert!(matches!(
            append_final_monitor_frame(&mut list, &p, &[invalid], false),
            Err(DisplayError::BadView(_))
        ));
        assert_eq!(list, before);
    }

    let mut short = list_new(ListCaps {
        lines: 0,
        rects: 0,
        circles: 1,
        polylines: 0,
        poly_vertices: 0,
        texts: 2,
        text_bytes: 128,
    })
    .unwrap();
    let before = short.clone();
    assert!(matches!(
        append_final_monitor_frame(&mut short, &p, &[valid], false),
        Err(DisplayError::Full {
            table: "circles",
            ..
        })
    ));
    assert_eq!(short, before);
}
