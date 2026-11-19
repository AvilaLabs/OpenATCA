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
    targets_len, targets_new, NO_ALT, NO_REACQ, NO_U16, T_COAST, T_UNCERTAIN,
};
use oa_scope_replay::cli;
use oa_scope_replay::frame::{replay_build, replay_new, replay_raster, ReplayConfig};
use oa_scope_replay::rows::*;
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
    store_add(&mut s, o, i64::MIN);
    store_finish(&mut s);
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
    assert_eq!(set.addr, vec![7, 8]);
    assert_eq!((set.gs_dkt[0], set.track_ddeg[0]), (1944, 900));
    assert_eq!(set.alt_ft[0], 35_000);
    assert_eq!(set.flags[0], 0);
    assert_eq!(set.alt_ft[1], NO_ALT, "no altitude flag, no altitude");
    assert_eq!(set.flags[1], T_COAST | T_UNCERTAIN);
    // 95% radius of a covariance of 100 m^2 per axis: sqrt(5.991 * 100).
    assert!(
        (f64::from(set.r95_m[0]) - 24.48).abs() < 0.05,
        "{}",
        set.r95_m[0]
    );
    assert!(set.r95_m[1] > 4000.0);
    assert_eq!(set.reacq_age_s, vec![NO_REACQ, NO_REACQ]);
    // A row with the altitude flag but a NaN altitude shows no altitude either.
    let mut o = output_new(2);
    push(&mut o, T0, 9, ORD, (1.0, 1.0), f32::NAN, COV, O_HAS_ALT);
    let mut set = targets_new(2).unwrap();
    targets_at(&store_of(&o), &c, T0, &DEFAULT_CONVERT, &proj(), &mut set);
    assert_eq!(set.alt_ft, vec![NO_ALT]);
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
    assert_eq!(set.lat_e7, vec![ORD.0 + 2000]);
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
    assert_eq!(set.trail_len, vec![3]);
    assert_eq!(
        &set.trail_lon_e7[..3],
        &[ORD.1 + 15_000, ORD.1 + 10_000, ORD.1 + 5_000]
    );
    // Ten seconds in, two steps back have rows; at six seconds, one.
    let mut set = targets_new(2).unwrap();
    targets_at(&store, &c, T0 + 10 * S, &cfg, &proj(), &mut set);
    assert_eq!(set.trail_len, vec![2]);
    let mut set = targets_new(2).unwrap();
    targets_at(&store, &c, T0 + 6 * S, &cfg, &proj(), &mut set);
    assert_eq!(set.trail_len, vec![1]);
    assert_eq!(set.trail_lon_e7[0], ORD.1 + 1_000);
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
        set.reacq_age_s[0]
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
    let c = callsigns_from_reports(&r);
    assert_eq!(c.by_addr[&1].len(), 2, "a repeat is not a change");
    assert_eq!(
        callsign_at(&c, 1, T0 + 4 * S),
        [0; 8],
        "never from the future"
    );
    assert_eq!(callsign_at(&c, 1, T0 + 5 * S), calls("AAL1"));
    assert_eq!(callsign_at(&c, 1, T0 + 19 * S), calls("AAL1"));
    assert_eq!(callsign_at(&c, 1, T0 + 20 * S), calls("AAL2"));
    assert_eq!(
        callsign_at(&c, 2, T0 + 7 * S),
        [0; 8],
        "an empty callsign is not heard"
    );
    assert_eq!(callsign_at(&c, 2, T0 + 9 * S), calls("DAL9"));
    assert_eq!(callsign_at(&c, 3, T0), [0; 8]);
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
        set.refused, 1,
        "the fourth on-map target was refused and counted"
    );
    assert_eq!(set.addr, vec![1, 2, 3]);
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

// REQ: OA-DSP-044
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
        &cli_args(&snap, &oaf, &o4, &["--max-targets", "0"]),
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
