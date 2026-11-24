//! Current MSAW marker presentation and bounded refusal behavior.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::core::alert_markers::*;
use oa_display::list::{list_new, DisplayError, DisplayList, ListCaps};
use oa_display::palette;
use oa_display::project::{project, projection_new, Projection};
use oa_display::text::text_str;

const CENTRE: (i32, i32) = (419_786_000, -879_048_000);

fn projection(width: u32, top_inset: u32) -> Projection {
    projection_new(CENTRE.0, CENTRE.1, 20.0, width, 600, top_inset).unwrap()
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
    let p = projection(800, 104);
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
    assert_eq!(list.texts.x.get(3).copied(), Some(8 * 16));
    assert_eq!(list.texts.y.get(3).copied(), Some(72 * 16));
    assert_eq!(list.texts.size.get(3), Some(&10));
    assert_eq!(list.texts.layer.get(3), Some(&13));
    let point = project(&p, CENTRE.0, CENTRE.1).unwrap();
    assert_eq!(list.texts.x.first().copied(), Some(point.0));
    assert_eq!(list.texts.y.first().copied(), Some(point.1 - 80));
}

// REQ: OA-DSP-077
#[test]
fn off_map_and_unlocated_alerts_are_counted_without_drawing_guessed_points() {
    let p = projection(800, 104);
    let markers = [
        marker(10, MarkerState::Caution, CENTRE.0, CENTRE.1),
        marker(20, MarkerState::Warning, CENTRE.0, -870_000_000),
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
    let p = projection(800, 104);
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
        append_alert_markers(&mut list, &projection(480, 104), &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    assert!(matches!(
        append_alert_markers(&mut list, &projection(800, 96), &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);

    assert!(matches!(
        append_alert_markers(&mut list, &projection(500, 104), &markers),
        Err(DisplayError::BadView(_))
    ));
    assert_eq!(list, before);
}
