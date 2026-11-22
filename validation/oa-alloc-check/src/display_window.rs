//! Allocation windows for the display core (`oa-display`, `oa-scope-replay`). Shell (B3): a
//! bounded fixture and its evidence. C5: after every table is built, a full traffic frame
//! (rows to targets, the traffic drawing over a full set, and the whole frame chain: the map from
//! a non-empty airspace model, the traffic, the stamp, the pixels) must not call the global
//! allocator.
#![forbid(unsafe_code)]
use super::allocator::{self, Counts};
use oa_airspace::model::{
    bit_set, bit_zeros, boundary_source, edge_interp, empty_model, level, point_kind, proc_kind,
    stratum, time_code, AirwayLegIx, Meta, Model, PointIx, PosE7, RouteIx, SegIx, Span, StrIx,
    StrTable, VertexIx, ALT_NONE, ELEV_NONE,
};
use oa_display::list::{list_clear, list_new, DisplayList};
use oa_display::palette::{ALERT, PALETTE};
use oa_display::project::projection_new;
use oa_display::raster::frame_digest;
use oa_display::safety::{FeedState, FeedStatus};
use oa_display::scene::{build_scene, scene_caps, scene_index, scratch_for, view_new};
use oa_display::traffic::{
    build_traffic, caps_add, targets_clear, targets_columns, targets_new, targets_push,
    traffic_caps, Target, TrafficError, TrafficStats, DEFAULT_STYLE, MAX_TRAIL, NO_ALT, NO_REACQ,
    NO_U16, T_COAST, T_CS_CONFLICT, T_UNCERTAIN,
};
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::state_table_new;
use oa_scope_replay::adapt::row_flags;
use oa_scope_replay::frame::{
    replay_build, replay_build_raw, replay_build_with_safety, replay_new, replay_raster,
    ReplayConfig, ReplaySafety,
};
use oa_scope_replay::raw::RawObservationColumns;
use oa_scope_replay::rows::{
    targets_at, ConvertConfig, ConvertStats, RowColumns, RowStore, DEFAULT_CONVERT, RF_HAS_ALT,
};
use oa_scope_replay::store::{callsigns_build, store_add, store_finish};
use oa_track::{r95_m_of, O_COAST, O_HAS_ALT, O_REACQUIRED, O_TICK, O_UNCERTAIN};

const S: i64 = 1_000_000_000;
const T0: i64 = 1_794_772_800 * S;
const ORD: (i32, i32) = (419_786_000, -879_048_000);
const AIRCRAFT: u32 = 200;
const SECONDS: i64 = 40;
const FRAMES: i64 = 20;
const FIRST_FRAME_S: i64 = 25;
/// A valid position the projection cannot place (opposite the centre), see the display tests.
const UNPLACED: (i32, i32) = (-422_586_000, 918_952_000);
/// A position outside the globe.
const OFF_GLOBE: i32 = 900_000_001;
/// A stamp longer than the frame's stamp buffer: clipped, counted.
const LONG_STAMP: &str = "2026-11-15T20:00:25Z 2026-11-15T20:00:25Z 2026-11-15T20:00:25Z 2026-11-15T20:00:25Z 2026-11-15T20:00:25Z 2026-11-15T20:00:25Z";

/// A7.2: cold evidence for the display windows, summed over the frames of every window.
pub(super) struct DisplayEvidence {
    pub counts: Counts,
    pub convert_added: u64,
    pub convert_ended: u64,
    pub traffic_drawn: u64,
    pub uncertain: u64,
    pub cs_conflict: u64,
    pub r95_unknown: u64,
    pub refused: u64,
    pub alt_clamped: u64,
    pub malformed: u64,
    pub frame_drawn: u64,
    pub scene_items: u64,
    pub refusing_refused: u64,
    pub convert_invalid_position: u64,
    pub convert_unprojectable: u64,
    pub convert_trail_dropped: u64,
    pub convert_time_overflow: u64,
    pub draw_trail_dropped: u64,
    pub stamp_clipped: u64,
    pub digests: u64,
    pub raw_drawn: u64,
    pub raw_capacity_refused: u64,
    pub raw_invalid_refused: u64,
    pub injected: bool,
}

/// A boundary ring: source, stratum, facility and vertices.
type Ring = (u8, u8, &'static str, Vec<(i32, i32)>);

fn e7(lat: f64, lon: f64) -> (i32, i32) {
    ((lat * 1e7).round() as i32, (lon * 1e7).round() as i32)
}

struct Strings(StrTable);

impl Strings {
    fn add(&mut self, s: &str) -> StrIx {
        self.0.text.push_str(s);
        self.0.offsets.push(self.0.text.len() as u32);
        StrIx::new((self.0.offsets.len().saturating_sub(2)) as u32)
    }
}

fn meta() -> Meta {
    Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    }
}

/// A small valid model near Chicago: six points (airports, fixes, a VOR, an NDB), an own HIGH
/// ring, a neighbour's ring, a closed own LOW ring, two airways. The map draws them with labels.
fn model() -> Model {
    let mut m = empty_model(meta());
    let mut s = Strings(StrTable::default());
    let pts = [
        ("1C5", point_kind::AIRPORT, e7(41.85, -87.75)),
        ("AAAAA", point_kind::FIX, e7(42.30, -88.30)),
        ("BBBBB", point_kind::FIX, e7(41.60, -87.40)),
        ("DPA", point_kind::VOR, e7(41.90, -88.26)),
        ("KORD", point_kind::AIRPORT, e7(41.9786, -87.9048)),
        ("ZNB", point_kind::NDB, e7(42.10, -87.70)),
    ];
    let region = s.add("K5");
    let empty = s.add("");
    for (ident, kind, (lat, lon)) in pts {
        let ix = s.add(ident);
        let p = &mut m.points;
        p.kind.push(kind);
        p.lat.push(lat);
        p.lon.push(lon);
        p.elev_ft.push(ELEV_NONE);
        p.ident.push(ix);
        p.alt_ident.push(StrIx::NONE);
        p.region.push(region);
        p.scope.push(empty);
        p.name.push(StrIx::NONE);
    }
    let n = pts.len();
    m.points.src_nasr = bit_zeros(n);
    m.points.src_cifp = bit_zeros(n);
    m.points.in_area = bit_zeros(n);
    m.points.cls_b = bit_zeros(n);
    m.points.cls_c = bit_zeros(n);
    m.points.cls_d = bit_zeros(n);
    m.points.cls_e = bit_zeros(n);
    // KORD has a coded approach.
    m.procs.kind.push(proc_kind::APPROACH);
    m.procs.airport.push(PointIx::new(4));
    m.procs.ident.push(s.add("I28R"));
    m.procs.name.push(StrIx::NONE);
    m.procs.routes.push(Span {
        start: RouteIx::new(0),
        len: 0,
    });
    m.procs.src_nasr = bit_zeros(1);
    m.procs.src_cifp = bit_zeros(1);
    m.procs.coded = bit_zeros(1);
    bit_set(&mut m.procs.coded, 0, true);

    let square = |lat: f64, lon: f64, d: f64| {
        vec![
            e7(lat + d, lon - d),
            e7(lat + d, lon + d),
            e7(lat - d, lon + d),
            e7(lat - d, lon - d),
        ]
    };
    let mut low = square(41.9, -87.9, 0.5);
    if let Some(first) = low.first().copied() {
        low.push(first);
    }
    let rings: Vec<Ring> = vec![
        (
            boundary_source::NASR_ARB,
            stratum::ARTCC_HIGH,
            "ZAU",
            square(41.9, -87.9, 0.8),
        ),
        (
            boundary_source::NASR_ARB,
            stratum::ARTCC_HIGH,
            "ZMP",
            square(43.2, -87.9, 0.4),
        ),
        (boundary_source::NASR_ARB, stratum::ARTCC_LOW, "ZAU", low),
    ];
    let b = &mut m.boundaries;
    let mut next = 0u32;
    for (source, strat, fac, ring) in &rings {
        b.source.push(*source);
        b.stratum.push(*strat);
        b.edge_interp.push(edge_interp::PER_SEGMENT);
        b.multiple.push(0);
        b.facility.push(s.add(fac));
        b.name.push(StrIx::NONE);
        b.floor_ft.push(ALT_NONE);
        b.ceiling_ft.push(ALT_NONE);
        b.time_code.push(time_code::UNKNOWN);
        b.level.push(level::UNKNOWN);
        b.vertices.push(Span {
            start: VertexIx::new(next),
            len: ring.len() as u32,
        });
        b.segs.push(Span {
            start: SegIx::new(0),
            len: 0,
        });
        next += ring.len() as u32;
        for (lat, lon) in ring {
            m.vertices.push(PosE7 {
                lat: *lat,
                lon: *lon,
            });
        }
    }
    b.floor_agl = bit_zeros(rings.len());
    b.ceiling_agl = bit_zeros(rings.len());
    b.incomplete = bit_zeros(rings.len());

    let p = PointIx::new;
    let legs = [(p(1), p(3)), (p(3), p(2))];
    m.airways.designator.push(s.add("J80"));
    m.airways.designator.push(s.add("V9"));
    m.airways.location.push(b'J');
    m.airways.location.push(b'V');
    m.airways.legs.push(Span {
        start: AirwayLegIx::new(0),
        len: 1,
    });
    m.airways.legs.push(Span {
        start: AirwayLegIx::new(1),
        len: 1,
    });
    for (f, t) in &legs {
        let l = &mut m.airway_legs;
        l.from.push(*f);
        l.to.push(*t);
        l.mea_ft.push(ALT_NONE);
        l.mea_opp_ft.push(ALT_NONE);
        l.gps_mea_ft.push(ALT_NONE);
        l.mea_dir.push(0);
        l.gps_mea_dir.push(0);
    }
    m.airway_legs.gap = bit_zeros(legs.len());
    m.airway_legs.signal_gap = bit_zeros(legs.len());
    m.airway_legs.dogleg = bit_zeros(legs.len());
    m.strings = s.0;
    m
}

/// Owned columns for the fixture: `AIRCRAFT` aircraft, one row a second for up to `SECONDS`, in
/// every state the display shows. The radius and flags are reduced as the shell reduces them.
// A7.2: cold fixture data, built once before measurement.
#[derive(Default)]
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

fn fixture_columns() -> Cols {
    let mut c = Cols::default();
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
            c.r95.push(r95_m_of(&cov));
            c.alt.push(alt);
            c.flags.push(row_flags(flags));
        }
    }
    // Rows the conversion must classify, one address each: a position off the globe, a valid
    // position that does not project, and an aircraft whose older rows (its history) are off the
    // globe while its newest are good.
    for k in 0..SECONDS {
        let good = (ORD.0 + 100_000, ORD.1 + 100_000 + (k as i32) * 900);
        let rows = [
            (0xB00001u32, (OFF_GLOBE, ORD.1)),
            (0xB00002, UNPLACED),
            (0xB00003, if k < 24 { (OFF_GLOBE, ORD.1) } else { good }),
        ];
        for (addr, (lat, lon)) in rows {
            c.t.push(T0 + k * S);
            c.addr.push(addr);
            c.lat.push(lat);
            c.lon.push(lon);
            c.v.push(110.0);
            c.r95.push(100.0);
            c.alt.push(11_000.0);
            c.flags.push(RF_HAS_ALT);
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
///
/// Not reached by any fixture, and why: a leader or a circle that fails (`leader_failed`,
/// `circle_failed`) needs the geodesic to fail on a finite distance, which it does not, or its
/// far end to fall in the few kilometres around the projection's antipode, which no leader
/// (at most 6 000 km from an on-screen target) and no radius in a scan of forty wraps of the
/// globe reached; `text_clipped` needs a buffer smaller than the longest block, which the
/// display's own test shows is never clipped. Their counters stay, and are summed and reported.
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
        let d = 1_000 * (k as i32 + 1);
        *slot = (t.lat_e7 - d, t.lon_e7 - d);
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
    if n % 17 == 0 {
        // Beyond what the data block shows: marked and counted.
        t.alt_ft = 250_000;
    }
    if n == 11 {
        // A history dot the projection cannot place: that dot is not drawn, counted.
        if let Some(slot) = t.trail.get_mut(2) {
            *slot = UNPLACED;
        }
    }
    t
}

/// Adds the counts the checks read, over the frames of a window (plain integer adds).
fn acc(a: &mut TrafficStats, b: &TrafficStats) {
    a.drawn += b.drawn;
    a.uncertain += b.uncertain;
    a.cs_conflict += b.cs_conflict;
    a.r95_unknown += b.r95_unknown;
    a.refused += b.refused;
    a.alt_display_clamped += b.alt_display_clamped;
    a.trail_dropped += b.trail_dropped;
}

fn add_counts(a: &mut Counts, b: Counts) {
    a.alloc = a.alloc.saturating_add(b.alloc);
    a.zeroed = a.zeroed.saturating_add(b.zeroed);
    a.realloc = a.realloc.saturating_add(b.realloc);
    a.overflow |= b.overflow;
}

fn list_has_text(list: &DisplayList, wanted: &str) -> bool {
    list.texts
        .start
        .iter()
        .zip(&list.texts.len)
        .any(|(&start, &len)| {
            let (Ok(start), Ok(len)) = (usize::try_from(start), usize::try_from(len)) else {
                return false;
            };
            list.text
                .get(start..start.saturating_add(len))
                .is_some_and(|text| text == wanted)
        })
}

/// Runs `work` for `frames` frames inside one armed window; with `inject`, one allocation is forced
/// inside it (the negative control), and a failure to force it is an error.
fn measure(inject: bool, frames: i64, mut work: impl FnMut(i64)) -> Result<Counts, &'static str> {
    if !allocator::start() {
        return Err("allocation counters were not clean before a display window");
    }
    for k in 0..frames {
        work(k);
    }
    if inject && !allocator::inject_core_allocation() {
        return Err("injected allocation failed");
    }
    allocator::stop().ok_or("display window did not disarm cleanly")
}

pub(super) fn display_window(inject: bool) -> Result<DisplayEvidence, &'static str> {
    // Everything up to the first `measure` is initialisation (C1): not counted.
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
    let mut replay = replay_new(model(), &cfg).map_err(|_| "replay_new")?;
    let small_cfg = ReplayConfig {
        max_targets: 8,
        ..cfg
    };
    let mut small = replay_new(model(), &small_cfg).map_err(|_| "replay_new small")?;

    let c = fixture_columns();
    let mut store = RowStore::default();
    let cols = RowColumns {
        t_ns: &c.t,
        addr: &c.addr,
        lat_e7: &c.lat,
        lon_e7: &c.lon,
        vel_east_mps: &c.v,
        vel_north_mps: &c.v,
        r95_m: &c.r95,
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

    let mut counts = Counts {
        alloc: 0,
        zeroed: 0,
        realloc: 0,
        overflow: false,
    };
    let mut convert = ConvertStats::default();

    // Window 1: rows to targets, every frame from a cleared set; a second conversion with a trail
    // step so large that the history times overflow.
    let wide = ConvertConfig {
        trail_step_ns: i64::MAX / 2,
        ..DEFAULT_CONVERT
    };
    let c = measure(inject, FRAMES, |k| {
        let t = T0 + (FIRST_FRAME_S + k) * S;
        targets_clear(&mut set);
        let st = targets_at(&store, &calls, t, &DEFAULT_CONVERT, &proj, &mut set);
        convert.added += st.added;
        convert.ended += st.ended;
        convert.off_map += st.off_map;
        convert.invalid_position += st.invalid_position;
        convert.unprojectable += st.unprojectable;
        convert.trail_dropped += st.trail_dropped;
        targets_clear(&mut set);
        let st = targets_at(&store, &calls, t, &wide, &proj, &mut set);
        convert.time_overflow += st.time_overflow;
    })?;
    add_counts(&mut counts, c);

    // Window 2: every frame fills a cleared set (some targets refused at capacity, one
    // malformed), then draws it into a cleared list.
    let mut malformed = 0u64;
    let mut refused_full = 0u64;
    let mut traffic = TrafficStats::default();
    let mut drawn_all = TrafficStats::default();
    let c = measure(inject, FRAMES, |_| {
        targets_clear(&mut set);
        for n in 0..70 {
            let mut t = busy_target(n);
            if n == 5 {
                t.lat_e7 = OFF_GLOBE;
            }
            match targets_push(&mut set, &t) {
                Ok(()) => {}
                Err(TrafficError::Malformed { .. }) => malformed += 1,
                Err(TrafficError::Full { .. }) => refused_full += 1,
            }
        }
        list_clear(&mut list);
        if let Ok(st) = build_traffic(&mut list, &proj, &ell, &set, &DEFAULT_STYLE) {
            traffic = st;
            acc(&mut drawn_all, &st);
        }
    })?;
    add_counts(&mut counts, c);

    // Window 3: the whole frame chain with a degraded safety overlay, then the pixels.
    let states = state_table_new(10 * S, 0).map_err(|_| "safety state table")?;
    let inhibits = inhibits_new();
    let feeds = [FeedStatus {
        name: *b"ADSB\0\0\0\0",
        state: FeedState::Lost { age_ms: 25_000 },
    }];
    let safety = ReplaySafety {
        states: &states,
        inhibits: &inhibits,
        feeds: &feeds,
        newest_track_ns: Some(T0),
        frozen_limit_ns: 5 * S,
    };
    let mut frame = TrafficStats::default();
    let mut scene_items = 0u64;
    let mut frame_ok = true;
    let (mut stamp_clipped, mut digests) = (0u64, 0u64);
    let c = measure(inject, FRAMES, |k| {
        let t = T0 + (FIRST_FRAME_S + k) * S;
        let stamp = if k % 4 == 3 {
            LONG_STAMP
        } else {
            "2026-11-15T20:00:25Z"
        };
        match replay_build_with_safety(&mut replay, &store, &calls, t, stamp, &safety) {
            Ok(info) => {
                stamp_clipped += u64::from(info.stamp_clipped);
                acc(&mut frame, &info.traffic);
                let sc = &info.scene;
                scene_items += u64::from(
                    sc.own_boundaries + sc.airway_legs + sc.airports + sc.fixes + sc.labels,
                );
            }
            Err(_) => frame_ok = false,
        }
        frame_ok &= replay_raster(&mut replay).is_ok();
        match frame_digest(&replay.spec, &replay.buf) {
            Ok(_) => digests += 1,
            Err(_) => frame_ok = false,
        }
    })?;
    add_counts(&mut counts, c);
    if !frame_ok {
        return Err("a measured display frame failed");
    }

    // Window 4: the same chain with a set too small for the traffic: the refusal notice.
    let mut refusing = TrafficStats::default();
    let mut refusing_ok = true;
    let c = measure(inject, FRAMES, |k| {
        let t = T0 + (FIRST_FRAME_S + k) * S;
        match replay_build(&mut small, &store, &calls, t, "2026-11-15T20:00:25Z") {
            Ok(info) => acc(&mut refusing, &info.traffic),
            Err(_) => refusing_ok = false,
        }
        refusing_ok &= replay_raster(&mut small).is_ok();
    })?;
    add_counts(&mut counts, c);
    if !refusing_ok {
        return Err("a measured refusing display frame failed");
    }

    // Window 5: independent raw display, including a drawn target, target-set capacity refusal,
    // and invalid-input fail-closed raster refusal. Fixtures are prepared before arming; returned
    // counters and rendered bytes witness each route, not merely the called method.
    let raw_as_of = T0 + FIRST_FRAME_S * S;
    let raw_address = 0xA2_0000;
    let raw_times = [raw_as_of];
    let raw_addresses = [raw_address];
    let raw_latitudes = [ORD.0];
    let raw_longitudes = [ORD.1];
    let raw_altitudes = [24_000];
    let raw_speeds = [NO_U16];
    let raw_tracks = [NO_U16];
    let raw_one = RawObservationColumns {
        t_ns: &raw_times,
        addr: &raw_addresses,
        lat_e7: &raw_latitudes,
        lon_e7: &raw_longitudes,
        alt_ft: &raw_altitudes,
        gs_dkt: &raw_speeds,
        track_ddeg: &raw_tracks,
    };
    let raw_many_times = [raw_as_of; 10];
    let raw_many_addresses = [
        0xA3_0000, 0xA3_0001, 0xA3_0002, 0xA3_0003, 0xA3_0004, 0xA3_0005, 0xA3_0006, 0xA3_0007,
        0xA3_0008, 0xA3_0009,
    ];
    let raw_many_latitudes = [ORD.0; 10];
    let raw_many_longitudes = [ORD.1; 10];
    let raw_many_altitudes = [24_000; 10];
    let raw_many_speeds = [NO_U16; 10];
    let raw_many_tracks = [NO_U16; 10];
    let raw_many = RawObservationColumns {
        t_ns: &raw_many_times,
        addr: &raw_many_addresses,
        lat_e7: &raw_many_latitudes,
        lon_e7: &raw_many_longitudes,
        alt_ft: &raw_many_altitudes,
        gs_dkt: &raw_many_speeds,
        track_ddeg: &raw_many_tracks,
    };
    let invalid_times = [raw_as_of; 2];
    let invalid_addresses = [0xA4_0001, 0xA4_0000];
    let invalid_latitudes = [ORD.0; 2];
    let invalid_longitudes = [ORD.1; 2];
    let invalid_altitudes = [24_000; 2];
    let invalid_speeds = [NO_U16; 2];
    let invalid_tracks = [NO_U16; 2];
    let invalid_raw = RawObservationColumns {
        t_ns: &invalid_times,
        addr: &invalid_addresses,
        lat_e7: &invalid_latitudes,
        lon_e7: &invalid_longitudes,
        alt_ft: &invalid_altitudes,
        gs_dkt: &invalid_speeds,
        track_ddeg: &invalid_tracks,
    };
    let mut raw_ok = true;
    let (mut raw_drawn, mut raw_capacity_refused, mut raw_invalid_refused) = (0u64, 0u64, 0u64);
    let raw_alert = PALETTE
        .get(usize::from(ALERT))
        .ok_or("raw alert color unavailable")?
        .to_le_bytes();
    let c = measure(inject, FRAMES, |_| {
        match replay_build_raw(&mut replay, &raw_one, raw_as_of, "RAW ALLOC") {
            Ok((info, evidence)) => {
                let target =
                    targets_columns(&replay.set).addr.first().copied() == Some(raw_address);
                let banner = list_has_text(&replay.list, "RAW SURVEILLANCE ONLY");
                let raster = replay_raster(&mut replay).is_ok();
                let pixels = replay.buf.chunks_exact(4).any(|pixel| pixel == raw_alert);
                let digest = frame_digest(&replay.spec, &replay.buf).is_ok();
                if target
                    && banner
                    && pixels
                    && raster
                    && digest
                    && evidence.targets_admitted == 1
                    && info.traffic.drawn == 1
                {
                    raw_drawn += 1;
                } else {
                    raw_ok = false;
                }
            }
            Err(_) => raw_ok = false,
        }
        match replay_build_raw(&mut small, &raw_many, raw_as_of, "RAW CAP") {
            Ok((info, evidence)) => {
                let raster = replay_raster(&mut small).is_ok();
                let digest = frame_digest(&small.spec, &small.buf).is_ok();
                if raster
                    && digest
                    && evidence.targets_admitted == 8
                    && evidence.refused_capacity == 2
                    && info.refused == 2
                    && info.traffic.drawn == 8
                {
                    raw_capacity_refused += 2;
                } else {
                    raw_ok = false;
                }
            }
            Err(_) => raw_ok = false,
        }
        let refused =
            replay_build_raw(&mut replay, &invalid_raw, raw_as_of, "RAW INVALID").is_err();
        let raster_refused = replay_raster(&mut replay).is_err();
        if refused && raster_refused {
            raw_invalid_refused += 1;
        } else {
            raw_ok = false;
        }
    })?;
    add_counts(&mut counts, c);
    if !raw_ok {
        return Err("a measured raw surveillance display branch failed its witness");
    }

    // Window 6: a scene built into a cloned label scratch. A clone of a sized scratch keeps every
    // buffer's length, so the frame writes by index and allocates nothing.
    let map = model();
    let view = view_new(ORD.0, ORD.1, 60.0, 1920, 1080).map_err(|_| "view_new")?;
    let index = scene_index(&map);
    let mut cloned = scratch_for(&map, &view).map_err(|_| "scratch_for")?.clone();
    let mut scene_list =
        list_new(scene_caps(&map).map_err(|_| "scene_caps")?).map_err(|_| "list_new")?;
    let mut cloned_ok = true;
    let c = measure(inject, FRAMES, |_| {
        cloned_ok &= build_scene(&mut scene_list, &map, &index, &view, &mut cloned).is_ok();
    })?;
    add_counts(&mut counts, c);
    if !cloned_ok {
        return Err("a measured cloned-scratch scene failed");
    }

    Ok(DisplayEvidence {
        counts,
        convert_added: u64::from(convert.added),
        convert_ended: u64::from(convert.ended),
        traffic_drawn: u64::from(traffic.drawn),
        uncertain: u64::from(traffic.uncertain) + u64::from(frame.uncertain),
        cs_conflict: u64::from(traffic.cs_conflict) + u64::from(frame.cs_conflict),
        r95_unknown: u64::from(traffic.r95_unknown) + u64::from(frame.r95_unknown),
        refused: refused_full,
        alt_clamped: u64::from(traffic.alt_display_clamped),
        malformed,
        frame_drawn: u64::from(frame.drawn),
        scene_items,
        refusing_refused: u64::from(refusing.refused),
        convert_invalid_position: u64::from(convert.invalid_position),
        convert_unprojectable: u64::from(convert.unprojectable),
        convert_trail_dropped: u64::from(convert.trail_dropped),
        convert_time_overflow: u64::from(convert.time_overflow),
        draw_trail_dropped: u64::from(drawn_all.trail_dropped),
        stamp_clipped,
        digests,
        raw_drawn,
        raw_capacity_refused,
        raw_invalid_refused,
        injected: true,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    // REQ: OA-DSP-052
    #[test]
    fn the_fixture_draws_the_map_and_every_target_state_it_claims() {
        // The zero-allocation claim is the harness run (libtest allocates in the background);
        // this checks that the fixture exercises what the run reports.
        let m = model();
        assert!(m.points.lat.len() >= 6 && !m.vertices.is_empty());
        let c = fixture_columns();
        assert_eq!(c.t.len(), c.r95.len());
        assert!(c.r95.iter().any(|r| r.is_nan()) && c.r95.iter().any(|r| *r > 1.0e3));
        let t = busy_target(17);
        assert_eq!(
            t.alt_ft, 250_000,
            "the fixture has an altitude beyond the data block"
        );
    }
}
