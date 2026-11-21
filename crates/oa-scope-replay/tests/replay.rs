//! Tracker rows to targets, the frame chain over a map with no features, and the command line end
//! to end over a synthetic clip: same inputs, same hashes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{encode_payload, reports_push, Reports, Row, ROW_EMPTY};
use oa_airspace::model::{empty_model, Meta, Model};
use oa_display::project::projection_new;
use oa_display::raster::frame_digest;
use oa_display::traffic::{
    targets_clear, targets_columns, targets_len, targets_new, targets_refused, NO_ALT, NO_REACQ,
    NO_U16, T_COAST, T_CS_CONFLICT, T_UNCERTAIN,
};
use oa_scope_replay::adapt::{callsigns_from_reports, row_columns, ColumnBuf};
use oa_scope_replay::cli;
use oa_scope_replay::frame::{
    replay_build, replay_build_with_safety, replay_new, replay_raster, ReplayConfig, ReplaySafety,
};
use oa_scope_replay::rows::*;
use oa_scope_replay::store::*;
use oa_sha256::to_hex;
use oa_track::{output_new, TrackOutput, O_COAST, O_HAS_ALT, O_REACQUIRED, O_TICK, O_UNCERTAIN};

const ORD: (i32, i32) = (419_786_000, -879_048_000);
const S: i64 = 1_000_000_000;
const T0: i64 = 1_794_772_800 * S;

fn model() -> Model {
    empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    })
}

/// One tracker row on every column of `o`.
#[allow(clippy::too_many_arguments)]
fn push(
    o: &mut TrackOutput,
    t_ns: i64,
    addr: u32,
    pos: (i32, i32),
    vel: (f32, f32),
    alt: f32,
    cov: [f32; 3],
    flags: u8,
) {
    o.t_ns.push(t_ns);
    o.addr.push(addr);
    o.lat_e7.push(pos.0);
    o.lon_e7.push(pos.1);
    o.vel_east_mps.push(vel.0);
    o.vel_north_mps.push(vel.1);
    o.pos_cov_m2.push(cov);
    o.vel_cov_m2s2.push([1.0, 0.0, 1.0]);
    o.turn_rate_rps.push(0.0);
    o.alt_ft.push(alt);
    o.alt_var_ft2.push(100.0);
    o.vrate_fpm.push(0.0);
    o.mu_h.push([1.0, 0.0]);
    o.mu_v.push([1.0, 0.0]);
    o.flags.push(flags);
}

fn store_of(o: &TrackOutput) -> RowStore {
    let mut s = RowStore::default();
    store_add(&mut s, &row_columns(o, &mut ColumnBuf::default()), i64::MIN).unwrap();
    assert_eq!(store_finish(&mut s), 0);
    s
}

fn proj() -> oa_display::project::Projection {
    projection_new(ORD.0, ORD.1, 30.0, 800, 600, 0).unwrap()
}

fn calls(s: &str) -> [u8; 8] {
    let mut c = [0u8; 8];
    for (d, b) in c.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    c
}

const COV: [f32; 3] = [100.0, 0.0, 100.0];

// REQ: OA-DSP-043
#[test]
fn a_velocity_becomes_ground_speed_and_track() {
    // 100 m/s is 194.4 knots.
    assert_eq!(velocity_of(100.0, 0.0), (1944, 900));
    assert_eq!(velocity_of(0.0, 100.0), (1944, 0));
    assert_eq!(velocity_of(-100.0, 0.0), (1944, 2700));
    assert_eq!(velocity_of(0.0, -100.0), (1944, 1800));
    // Just west of north is still near 360, never 3600.
    let (_, trk) = velocity_of(-0.001, 100.0);
    assert!(trk == 0 || trk > 3590, "{trk}");
    // Standing still has a speed and no track to draw a leader along.
    assert_eq!(velocity_of(0.0, 0.0), (0, NO_U16));
    assert_eq!(velocity_of(f32::NAN, 1.0), (NO_U16, NO_U16));
    assert_eq!(velocity_of(1.0, f32::INFINITY), (NO_U16, NO_U16));
    let (gs, _) = velocity_of(1e9, 0.0);
    assert!(gs < NO_U16, "a huge speed never reads as absent");
}

// REQ: OA-DSP-043
#[test]
fn row_flags_altitude_and_radius_map_to_the_target() {
    let mut o = output_new(16);
    push(
        &mut o,
        T0,
        7,
        ORD,
        (100.0, 0.0),
        35_000.4,
        COV,
        O_HAS_ALT | O_TICK,
    );
    push(
        &mut o,
        T0 + S,
        8,
        ORD,
        (0.0, 0.0),
        f32::NAN,
        [4.0e6, 0.0, 4.0e6],
        O_COAST | O_UNCERTAIN,
    );
    let store = store_of(&o);
    let mut set = targets_new(8).unwrap();
    let c = Callsigns::default();
    let st = targets_at(&store, &c, T0 + S, &DEFAULT_CONVERT, &proj(), &mut set);
    assert_eq!((st.added, st.ended, st.off_map), (2, 0, 0));
    // Address order: 7 then 8.
    assert_eq!(targets_columns(&set).addr, vec![7, 8]);
    assert_eq!(
        (
            targets_columns(&set).gs_dkt[0],
            targets_columns(&set).track_ddeg[0]
        ),
        (1944, 900)
    );
    assert_eq!(targets_columns(&set).alt_ft[0], 35_000);
    assert_eq!(targets_columns(&set).flags[0], 0);
    assert_eq!(
        targets_columns(&set).alt_ft[1],
        NO_ALT,
        "no altitude flag, no altitude"
    );
    assert_eq!(targets_columns(&set).flags[1], T_COAST | T_UNCERTAIN);
    // 95% radius of a covariance of 100 m^2 per axis: sqrt(5.991 * 100).
    assert!(
        (f64::from(targets_columns(&set).r95_m[0]) - 24.48).abs() < 0.05,
        "{}",
        targets_columns(&set).r95_m[0]
    );
    assert!(targets_columns(&set).r95_m[1] > 4000.0);
    assert_eq!(targets_columns(&set).reacq_age_s, vec![NO_REACQ, NO_REACQ]);
    // A row with the altitude flag but a NaN altitude shows no altitude either.
    let mut o = output_new(2);
    push(&mut o, T0, 9, ORD, (1.0, 1.0), f32::NAN, COV, O_HAS_ALT);
    let mut set = targets_new(2).unwrap();
    targets_at(&store_of(&o), &c, T0, &DEFAULT_CONVERT, &proj(), &mut set);
    assert_eq!(targets_columns(&set).alt_ft, vec![NO_ALT]);
}

// REQ: OA-DSP-043
#[test]
fn a_frame_uses_only_rows_up_to_its_time_and_drops_ended_tracks() {
    let mut o = output_new(16);
    for k in 0..5 {
        push(
            &mut o,
            T0 + k * S,
            1,
            (ORD.0 + k as i32 * 1000, ORD.1),
            (0.0, 50.0),
            0.0,
            COV,
            0,
        );
    }
    let store = store_of(&o);
    let c = Callsigns::default();
    let mut set = targets_new(4).unwrap();
    // Half a second after row 2: row 2, not the later ones.
    targets_at(
        &store,
        &c,
        T0 + 2 * S + S / 2,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert_eq!(targets_columns(&set).lat_e7, vec![ORD.0 + 2000]);
    // Before the first row: nothing, and it is not an ended track.
    let mut set = targets_new(4).unwrap();
    let st = targets_at(&store, &c, T0 - S, &DEFAULT_CONVERT, &proj(), &mut set);
    assert_eq!((targets_len(&set), st.ended), (0, 0));
    // Longer than the stale time after the last row: the track has ended.
    let mut set = targets_new(4).unwrap();
    let st = targets_at(
        &store,
        &c,
        T0 + 4 * S + 4 * S,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert_eq!((targets_len(&set), st.ended), (0, 1));
    // Exactly at the stale time it is still shown.
    let mut set = targets_new(4).unwrap();
    targets_at(
        &store,
        &c,
        T0 + 4 * S + 3 * S,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert_eq!(targets_len(&set), 1);
}

// REQ: OA-DSP-043
#[test]
fn replay_frame_expires_only_the_silent_track_and_restores_it_on_a_new_row() {
    const A: u32 = 101;
    const B: u32 = 202;
    let mut o = output_new(8);
    push(&mut o, T0, A, ORD, (0.0, 50.0), 0.0, COV, 0);
    push(
        &mut o,
        T0,
        B,
        (ORD.0 + 10_000, ORD.1),
        (0.0, 50.0),
        0.0,
        COV,
        0,
    );
    // B continues updating while A is silent.
    push(
        &mut o,
        T0 + 3 * S,
        B,
        (ORD.0 + 13_000, ORD.1),
        (0.0, 50.0),
        0.0,
        COV,
        0,
    );
    // A becomes a real target again only when its own new row arrives.
    push(
        &mut o,
        T0 + 4 * S,
        A,
        (ORD.0 + 4_000, ORD.1),
        (0.0, 50.0),
        0.0,
        COV,
        0,
    );
    push(
        &mut o,
        T0 + 4 * S,
        B,
        (ORD.0 + 14_000, ORD.1),
        (0.0, 50.0),
        0.0,
        COV,
        0,
    );
    let store = store_of(&o);
    let mut replay = replay_new(model(), &frame_cfg()).unwrap();
    let build = |r: &mut oa_scope_replay::frame::Replay, at: i64| {
        replay_build(r, &store, &Callsigns::default(), at, "SYNTH").unwrap()
    };

    // At the documented inclusive boundary, A is still present (age == stale_ns).
    let boundary = build(&mut replay, T0 + 3 * S);
    assert_eq!(boundary.convert.added, 2);
    assert_eq!(targets_columns(&replay.set).addr, &[A, B]);

    // One nanosecond later, B remains current on its update while A expires alone.
    let expired = build(&mut replay, T0 + 3 * S + 1);
    assert_eq!((expired.convert.added, expired.convert.ended), (1, 1));
    assert_eq!(targets_columns(&replay.set).addr, &[B]);

    // A's later row restores it through the ordinary replay conversion and drawing path.
    let recovered = build(&mut replay, T0 + 4 * S);
    assert_eq!(recovered.convert.added, 2);
    assert_eq!(targets_columns(&replay.set).addr, &[A, B]);
}

// REQ: OA-DSP-043
#[test]
fn the_trail_takes_rows_a_step_apart_and_stops_at_the_start_of_the_track() {
    let mut o = output_new(64);
    for k in 0..=20 {
        push(
            &mut o,
            T0 + k * S,
            1,
            (ORD.0, ORD.1 + k as i32 * 1000),
            (0.0, 0.0),
            0.0,
            COV,
            O_TICK,
        );
    }
    let store = store_of(&o);
    let c = Callsigns::default();
    let cfg = ConvertConfig {
        trail_n: 3,
        trail_step_ns: 5 * S,
        ..DEFAULT_CONVERT
    };
    let mut set = targets_new(2).unwrap();
    targets_at(&store, &c, T0 + 20 * S, &cfg, &proj(), &mut set);
    assert_eq!(targets_columns(&set).trail_len, vec![3]);
    assert_eq!(
        &targets_columns(&set).trail_lon_e7[..3],
        &[ORD.1 + 15_000, ORD.1 + 10_000, ORD.1 + 5_000]
    );
    // Ten seconds in, two steps back have rows; at six seconds, one.
    let mut set = targets_new(2).unwrap();
    targets_at(&store, &c, T0 + 10 * S, &cfg, &proj(), &mut set);
    assert_eq!(targets_columns(&set).trail_len, vec![2]);
    let mut set = targets_new(2).unwrap();
    targets_at(&store, &c, T0 + 6 * S, &cfg, &proj(), &mut set);
    assert_eq!(targets_columns(&set).trail_len, vec![1]);
    assert_eq!(targets_columns(&set).trail_lon_e7[0], ORD.1 + 1_000);
}

// REQ: OA-DSP-043
#[test]
fn reacquired_age_counts_from_the_flagged_row_inside_the_window() {
    let mut o = output_new(64);
    for k in 0..=30 {
        let f = if k == 10 { O_REACQUIRED } else { 0 };
        push(&mut o, T0 + k * S, 1, ORD, (0.0, 0.0), 0.0, COV, f);
    }
    let store = store_of(&o);
    let c = Callsigns::default();
    let at = |t: i64, win: u16| {
        let cfg = ConvertConfig {
            reacq_window_s: win,
            ..DEFAULT_CONVERT
        };
        let mut set = targets_new(2).unwrap();
        targets_at(&store, &c, T0 + t * S, &cfg, &proj(), &mut set);
        targets_columns(&set).reacq_age_s[0]
    };
    assert_eq!(at(9, 10), NO_REACQ, "not yet");
    assert_eq!(at(10, 10), 0);
    assert_eq!(at(14, 10), 4);
    assert_eq!(at(20, 10), 10, "the window includes its last second");
    assert_eq!(at(21, 10), NO_REACQ);
    assert_eq!(at(25, 30), 15);
}

// REQ: OA-DSP-043
#[test]
fn callsigns_are_the_newest_heard_by_the_time_of_the_frame() {
    let mut r = Reports::default();
    for (t, icao, cs) in [
        (5, 1, "AAL1"),
        (9, 1, "AAL1"),
        (20, 1, "AAL2"),
        (7, 2, ""),
        (8, 2, "DAL9"),
    ] {
        let mut row: Row = ROW_EMPTY;
        row.time_ns = T0 + t * S;
        row.icao = icao;
        row.callsign = calls(cs);
        reports_push(&mut r, &row);
    }
    let (c, st) = callsigns_from_reports(&r).unwrap();
    assert_eq!(callsign_changes(&c, 1), 2, "a repeat is not a change");
    assert_eq!((st.reports_in, st.empty, st.conflicts), (5, 1, 0));
    let at = |addr: u32, t: i64| callsign_at(&c, addr, T0 + t * S);
    assert_eq!(at(1, 4).callsign, [0; 8], "never from the future");
    assert_eq!(at(1, 5).callsign, calls("AAL1"));
    assert_eq!(at(1, 19).callsign, calls("AAL1"));
    assert_eq!(at(1, 20).callsign, calls("AAL2"));
    assert_eq!(at(2, 7).callsign, [0; 8], "an empty callsign is not heard");
    assert_eq!(at(2, 9).callsign, calls("DAL9"));
    assert_eq!(at(3, 0).callsign, [0; 8]);
    assert!(!at(1, 20).conflict);
}

// REQ: OA-DSP-043
#[test]
fn off_map_targets_do_not_use_capacity_and_a_full_set_counts_its_refusals() {
    let mut o = output_new(16);
    for addr in 1..=4u32 {
        push(&mut o, T0, addr, ORD, (0.0, 0.0), 0.0, COV, 0);
    }
    push(
        &mut o,
        T0,
        9,
        (ORD.0 + 100_000_000, ORD.1),
        (0.0, 0.0),
        0.0,
        COV,
        0,
    );
    let store = store_of(&o);
    let mut set = targets_new(3).unwrap();
    let st = targets_at(
        &store,
        &Callsigns::default(),
        T0,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert_eq!((st.added, st.off_map), (3, 1));
    assert_eq!(
        targets_refused(&set),
        1,
        "the fourth on-map target was refused and counted"
    );
    assert_eq!(targets_columns(&set).addr, vec![1, 2, 3]);
}

fn frame_cfg() -> ReplayConfig {
    ReplayConfig {
        centre_lat_e7: ORD.0,
        centre_lon_e7: ORD.1,
        range_nm: 30.0,
        width: 640,
        height: 480,
        max_targets: 8,
        style: oa_display::traffic::DEFAULT_STYLE,
        convert: DEFAULT_CONVERT,
    }
}

/// Three synthetic targets: steady, coasting and uncertain, then reacquired.
fn synthetic_store() -> RowStore {
    let mut o = output_new(256);
    for k in 0..30i64 {
        let kk = k as i32;
        push(
            &mut o,
            T0 + k * S,
            1,
            (ORD.0, ORD.1 - 60_000 + kk * 2_000),
            (120.0, 0.0),
            11_000.0,
            COV,
            O_HAS_ALT | O_TICK,
        );
        let f = if k >= 15 { O_COAST | O_UNCERTAIN } else { 0 };
        push(
            &mut o,
            T0 + k * S,
            2,
            (ORD.0 + 80_000, ORD.1 + 90_000 - kk * 1_000),
            (-60.0, -20.0),
            f32::NAN,
            if k >= 15 { [9.0e5, 0.0, 9.0e5] } else { COV },
            f | O_TICK,
        );
        let r = if k == 20 { O_REACQUIRED } else { 0 };
        push(
            &mut o,
            T0 + k * S,
            3,
            (ORD.0 - 90_000, ORD.1 + kk * 500),
            (30.0, 10.0),
            2_000.0,
            COV,
            O_HAS_ALT | O_TICK | r,
        );
    }
    store_of(&o)
}

fn hash_at(r: &mut oa_scope_replay::frame::Replay, store: &RowStore, t: i64) -> String {
    let stamp = cli::stamp_of(t);
    replay_build(r, store, &Callsigns::default(), t, &stamp).unwrap();
    replay_raster(r).unwrap();
    to_hex(&frame_digest(&r.spec, &r.buf).unwrap())
}

// REQ: OA-DSP-044
#[test]
fn replayed_frames_are_the_same_in_every_run_and_do_not_depend_on_earlier_frames() {
    let store = synthetic_store();
    let times = [T0 + 5 * S, T0 + 17 * S, T0 + 22 * S, T0 + 5 * S];
    let mut a = replay_new(model(), &frame_cfg()).unwrap();
    let first: Vec<String> = times.iter().map(|&t| hash_at(&mut a, &store, t)).collect();
    let mut b = replay_new(model(), &frame_cfg()).unwrap();
    let second: Vec<String> = times.iter().map(|&t| hash_at(&mut b, &store, t)).collect();
    assert_eq!(first, second);
    assert_eq!(
        first[0], first[3],
        "frame 0 again after others is identical"
    );
    assert_ne!(first[0], first[1]);
    assert_ne!(first[1], first[2]);
    // The golden hash of one frame: three targets, one coasting and uncertain, one reacquired.
    assert_eq!(first[2], GOLDEN, "the replay frame changed");
}

const GOLDEN: &str = "3d27683429165aaedf3ff3e8341b2ed667adcbf711d356c223222a222efd2980";

// REQ: OA-DSP-044
#[test]
fn a_replay_frame_marks_state_and_says_when_targets_were_not_drawn() {
    let store = synthetic_store();
    let mut r = replay_new(model(), &frame_cfg()).unwrap();
    let t = T0 + 22 * S;
    let info = replay_build(&mut r, &store, &Callsigns::default(), t, "STAMP").unwrap();
    assert_eq!(info.traffic.drawn, 3);
    assert_eq!(
        (
            info.traffic.coasting,
            info.traffic.uncertain,
            info.traffic.reacquired
        ),
        (1, 1, 1)
    );
    let texts: Vec<&str> = (0..r.list.texts.x.len())
        .map(|i| {
            let (s, n) = (r.list.texts.start[i] as usize, r.list.texts.len[i] as usize);
            &r.list.text[s..s + n]
        })
        .collect();
    assert!(texts.contains(&"STAMP  TARGETS 3"));
    assert!(texts.contains(&"CST UNC"));
    assert!(texts.contains(&"REACQ"));
    // A capacity of two: the third is refused, counted and said on the frame.
    let cfg = ReplayConfig {
        max_targets: 2,
        ..frame_cfg()
    };
    let mut r = replay_new(model(), &cfg).unwrap();
    let info = replay_build(&mut r, &store, &Callsigns::default(), t, "STAMP").unwrap();
    assert_eq!((info.traffic.drawn, info.refused), (2, 1));
    assert!((0..r.list.texts.x.len()).any(|i| {
        let (s, n) = (r.list.texts.start[i] as usize, r.list.texts.len[i] as usize);
        r.list.text[s..s + n].starts_with("TARGETS NOT DRAWN: 1")
    }));
}

#[test]
fn safety_overlay_keeps_each_feed_outage_independent_of_track_freshness() {
    use oa_display::safety::{FeedState, FeedStatus};
    use oa_safety::inhibit::inhibits_new;
    use oa_safety::net::{state_report, state_table_new, NetReport, NETS};

    let mut states = state_table_new(30 * S, S).unwrap();
    for net in NETS {
        state_report(&mut states, net, NetReport::Running, T0, T0).unwrap();
    }
    let inhibits = inhibits_new();
    let feeds = [
        FeedStatus {
            name: *b"RADAR1\0\0",
            state: FeedState::Live,
        },
        FeedStatus {
            name: *b"ADSB\0\0\0\0",
            state: FeedState::Lost { age_ms: 12_000 },
        },
    ];
    let mut replay = replay_new(model(), &frame_cfg()).unwrap();
    let safety = ReplaySafety {
        states: &states,
        inhibits: &inhibits,
        feeds: &feeds,
        newest_track_ns: Some(T0),
        frozen_limit_ns: 5 * S,
    };
    replay_build_with_safety(
        &mut replay,
        &RowStore::default(),
        &Callsigns::default(),
        T0 + 6 * S,
        "SYNTH",
        &safety,
    )
    .unwrap();
    let texts: Vec<&str> = (0..replay.list.texts.x.len())
        .map(|i| {
            let (start, len) = (
                replay.list.texts.start[i] as usize,
                replay.list.texts.len[i] as usize,
            );
            &replay.list.text[start..start + len]
        })
        .collect();
    assert!(texts.iter().any(|s| s.starts_with("PICTURE FROZEN")));
    assert!(texts.iter().any(|s| s.starts_with("FEED ADSB LOST")));
    assert!(!texts.iter().any(|s| s.starts_with("FEED RADAR1")));
}

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// A clip of three aircraft flying east at 250 knots from near Chicago, one report a second for
/// 150 s, in one frame a second like a real clip; and its snapshot.
fn write_inputs(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let snap = dir.join("m.snap");
    std::fs::write(&snap, oa_airspace::snapshot::encode(&model()).unwrap()).unwrap();
    let oaf = dir.join("clip.oaf");
    let mut file = Vec::new();
    for sec in 0..150i64 {
        let mut r = Reports::default();
        for (k, addr) in [0xA00001u32, 0xA00002, 0xA00003].into_iter().enumerate() {
            let mut row: Row = ROW_EMPTY;
            row.time_ns = T0 + sec * S;
            row.icao = addr;
            row.lat_e7 = ORD.0 + 200_000 * k as i32;
            // 250 kt is 0.1286 km/s east: 0.00157 degree of longitude a second at this latitude.
            row.lon_e7 = ORD.1 - 1_000_000 + (sec as i32) * 15_700;
            row.baro_alt_ft = 10_000 + 1_000 * k as i32;
            row.gs_dkt = 2_500;
            row.track_ddeg = 900;
            row.callsign = calls(["UAL1", "DAL2", "N3"][k]);
            row.nic = 8;
            row.nac_p = 10;
            row.nac_v = 2;
            row.sil = 3;
            row.version = 2;
            reports_push(&mut r, &row);
        }
        let mut payload = Vec::new();
        encode_payload(&mut payload, &r, 0, 3).unwrap();
        let mut buf = vec![0u8; payload.len() + 256];
        let n =
            oa_feed::encode_frame(&mut buf, 1, 0, sec as u64, T0 + sec * S, 0, &payload).unwrap();
        file.extend_from_slice(&buf[..n]);
    }
    std::fs::write(&oaf, file).unwrap();
    (snap, oaf)
}

fn cli_args(
    snap: &std::path::Path,
    oaf: &std::path::Path,
    out: &std::path::Path,
    extra: &[&str],
) -> Vec<String> {
    let mut v = args(&[
        "--snapshot",
        snap.to_str().unwrap(),
        "--adsb",
        oaf.to_str().unwrap(),
        "--centre",
        "41.9786,-87.9048",
        "--range-nm",
        "30",
        "--size",
        "320x240",
        "--start",
        "1794772920",
        "--duration-s",
        "4",
        "--step-s",
        "1",
        "--warmup-s",
        "100",
        "--out-dir",
        out.to_str().unwrap(),
    ]);
    v.extend(extra.iter().map(|s| (*s).to_string()));
    v
}

// REQ: OA-DSP-044, OA-SEAL-014
#[test]
fn the_command_writes_one_ppm_a_step_and_a_hashes_file_that_repeats() {
    let dir = tmp("replay-cli");
    let (snap, oaf) = write_inputs(&dir);
    let (o1, o2, o3) = (dir.join("a"), dir.join("b"), dir.join("c"));
    let mut said = Vec::new();
    cli::run(&cli_args(&snap, &oaf, &o1, &[]), &mut said).unwrap();
    let said = String::from_utf8(said).unwrap();
    assert!(
        said.contains("window   2026-11-15T20:02:00Z for 4 frames"),
        "{said}"
    );
    assert!(said.contains("rows lost"), "{said}");
    // The clip line says how many frames and rows were left out for a sealed window.
    assert!(
        said.contains("0 frames and rows in a sealed window dropped"),
        "{said}"
    );
    assert!(
        said.contains("loss     none"),
        "a clean run says so: {said}"
    );
    for line in [
        "clip     ",
        "adsb     ",
        "rows     ",
        "calls    ",
        "targets  ",
    ] {
        assert!(said.contains(line), "{line:?} missing from {said}");
    }
    let h1 = std::fs::read_to_string(o1.join("hashes.txt")).unwrap();
    let lines: Vec<&str> = h1.lines().collect();
    assert_eq!(lines.len(), 4);
    for (i, l) in lines.iter().enumerate() {
        let f: Vec<&str> = l.split(' ').collect();
        assert_eq!(f.len(), 3, "{l}");
        assert_eq!(f[0], format!("{i:04}"));
        assert_eq!(f[1], format!("{}.000", 1_794_772_920 + i as i64));
        assert_eq!(f[2].len(), 64);
        assert!(std::path::Path::new(&o1.join(format!("frame-{i:04}.ppm"))).exists());
    }
    // The hash is of the pixels the file holds.
    let ppm = std::fs::read(o1.join("frame-0000.ppm")).unwrap();
    assert!(ppm.starts_with(b"P6\n320 240\n255\n"));
    let header = b"P6\n320 240\n255\n".len();
    let mut xrgb = Vec::new();
    for px in ppm[header..].chunks(3) {
        xrgb.extend_from_slice(&[px[2], px[1], px[0], 0]);
    }
    assert_eq!(
        to_hex(&oa_sha256::sha256(&xrgb)),
        lines[0].split(' ').nth(2).unwrap()
    );
    // Same inputs, same hashes; and without the images, the same hashes again.
    cli::run(&cli_args(&snap, &oaf, &o2, &[]), &mut Vec::new()).unwrap();
    assert_eq!(std::fs::read_to_string(o2.join("hashes.txt")).unwrap(), h1);
    cli::run(
        &cli_args(&snap, &oaf, &o3, &["--hashes-only"]),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(o3.join("hashes.txt")).unwrap(), h1);
    assert!(!o3.join("frame-0000.ppm").exists());
    // The aircraft move: consecutive frames differ. A different step is a different run.
    let hs: Vec<&str> = lines.iter().map(|l| l.split(' ').nth(2).unwrap()).collect();
    assert_ne!(hs[0], hs[1]);
    // The traffic is really on the picture: with no targets allowed the frame differs.
    let o4 = dir.join("d");
    cli::run(
        &cli_args(&snap, &oaf, &o4, &["--max-targets", "0", "--allow-loss"]),
        &mut Vec::new(),
    )
    .unwrap();
    assert_ne!(std::fs::read_to_string(o4.join("hashes.txt")).unwrap(), h1);
}

// REQ: OA-DSP-044
#[test]
fn bad_arguments_are_usage_errors_and_a_window_without_reports_fails_plainly() {
    let dir = tmp("replay-cli-bad");
    let (snap, oaf) = write_inputs(&dir);
    let out = dir.join("o");
    let ok = cli_args(&snap, &oaf, &out, &[]);
    let with = |flag: &str, value: &str| {
        let mut v = ok.clone();
        let i = v.iter().position(|a| a == flag).unwrap();
        v[i + 1] = value.to_string();
        v
    };
    for (flag, value) in [
        ("--start", "tomorrow"),
        ("--start", "2026-13-01T00:00:00Z"),
        ("--start", "nan"),
        ("--duration-s", "-1"),
        ("--duration-s", "0"),
        ("--step-s", "0.01"),
        ("--step-s", "inf"),
        ("--duration-s", "86401"),
        ("--range-nm", "0"),
        ("--size", "10x10"),
        ("--centre", "91,0"),
    ] {
        let e = cli::run(&with(flag, value), &mut Vec::new()).unwrap_err();
        assert!(matches!(e, cli::CliError::Usage(_)), "{flag} {value}: {e}");
    }
    let mut extra = ok.clone();
    extra.extend(args(&["--trail-n", "17"]));
    assert!(matches!(
        cli::run(&extra, &mut Vec::new()),
        Err(cli::CliError::Usage(_))
    ));
    let mut extra = ok.clone();
    extra.extend(args(&["--leader-min", "31"]));
    assert!(matches!(
        cli::run(&extra, &mut Vec::new()),
        Err(cli::CliError::Usage(_))
    ));
    let mut twice = ok.clone();
    twice.extend(args(&["--step-s", "1"]));
    assert!(matches!(
        cli::run(&twice, &mut Vec::new()),
        Err(cli::CliError::Usage(_))
    ));
    assert!(matches!(
        cli::run(&args(&["--snapshot", "x"]), &mut Vec::new()),
        Err(cli::CliError::Usage(_))
    ));
    // A start long after the clip: no reports, a plain failure, no images.
    let e = cli::run(&with("--start", "1894772920"), &mut Vec::new()).unwrap_err();
    assert!(matches!(e, cli::CliError::Failed(_)), "{e}");
    assert!(e.to_string().contains("no reports"), "{e}");
}

// REQ: OA-DSP-044
#[test]
fn times_parse_as_unix_seconds_or_utc_and_print_back() {
    assert_eq!(cli::parse_time("1794772800").unwrap(), T0);
    assert_eq!(cli::parse_time("2026-11-15T20:00:00Z").unwrap(), T0);
    assert_eq!(
        cli::parse_time("2026-11-15T20:00:01.5Z").unwrap(),
        T0 + 1_500_000_000
    );
    assert_eq!(cli::parse_time("1970-01-01T00:00:00Z").unwrap(), 0);
    assert_eq!(
        cli::parse_time("2000-03-01T00:00:00Z").unwrap(),
        951_868_800 * S
    );
    assert!(cli::parse_time("2026-11-15 20:00:00").is_err());
    assert!(cli::parse_time("2026-11-15T20:00:60Z").is_err());
    assert!(cli::parse_time("2026-11-15T24:00:00Z").is_err());
    assert_eq!(cli::stamp_of(T0), "2026-11-15T20:00:00Z");
    assert_eq!(
        cli::stamp_of(T0 + 1_500_000_000),
        "2026-11-15T20:00:01.500Z"
    );
    assert_eq!(cli::unix_text(T0 + 1_500_000_000), "1794772801.500");
}

/// Owned columns for `store_add`: every row at `T0` for address 1 at Chicago, standing still.
struct Cols {
    t: Vec<i64>,
    addr: Vec<u32>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    v: Vec<f32>,
    r95: Vec<f64>,
    alt: Vec<f32>,
    flags: Vec<u8>,
}

fn cols(n: usize) -> Cols {
    Cols {
        t: vec![T0; n],
        addr: vec![1; n],
        lat: vec![ORD.0; n],
        lon: vec![ORD.1; n],
        v: vec![0.0; n],
        r95: vec![oa_track::r95_m_of(&COV); n],
        alt: vec![0.0; n],
        flags: vec![0; n],
    }
}

fn view(c: &Cols) -> RowColumns<'_> {
    RowColumns {
        t_ns: &c.t,
        addr: &c.addr,
        lat_e7: &c.lat,
        lon_e7: &c.lon,
        vel_east_mps: &c.v,
        vel_north_mps: &c.v,
        r95_m: &c.r95,
        alt_ft: &c.alt,
        flags: &c.flags,
    }
}

// REQ: OA-DSP-047
#[test]
fn columns_of_different_lengths_are_an_error_before_any_row_is_read() {
    let mut s = RowStore::default();
    let mut c = cols(2);
    c.lat.pop();
    let e = store_add(&mut s, &view(&c), i64::MIN).unwrap_err();
    assert_eq!(
        e,
        RowsError::ColumnLength {
            column: "lat_e7",
            got: 1,
            want: 2
        }
    );
    assert_eq!(store_rows(&s), 0, "nothing was stored");
    assert!(e.to_string().contains("lat_e7"));
}

// REQ: OA-DSP-047
#[test]
fn every_row_is_counted_by_reason_and_rows_before_the_window_are_not_a_loss() {
    let mut s = RowStore::default();
    let mut c = cols(3);
    c.t = vec![T0 - 10 * S, T0, T0 + S];
    c.addr = vec![1, 1, 2];
    let st = store_add(&mut s, &view(&c), T0).unwrap();
    assert_eq!((st.rows_in, st.before_window, st.kept), (3, 1, 2));
    assert_eq!(store_loss(&st), 0);
    assert_eq!((store_rows(&s), store_addresses(&s)), (2, 2));
}

// REQ: OA-DSP-047
#[test]
fn a_value_that_does_not_fit_is_clamped_or_unknown_and_counted_never_the_absent_value() {
    let mut s = RowStore::default();
    let mut c = cols(5);
    c.t = (0..5).map(|k| T0 + k * S).collect();
    c.alt = vec![3.0e38, -3.0e9, f32::NAN, 35_000.0, 100.0];
    c.flags = vec![RF_HAS_ALT, RF_HAS_ALT, RF_HAS_ALT, 0, RF_HAS_ALT];
    c.r95[4] = f64::NAN;
    let st = store_add(&mut s, &view(&c), i64::MIN).unwrap();
    assert_eq!((st.alt_clamped, st.alt_not_finite, st.r95_bad), (2, 1, 1));
    let mut set = targets_new(2).unwrap();
    let alts: Vec<i32> =
        c.t.iter()
            .map(|&t| {
                targets_clear(&mut set);
                targets_at(
                    &s,
                    &Callsigns::default(),
                    t,
                    &DEFAULT_CONVERT,
                    &proj(),
                    &mut set,
                );
                targets_columns(&set).alt_ft[0]
            })
            .collect();
    assert_eq!(alts[0], i32::MAX);
    assert_eq!(
        alts[1],
        NO_ALT + 1,
        "clamped to one above the sentinel, not the sentinel"
    );
    assert_eq!(
        alts[2], NO_ALT,
        "a flagged altitude that is not a number is counted and absent"
    );
    assert_eq!(alts[3], NO_ALT, "no altitude flag, no altitude");
    assert_eq!(alts[4], 100);
    // The unknown 95% radius is NaN on the target, which the display marks.
    assert!(targets_columns(&set).r95_m[0].is_nan());
    // A speed beyond the field is the largest the field holds, never the absent value.
    let mut fast = cols(1);
    fast.addr = vec![2];
    fast.v = vec![1e9];
    let st = store_add(&mut s, &view(&fast), i64::MIN).unwrap();
    assert_eq!(st.speed_clamped, 1);
    targets_clear(&mut set);
    targets_at(
        &s,
        &Callsigns::default(),
        T0,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert!(targets_columns(&set).gs_dkt.contains(&(NO_U16 - 1)));
}

// REQ: OA-DSP-047
#[test]
fn extreme_times_neither_panic_nor_show_a_row_as_current() {
    let mut s = RowStore::default();
    let mut old = cols(1);
    old.t = vec![i64::MIN + 5];
    old.flags = vec![RF_REACQUIRED];
    store_add(&mut s, &view(&old), i64::MIN).unwrap();
    let mut set = targets_new(2).unwrap();
    let st = targets_at(
        &s,
        &Callsigns::default(),
        i64::MAX,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert_eq!(
        (st.ended, st.added),
        (1, 0),
        "a difference beyond i64 is an ended track"
    );
    // A trail step that overflows the arithmetic is counted, not wrapped.
    let mut now = cols(1);
    now.addr = vec![2];
    store_add(&mut s, &view(&now), i64::MIN).unwrap();
    let cfg = ConvertConfig {
        trail_step_ns: i64::MAX,
        ..DEFAULT_CONVERT
    };
    let st = targets_at(&s, &Callsigns::default(), T0, &cfg, &proj(), &mut set);
    assert_eq!((st.added, st.time_overflow), (1, 1));
}

// REQ: OA-DSP-047
#[test]
fn a_reacquired_window_that_could_reach_the_absent_age_is_refused() {
    let cfg = ConvertConfig {
        reacq_window_s: u16::MAX,
        ..DEFAULT_CONVERT
    };
    assert!(convert_check(&cfg).is_err());
    let c = ReplayConfig {
        convert: cfg,
        ..frame_cfg()
    };
    assert!(matches!(
        replay_new(model(), &c),
        Err(oa_scope_replay::frame::ReplayError::Config(_))
    ));
    assert!(convert_check(&ConvertConfig {
        reacq_window_s: 65_534,
        ..DEFAULT_CONVERT
    })
    .is_ok());
}

// REQ: OA-DSP-047
#[test]
fn the_store_refuses_at_its_named_capacities_and_counts_each_refusal() {
    // One row past the rows-per-address capacity.
    let mut s = RowStore::default();
    let mut c = cols(MAX_ROWS_PER_ADDRESS + 1);
    c.t = (0..c.t.len() as i64).map(|k| T0 + k).collect();
    let st = store_add(&mut s, &view(&c), i64::MIN).unwrap();
    assert_eq!(
        (st.kept, st.address_rows_full),
        (MAX_ROWS_PER_ADDRESS as u64, 1)
    );
    assert_eq!(store_loss(&st), 1);
    // One address past the address table.
    let mut s = RowStore::default();
    let mut c = cols(MAX_STORE_ADDRESSES + 1);
    c.addr = (0..c.addr.len() as u32).collect();
    let st = store_add(&mut s, &view(&c), i64::MIN).unwrap();
    assert_eq!(
        (st.kept, st.address_table_full),
        (MAX_STORE_ADDRESSES as u64, 1)
    );
    assert_eq!(store_addresses(&s), MAX_STORE_ADDRESSES);
}

fn call_row(t: i64, addr: u32, cs: &str) -> (i64, u32, [u8; 8]) {
    (T0 + t * S, addr, calls(cs))
}

fn build_calls(rows: &[(i64, u32, [u8; 8])]) -> (Callsigns, CallsignStats) {
    let t: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let a: Vec<u32> = rows.iter().map(|r| r.1).collect();
    let c: Vec<[u8; 8]> = rows.iter().map(|r| r.2).collect();
    callsigns_build(&t, &a, &c).unwrap()
}

// REQ: OA-DSP-048
#[test]
fn two_callsigns_for_one_address_at_one_instant_are_a_conflict_whatever_the_order() {
    let rows = [
        call_row(5, 1, "AAL1"),
        call_row(9, 1, "UAL7"),
        call_row(9, 1, "DAL3"),
        call_row(12, 1, "AAL1"),
    ];
    let mut rev = rows;
    rev.reverse();
    let (a, sa) = build_calls(&rows);
    let (b, sb) = build_calls(&rev);
    assert_eq!(a, b, "the result does not depend on the order of the input");
    assert_eq!((sa.conflicts, sb.conflicts), (1, 1));
    let at = |t: i64| callsign_at(&a, 1, T0 + t * S);
    assert_eq!((at(6).callsign, at(6).conflict), (calls("AAL1"), false));
    assert_eq!((at(9).callsign, at(9).conflict), ([0; 8], true));
    assert!(
        at(11).conflict,
        "a conflict stands until the next report with a callsign"
    );
    assert_eq!((at(12).callsign, at(12).conflict), (calls("AAL1"), false));
    // The same callsign twice at one instant is no conflict.
    let (c, sc) = build_calls(&[call_row(1, 2, "N1"), call_row(1, 2, "N1")]);
    assert_eq!(sc.conflicts, 0);
    assert!(!callsign_at(&c, 2, T0 + S).conflict);
    // Mismatched columns are an error.
    assert!(callsigns_build(&[1, 2], &[1], &[[0; 8]]).is_err());
}

// REQ: OA-DSP-048
#[test]
fn a_conflicting_target_carries_the_conflict_flag_and_no_callsign() {
    let mut o = output_new(4);
    push(&mut o, T0 + 9 * S, 1, ORD, (0.0, 0.0), 0.0, COV, 0);
    let (c, _) = build_calls(&[call_row(9, 1, "UAL7"), call_row(9, 1, "DAL3")]);
    let mut set = targets_new(2).unwrap();
    targets_at(
        &store_of(&o),
        &c,
        T0 + 9 * S,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    let cols = targets_columns(&set);
    assert_eq!(cols.flags[0] & T_CS_CONFLICT, T_CS_CONFLICT);
    assert_eq!(cols.callsign[0], [0; 8]);
}

// REQ: OA-DSP-049
#[test]
fn start_times_are_integer_exact_calendar_checked_and_bounded() {
    // Nanoseconds survive: an f64 could not hold 1.79e18 to the nanosecond.
    assert_eq!(
        cli::parse_time("2026-11-15T20:00:00.123456789Z").unwrap(),
        T0 + 123_456_789
    );
    assert_eq!(
        cli::parse_time("1794772800.123456789").unwrap(),
        T0 + 123_456_789
    );
    assert_eq!(cli::parse_time("1794772800.5").unwrap(), T0 + S / 2);
    // Leap years and the days of each month.
    for (ok, text) in [
        (true, "2024-02-29T00:00:00Z"),
        (true, "2000-02-29T00:00:00Z"),
        (false, "2025-02-29T00:00:00Z"),
        (false, "2100-02-29T00:00:00Z"),
        (false, "2026-02-30T00:00:00Z"),
        (false, "2026-04-31T00:00:00Z"),
        (true, "2026-04-30T23:59:59Z"),
        (false, "2026-00-10T00:00:00Z"),
        (false, "2026-11-00T00:00:00Z"),
    ] {
        assert_eq!(cli::parse_time(text).is_ok(), ok, "{text}");
    }
    // The last whole year an i64 of ns holds is 2261; 2262 and 1969 are refused, not saturated.
    assert!(cli::parse_time("2261-12-31T23:59:59.999999999Z").is_ok());
    assert!(cli::parse_time("2262-01-01T00:00:00Z").is_err());
    assert!(cli::parse_time("1969-12-31T23:59:59Z").is_err());
    assert_eq!(cli::parse_time("9223372036.854775807").unwrap(), i64::MAX);
    assert!(cli::parse_time("9223372036.854775808").is_err());
    assert!(
        cli::parse_time("253402300799").is_err(),
        "year 9999 does not fit ns"
    );
    // Not integer-exact forms.
    for text in [
        "1e9",
        "inf",
        "-5",
        "+5",
        "1794772800.",
        ".5",
        "1794772800.1234567891",
        "2026-11-15T20:00:00.Z",
        "2026-11-15T20:00:00.1234567891Z",
        "2026-1-5T20:00:00Z",
        "2026-11-15T20:00Z",
    ] {
        assert!(cli::parse_time(text).is_err(), "{text}");
    }
}

// REQ: OA-DSP-050
#[test]
fn a_non_empty_out_dir_is_refused_unless_overwrite_and_overwrite_deletes_only_our_files() {
    let dir = tmp("replay-cli-outdir");
    let (snap, oaf) = write_inputs(&dir);
    let out = dir.join("o");
    cli::run(&cli_args(&snap, &oaf, &out, &[]), &mut Vec::new()).unwrap();
    // An earlier, longer run left a frame that this run would not overwrite.
    std::fs::write(out.join("frame-0007.ppm"), b"old").unwrap();
    std::fs::write(out.join("notes.txt"), b"keep").unwrap();
    std::fs::write(out.join("frame-x.ppm"), b"keep").unwrap();
    let e = cli::run(&cli_args(&snap, &oaf, &out, &[]), &mut Vec::new()).unwrap_err();
    assert!(matches!(e, cli::CliError::Failed(_)), "{e}");
    assert!(e.to_string().contains("--overwrite"), "{e}");
    assert!(
        out.join("frame-0007.ppm").exists(),
        "a refused run deletes nothing"
    );
    cli::run(
        &cli_args(&snap, &oaf, &out, &["--overwrite"]),
        &mut Vec::new(),
    )
    .unwrap();
    assert!(
        !out.join("frame-0007.ppm").exists(),
        "the stray frame of the earlier run is gone"
    );
    assert!(out.join("notes.txt").exists() && out.join("frame-x.ppm").exists());
    assert!(out.join("frame-0003.ppm").exists() && out.join("hashes.txt").exists());
    // A directory that does not exist, and an empty one, are fine.
    cli::run(
        &cli_args(&snap, &oaf, &dir.join("new"), &[]),
        &mut Vec::new(),
    )
    .unwrap();
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    cli::run(&cli_args(&snap, &oaf, &empty, &[]), &mut Vec::new()).unwrap();
}

// REQ: OA-DSP-051
#[test]
fn a_run_that_loses_work_writes_everything_says_so_and_fails_unless_allowed() {
    let dir = tmp("replay-cli-loss");
    let (snap, oaf) = write_inputs(&dir);
    // Room for one target of three: the display refuses the others, in every frame.
    let (o1, o2) = (dir.join("a"), dir.join("b"));
    let mut said = Vec::new();
    let e = cli::run(
        &cli_args(&snap, &oaf, &o1, &["--max-targets", "1"]),
        &mut said,
    )
    .unwrap_err();
    let said = String::from_utf8(said).unwrap();
    assert!(matches!(e, cli::CliError::Loss(_)), "{e}");
    assert!(
        said.contains("loss     targets refused, display full (target-frames): 8"),
        "{said}"
    );
    assert!(
        o1.join("hashes.txt").exists(),
        "the output is written before the verdict"
    );
    // With --allow-loss the same run succeeds, and the report still says what was lost.
    let mut said2 = Vec::new();
    cli::run(
        &cli_args(&snap, &oaf, &o2, &["--max-targets", "1", "--allow-loss"]),
        &mut said2,
    )
    .unwrap();
    let said2 = String::from_utf8(said2).unwrap();
    assert!(said2.contains("allowed by --allow-loss"), "{said2}");
    assert_eq!(
        std::fs::read_to_string(o1.join("hashes.txt")).unwrap(),
        std::fs::read_to_string(o2.join("hashes.txt")).unwrap()
    );
}

// REQ: OA-DSP-051
#[test]
fn removal_names_match_oa_truth() {
    assert_eq!(cli::REMOVE_NAMES, oa_truth::REMOVE_NAMES);
}

// REQ: OA-DSP-046
#[test]
fn a_position_off_the_globe_is_a_loss_not_the_edge_of_the_scope() {
    let mut s = RowStore::default();
    let mut bad = cols(2);
    bad.addr = vec![1, 2];
    bad.lat = vec![ORD.0, 900_000_001];
    store_add(&mut s, &view(&bad), i64::MIN).unwrap();
    let mut set = targets_new(4).unwrap();
    let st = targets_at(
        &s,
        &Callsigns::default(),
        T0,
        &DEFAULT_CONVERT,
        &proj(),
        &mut set,
    );
    assert_eq!((st.added, st.off_map, st.invalid_position), (1, 0, 1));
}

// REQ: OA-DSP-049
#[test]
fn a_duration_that_is_not_a_whole_number_of_steps_is_a_usage_error() {
    let dir = tmp("replay-cli-multiple");
    let (snap, oaf) = write_inputs(&dir);
    let mut a = cli_args(&snap, &oaf, &dir.join("o"), &[]);
    let i = a.iter().position(|x| x == "--duration-s").unwrap();
    a[i + 1] = "4.5".into();
    assert!(matches!(
        cli::run(&a, &mut Vec::new()),
        Err(cli::CliError::Usage(_))
    ));
}

// REQ: OA-DSP-047
#[test]
fn the_capacities_are_a_parameter_and_each_refuses_with_a_named_error_or_count() {
    // Total rows: checked from the declared sizes before any row is stored.
    let caps = StoreCaps {
        addresses: 2,
        rows_per_address: 2,
        rows: 3,
    };
    let mut s = RowStore::default();
    let e = store_add_with(&mut s, &view(&cols(4)), i64::MIN, &caps).unwrap_err();
    assert_eq!(
        e,
        RowsError::TooMany {
            what: "stored tracker rows",
            wanted: 4,
            max: 3
        }
    );
    assert_eq!(store_rows(&s), 0, "nothing was stored");
    // Rows before the window do not count against the cap.
    let mut c = cols(4);
    c.t = vec![T0 - S, T0 - S, T0, T0];
    let st = store_add_with(&mut s, &view(&c), T0, &caps).unwrap();
    assert_eq!((st.kept, st.before_window), (2, 2));
    // Rows per address and addresses, with the small caps.
    let mut s = RowStore::default();
    let mut c = cols(3);
    c.addr = vec![1, 1, 1];
    let st = store_add_with(&mut s, &view(&c), i64::MIN, &caps).unwrap();
    assert_eq!((st.kept, st.address_rows_full), (2, 1));
    let mut c = cols(3);
    c.addr = vec![1, 2, 3];
    let st = store_add_with(&mut RowStore::default(), &view(&c), i64::MIN, &caps).unwrap();
    assert_eq!((st.kept, st.address_table_full), (2, 1));
    // The production caps are the constants.
    assert_eq!(STORE_CAPS.rows, MAX_STORE_ROWS);
    // The callsign reports.
    let e = callsigns_build_with(&[1, 2, 3], &[1, 1, 1], &[[0; 8]; 3], 2).unwrap_err();
    assert_eq!(
        e,
        RowsError::TooMany {
            what: "reports for callsigns",
            wanted: 3,
            max: 2
        }
    );
    assert!(e.to_string().contains("over the limit of 2"));
    assert!(callsigns_build_with(&[1, 2], &[1, 1], &[[0; 8]; 2], 2).is_ok());
}

// REQ: OA-DSP-047
#[test]
fn a_velocity_that_is_not_a_number_is_counted_and_shown_as_unknown() {
    let mut s = RowStore::default();
    let mut c = cols(3);
    c.addr = vec![1, 2, 3];
    c.v = vec![f32::NAN, 0.0, f32::INFINITY];
    let st = store_add(&mut s, &view(&c), i64::MIN).unwrap();
    assert_eq!((st.kept, st.velocity_not_finite), (3, 2));
    assert_eq!(store_loss(&st), 0, "shown as unknown, not a loss");
    let mut total = StoreStats::default();
    store_stats_add(&mut total, &st);
    store_stats_add(&mut total, &st);
    assert_eq!(total.velocity_not_finite, 4);
}

// REQ: OA-DSP-046
#[test]
fn a_bad_history_position_drops_that_dot_only_and_the_target_stays() {
    let mut s = RowStore::default();
    let mut c = cols(3);
    c.t = vec![T0 - 10 * S, T0 - 5 * S, T0];
    c.lat = vec![ORD.0, 900_000_001, ORD.0];
    store_add(&mut s, &view(&c), i64::MIN).unwrap();
    assert_eq!(store_finish(&mut s), 0);
    let mut set = targets_new(4).unwrap();
    let cfg = ConvertConfig {
        trail_n: 2,
        trail_step_ns: 5 * S,
        stale_ns: 3 * S,
        reacq_window_s: 10,
    };
    let st = targets_at(&s, &Callsigns::default(), T0, &cfg, &proj(), &mut set);
    assert_eq!((st.added, st.trail_dropped), (1, 1));
    assert_eq!(oa_display::traffic::targets_malformed(&set), 0);
    assert_eq!(
        targets_columns(&set).trail_len,
        vec![1],
        "the good dot stays"
    );
}

// REQ: OA-DSP-047
#[test]
fn the_reacquired_age_is_found_by_search_and_only_inside_the_window() {
    let mut s = RowStore::default();
    let mut c = cols(4);
    c.t = vec![T0 - 30 * S, T0 - 6 * S, T0 - 2 * S, T0];
    c.flags = vec![RF_REACQUIRED, RF_REACQUIRED, 0, 0];
    store_add(&mut s, &view(&c), i64::MIN).unwrap();
    assert_eq!(store_finish(&mut s), 0);
    let ages = |win: u16, at: i64| {
        let mut set = targets_new(2).unwrap();
        let cfg = ConvertConfig {
            reacq_window_s: win,
            ..DEFAULT_CONVERT
        };
        targets_at(&s, &Callsigns::default(), at, &cfg, &proj(), &mut set);
        targets_columns(&set).reacq_age_s.first().copied()
    };
    assert_eq!(ages(10, T0), Some(6), "the newest reacquired row, 6 s ago");
    assert_eq!(ages(5, T0), Some(NO_REACQ), "outside a 5 s window");
    assert_eq!(
        ages(60, T0 - 2 * S),
        Some(4),
        "never a row later than the time asked"
    );
}

// REQ: OA-DSP-048
#[test]
fn space_padding_and_nul_padding_are_the_same_callsign() {
    let (c, st) = build_calls(&[
        (T0, 1, *b"UAL1    "),
        (T0, 1, *b"UAL1\0\0\0\0"),
        (T0 + S, 2, *b"        "),
    ]);
    assert_eq!(st.conflicts, 0, "no conflict between two spellings");
    assert_eq!(st.empty, 1, "spaces only is no callsign");
    assert_eq!(callsign_at(&c, 1, T0).callsign, calls("UAL1"));
}

// REQ: OA-DSP-044
#[test]
fn replay_new_refuses_two_trail_lengths() {
    let cfg = ReplayConfig {
        centre_lat_e7: ORD.0,
        centre_lon_e7: ORD.1,
        range_nm: 30.0,
        width: 320,
        height: 240,
        max_targets: 4,
        style: oa_display::traffic::TrafficStyle {
            trail_n: 3,
            ..oa_display::traffic::DEFAULT_STYLE
        },
        convert: DEFAULT_CONVERT,
    };
    assert!(matches!(
        replay_new(model(), &cfg),
        Err(oa_scope_replay::frame::ReplayError::Config(_))
    ));
}

/// The clip of `write_inputs` with `extra` bytes appended.
fn clip_with(dir: &std::path::Path, extra: &[u8]) -> std::path::PathBuf {
    let (_, oaf) = write_inputs(dir);
    let mut bytes = std::fs::read(&oaf).unwrap();
    bytes.extend_from_slice(extra);
    let out = dir.join("clip2.oaf");
    std::fs::write(&out, bytes).unwrap();
    out
}

fn frame_bytes(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; payload.len() + 256];
    let n = oa_feed::encode_frame(&mut buf, kind, 0, 9_999, T0 + 120 * S, 0, payload).unwrap();
    buf.truncate(n);
    buf
}

fn run_text(a: &[String]) -> (Result<(), cli::CliError>, String) {
    let mut said = Vec::new();
    let r = cli::run(a, &mut said);
    (r, String::from_utf8(said).unwrap())
}

// REQ: OA-DSP-051
#[test]
fn reject_frames_are_loss_and_status_frames_are_counted_not_loss() {
    let dir = tmp("replay-cli-kinds");
    let (snap, _) = write_inputs(&dir);
    let reject = clip_with(&dir, &frame_bytes(2, b"too long"));
    let (r, said) = run_text(&cli_args(&snap, &reject, &dir.join("a"), &[]));
    assert!(matches!(r, Err(cli::CliError::Loss(_))), "{said}");
    assert!(said.contains("1 reject frames"), "{said}");
    assert!(
        said.contains("clip reject frames (input refused upstream): 1"),
        "{said}"
    );
    let status = clip_with(&dir, &frame_bytes(3, b"state=up\ndelay_ms=0\n"));
    let (r, said) = run_text(&cli_args(&snap, &status, &dir.join("b"), &[]));
    assert!(r.is_ok(), "{said}");
    assert!(said.contains("1 status frames"), "{said}");
    assert!(said.contains("loss     none"), "{said}");
}

// REQ: OA-DSP-051
#[test]
fn undecodable_and_damaged_clip_frames_are_loss_at_the_command_line() {
    let dir = tmp("replay-cli-damaged");
    let (snap, _) = write_inputs(&dir);
    let undecodable = clip_with(&dir, &frame_bytes(1, b"this is not a payload"));
    let (r, said) = run_text(&cli_args(&snap, &undecodable, &dir.join("a"), &[]));
    assert!(matches!(r, Err(cli::CliError::Loss(_))), "{said}");
    assert!(said.contains("undecodable clip payloads: 1"), "{said}");
    // A frame whose bytes were damaged (the checksum no longer matches).
    let mut bad = frame_bytes(1, b"payload bytes");
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;
    let damaged = clip_with(&dir, &bad);
    let (r, said) = run_text(&cli_args(&snap, &damaged, &dir.join("b"), &[]));
    assert!(matches!(r, Err(cli::CliError::Loss(_))), "{said}");
    assert!(said.contains("damaged clip frames: "), "{said}");
}

// REQ: OA-DSP-051
#[test]
fn the_binary_maps_a_loss_to_3_a_usage_error_to_2_and_a_failure_to_1() {
    let dir = tmp("replay-cli-exit");
    let (snap, oaf) = write_inputs(&dir);
    let bin = env!("CARGO_BIN_EXE_oa-scope-replay");
    let code = |a: Vec<String>| {
        std::process::Command::new(bin)
            .args(a)
            .output()
            .unwrap()
            .status
            .code()
    };
    assert_eq!(code(cli_args(&snap, &oaf, &dir.join("ok"), &[])), Some(0));
    assert_eq!(
        code(cli_args(
            &snap,
            &oaf,
            &dir.join("loss"),
            &["--max-targets", "1"]
        )),
        Some(3)
    );
    assert_eq!(code(args(&["--no-such-flag"])), Some(2));
    // The output directory of the first run is not empty: a failure.
    assert_eq!(code(cli_args(&snap, &oaf, &dir.join("ok"), &[])), Some(1));
}

// REQ: OA-DSP-050
#[test]
fn only_exact_output_names_are_ours_and_a_link_is_never_followed() {
    let dir = tmp("replay-cli-links");
    let (snap, oaf) = write_inputs(&dir);
    let out = dir.join("o");
    cli::run(&cli_args(&snap, &oaf, &out, &[]), &mut Vec::new()).unwrap();
    // Five digits and no digits are not ours.
    std::fs::write(out.join("frame-00007.ppm"), b"keep").unwrap();
    std::fs::write(out.join("frame-.ppm"), b"keep").unwrap();
    cli::run(
        &cli_args(&snap, &oaf, &out, &["--overwrite"]),
        &mut Vec::new(),
    )
    .unwrap();
    assert!(out.join("frame-00007.ppm").exists() && out.join("frame-.ppm").exists());
    // A link named like an output fails the run, and its target is untouched.
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, b"secret").unwrap();
    std::fs::remove_file(out.join("frame-0001.ppm")).unwrap();
    std::os::unix::fs::symlink(&secret, out.join("frame-0001.ppm")).unwrap();
    let e = cli::run(
        &cli_args(&snap, &oaf, &out, &["--overwrite"]),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(matches!(e, cli::CliError::Failed(_)), "{e}");
    assert!(e.to_string().contains("not a plain file"), "{e}");
    assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
}

// REQ: OA-SEAL-014
#[test]
fn the_clip_reader_drops_sealed_window_frames_and_rows_and_counts_them() {
    // The list's window: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
    const FROM: i64 = 1_795_131_900_000_000_000;
    const TO: i64 = 1_795_393_920_000_000_000;
    let dir = tmp("seal_window");
    // One frame per entry: (frame receive time, [row times]).
    let frames: [(i64, &[i64]); 4] = [
        // Received just before the window but carrying rows timed at from - 1 (kept), from and
        // to - 1 (dropped), and to (kept).
        (FROM - 1, &[FROM - 1, FROM, TO - 1, TO]),
        // Received in the window: the whole frame is dropped.
        (FROM, &[TO]),
        (TO - 1, &[TO]),
        (TO, &[TO + 1_000_000_000]),
    ];
    let mut bytes = Vec::new();
    for (i, (recv, times)) in frames.iter().enumerate() {
        let mut r = Reports::default();
        for (k, t) in times.iter().enumerate() {
            let mut row: Row = ROW_EMPTY;
            row.time_ns = *t;
            row.icao = 0xA00001 + k as u32;
            row.lat_e7 = ORD.0;
            row.lon_e7 = ORD.1;
            row.nic = 8;
            row.nac_p = 10;
            row.sil = 3;
            row.version = 2;
            reports_push(&mut r, &row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, 0, times.len()).unwrap();
        let mut buf = vec![0u8; oa_feed::MAX_FRAME_LEN];
        let n = oa_feed::encode_frame(
            &mut buf,
            1,
            oa_feed::FLAG_BYTES,
            i as u64,
            *recv,
            recv / 1_000_000,
            &payload,
        )
        .unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let clip = dir.join("neutral-name.oaf");
    std::fs::write(&clip, bytes).unwrap();
    let (_, _, tr) = cli::track_window(&clip, FROM - 10, TO + 10_000_000_000, TO + 5, 0).unwrap();
    // Two frames by receive time (at from and at to - 1 ns), counted one each; and the frame
    // received at from - 1 ns, dropped whole for its rows timed in the window, its four rows
    // counted.
    assert_eq!(tr.sealed_window_dropped, 6);
    // Kept: the row at to + 1 s, in the frame received at to.
    assert_eq!(tr.reports, 1);
    // The decoder's counters hold only the frame that was kept: the dropped ones are in none.
    assert_eq!(
        (tr.decode.records, tr.decode.kept, tr.decode.dropped),
        (1, 1, 0)
    );
    // On every exit the count is given: a window that holds only sealed frames is an error, and
    // the count of what was dropped is still returned.
    let mut sealed = 0;
    let e = cli::track_window_counted(&clip, FROM, TO - 1, TO + 5, 0, &mut sealed).unwrap_err();
    assert!(e.to_string().contains("no reports in the window"), "{e}");
    // The two frames received in the window; the frame just before it is outside the read span.
    assert_eq!(sealed, 2);
}

// REQ: OA-SEAL-006
#[test]
fn a_sealed_snapshot_path_is_refused_by_the_guard_before_its_size_is_read() {
    let out = std::env::temp_dir().join(format!("oa-scope-seal-{}", std::process::id()));
    let a = cli_args(
        std::path::Path::new("/nonexistent/scds/stdds-20261119-holdout.bak"),
        std::path::Path::new("/nonexistent/clip.oaf"),
        &out,
        &[],
    );
    let e = cli::run(&a, &mut Vec::new()).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
    let _ = std::fs::remove_dir_all(&out);
}
