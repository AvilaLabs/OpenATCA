//! Live traffic: the exact display-list commands of each symbol, the data block text, capacity
//! refusal, determinism and a golden frame hash, over a synthetic scene near Chicago.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::list::*;
use oa_display::palette as pal;
use oa_display::probe::list_capacities;
use oa_display::project::{project, projection_new, Projection};
use oa_display::raster::{frame_digest, frame_len, rasterize, FrameSpec};
use oa_display::text::{text_new, text_str};
use oa_display::traffic::*;
use oa_sha256::to_hex;

const ORD: (i32, i32) = (419_786_000, -879_048_000);

fn proj() -> Projection {
    projection_new(ORD.0, ORD.1, 20.0, 800, 600, 0).unwrap()
}

fn plain(lat: i32, lon: i32) -> Target {
    Target {
        lat_e7: lat,
        lon_e7: lon,
        track_ddeg: NO_U16,
        gs_dkt: NO_U16,
        alt_ft: NO_ALT,
        addr: 0xA1B2C3,
        ident: NO_IDENT,
        ident_age_s: u32::MAX,
        ident_state: IDENT_UNKNOWN,
        callsign: [0; 8],
        flags: 0,
        reacq_age_s: NO_REACQ,
        r95_m: f32::NAN,
        trail: [(0, 0); MAX_TRAIL],
        trail_len: 0,
    }
}

#[test]
fn mode_a_data_block_distinguishes_observed_zero_unknown_conflict_and_age() {
    let mut target = plain(ORD.0, ORD.1);
    target.ident = 0;
    target.ident_age_s = 18;
    target.ident_state = IDENT_OBSERVED;
    let texts = texts_of(&draw(&[target], &DEFAULT_STYLE).0);
    assert!(texts
        .iter()
        .any(|(text, _, _)| text == "MODE A 0000 AGE 18s"));

    target.ident = NO_IDENT;
    target.ident_age_s = u32::MAX;
    target.ident_state = IDENT_CONFLICT;
    let texts = texts_of(&draw(&[target], &DEFAULT_STYLE).0);
    assert!(texts.iter().any(|(text, _, _)| text == "MODE A CONFLICT"));

    target.ident_state = IDENT_UNKNOWN;
    let texts = texts_of(&draw(&[target], &DEFAULT_STYLE).0);
    assert!(texts.iter().all(|(text, _, _)| !text.starts_with("MODE A")));
}

fn callsign(s: &str) -> [u8; 8] {
    let mut c = [0u8; 8];
    for (d, b) in c.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    c
}

/// Draws `ts` over an empty list with the given style.
fn draw(ts: &[Target], style: &TrafficStyle) -> (DisplayList, TrafficStats) {
    let p = proj();
    let mut set = targets_new(ts.len().max(1)).unwrap();
    for t in ts {
        targets_push(&mut set, t).unwrap();
    }
    let mut l = list_new(traffic_caps(targets_capacity(&set), style).unwrap()).unwrap();
    let st = build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, style).unwrap();
    (l, st)
}

#[test]
fn regional_block_policy_limits_unmatched_detail_without_dropping_symbols() {
    let p = proj();
    let mut full = plain(ORD.0, ORD.1);
    full.addr = 1;
    full.callsign = callsign("AAL1234");
    full.alt_ft = 35_000;
    full.ident = 0o1200;
    full.ident_age_s = 0;
    full.ident_state = IDENT_OBSERVED;
    let mut altitude = plain(ORD.0 + 100_000, ORD.1);
    altitude.addr = 2;
    altitude.alt_ft = 27_000;
    altitude.ident = 0o2345;
    altitude.ident_age_s = 0;
    altitude.ident_state = IDENT_OBSERVED;
    let mut symbol = plain(ORD.0 - 100_000, ORD.1);
    symbol.addr = 3;
    let mut set = targets_new(3).unwrap();
    for target in [full, altitude, symbol] {
        targets_push(&mut set, &target).unwrap();
    }
    let policy = [
        TrafficBlockPolicy {
            address: 1,
            detail: TrafficBlockDetail::Full,
            flight_callsign: Some(callsign("UAL4321")),
        },
        TrafficBlockPolicy {
            address: 2,
            detail: TrafficBlockDetail::AltitudeOnly,
            flight_callsign: None,
        },
        TrafficBlockPolicy {
            address: 3,
            detail: TrafficBlockDetail::SymbolOnly,
            flight_callsign: None,
        },
    ];
    let mut list = list_new(traffic_caps(3, &DEFAULT_STYLE).unwrap()).unwrap();
    let stats = build_traffic_with_policy(
        &mut list,
        &p,
        &oa_geo::wgs84(),
        &set,
        &DEFAULT_STYLE,
        Some(&policy),
    )
    .unwrap();
    let texts = texts_of(&list);
    assert!(texts.iter().any(|(text, _, _)| text.contains("UAL4321")));
    assert!(texts.iter().any(|(text, _, _)| text == "270"));
    assert!(texts.iter().all(|(text, _, _)| text != "270 --"));
    assert!(texts.iter().all(|(text, _, _)| !text.contains("2345")));
    assert_eq!(
        texts
            .iter()
            .filter(|(text, _, _)| text.contains("MODE A"))
            .count(),
        1
    );
    assert_eq!(stats.drawn, 3, "all position symbols remain represented");
    assert_eq!(
        (
            stats.full_blocks,
            stats.altitude_only_blocks,
            stats.symbol_only_targets
        ),
        (1, 1, 1)
    );
}

#[test]
fn regional_block_policy_refuses_missing_target_entries() {
    let mut set = targets_new(1).unwrap();
    targets_push(&mut set, &plain(ORD.0, ORD.1)).unwrap();
    let mut list = list_new(traffic_caps(1, &DEFAULT_STYLE).unwrap()).unwrap();
    let err = build_traffic_with_policy(
        &mut list,
        &proj(),
        &oa_geo::wgs84(),
        &set,
        &DEFAULT_STYLE,
        Some(&[]),
    )
    .unwrap_err();
    assert!(matches!(err, DisplayError::BadView(_)));
}

fn assert_bad_policy_is_atomic(target: &Target, policy: &[TrafficBlockPolicy]) {
    let mut set = targets_new(1).unwrap();
    targets_push(&mut set, target).unwrap();
    let mut list = list_new(traffic_caps(1, &DEFAULT_STYLE).unwrap()).unwrap();
    let result = build_traffic_with_policy(
        &mut list,
        &proj(),
        &oa_geo::wgs84(),
        &set,
        &DEFAULT_STYLE,
        Some(policy),
    );
    assert!(matches!(result, Err(DisplayError::BadView(_))));
    assert!(!list.faulted);
    assert!(list.lines.x0.is_empty());
    assert!(list.rects.x0.is_empty());
    assert!(list.circles.x.is_empty());
    assert!(texts_of(&list).is_empty());
}

// REQ: OA-DSP-041
#[test]
fn regional_policy_rejects_over_capacity_sorted_complete_input_atomically() {
    let mut target = plain(ORD.0, ORD.1);
    target.addr = 1;
    let policy: Vec<_> = (0..=MAX_TARGETS)
        .map(|address| TrafficBlockPolicy {
            address: address as u32,
            detail: TrafficBlockDetail::SymbolOnly,
            flight_callsign: None,
        })
        .collect();
    assert_bad_policy_is_atomic(&target, &policy);
}

// REQ: OA-DSP-041
#[test]
fn regional_policy_accepts_its_exact_capacity_boundary() {
    let mut target = plain(ORD.0, ORD.1);
    target.addr = 1;
    let policy: Vec<_> = (0..MAX_TARGETS)
        .map(|address| TrafficBlockPolicy {
            address: address as u32,
            detail: TrafficBlockDetail::SymbolOnly,
            flight_callsign: None,
        })
        .collect();
    let mut set = targets_new(1).unwrap();
    targets_push(&mut set, &target).unwrap();
    let mut list = list_new(traffic_caps(1, &DEFAULT_STYLE).unwrap()).unwrap();
    let stats = build_traffic_with_policy(
        &mut list,
        &proj(),
        &oa_geo::wgs84(),
        &set,
        &DEFAULT_STYLE,
        Some(&policy),
    )
    .unwrap();
    assert_eq!(stats.symbol_only_targets, 1);
    assert_eq!(stats.drawn, 1);
}

// REQ: OA-DSP-041
#[test]
fn regional_policy_rejects_each_detail_that_disagrees_with_target_state() {
    let mut target = plain(ORD.0, ORD.1);
    target.addr = 1;
    let invalid_full = [TrafficBlockPolicy {
        address: target.addr,
        detail: TrafficBlockDetail::Full,
        flight_callsign: None,
    }];
    assert_bad_policy_is_atomic(&target, &invalid_full);

    let invalid_altitude_only = [TrafficBlockPolicy {
        address: target.addr,
        detail: TrafficBlockDetail::AltitudeOnly,
        flight_callsign: None,
    }];
    assert_bad_policy_is_atomic(&target, &invalid_altitude_only);

    target.alt_ft = 27_000;
    let invalid_symbol_only = [TrafficBlockPolicy {
        address: target.addr,
        detail: TrafficBlockDetail::SymbolOnly,
        flight_callsign: None,
    }];
    assert_bad_policy_is_atomic(&target, &invalid_symbol_only);
}

type Line = ((i32, i32), (i32, i32), u8, u8);

fn lines_of(l: &DisplayList) -> Vec<Line> {
    (0..l.lines.x0.len())
        .map(|i| {
            (
                (l.lines.x0[i], l.lines.y0[i]),
                (l.lines.x1[i], l.lines.y1[i]),
                l.lines.colour[i],
                l.lines.layer[i],
            )
        })
        .collect()
}

fn texts_of(l: &DisplayList) -> Vec<(String, (i32, i32), u8)> {
    (0..l.texts.x.len())
        .map(|i| {
            let (s, n) = (l.texts.start[i] as usize, l.texts.len[i] as usize);
            (
                l.text[s..s + n].to_string(),
                (l.texts.x[i], l.texts.y[i]),
                l.texts.colour[i],
            )
        })
        .collect()
}

fn alt(a: i32) -> String {
    let mut b = text_new();
    alt_text(&mut b, a);
    text_str(&b).to_string()
}

fn speed(g: u16) -> String {
    let mut b = text_new();
    speed_text(&mut b, g);
    text_str(&b).to_string()
}

fn ident(c: &[u8; 8], addr: u32) -> String {
    let mut b = text_new();
    ident_text(&mut b, c, addr, false);
    text_str(&b).to_string()
}

fn tags(flags: u8, reacq: bool, r95: bool) -> String {
    let mut b = text_new();
    tags_text(&mut b, flags, reacq, r95);
    text_str(&b).to_string()
}

fn centre() -> (i32, i32) {
    project(&proj(), ORD.0, ORD.1).unwrap()
}

fn off(p: (i32, i32), dx: i32, dy: i32) -> (i32, i32) {
    (p.0 + dx * 16, p.1 + dy * 16)
}

// REQ: OA-DSP-039
#[test]
fn a_tracking_target_is_a_square_with_a_centre_dot_and_no_leader() {
    let mut t = plain(ORD.0, ORD.1);
    t.callsign = callsign("UAL123");
    t.alt_ft = 35_000;
    t.gs_dkt = 4_200;
    let style = TrafficStyle {
        leader_min: 0.0,
        ..DEFAULT_STYLE
    };
    let (l, st) = draw(&[t], &style);
    let c = centre();
    let (a, b, d, e) = (off(c, -4, -4), off(c, 4, -4), off(c, 4, 4), off(c, -4, 4));
    let want: Vec<_> = [(a, b), (b, d), (d, e), (e, a)]
        .into_iter()
        .map(|(p, q)| (p, q, pal::TARGET, LAYER_SYMBOL))
        .collect();
    assert_eq!(lines_of(&l), want);
    assert_eq!(l.rects.x0, vec![c.0 - 8]);
    assert_eq!(
        (l.rects.y0[0], l.rects.x1[0], l.rects.y1[0]),
        (c.1 - 8, c.0 + 8, c.1 + 8)
    );
    assert_eq!(l.rects.colour, vec![pal::TARGET]);
    assert!(l.circles.x.is_empty());
    assert_eq!(
        texts_of(&l),
        vec![
            ("UAL123".to_string(), off(c, 14, -30), pal::TARGET),
            ("350 42".to_string(), off(c, 14, -16), pal::TARGET),
        ]
    );
    assert_eq!(
        (st.drawn, st.with_leader, st.coasting, st.uncertain),
        (1, 0, 0, 0)
    );
}

// REQ: OA-DSP-039
#[test]
fn a_coasting_target_has_an_x_through_the_square_instead_of_the_dot() {
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_COAST;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let c = centre();
    let ls = lines_of(&l);
    assert_eq!(ls.len(), 6);
    assert_eq!(
        ls[4],
        (off(c, -4, -4), off(c, 4, 4), pal::TARGET, LAYER_SYMBOL)
    );
    assert_eq!(
        ls[5],
        (off(c, -4, 4), off(c, 4, -4), pal::TARGET, LAYER_SYMBOL)
    );
    assert!(l.rects.x0.is_empty(), "no centre dot");
    let tx = texts_of(&l);
    assert_eq!(tx.last().unwrap().0, "CST");
    assert_eq!(st.coasting, 1);
}

// REQ: OA-DSP-039
#[test]
fn an_uncertain_target_is_a_diamond_with_its_r95_circle_at_true_scale() {
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_UNCERTAIN;
    t.r95_m = 1_852.0;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let c = centre();
    let (n, e, s, w) = (off(c, 0, -6), off(c, 6, 0), off(c, 0, 6), off(c, -6, 0));
    let want: Vec<_> = [(n, e), (e, s), (s, w), (w, n)]
        .into_iter()
        .map(|(p, q)| (p, q, pal::TARGET, LAYER_SYMBOL))
        .collect();
    assert_eq!(lines_of(&l), want);
    assert_eq!(l.rects.x0.len(), 1, "the centre dot");
    assert_eq!(l.circles.x, vec![c.0]);
    assert_eq!(l.circles.y, vec![c.1]);
    // One nautical mile at the centre of the view, within a part in a thousand.
    let want_r = 1_852.0 * proj().scale;
    let got = f64::from(l.circles.r[0]);
    assert!((got - want_r).abs() < want_r * 1e-3, "{got} vs {want_r}");
    assert_eq!(l.circles.colour, vec![pal::TARGET]);
    assert_eq!(texts_of(&l).last().unwrap().0, "UNC");
    assert_eq!(st.uncertain, 1);
}

// REQ: OA-DSP-039
#[test]
fn an_uncertain_coasting_target_has_the_diamond_the_x_and_both_tags() {
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_UNCERTAIN | T_COAST;
    t.r95_m = f32::NAN;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let c = centre();
    let ls = lines_of(&l);
    assert_eq!(ls.len(), 6);
    assert_eq!(
        ls[4],
        (off(c, -6, -6), off(c, 6, 6), pal::TARGET, LAYER_SYMBOL)
    );
    // No usable r95: no circle, and still the diamond and the tag, never a guess; the data block
    // says the radius is missing and the frame counts it.
    assert!(l.circles.x.is_empty());
    assert_eq!(texts_of(&l).last().unwrap().0, "CST UNC R95?");
    assert_eq!(st.r95_unknown, 1);
}

// REQ: OA-DSP-039
#[test]
fn the_reacquired_mark_lasts_the_configured_seconds() {
    let mut t = plain(ORD.0, ORD.1);
    t.reacq_age_s = 9;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    assert_eq!(l.circles.r, vec![9 * 16]);
    assert_eq!(l.circles.colour, vec![pal::ALERT]);
    assert_eq!(texts_of(&l).last().unwrap().0, "REACQ");
    assert_eq!(st.reacquired, 1);
    t.reacq_age_s = 10;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    assert!(l.circles.r.is_empty());
    assert_eq!(st.reacquired, 0);
    t.reacq_age_s = 100;
    let longer = TrafficStyle {
        reacquired_s: 120,
        ..DEFAULT_STYLE
    };
    assert_eq!(draw(&[t], &longer).0.circles.r.len(), 1);
}

// REQ: OA-DSP-040
#[test]
fn the_leader_points_along_the_track_and_is_as_long_as_the_minutes_say() {
    let mut t = plain(ORD.0, ORD.1);
    // 600 knots due east: ten nautical miles in a minute.
    t.track_ddeg = 900;
    t.gs_dkt = 6_000;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let c = centre();
    let lead: Vec<_> = lines_of(&l)
        .into_iter()
        .filter(|x| x.3 == LAYER_LEADER)
        .collect();
    assert_eq!(lead.len(), 1);
    assert_eq!((lead[0].0, lead[0].2), (c, pal::TARGET));
    let want = 10.0 * 1_852.0 * proj().scale;
    let (dx, dy) = (f64::from(lead[0].1 .0 - c.0), f64::from(lead[0].1 .1 - c.1));
    assert!((dx - want).abs() < want * 5e-3, "{dx} vs {want}");
    assert!(dy.abs() < 16.0 * 3.0, "due east is level, dy {dy}");
    assert_eq!(st.with_leader, 1);
    // Two minutes is twice as long.
    let two = TrafficStyle {
        leader_min: 2.0,
        ..DEFAULT_STYLE
    };
    let l2 = draw(&[t], &two).0;
    let end2 = lines_of(&l2)
        .into_iter()
        .find(|x| x.3 == LAYER_LEADER)
        .unwrap()
        .1;
    assert!((f64::from(end2.0 - c.0) / dx - 2.0).abs() < 0.01);
    // Due north goes up the screen.
    t.track_ddeg = 0;
    let n = draw(&[t], &DEFAULT_STYLE).0;
    let end = lines_of(&n)
        .into_iter()
        .find(|x| x.3 == LAYER_LEADER)
        .unwrap()
        .1;
    assert!(end.1 < c.1 - 1000 && (end.0 - c.0).abs() < 48);
}

// REQ: OA-DSP-040
#[test]
fn no_velocity_or_no_leader_minutes_draws_no_leader() {
    let mut t = plain(ORD.0, ORD.1);
    t.gs_dkt = 4_000;
    assert_eq!(draw(&[t], &DEFAULT_STYLE).1.with_leader, 0, "no track");
    t.track_ddeg = 1_800;
    t.gs_dkt = NO_U16;
    assert_eq!(draw(&[t], &DEFAULT_STYLE).1.with_leader, 0, "no speed");
    t.gs_dkt = 4_000;
    let none = TrafficStyle {
        leader_min: 0.0,
        ..DEFAULT_STYLE
    };
    assert_eq!(draw(&[t], &none).1.with_leader, 0, "zero minutes");
}

// REQ: OA-DSP-040
#[test]
fn the_trail_is_the_last_n_positions_as_dim_dots_newest_first() {
    let mut t = plain(ORD.0, ORD.1);
    let pts = [
        (ORD.0 - 1000, ORD.1),
        (ORD.0 - 2000, ORD.1),
        (ORD.0 - 3000, ORD.1),
    ];
    for (k, p) in pts.iter().enumerate() {
        t.trail[k] = *p;
    }
    t.trail_len = 3;
    let style = TrafficStyle {
        trail_n: 2,
        ..DEFAULT_STYLE
    };
    let (l, _) = draw(&[t], &style);
    let dots: Vec<usize> = (0..l.rects.x0.len())
        .filter(|&i| l.rects.layer[i] == LAYER_TRAIL)
        .collect();
    assert_eq!(dots.len(), 2);
    for (&i, p) in dots.iter().zip(&pts) {
        let q = project(&proj(), p.0, p.1).unwrap();
        assert_eq!(
            (l.rects.x0[i], l.rects.y0[i], l.rects.x1[i], l.rects.y1[i]),
            (q.0 - 16, q.1 - 16, q.0 + 16, q.1 + 16)
        );
        assert_eq!(l.rects.colour[i], pal::DIM_TEXT);
    }
    let all = TrafficStyle {
        trail_n: 5,
        ..DEFAULT_STYLE
    };
    let n = (0..draw(&[t], &all).0.rects.x0.len()).count();
    assert_eq!(
        n, 4,
        "three dots and the centre dot; never more than the target has"
    );
}

// REQ: OA-DSP-040
#[test]
fn data_block_text_rounds_and_shows_what_is_missing() {
    assert_eq!(alt(35_000), "350");
    assert_eq!(alt(34_949), "349");
    assert_eq!(alt(34_950), "350");
    assert_eq!(alt(800), "008");
    assert_eq!(alt(0), "000");
    assert_eq!(alt(49), "000");
    assert_eq!(alt(50), "001");
    assert_eq!(alt(-200), "-02");
    assert_eq!(alt(-49), "000");
    assert_eq!(alt(NO_ALT), "---");
    assert_eq!(alt(99_949), "999");
    assert_eq!(
        alt(99_950),
        "999+",
        "beyond three digits is marked, not shown as a flight level"
    );
    assert_eq!(alt(i32::MAX), "999+");
    assert_eq!(alt(-9_949), "-99");
    assert_eq!(alt(-9_950), "-99-");
    assert_eq!(speed(4_200), "42");
    assert_eq!(speed(4_249), "42");
    assert_eq!(speed(4_250), "43");
    assert_eq!(speed(0), "00");
    assert_eq!(speed(60), "01");
    assert_eq!(speed(NO_U16 - 1), "655");
    assert_eq!(speed(NO_U16), "--");
    assert_eq!(ident(&callsign("DAL9"), 1), "DAL9");
    assert_eq!(ident(&callsign("DAL9   "), 1), "DAL9");
    assert_eq!(ident(&callsign("AAL1234T"), 1), "AAL1234T");
    assert_eq!(ident(&[0; 8], 0xA1B2C3), "A1B2C3");
    assert_eq!(ident(&[0; 8], 0xABC), "000ABC");
    assert_eq!(
        ident(&[0; 8], 0x0100_0ABC),
        "1000ABC",
        "non-ICAO shows its bit 24"
    );
    assert_eq!(ident(&[b' '; 8], 0x1), "000001");
    assert_eq!(ident(&[b'A', 0x07, b'B', 0, 0, 0, 0, 0], 1), "A#B");
    assert_eq!(tags(0, false, false), "");
    assert_eq!(tags(T_COAST | T_UNCERTAIN, true, false), "CST UNC REACQ");
    assert_eq!(
        tags(T_COAST | T_UNCERTAIN | T_CS_CONFLICT, true, true),
        "CST UNC CS? R95? REACQ"
    );
}

// REQ: OA-DSP-040
#[test]
fn a_target_without_a_callsign_or_altitude_shows_its_hex_address_and_dashes() {
    let t = plain(ORD.0, ORD.1);
    let (l, _) = draw(&[t], &DEFAULT_STYLE);
    let c = centre();
    assert_eq!(
        texts_of(&l),
        vec![
            ("A1B2C3".to_string(), off(c, 14, -30), pal::TARGET),
            ("--- --".to_string(), off(c, 14, -16), pal::TARGET),
        ]
    );
}

// REQ: OA-DSP-041
#[test]
fn a_full_set_refuses_counts_and_the_frame_says_so() {
    let p = proj();
    let mut set = targets_new(2).unwrap();
    let t = plain(ORD.0, ORD.1);
    assert!(targets_push(&mut set, &t).is_ok());
    assert!(targets_push(&mut set, &t).is_ok());
    assert_eq!(
        targets_push(&mut set, &t),
        Err(TrafficError::Full { capacity: 2 })
    );
    assert!(targets_push(&mut set, &t)
        .unwrap_err()
        .to_string()
        .contains("refused"));
    assert_eq!((targets_len(&set), targets_refused(&set)), (2, 2));
    let mut l = list_new(traffic_caps(2, &DEFAULT_STYLE).unwrap()).unwrap();
    let st = build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, &DEFAULT_STYLE).unwrap();
    assert_eq!((st.drawn, st.refused), (2, 2));
    let notice = texts_of(&l)
        .into_iter()
        .find(|t| t.0.starts_with("TARGETS NOT DRAWN"))
        .unwrap();
    assert_eq!(notice.0, "TARGETS NOT DRAWN: 2 (DISPLAY FULL AT 2)");
    assert_eq!(notice.2, pal::ALERT);
    assert_eq!(notice.1, (8 * 16, 600 * 16 - 22 * 16));
    // The next frame starts clean, and with no refusal there is no notice.
    targets_clear(&mut set);
    assert_eq!((targets_len(&set), targets_refused(&set)), (0, 0));
    assert!(targets_push(&mut set, &t).is_ok());
    let mut l = list_new(traffic_caps(2, &DEFAULT_STYLE).unwrap()).unwrap();
    build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, &DEFAULT_STYLE).unwrap();
    assert!(texts_of(&l).iter().all(|t| !t.0.contains("NOT DRAWN")));
}

// REQ: OA-DSP-041
#[test]
fn the_capacities_hold_the_busiest_target_at_the_limit_of_the_set() {
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_COAST | T_UNCERTAIN;
    t.r95_m = 2000.0;
    t.reacq_age_s = 0;
    t.callsign = callsign("AAL1234T");
    t.alt_ft = -1200;
    t.gs_dkt = 9_990;
    t.track_ddeg = 450;
    t.trail_len = MAX_TRAIL as u8;
    for k in 0..MAX_TRAIL {
        t.trail[k] = (ORD.0 + 100 * k as i32, ORD.1);
    }
    let style = TrafficStyle {
        trail_n: MAX_TRAIL as u8,
        ..DEFAULT_STYLE
    };
    let ts = vec![t; 40];
    let p = proj();
    let mut set = targets_new(40).unwrap();
    for x in &ts {
        targets_push(&mut set, x).unwrap();
    }
    let mut l = list_new(traffic_caps(40, &style).unwrap()).unwrap();
    let before = list_capacities(&l);
    build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, &style).unwrap();
    assert!(!l.faulted);
    assert_eq!(list_capacities(&l), before, "a frame allocates nothing");
    // A list built for fewer targets faults instead of dropping: full is an error, not a trim.
    let mut small = list_new(traffic_caps(2, &style).unwrap()).unwrap();
    let e = build_traffic(&mut small, &p, &oa_geo::wgs84(), &set, &style).unwrap_err();
    assert!(matches!(e, DisplayError::Full { .. }));
    assert!(small.faulted);
}

// REQ: OA-DSP-041
#[test]
fn targets_off_the_map_or_under_the_legend_strip_are_counted_not_drawn() {
    let p = projection_new(ORD.0, ORD.1, 20.0, 800, 600, 104).unwrap();
    let far = plain(ORD.0 + 100_000_000, ORD.1);
    let mut set = targets_new(3).unwrap();
    targets_push(&mut set, &far).unwrap();
    targets_push(&mut set, &plain(ORD.0, ORD.1)).unwrap();
    // North of the centre far enough to sit in the strip but inside the screen.
    targets_push(&mut set, &plain(ORD.0 + 3_600_000, ORD.1)).unwrap();
    let mut l = list_new(traffic_caps(3, &DEFAULT_STYLE).unwrap()).unwrap();
    let st = build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, &DEFAULT_STYLE).unwrap();
    assert_eq!((st.drawn, st.off_screen), (1, 2));
}

// REQ: OA-DSP-038
#[test]
fn a_bad_style_is_refused_and_a_huge_set_is_not_allocated() {
    for bad in [
        TrafficStyle {
            leader_min: f64::NAN,
            ..DEFAULT_STYLE
        },
        TrafficStyle {
            leader_min: -1.0,
            ..DEFAULT_STYLE
        },
        TrafficStyle {
            leader_min: 31.0,
            ..DEFAULT_STYLE
        },
        TrafficStyle {
            trail_n: MAX_TRAIL as u8 + 1,
            ..DEFAULT_STYLE
        },
    ] {
        assert!(matches!(
            traffic_caps(1, &bad),
            Err(DisplayError::BadView(_))
        ));
    }
    assert!(matches!(
        targets_new(MAX_TARGETS + 1),
        Err(DisplayError::TooLarge { .. })
    ));
    assert!(matches!(
        traffic_caps(usize::MAX, &DEFAULT_STYLE),
        Err(DisplayError::TooLarge { .. })
    ));
}

/// The synthetic scene: five targets around O'Hare, one of each kind.
fn scene() -> Vec<Target> {
    let mut a = plain(ORD.0, ORD.1);
    a.callsign = callsign("UAL123");
    a.alt_ft = 35_000;
    a.gs_dkt = 4_200;
    a.track_ddeg = 900;
    a.trail_len = 3;
    a.trail[0] = (ORD.0, ORD.1 - 40_000);
    a.trail[1] = (ORD.0, ORD.1 - 80_000);
    a.trail[2] = (ORD.0, ORD.1 - 120_000);
    let mut b = plain(ORD.0 + 2_000_000, ORD.1 + 3_000_000);
    b.flags = T_COAST;
    b.alt_ft = 12_400;
    b.gs_dkt = 2_500;
    b.track_ddeg = 2_700;
    let mut c = plain(ORD.0 - 2_500_000, ORD.1 + 1_000_000);
    c.flags = T_UNCERTAIN;
    c.r95_m = 3_000.0;
    c.callsign = callsign("N12345");
    c.gs_dkt = 900;
    c.track_ddeg = 1_800;
    let mut d = plain(ORD.0 + 1_000_000, ORD.1 - 4_000_000);
    d.reacq_age_s = 3;
    d.alt_ft = 8_000;
    d.gs_dkt = 3_100;
    d.track_ddeg = 3_000;
    let mut e = plain(ORD.0 - 500_000, ORD.1 - 2_000_000);
    e.flags = T_COAST | T_UNCERTAIN;
    e.r95_m = 1_500.0;
    e.alt_ft = 3_100;
    vec![a, b, c, d, e]
}

fn frame_hash(ts: &[Target]) -> String {
    let p = proj();
    let (l, _) = draw(ts, &DEFAULT_STYLE);
    // Draw into a 800x600 frame; the list does not depend on anything else.
    let spec = FrameSpec {
        width: p.width,
        height: p.height,
        stride: p.width * 4,
    };
    let mut buf = vec![0u8; frame_len(&spec)];
    rasterize(&l, &spec, &mut buf).unwrap();
    if let Some(dir) = std::env::var_os("OA_DISPLAY_DUMP") {
        let path = std::path::Path::new(&dir).join("traffic.ppm");
        let mut f = std::fs::File::create(path).unwrap();
        oa_display::ppm::write_ppm(&mut f, &spec, &buf).unwrap();
    }
    to_hex(&frame_digest(&spec, &buf).unwrap())
}

// REQ: OA-DSP-042
#[test]
fn the_synthetic_scene_is_deterministic_and_matches_its_golden_hash() {
    let a = frame_hash(&scene());
    assert_eq!(a, frame_hash(&scene()));
    // One target moved by 1e-7 degree changes the frame, so the hash is looking at the traffic.
    let mut moved = scene();
    moved[0].lat_e7 += 5_000;
    assert_ne!(a, frame_hash(&moved));
    if let Some(dir) = std::env::var_os("OA_DISPLAY_DUMP") {
        eprintln!("traffic golden {a} (dump dir {dir:?})");
    }
    // On the machines checked (x86_64, aarch64 expected the same: integer raster, f64 geodesics).
    assert_eq!(a, GOLDEN, "the synthetic traffic frame changed");
}

const GOLDEN: &str = "2069f5a641515639f69c5d8c50d255103afd6a2229efc581820e5178ce7caa95";

// REQ: OA-DSP-045
#[test]
fn traffic_draws_above_the_map_and_under_the_legend_strip() {
    use oa_display::scene::{
        LAYER_LEGEND, LAYER_LEGEND_BG, LAYER_SYMBOL as MAP_SYMBOL, LAYER_TEXT,
    };
    let traffic = [LAYER_TRAIL, LAYER_LEADER, LAYER_SYMBOL, LAYER_BLOCK];
    assert!(traffic.windows(2).all(|w| w[0] < w[1]));
    assert!(traffic.iter().all(|&l| l > MAP_SYMBOL.max(LAYER_TEXT)));
    assert!(traffic.iter().all(|&l| l < LAYER_LEGEND_BG));
    assert!(usize::from(LAYER_LEGEND) < MAX_LAYERS);
    // The traffic of a scene marks exactly its own layers.
    let (l, _) = draw(&scene(), &DEFAULT_STYLE);
    assert_eq!(l.layers_used & !0b0111_1000_0000, 0);
}

// REQ: OA-DSP-046
#[test]
fn an_invalid_target_is_refused_counted_and_said_on_the_frame_never_defaulted() {
    let p = proj();
    let mut set = targets_new(8).unwrap();
    let mut off_globe = plain(ORD.0, ORD.1);
    off_globe.lat_e7 = 900_000_001;
    let mut long_trail = plain(ORD.0, ORD.1);
    long_trail.trail_len = MAX_TRAIL as u8 + 1;
    let mut bad_history = plain(ORD.0, ORD.1);
    bad_history.trail_len = 1;
    bad_history.trail[0] = (0, 1_800_000_001);
    let mut bad_flags = plain(ORD.0, ORD.1);
    bad_flags.flags = 16;
    for t in [off_globe, long_trail, bad_history, bad_flags] {
        assert!(matches!(
            targets_push(&mut set, &t),
            Err(TrafficError::Malformed { .. })
        ));
    }
    assert!(targets_push(&mut set, &plain(ORD.0, ORD.1)).is_ok());
    assert_eq!(
        (
            targets_len(&set),
            targets_malformed(&set),
            targets_refused(&set)
        ),
        (1, 4, 0)
    );
    // Every column has one entry for the one target.
    let c = targets_columns(&set);
    assert_eq!(
        (
            c.lat_e7.len(),
            c.addr.len(),
            c.callsign.len(),
            c.trail_lat_e7.len()
        ),
        (1, 1, 1, MAX_TRAIL)
    );
    let mut l = list_new(traffic_caps(8, &DEFAULT_STYLE).unwrap()).unwrap();
    let st = build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, &DEFAULT_STYLE).unwrap();
    assert_eq!((st.drawn, st.malformed, st.refused), (1, 4, 0));
    assert_eq!(traffic_loss(&st), 4);
    let notice = texts_of(&l)
        .into_iter()
        .find(|t| t.0.starts_with("TARGETS MALFORMED"))
        .unwrap();
    assert_eq!(notice.0, "TARGETS MALFORMED: 4 (REFUSED)");
    assert_eq!(notice.2, pal::ALERT);
    // One line above the targets-not-drawn notice.
    assert_eq!(notice.1, (8 * 16, 600 * 16 - 36 * 16));
    targets_clear(&mut set);
    assert_eq!(targets_malformed(&set), 0);
}

// REQ: OA-DSP-046
#[test]
fn a_callsign_conflict_shows_the_address_and_the_tag_not_either_callsign() {
    let mut t = plain(ORD.0, ORD.1);
    t.callsign = callsign("UAL123");
    t.flags = T_CS_CONFLICT;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let tx = texts_of(&l);
    assert_eq!(tx[0].0, "A1B2C3");
    assert_eq!(tx.last().unwrap().0, "CS?");
    assert_eq!(st.cs_conflict, 1);
    assert_eq!(traffic_loss(&st), 0, "it is shown, not lost");
}

// REQ: OA-DSP-046
#[test]
fn a_zero_r95_is_unknown_and_does_not_emit_a_radius_circle() {
    let mut target = plain(ORD.0, ORD.1);
    target.flags = T_UNCERTAIN;
    target.r95_m = 0.0;
    let (list, stats) = draw(&[target], &DEFAULT_STYLE);
    assert_eq!(stats.r95_unknown, 1);
    assert_eq!(stats.circle_failed, 0);
    assert!(list.circles.x.is_empty());
    assert!(texts_of(&list)
        .iter()
        .any(|(text, _, _)| text.contains("R95?")));
}

// REQ: OA-DSP-046
#[test]
fn unrelated_target_flags_do_not_create_a_callsign_conflict() {
    let (list, stats) = draw(&[plain(ORD.0, ORD.1)], &DEFAULT_STYLE);
    assert_eq!(stats.cs_conflict, 0);
    assert!(texts_of(&list).iter().all(|(text, _, _)| text != "CS?"));
}

// REQ: OA-DSP-046
#[test]
fn radar_disagreement_is_a_visible_tag_without_changing_the_track_symbol() {
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_RADAR_DISAGREES;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let tx = texts_of(&l);
    assert_eq!(tx.last().unwrap().0, "RADAR?");
    assert_eq!((st.drawn, st.uncertain, st.coasting), (1, 0, 0));
    assert_eq!(
        traffic_loss(&st),
        0,
        "the contradictory evidence remains displayed"
    );
}

// REQ: OA-DSP-046
#[test]
fn a_track_outside_the_circle_is_counted_not_silently_left_without_a_leader() {
    let mut t = plain(ORD.0, ORD.1);
    t.gs_dkt = 4_000;
    t.track_ddeg = 3_600;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    assert_eq!((st.with_leader, st.leader_bad_track), (0, 1));
    assert_eq!(traffic_loss(&st), 1);
    assert!(lines_of(&l).iter().all(|x| x.3 != LAYER_LEADER));
    // No velocity at all is not a loss.
    let (_, st) = draw(&[plain(ORD.0, ORD.1)], &DEFAULT_STYLE);
    assert_eq!(traffic_loss(&st), 0);
}

// REQ: OA-DSP-046
#[test]
fn the_longest_data_block_fits_the_per_target_text_budget_and_is_never_clipped() {
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_COAST | T_UNCERTAIN | T_CS_CONFLICT | T_RADAR_DISAGREES;
    t.r95_m = f32::NAN;
    t.reacq_age_s = 0;
    t.addr = 0x1FF_FFFF;
    t.alt_ft = -1_200_000;
    t.gs_dkt = NO_U16 - 1;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    let tx = texts_of(&l);
    assert_eq!(tx[0].0, "1FFFFFF");
    assert_eq!(tx[1].0, "-99- 655");
    assert_eq!(tx[2].0, "CST UNC CS? RADAR? R95? REACQ");
    assert!(l.text.len() <= 64, "{} bytes", l.text.len());
    assert_eq!(st.text_clipped, 0);
}

// REQ: OA-DSP-046
#[test]
fn an_altitude_beyond_the_data_block_is_marked_and_counted() {
    let mut t = plain(ORD.0, ORD.1);
    t.alt_ft = 250_000;
    let (l, st) = draw(&[t], &DEFAULT_STYLE);
    assert_eq!(texts_of(&l)[1].0, "999+ --");
    assert_eq!(st.alt_display_clamped, 1);
    assert_eq!(traffic_loss(&st), 0, "it is shown, not lost");
    t.alt_ft = 35_000;
    assert_eq!(draw(&[t], &DEFAULT_STYLE).1.alt_display_clamped, 0);
}

// REQ: OA-DSP-046
#[test]
fn a_position_is_checked_before_it_can_count_as_the_edge_of_the_scope() {
    let p = proj();
    assert_eq!(map_position(&p, 900_000_001, 0), MapPos::Invalid);
    assert_eq!(map_position(&p, 0, -1_800_000_001), MapPos::Invalid);
    assert_eq!(map_position(&p, ORD.0 + 100_000_000, ORD.1), MapPos::Off);
    assert!(matches!(map_position(&p, ORD.0, ORD.1), MapPos::On(_)));
    assert!(on_map(&p, ORD.0, ORD.1) && !on_map(&p, 900_000_001, 0));
}

/// A valid position that `proj()` cannot place: opposite the centre.
const UNPLACED: (i32, i32) = (-422_586_000, 918_952_000);

/// Draws `ts` with the projection `p`.
fn draw_with(p: &Projection, ts: &[Target]) -> TrafficStats {
    let mut set = targets_new(ts.len().max(1)).unwrap();
    for t in ts {
        targets_push(&mut set, t).unwrap();
    }
    let st = DEFAULT_STYLE;
    let mut l = list_new(traffic_caps(targets_capacity(&set), &st).unwrap()).unwrap();
    build_traffic(&mut l, p, &oa_geo::wgs84(), &set, &st).unwrap()
}

// REQ: OA-DSP-046
#[test]
fn the_antipode_is_a_valid_position_that_does_not_project() {
    // Unprojectable is reachable: the point opposite the centre is on the globe and has no place
    // on the plane (the projection's denominator is below its floor there).
    let p = proj();
    // (The antipode of the conformal sphere, not of the ellipsoid: found by scanning.)
    assert_eq!(
        map_position(&p, UNPLACED.0, UNPLACED.1),
        MapPos::Unprojectable
    );
    // A degree away the position projects, and is far off the screen.
    assert_eq!(
        map_position(&p, UNPLACED.0 + 10_000_000, UNPLACED.1),
        MapPos::Off
    );
}

// REQ: OA-DSP-046
#[test]
fn a_target_that_cannot_be_placed_is_counted_as_unprojectable_not_as_off_screen() {
    let t = plain(UNPLACED.0, UNPLACED.1);
    let st = draw_with(&proj(), &[t]);
    assert_eq!((st.unprojectable, st.off_screen, st.drawn), (1, 0, 0));
    assert_eq!(traffic_loss(&st), 1);
}

// REQ: OA-DSP-046
#[test]
fn a_95_percent_radius_that_does_not_fit_the_list_is_clamped_and_counted() {
    // A 0.05 NM scope is about 10 pixels a metre; a radius of 20 000 km is beyond 2^28 sixteenths.
    let p = projection_new(ORD.0, ORD.1, 0.05, 800, 600, 0).unwrap();
    let mut t = plain(ORD.0, ORD.1);
    t.flags = T_UNCERTAIN;
    t.r95_m = 1.0e5;
    let near = draw_with(&p, &[t]);
    assert_eq!(near.circle_radius_clamped, 0);
    t.r95_m = 1.0e7;
    let far = draw_with(&p, &[t]);
    assert_eq!(
        (far.circle_radius_clamped, far.circle_failed),
        (1, 0),
        "{far:?}"
    );
    assert_eq!(traffic_loss(&far), 1);
}
