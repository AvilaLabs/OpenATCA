//! Controller-facing ownership state: typed inputs, legible state distinctions, and fixed caps.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::list::{list_new, DisplayList, ALIGN_RIGHT};
use oa_display::ownership::{
    build_ownership_status, ownership_caps, OwnershipPhase, OwnershipStatus, LAYER_OWNERSHIP,
    OWNERSHIP_ID_BYTES, OWNERSHIP_TEXT_BYTES,
};
use oa_display::palette;
use oa_display::project::{projection_new, Projection};
use oa_display::raster::{frame_digest, frame_len, rasterize, FrameSpec};
use oa_display::scene::LEGEND_STRIP_PX;

const SCREEN: FrameSpec = FrameSpec {
    width: 800,
    height: 200,
    stride: 3200,
};

fn projection(width: u32) -> Projection {
    projection_new(419_786_000, -879_048_000, 20.0, width, 200, LEGEND_STRIP_PX).unwrap()
}

fn id(s: &str) -> [u8; OWNERSHIP_ID_BYTES] {
    let mut out = [0; OWNERSHIP_ID_BYTES];
    for (to, from) in out.iter_mut().zip(s.bytes()) {
        *to = from;
    }
    out
}

fn status(phase: OwnershipPhase, owner: &str) -> OwnershipStatus {
    OwnershipStatus {
        callsign: id("DAL56"),
        current_owner: id(owner),
        phase,
    }
}

fn draw(input: &OwnershipStatus) -> (DisplayList, Vec<u8>) {
    let mut list = list_new(ownership_caps()).unwrap();
    build_ownership_status(&mut list, &projection(SCREEN.width), input).unwrap();
    let mut pixels = vec![0; frame_len(&SCREEN)];
    rasterize(&list, &SCREEN, &mut pixels).unwrap();
    (list, pixels)
}

fn text(list: &DisplayList) -> &str {
    let start = usize::try_from(list.texts.start[0]).unwrap();
    let len = usize::try_from(list.texts.len[0]).unwrap();
    list.text.get(start..start + len).unwrap()
}

#[test]
fn owner_offer_acceptance_and_stale_delivery_have_distinct_pixel_evidence() {
    let views = [
        (
            status(OwnershipPhase::Current, "ZAU12"),
            "FLIGHT DAL56 OWNER ZAU12 | CURRENT",
            palette::TARGET,
        ),
        (
            status(OwnershipPhase::Offered { to: id("ZMP34") }, "ZAU12"),
            "FLIGHT DAL56 OWNER ZAU12 | OFFERED TO ZMP34",
            palette::ARTCC_HIGH,
        ),
        (
            status(OwnershipPhase::Accepted { by: id("ZMP34") }, "ZAU12"),
            "FLIGHT DAL56 OWNER ZAU12 | ACCEPTED BY ZMP34",
            palette::FIX,
        ),
        (
            status(OwnershipPhase::StaleOffer { to: id("ZAU12") }, "ZMP34"),
            "FLIGHT DAL56 OWNER ZMP34 | STALE OFFER TO ZAU12",
            palette::ALERT,
        ),
    ];
    let mut digests = Vec::new();
    for (input, expected_text, expected_colour) in views {
        let (list, pixels) = draw(&input);
        assert_eq!(text(&list), expected_text);
        assert_eq!(list.texts.colour, [expected_colour]);
        assert_eq!(list.texts.layer, [LAYER_OWNERSHIP]);
        assert_eq!(list.texts.align, [ALIGN_RIGHT]);
        assert_eq!(list.texts.y, [(SCREEN.height as i32 - 42) * 16]);
        assert!(list.rects.x0.is_empty());
        assert_eq!(list.texts.x.len(), 1);
        assert!(list.texts.len[0] as usize <= OWNERSHIP_TEXT_BYTES);
        digests.push(frame_digest(&SCREEN, &pixels).unwrap());
    }
    for i in 0..digests.len() {
        for j in i + 1..digests.len() {
            assert_ne!(
                digests[i], digests[j],
                "each ownership state changes pixels"
            );
        }
    }
}

#[test]
fn accepted_phase_keeps_the_existing_owner_visible() {
    let (list, _) = draw(&status(
        OwnershipPhase::Accepted { by: id("ZMP34") },
        "ZAU12",
    ));
    assert_eq!(text(&list), "FLIGHT DAL56 OWNER ZAU12 | ACCEPTED BY ZMP34");
}

#[test]
fn the_one_row_budget_covers_the_maximum_ids_and_refuses_bad_input_atomically() {
    let caps = ownership_caps();
    assert_eq!(
        (
            caps.lines,
            caps.rects,
            caps.circles,
            caps.polylines,
            caps.poly_vertices
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!((caps.texts, caps.text_bytes), (1, OWNERSHIP_TEXT_BYTES));

    let maximum = OwnershipStatus {
        callsign: [b'Z'; OWNERSHIP_ID_BYTES],
        current_owner: [b'Z'; OWNERSHIP_ID_BYTES],
        phase: OwnershipPhase::StaleOffer {
            to: [b'Z'; OWNERSHIP_ID_BYTES],
        },
    };
    let (list, _) = draw(&maximum);
    assert!(text(&list).len() <= OWNERSHIP_TEXT_BYTES);

    let mut invalid = status(OwnershipPhase::Current, "ZAU12");
    invalid.current_owner = *b"Zau12\0\0\0";
    let mut list = list_new(caps).unwrap();
    let before = list.clone();
    assert!(build_ownership_status(&mut list, &projection(SCREEN.width), &invalid).is_err());
    assert_eq!(
        list, before,
        "invalid identifiers do not leave partial evidence"
    );

    let too_narrow = status(OwnershipPhase::StaleOffer { to: id("ZAU12") }, "ZMP34");
    assert!(build_ownership_status(&mut list, &projection(320), &too_narrow).is_err());
    assert_eq!(list, before, "a clipped line is refused before drawing");
}
