//! The safety-net indications: nothing drawn when all is well (so every other frame is unchanged),
//! the banner with each state, inhibition areas, feed states and the frozen picture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::list::*;
use oa_display::palette as pal;
use oa_display::project::{projection_new, Projection};
use oa_display::raster::{frame_digest, frame_len, rasterize, FrameSpec};
use oa_display::safety::*;
use oa_display::scene::LEGEND_STRIP_PX;
use oa_display::traffic::*;
use oa_safety::geom::Vertex;
use oa_safety::inhibit::*;
use oa_safety::net::*;

const S: i64 = 1_000_000_000;
const ORD: (i32, i32) = (419_786_000, -879_048_000);
/// 2026-11-19 14:00:00 UTC.
const NOW: i64 = 1_795_096_800 * S;

fn proj() -> Projection {
    projection_new(ORD.0, ORD.1, 20.0, 800, 600, LEGEND_STRIP_PX).unwrap()
}

/// A square of about 6 NM about the centre.
const SQUARE: [Vertex; 4] = [
    (418_786_000, -880_048_000),
    (420_786_000, -880_048_000),
    (420_786_000, -878_048_000),
    (418_786_000, -878_048_000),
];

fn well() -> (StateTable, Box<InhibitTable>, Vec<FeedStatus>) {
    let mut st = state_table_new(10 * S).unwrap();
    for n in NETS {
        state_report(&mut st, n, NetReport::Running, NOW).unwrap();
    }
    let feeds = vec![
        FeedStatus {
            name: *b"ADSB\0\0\0\0",
            state: FeedState::Live,
        },
        FeedStatus {
            name: *b"ASR9\0\0\0\0",
            state: FeedState::Live,
        },
    ];
    (st, Box::new(inhibits_new()), feeds)
}

fn scene<'a>(
    st: &'a StateTable,
    inh: &'a InhibitTable,
    feeds: &'a [FeedStatus],
) -> SafetyScene<'a> {
    SafetyScene {
        states: st,
        inhibits: inh,
        now_ns: NOW,
        feeds,
        newest_track_ns: Some(NOW - S),
        frozen_limit_ns: 5 * S,
    }
}

fn draw(sc: &SafetyScene<'_>) -> (DisplayList, SafetyStats) {
    let mut l = list_new(safety_caps()).unwrap();
    let stats = build_safety(&mut l, &proj(), sc).unwrap();
    (l, stats)
}

fn texts(l: &DisplayList) -> Vec<String> {
    (0..l.texts.x.len())
        .map(|i| {
            let s = l.texts.start[i] as usize;
            l.text[s..s + l.texts.len[i] as usize].to_string()
        })
        .collect()
}

fn empty(l: &DisplayList) -> bool {
    l.lines.x0.is_empty()
        && l.rects.x0.is_empty()
        && l.circles.x.is_empty()
        && l.polylines.start.is_empty()
        && l.texts.x.is_empty()
        && l.layers_used == 0
}

fn target() -> Target {
    Target {
        lat_e7: ORD.0,
        lon_e7: ORD.1,
        track_ddeg: 900,
        gs_dkt: 2500,
        alt_ft: 3000,
        addr: 0xA1B2C3,
        callsign: *b"UAL123\0\0",
        flags: 0,
        reacq_age_s: NO_REACQ,
        r95_m: f32::NAN,
        trail: [(0, 0); MAX_TRAIL],
        trail_len: 0,
    }
}

// REQ: OA-DSP-060
#[test]
fn nothing_is_drawn_when_every_net_runs_every_feed_is_live_and_the_picture_is_fresh() {
    let (st, inh, feeds) = well();
    let (l, stats) = draw(&scene(&st, &inh, &feeds));
    assert!(empty(&l));
    assert_eq!(stats, SafetyStats::default());
    // On top of a traffic frame it changes no pixel: the same digest with and without.
    let p = proj();
    let mut set = targets_new(4).unwrap();
    targets_push(&mut set, &target()).unwrap();
    let caps = caps_add(&traffic_caps(4, &DEFAULT_STYLE).unwrap(), &safety_caps()).unwrap();
    let mut base = list_new(caps).unwrap();
    build_traffic(&mut base, &p, &oa_geo::wgs84(), &set, &DEFAULT_STYLE).unwrap();
    let mut with = base.clone();
    build_safety(&mut with, &p, &scene(&st, &inh, &feeds)).unwrap();
    assert_eq!(base, with);
    let spec = FrameSpec {
        width: 800,
        height: 600,
        stride: 3200,
    };
    let (mut a, mut b) = (vec![0u8; frame_len(&spec)], vec![0u8; frame_len(&spec)]);
    rasterize(&base, &spec, &mut a).unwrap();
    rasterize(&with, &spec, &mut b).unwrap();
    assert_eq!(
        frame_digest(&spec, &a).unwrap(),
        frame_digest(&spec, &b).unwrap()
    );
}

// REQ: OA-DSP-061
#[test]
fn every_net_state_that_is_not_running_is_a_banner_line_in_the_alert_colour() {
    let (_, inh, feeds) = well();
    // The area proximity net last reported 11 s ago: stale. The approach net is running.
    let mut st2 = state_table_new(10 * S).unwrap();
    for n in NETS {
        let rep = match n {
            NetId::ConflictAlert => NetReport::Degraded {
                reason: Reason::BudgetExceeded,
                area: Area::Polygon(7),
                count: 12,
            },
            NetId::Msaw => NetReport::Unavailable {
                reason: Reason::AltimeterRefused,
            },
            _ => NetReport::Running,
        };
        let at = if n == NetId::AreaProximity {
            NOW - 11 * S
        } else {
            NOW - 3 * S
        };
        state_report(&mut st2, n, rep, at).unwrap();
    }
    let (l, stats) = draw(&scene(&st2, &inh, &feeds));
    assert_eq!(stats.nets_shown, 3);
    assert_eq!(stats.banner_lines, 3);
    assert!(!stats.frozen);
    let t = texts(&l);
    assert_eq!(
        t,
        [
            "CONFLICT ALERT DEGRADED: BUDGET EXCEEDED, AREA 7, COUNT 12, AGE 3S",
            "MSAW UNAVAILABLE: ALTIMETER REFUSED, AGE 3S",
            "AREA PROXIMITY UNAVAILABLE: STATE STALE, AGE 11S",
        ]
    );
    assert!(l.texts.colour.iter().all(|&c| c == pal::ALERT));
    assert!(l.texts.layer.iter().all(|&y| y == LAYER_BANNER));
    // The box sits under the legend strip.
    assert!(l
        .texts
        .y
        .iter()
        .all(|&y| y >= (LEGEND_STRIP_PX as i32) * 16));
    // Back to well: nothing at all.
    let (st3, inh3, feeds3) = well();
    assert!(empty(&draw(&scene(&st3, &inh3, &feeds3)).0));
}

// REQ: OA-DSP-062
#[test]
fn an_inhibition_is_drawn_as_a_labelled_outline_and_the_net_reads_inhibited() {
    let (st, mut inh, feeds) = well();
    let mut cfg = inhibit_config_none();
    cfg.max_duration_ns[net_index(NetId::Msaw)] = Some(3600 * S);
    let r = InhibitRequest {
        net: NetId::Msaw,
        vertices: &SQUARE,
        alt_low_ft: 0,
        alt_high_ft: 3000,
        start_ns: NOW - 60 * S,
        end_ns: NOW + 1800 * S,
        reason: "RANGE 3 TEST FIRING, SEE NOTAM",
        author: "SUP J SMITH",
    };
    inhibit_create(&mut inh, &cfg, &r, NOW - 60 * S).unwrap();
    // One not yet started is not drawn.
    let mut later = r;
    later.start_ns = NOW + 60 * S;
    inhibit_create(&mut inh, &cfg, &later, NOW).unwrap();
    let (l, stats) = draw(&scene(&st, &inh, &feeds));
    assert_eq!(
        (stats.inhibitions_drawn, stats.inhibitions_unplaced),
        (1, 0)
    );
    assert_eq!(l.polylines.start.len(), 1);
    assert_eq!(l.polylines.len[0], 5);
    assert_eq!(l.polylines.colour[0], pal::ALERT);
    assert_eq!(l.polylines.layer[0], LAYER_INHIBIT);
    // Closed: the first vertex is repeated at the end.
    assert_eq!((l.poly_x[0], l.poly_y[0]), (l.poly_x[4], l.poly_y[4]));
    let t = texts(&l);
    assert_eq!(
        t[0],
        "MSAW UNTIL 2026-11-19 14:30Z BY SUP J SMITH: RANGE 3 TEST FIR"
    );
    // The banner says the net is inhibited, with the count.
    assert_eq!(t[1], "MSAW INHIBITED: 1 IN FORCE, AGE 0S");
    assert_eq!(stats.nets_shown, 1);
    // An expired inhibition is no longer drawn.
    let mut late = scene(&st, &inh, &feeds);
    late.now_ns = NOW + 1800 * S;
    let (l, _) = draw(&late);
    assert!(l.polylines.start.is_empty());
    assert!(texts(&l).iter().all(|s| !s.contains("UNTIL")));
}

// REQ: OA-DSP-063
#[test]
fn the_picture_is_frozen_at_the_limit_and_not_one_nanosecond_before() {
    let (st, inh, feeds) = well();
    let mut sc = scene(&st, &inh, &feeds);
    sc.newest_track_ns = Some(NOW - 5 * S + 1);
    assert!(empty(&draw(&sc).0));
    sc.newest_track_ns = Some(NOW - 5 * S);
    let (l, stats) = draw(&sc);
    assert!(stats.frozen);
    assert_eq!(texts(&l), ["PICTURE FROZEN 5.0S"]);
    sc.newest_track_ns = Some(NOW - 73_456_000_000);
    assert_eq!(texts(&draw(&sc).0), ["PICTURE FROZEN 73.4S"]);
    // No update at all is frozen too.
    sc.newest_track_ns = None;
    assert_eq!(texts(&draw(&sc).0), ["PICTURE FROZEN, NO TRACK UPDATE"]);
    // Not a usable limit.
    sc.frozen_limit_ns = 0;
    let mut l = list_new(safety_caps()).unwrap();
    assert!(build_safety(&mut l, &proj(), &sc).is_err());
}

// REQ: OA-DSP-064
#[test]
fn a_feed_that_is_slow_or_lost_is_a_banner_line_with_its_age() {
    let (st, inh, mut feeds) = well();
    feeds[0].state = FeedState::Slow { age_ms: 3200 };
    feeds[1].state = FeedState::Lost { age_ms: 31_050 };
    let sc = scene(&st, &inh, &feeds);
    let (l, stats) = draw(&sc);
    assert_eq!(stats.feeds_shown, 2);
    assert_eq!(texts(&l), ["FEED ADSB SLOW 3.2S", "FEED ASR9 LOST 31.0S"]);
    assert!(l.texts.colour.iter().all(|&c| c == pal::ALERT));
    // More feeds than the display lists is refused, not cut.
    let many = vec![feeds[0]; MAX_FEEDS + 1];
    let sc = scene(&st, &inh, &many);
    let mut l = list_new(safety_caps()).unwrap();
    assert!(build_safety(&mut l, &proj(), &sc).is_err());
}

// REQ: OA-DSP-065
#[test]
fn the_worst_state_fits_the_capacities_and_the_work_budget() {
    let mut st = state_table_new(10 * S).unwrap();
    for n in NETS {
        state_report(
            &mut st,
            n,
            NetReport::Degraded {
                reason: Reason::AltimeterRefused,
                area: Area::Polygon(u32::MAX),
                count: u32::MAX,
            },
            NOW,
        )
        .unwrap();
    }
    let mut inh = Box::new(inhibits_new());
    let mut cfg = inhibit_config_none();
    cfg.max_duration_ns = [Some(3600 * S); NET_COUNT];
    // A 32-vertex convex ring.
    let ring: Vec<Vertex> = (0..32)
        .map(|k| {
            let a = f64::from(k) * core::f64::consts::TAU / 32.0;
            (
                ORD.0 + (a.sin() * 1_000_000.0) as i32,
                ORD.1 + (a.cos() * 1_000_000.0) as i32,
            )
        })
        .collect();
    for k in 0..INHIBIT_CAP {
        let r = InhibitRequest {
            net: NETS[k % 4],
            vertices: &ring,
            alt_low_ft: 0,
            alt_high_ft: 99_999,
            start_ns: NOW - S,
            end_ns: NOW + 1000 * S,
            reason: "0123456789ABCDEF0123456789ABCDEF",
            author: "ABCDEFGHIJKLMNOPQRSTUVWX",
        };
        inhibit_create(&mut inh, &cfg, &r, NOW).unwrap();
    }
    let feeds = vec![
        FeedStatus {
            name: *b"ABCDEFGH",
            state: FeedState::Lost { age_ms: u32::MAX },
        };
        MAX_FEEDS
    ];
    let mut sc = scene(&st, &inh, &feeds);
    sc.newest_track_ns = None;
    assert!(safety_work_charge(&sc) <= MAX_SAFETY_WORK as u128);
    let (l, stats) = draw(&sc);
    assert_eq!(stats.inhibitions_drawn as usize, INHIBIT_CAP);
    assert_eq!(stats.banner_lines as usize, 1 + 4 + MAX_FEEDS);
    assert_eq!(stats.text_clipped, 0, "{:?}", texts(&l));
    // The huge numbers are capped, the line whole.
    assert!(texts(&l).contains(
        &"CONFLICT ALERT DEGRADED: ALTIMETER REFUSED, AREA 999999+, COUNT 999999+, AGE 0S, INH 16"
            .to_string()
    ));
    assert!(!l.faulted);
    // The frame draws.
    let spec = FrameSpec {
        width: 800,
        height: 600,
        stride: 3200,
    };
    let mut buf = vec![0u8; frame_len(&spec)];
    rasterize(&l, &spec, &mut buf).unwrap();
}

// REQ: OA-DSP-066
#[test]
fn utc_time_is_civil_time_from_the_epoch() {
    assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0));
    assert_eq!(utc_parts(-1), (1969, 12, 31, 23, 59));
    assert_eq!(utc_parts(1_709_164_800 * S), (2024, 2, 29, 0, 0));
    assert_eq!(utc_parts(1_709_251_199 * S), (2024, 2, 29, 23, 59));
    assert_eq!(utc_parts(951_782_400 * S), (2000, 2, 29, 0, 0));
    assert_eq!(utc_parts(4_102_444_799 * S), (2099, 12, 31, 23, 59));
    assert_eq!(utc_parts(NOW + 1800 * S), (2026, 11, 19, 14, 30));
    let _ = utc_parts(i64::MIN);
    let _ = utc_parts(i64::MAX);
}
