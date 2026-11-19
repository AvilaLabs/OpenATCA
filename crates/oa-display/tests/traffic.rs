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
use oa_display::project::{project, projection_new, Projection};
use oa_display::raster::{frame_digest, frame_len, rasterize, FrameSpec};
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
        callsign: [0; 8],
        flags: 0,
        reacq_age_s: NO_REACQ,
        r95_m: f32::NAN,
        trail: [(0, 0); MAX_TRAIL],
        trail_len: 0,
    }
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
    let mut l = list_new(traffic_caps(set.capacity, style).unwrap()).unwrap();
    let st = build_traffic(&mut l, &p, &oa_geo::wgs84(), &set, style).unwrap();
    (l, st)
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
    let (l, _) = draw(&[t], &DEFAULT_STYLE);
    let c = centre();
    let ls = lines_of(&l);
    assert_eq!(ls.len(), 6);
    assert_eq!(
        ls[4],
        (off(c, -6, -6), off(c, 6, 6), pal::TARGET, LAYER_SYMBOL)
    );
    // No usable r95: no circle, and still the diamond and the tag, never a guess.
    assert!(l.circles.x.is_empty());
    assert_eq!(texts_of(&l).last().unwrap().0, "CST UNC");
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
    assert_eq!(alt_text(35_000), "350");
    assert_eq!(alt_text(34_949), "349");
    assert_eq!(alt_text(34_950), "350");
    assert_eq!(alt_text(800), "008");
    assert_eq!(alt_text(0), "000");
    assert_eq!(alt_text(49), "000");
    assert_eq!(alt_text(50), "001");
    assert_eq!(alt_text(-200), "-02");
    assert_eq!(alt_text(-49), "000");
    assert_eq!(alt_text(NO_ALT), "---");
    assert_eq!(alt_text(i32::MAX), "999");
    assert_eq!(speed_text(4_200), "42");
    assert_eq!(speed_text(4_249), "42");
    assert_eq!(speed_text(4_250), "43");
    assert_eq!(speed_text(0), "00");
    assert_eq!(speed_text(60), "01");
    assert_eq!(speed_text(NO_U16 - 1), "655");
    assert_eq!(speed_text(NO_U16), "--");
    assert_eq!(ident_text(&callsign("DAL9"), 1), "DAL9");
    assert_eq!(ident_text(&callsign("DAL9   "), 1), "DAL9");
    assert_eq!(ident_text(&callsign("AAL1234T"), 1), "AAL1234T");
    assert_eq!(ident_text(&[0; 8], 0xA1B2C3), "A1B2C3");
    assert_eq!(ident_text(&[0; 8], 0xABC), "000ABC");
    assert_eq!(
        ident_text(&[0; 8], 0x0100_0ABC),
        "1000ABC",
        "non-ICAO shows its bit 24"
    );
    assert_eq!(ident_text(&[b' '; 8], 0x1), "000001");
    assert_eq!(ident_text(&[b'A', 0x07, b'B', 0, 0, 0, 0, 0], 1), "A#B");
    assert_eq!(tags_text(0, false), "");
    assert_eq!(tags_text(T_COAST | T_UNCERTAIN, true), "CST UNC REACQ");
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
    assert_eq!((targets_len(&set), set.refused), (2, 2));
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
    assert_eq!((targets_len(&set), set.refused), (0, 0));
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
