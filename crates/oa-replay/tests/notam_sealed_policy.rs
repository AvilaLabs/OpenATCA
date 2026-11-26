//! The production sealed-window policy belongs at the shell/caller boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../oa-notam/tests/common/mod.rs"]
mod notam_fixture;

use oa_notam::{decode_bytes, new_decoder, Limits};

// REQ: OA-SEAL-014
#[test]
fn a_window_that_holds_all_time_drops_every_notam_and_the_production_list_drops_its_window() {
    let issued = "<event:issued>2030-03-05T09:00:00.000Z</event:issued>";
    let base = notam_fixture::msg(&[&notam_fixture::airport()]);
    let mut parser = notam_fixture::parser();

    // All-time is the closed failure of a drop window.
    let mut decoder = new_decoder(Limits::new(oa_sealed::DropWindow::all_time()));
    let mut rows = notam_fixture::small();
    let mut stats = notam_fixture::default_stats();
    let outcome = decode_bytes(
        &mut parser,
        &mut decoder,
        base.as_bytes(),
        &mut rows,
        &mut stats,
    )
    .unwrap();
    assert!(outcome.sealed_dropped);
    assert_eq!(rows.len, 0);
    assert_eq!(stats.sealed_window_dropped, 1);

    // The production list drops an in-window issue time and retains an outside one.
    let window = oa_sealed::sealed_drop_window();
    let inside = notam_fixture::rep(
        &base,
        issued,
        "<event:issued>2026-11-21T00:00:00.000Z</event:issued>",
    );
    let mut decoder = new_decoder(Limits::new(window));
    let mut rows = notam_fixture::small();
    let mut stats = notam_fixture::default_stats();
    let outcome = decode_bytes(
        &mut parser,
        &mut decoder,
        inside.as_bytes(),
        &mut rows,
        &mut stats,
    )
    .unwrap();
    assert!(outcome.sealed_dropped);
    assert_eq!(rows.len, 0);
    assert_eq!(stats.sealed_window_dropped, 1);

    let mut decoder = new_decoder(Limits::new(window));
    let mut rows = notam_fixture::small();
    let mut stats = notam_fixture::default_stats();
    let outcome = decode_bytes(
        &mut parser,
        &mut decoder,
        base.as_bytes(),
        &mut rows,
        &mut stats,
    )
    .unwrap();
    assert!(!outcome.sealed_dropped);
    assert_eq!((rows.len, stats.docs, stats.rows), (1, 1, 1));
    assert_eq!(stats.sealed_window_dropped, 0);
}
