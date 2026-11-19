#![deny(unsafe_code)]
//! Standalone allocation validation of the display core (RULESET C5). Shell (B3): fixtures,
//! execution and the report.
//!
//! After every table is built, a full traffic frame is made under a counting allocator and must
//! allocate nothing: the conversion of tracker rows to targets (`targets_at`), the traffic
//! drawing (`build_traffic`) over a full set, and the whole frame chain (`replay_build`, which is
//! the map, the traffic and the stamp, then `replay_raster`). The fixture exercises every state of
//! a target (coasting, uncertain, an unknown 95% radius, reacquired, a callsign conflict, a
//! trail), a set that refuses at capacity and a malformed target, and the exercised path is
//! checked after counting is disarmed, so a window that drew nothing cannot pass.

mod allocator;

use allocator::{control_alloc, control_realloc, control_zeroed, inject_core_allocation, Counts};
use oa_airspace::model::{empty_model, Meta};
use oa_display::list::{list_clear, list_new};
use oa_display::project::projection_new;
use oa_display::traffic::{
    build_traffic, caps_add, targets_clear, targets_new, targets_push, traffic_caps, Target,
    TrafficStats, DEFAULT_STYLE, MAX_TRAIL, NO_ALT, NO_REACQ, T_COAST, T_CS_CONFLICT, T_UNCERTAIN,
};
use oa_scope_replay::frame::{replay_build, replay_new, replay_raster, ReplayConfig};
use oa_scope_replay::rows::{
    callsigns_build, store_add, store_finish, targets_at, ConvertStats, RowColumns, RowStore,
    DEFAULT_CONVERT,
};
use oa_track::{O_COAST, O_HAS_ALT, O_REACQUIRED, O_TICK, O_UNCERTAIN};

const S: i64 = 1_000_000_000;
const T0: i64 = 1_794_772_800 * S;
const ORD: (i32, i32) = (419_786_000, -879_048_000);
const AIRCRAFT: u32 = 200;
const SECONDS: i64 = 40;
const FRAMES: i64 = 20;
const FIRST_FRAME_S: i64 = 25;

fn fail(message: &str) -> ! {
    println!("{{\"status\":\"FAIL_DSP_ALLOCATION\",\"message\":\"{message}\"}}");
    std::process::exit(1);
}

fn run_controls() -> Option<Counts> {
    if !allocator::start() {
        return None;
    }
    let ok = control_alloc() && control_zeroed() && control_realloc();
    let counts = allocator::stop()?;
    (ok && !counts.overflow && counts.alloc >= 1 && counts.zeroed >= 1 && counts.realloc >= 1)
        .then_some(counts)
}

// A7.2: cold fixture report, one per window.
struct Window {
    name: &'static str,
    counts: Counts,
    frames: i64,
}

/// Owned tracker columns for the fixture: `AIRCRAFT` aircraft, one row a second for `SECONDS`.
// A7.2: cold fixture data, built once before measurement.
struct Cols {
    t: Vec<i64>,
    addr: Vec<u32>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    v: Vec<f32>,
    cov: Vec<[f32; 3]>,
    alt: Vec<f32>,
    flags: Vec<u8>,
}

fn fixture_columns() -> Cols {
    let mut c = Cols {
        t: Vec::new(),
        addr: Vec::new(),
        lat: Vec::new(),
        lon: Vec::new(),
        v: Vec::new(),
        cov: Vec::new(),
        alt: Vec::new(),
        flags: Vec::new(),
    };
    for a in 0..AIRCRAFT {
        let (row, col) = ((a % 20) as i32 - 10, ((a / 20) % 10) as i32 - 5);
        for k in 0..SECONDS {
            let mut flags = O_TICK;
            let mut cov = [100.0f32, 0.0, 100.0];
            let mut alt = 11_000.0f32;
            if a % 5 == 0 {
                flags |= O_COAST;
            }
            if a % 7 == 0 {
                flags |= O_UNCERTAIN;
                cov = [9.0e5, 0.0, 9.0e5];
            }
            if a % 11 == 0 {
                flags |= O_UNCERTAIN;
                cov = [f32::NAN, 0.0, 1.0];
            }
            if a % 3 == 0 && k == 20 {
                flags |= O_REACQUIRED;
            }
            if a % 13 == 0 {
                alt = f32::NAN;
            }
            if a % 4 != 0 {
                flags |= O_HAS_ALT;
            }
            // Aircraft a stops reporting after row 30 + a % 10, so some tracks end inside the
            // measured frames.
            if k > 30 + i64::from(a % 10) {
                continue;
            }
            c.t.push(T0 + k * S);
            c.addr.push(0xA00000 + a);
            c.lat.push(ORD.0 + row * 200_000 + (k as i32) * 300);
            c.lon.push(ORD.1 + col * 300_000 + (k as i32) * 900);
            c.v.push(110.0);
            c.cov.push(cov);
            c.alt.push(alt);
            c.flags.push(flags);
        }
    }
    c
}

fn callsign(s: &str) -> [u8; 8] {
    let mut c = [0u8; 8];
    for (d, b) in c.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    c
}

/// A target with every state on, for the standalone `build_traffic` window.
fn busy_target(n: i32) -> Target {
    let mut t = Target {
        lat_e7: ORD.0 + (n % 20 - 10) * 150_000,
        lon_e7: ORD.1 + (n / 20 - 5) * 250_000,
        alt_ft: if n % 3 == 0 { NO_ALT } else { 3_000 + 100 * n },
        addr: 0xA10000 + n as u32,
        r95_m: if n % 4 == 0 { f32::NAN } else { 1_500.0 },
        trail: [(0, 0); MAX_TRAIL],
        callsign: if n % 2 == 0 {
            callsign("UAL123")
        } else {
            [0; 8]
        },
        track_ddeg: (n as u16 * 37) % 3_600,
        gs_dkt: 2_400 + n as u16,
        reacq_age_s: if n % 5 == 0 { 2 } else { NO_REACQ },
        flags: 0,
        trail_len: 5,
    };
    for (k, slot) in t.trail.iter_mut().enumerate() {
        *slot = (
            t.lat_e7 - 1_000 * (k as i32 + 1),
            t.lon_e7 - 1_000 * (k as i32 + 1),
        );
    }
    if n % 2 == 0 {
        t.flags |= T_COAST;
    }
    if n % 4 == 0 || n % 7 == 0 {
        t.flags |= T_UNCERTAIN;
    }
    if n % 9 == 0 {
        t.flags |= T_CS_CONFLICT;
    }
    t
}

// A7.2: cold fixture report, one per executable run.
struct Evidence {
    controls: Counts,
    windows: Vec<Window>,
    convert: ConvertStats,
    traffic: TrafficStats,
    frame_traffic: TrafficStats,
    frame_refused_traffic: TrafficStats,
    pushed_refused: u32,
    pushed_malformed: u32,
    injected: bool,
}

fn measure(
    name: &'static str,
    inject: bool,
    frames: i64,
    mut work: impl FnMut(i64),
) -> Result<Window, &'static str> {
    if !allocator::start() {
        return Err("allocation counters were not clean before a window");
    }
    for k in 0..frames {
        work(k);
    }
    if inject && !inject_core_allocation() {
        return Err("injected allocation failed");
    }
    let counts = allocator::stop().ok_or("allocation measurement did not disarm")?;
    Ok(Window {
        name,
        counts,
        frames,
    })
}

/// Adds the counts the checks read, over the frames of a window (plain integer adds).
fn acc(a: &mut TrafficStats, b: &TrafficStats) {
    a.drawn += b.drawn;
    a.coasting += b.coasting;
    a.uncertain += b.uncertain;
    a.reacquired += b.reacquired;
    a.cs_conflict += b.cs_conflict;
    a.r95_unknown += b.r95_unknown;
    a.with_leader += b.with_leader;
    a.refused += b.refused;
}

fn run(inject: bool, controls: Counts) -> Result<Evidence, &'static str> {
    // Everything below up to the first `measure` is initialisation (C1): not counted.
    let meta = Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    };
    let cfg = ReplayConfig {
        centre_lat_e7: ORD.0,
        centre_lon_e7: ORD.1,
        range_nm: 60.0,
        width: 1920,
        height: 1080,
        max_targets: 256,
        style: DEFAULT_STYLE,
        convert: DEFAULT_CONVERT,
    };
    let mut replay = replay_new(empty_model(meta.clone()), &cfg).map_err(|_| "replay_new")?;
    let small_cfg = ReplayConfig {
        max_targets: 8,
        ..cfg
    };
    let mut small = replay_new(empty_model(meta), &small_cfg).map_err(|_| "replay_new small")?;

    let c = fixture_columns();
    let mut store = RowStore::default();
    let cols = RowColumns {
        t_ns: &c.t,
        addr: &c.addr,
        lat_e7: &c.lat,
        lon_e7: &c.lon,
        vel_east_mps: &c.v,
        vel_north_mps: &c.v,
        pos_cov_m2: &c.cov,
        alt_ft: &c.alt,
        flags: &c.flags,
    };
    store_add(&mut store, &cols, i64::MIN).map_err(|_| "store_add")?;
    if store_finish(&mut store) != 0 {
        return Err("store_finish");
    }
    // Callsigns: every third aircraft has one; aircraft 3 has two at one instant (a conflict).
    let (mut ct, mut ca, mut cc) = (Vec::new(), Vec::new(), Vec::new());
    for a in (0..AIRCRAFT).step_by(3) {
        ct.push(T0);
        ca.push(0xA00000 + a);
        cc.push(callsign("DAL42"));
    }
    ct.extend([T0 + 10 * S, T0 + 10 * S]);
    ca.extend([0xA00003, 0xA00003]);
    cc.extend([callsign("UAL1"), callsign("AAL2")]);
    let (calls, _) = callsigns_build(&ct, &ca, &cc).map_err(|_| "callsigns_build")?;

    // A standalone set and list for the build_traffic window.
    let mut set = targets_new(64).map_err(|_| "targets_new")?;
    let mut list = list_new(
        caps_add(
            &traffic_caps(64, &DEFAULT_STYLE).map_err(|_| "traffic_caps")?,
            &traffic_caps(0, &DEFAULT_STYLE).map_err(|_| "traffic_caps")?,
        )
        .map_err(|_| "caps_add")?,
    )
    .map_err(|_| "list_new")?;
    let proj = projection_new(ORD.0, ORD.1, 60.0, 1920, 1080, 0).map_err(|_| "projection_new")?;
    let ell = oa_geo::wgs84();

    let mut convert = ConvertStats::default();
    let mut windows = Vec::new();

    // Window 1: rows to targets, every frame from a cleared set.
    windows.push(measure("convert", inject, FRAMES, |k| {
        targets_clear(&mut set);
        let st = targets_at(
            &store,
            &calls,
            T0 + (FIRST_FRAME_S + k) * S,
            &DEFAULT_CONVERT,
            &proj,
            &mut set,
        );
        // Summed over the frames (plain integer adds, no allocation).
        convert.added += st.added;
        convert.ended += st.ended;
        convert.off_map += st.off_map;
    })?);

    // Window 2: a full set (some refused at capacity, one malformed) drawn into a cleared list.
    targets_clear(&mut set);
    let mut pushed_refused = 0u32;
    let mut pushed_malformed = 0u32;
    for n in 0..70 {
        let mut t = busy_target(n);
        if n == 5 {
            t.lat_e7 = 900_000_001;
        }
        match targets_push(&mut set, &t) {
            Ok(()) => {}
            Err(oa_display::traffic::TrafficError::Full { .. }) => pushed_refused += 1,
            Err(oa_display::traffic::TrafficError::Malformed { .. }) => pushed_malformed += 1,
        }
    }
    let mut traffic = TrafficStats::default();
    windows.push(measure("build_traffic", inject, FRAMES, |_| {
        list_clear(&mut list);
        if let Ok(st) = build_traffic(&mut list, &proj, &ell, &set, &DEFAULT_STYLE) {
            traffic = st;
        }
    })?);

    // Window 3: the whole frame chain, map and traffic and stamp, then the pixels.
    let mut frame_traffic = TrafficStats::default();
    let mut frame_ok = true;
    windows.push(measure("replay_frame", inject, FRAMES, |k| {
        let t = T0 + (FIRST_FRAME_S + k) * S;
        match replay_build(&mut replay, &store, &calls, t, "2026-11-15T20:00:25Z") {
            Ok(info) => acc(&mut frame_traffic, &info.traffic),
            Err(_) => frame_ok = false,
        }
        frame_ok &= replay_raster(&mut replay).is_ok();
    })?);
    if !frame_ok {
        return Err("a measured frame failed");
    }

    // Window 4: the same chain with a set too small for the traffic: the refusal notice.
    let mut frame_refused_traffic = TrafficStats::default();
    let mut refused_ok = true;
    windows.push(measure("replay_frame_refusing", inject, FRAMES, |k| {
        let t = T0 + (FIRST_FRAME_S + k) * S;
        match replay_build(&mut small, &store, &calls, t, "2026-11-15T20:00:25Z") {
            Ok(info) => acc(&mut frame_refused_traffic, &info.traffic),
            Err(_) => refused_ok = false,
        }
        refused_ok &= replay_raster(&mut small).is_ok();
    })?);
    if !refused_ok {
        return Err("a measured refusing frame failed");
    }

    Ok(Evidence {
        controls,
        windows,
        convert,
        traffic,
        frame_traffic,
        frame_refused_traffic,
        pushed_refused,
        pushed_malformed,
        injected: inject,
    })
}

/// What the exercised windows must have drawn before zero allocations can pass.
fn check_paths(e: &Evidence) -> Result<(), &'static str> {
    if e.convert.added < 50 || e.convert.ended == 0 || e.convert.off_map != 0 {
        return Err("the convert window did not exercise live and ended tracks");
    }
    let t = &e.traffic;
    if t.drawn < 30
        || t.coasting == 0
        || t.uncertain == 0
        || t.r95_unknown == 0
        || t.reacquired == 0
        || t.cs_conflict == 0
        || t.with_leader == 0
        || t.refused == 0
        || t.malformed == 0
    {
        return Err("the build_traffic window did not exercise every target state");
    }
    if e.pushed_refused == 0 || e.pushed_malformed != 1 {
        return Err("the set did not refuse as the fixture requires");
    }
    let f = &e.frame_traffic;
    if f.drawn < 50 || f.coasting == 0 || f.uncertain == 0 || f.reacquired == 0 {
        return Err("the replay frame did not draw the fixture's states");
    }
    if f.cs_conflict == 0 || f.r95_unknown == 0 || f.with_leader == 0 {
        return Err("the replay frame did not draw conflicts, unknown radii and leaders");
    }
    if e.frame_refused_traffic.refused == 0 {
        return Err("the refusing frame did not refuse");
    }
    Ok(())
}

fn print(e: &Evidence, status: &str) {
    let windows: Vec<String> = e
        .windows
        .iter()
        .map(|w| {
            format!(
                "{{\"name\":\"{}\",\"frames\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"overflow\":{}}}",
                w.name, w.frames, w.counts.alloc, w.counts.zeroed, w.counts.realloc, w.counts.overflow
            )
        })
        .collect();
    println!(
        "{{\"status\":\"{}\",\"injection_requested\":{},\"controls\":{{\"alloc\":{},\"zeroed\":{},\"realloc\":{}}},\"windows\":[{}],\"drawn\":{},\"uncertain\":{},\"cs_conflict\":{},\"r95_unknown\":{},\"refused\":{},\"malformed\":{},\"frame_drawn\":{}}}",
        status,
        e.injected,
        e.controls.alloc,
        e.controls.zeroed,
        e.controls.realloc,
        windows.join(","),
        e.traffic.drawn,
        e.traffic.uncertain,
        e.traffic.cs_conflict,
        e.traffic.r95_unknown,
        e.traffic.refused,
        e.traffic.malformed,
        e.frame_traffic.drawn
    );
}

fn main() {
    let (mut controls_only, mut inject) = (false, false);
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--controls-only" if !controls_only && !inject => controls_only = true,
            "--inject-allocation" if !inject && !controls_only => inject = true,
            _ => fail("usage: oa-display-alloc-check [--controls-only | --inject-allocation]"),
        }
    }
    let Some(controls) = run_controls() else {
        fail("positive control did not observe alloc, alloc_zeroed, and realloc");
    };
    if controls_only {
        println!(
            "{{\"status\":\"PASS_CONTROLS\",\"alloc\":{},\"zeroed\":{},\"realloc\":{}}}",
            controls.alloc, controls.zeroed, controls.realloc
        );
        return;
    }
    let evidence = match run(inject, controls) {
        Ok(e) => e,
        Err(m) => fail(m),
    };
    if let Err(m) = check_paths(&evidence) {
        fail(m);
    }
    let dirty = evidence.windows.iter().any(|w| {
        w.counts.alloc != 0 || w.counts.zeroed != 0 || w.counts.realloc != 0 || w.counts.overflow
    });
    if dirty {
        print(&evidence, "FAIL_DSP_ALLOCATION");
        std::process::exit(1);
    }
    print(&evidence, "PASS_DSP_EXERCISED");
}
