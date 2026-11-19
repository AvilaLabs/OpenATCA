//! The ADS-B persistent-error screen on synthetic aircraft with a known injected latency and
//! offset. Everything is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_sha256::{sha256, to_hex};
use oa_shadow::model::{EramFlight, EramSample};
use oa_shadow::screen::{
    bias_threshold, bootstrap, build_samples, is_bias, is_early, is_lag, most_supporting,
    render_screen, screen_aircraft, screen_fleet, RawReport, Reference, SampleRow, SplitMix, Tally,
    TruthPoint, ALT_UNKNOWN, CAVEAT, LAG_LOWER_S, MAX_OFFSET_M,
};

/// No fleet offsets: the intercept and the cross reference are zero.
const R0: Reference = Reference { c_m: 0.0, b_m: 0.0 };

/// A northbound aircraft with speed `v0 + amp sin(t / 300)` m/s: its north position, metres.
fn north_m(v0: f64, amp: f64, t_s: f64) -> f64 {
    v0 * t_s - amp * 300.0 * ((t_s / 300.0).cos() - 1.0)
}

fn speed_mps(v0: f64, amp: f64, t_s: f64) -> f64 {
    v0 + amp * (t_s / 300.0).sin()
}

/// The position `east`/`north` metres from the start, the east offset taken at the local latitude
/// (the aircraft flies hundreds of kilometres, so a fixed longitude scale would be wrong).
fn at(east: f64, north: f64) -> (i32, i32) {
    let m_per_e7 = 6_371_008.8 * std::f64::consts::PI / 180.0 * 1e-7;
    let lat = f64::from(LAT) + north / m_per_e7;
    let cos = (lat * 1e-7).to_radians().cos();
    let lon = f64::from(LON) + east / (m_per_e7 * cos);
    (lat.round() as i32, lon.round() as i32)
}

/// One synthetic aircraft: its ADS-B is `latency_s` late, `along_m` ahead along the track and
/// `bias_east_m` to the east of what ERAM sees (which is the truth with a uniform error of up to
/// 60 m, a report every 12 s for an hour). The smoother's answer is the ADS-B state.
struct Plane {
    flight: EramFlight,
    raw: Vec<RawReport>,
    v0: f64,
    amp: f64,
    latency_s: f64,
    along_m: f64,
    bias_east_m: f64,
}

impl Plane {
    fn new(v0: f64, amp: f64, latency_s: f64, along_m: f64, bias_east_m: f64, seed: u64) -> Plane {
        let mut rng = SplitMix(seed);
        let mut u = || (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 120.0 - 60.0;
        let samples = (0..300)
            .map(|k| {
                let t = 100 + 12 * k;
                let (e, n) = (u(), u());
                let (lat, lon) = at(e, north_m(v0, amp, t as f64) + n);
                EramSample {
                    t_ms: T0 + t * 1000,
                    lat_e7: lat,
                    lon_e7: lon,
                    alt_ft: Some(10_000),
                    coast: false,
                }
            })
            .collect();
        let raw = (0..=3700)
            .map(|s| RawReport {
                t_ms: T0 + s * 1000,
                lat_e7: LAT,
                lon_e7: LON,
                vel: None,
                nacp: 8,
            })
            .collect();
        Plane {
            flight: flight(Some(0xABCDEF), samples),
            raw,
            v0,
            amp,
            latency_s,
            along_m,
            bias_east_m,
        }
    }

    fn truth(&self, t_ms: i64) -> Option<TruthPoint> {
        let t = (t_ms - T0) as f64 / 1000.0 - self.latency_s;
        let (lat, lon) = at(
            self.bias_east_m,
            north_m(self.v0, self.amp, t) + self.along_m,
        );
        Some(TruthPoint {
            lat_deg: f64::from(lat) * 1e-7,
            lon_deg: f64::from(lon) * 1e-7,
            vel_east_mps: 0.0,
            vel_north_mps: speed_mps(self.v0, self.amp, t),
            sigma95_m: 5.0,
            vert_fpm: Some(0.0),
            turn_dps: Some(0.0),
        })
    }

    fn rows(&self) -> (Vec<SampleRow>, Tally) {
        build_samples(&self.raw, &[&self.flight], &|t| self.truth(t))
    }
}

fn rows_of(latency_s: f64, bias_east_m: f64, seed: u64) -> Vec<SampleRow> {
    Plane::new(200.0, 40.0, latency_s, 0.0, bias_east_m, seed)
        .rows()
        .0
}

// REQ: OA-SHD-030
#[test]
fn an_injected_latency_is_recovered_within_its_interval_when_the_fleet_intercept_is_known() {
    let rows = rows_of(2.0, 0.0, 7);
    assert_eq!(rows.len(), 300);
    // Stage (b) alone: the common intercept is zero here.
    let s = screen_aircraft(0xABCDEF, &rows, R0).unwrap();
    assert!(s.l_ci.0 < 2.0 && 2.0 < s.l_ci.1, "{:?}", s.l_ci);
    assert!((s.l_s - 2.0).abs() < 0.1, "L {}", s.l_s);
    assert!(s.b_m.abs() < 20.0, "b {}", s.b_m);
    // The speed spans 80 m/s: the two-parameter cross-check is there and agrees.
    let two = s.two.unwrap();
    assert!((two.l_s - 2.0).abs() < 0.6 && two.l_ci.0 < 2.0 && 2.0 < two.l_ci.1);
}

// REQ: OA-SHD-030
#[test]
fn an_injected_cross_track_offset_is_recovered_within_its_interval() {
    let rows = rows_of(0.0, 150.0, 11);
    let s = screen_aircraft(0xABCDEF, &rows, R0).unwrap();
    // East of a northbound aircraft is to the right: positive.
    assert!(s.b_m > 0.0);
    assert!(
        s.absb_ci.0 < 150.0 && 150.0 < s.absb_ci.1,
        "{:?}",
        s.absb_ci
    );
    assert!((s.b_m - 150.0).abs() < 15.0 && (s.b_rel_m - s.b_m).abs() < 1e-12);
    assert!(s.l_s.abs() < 0.1, "L {}", s.l_s);
    // NACp 8 bounds the position at 92.6 m: 150 m is under 3 bounds (277.8 m).
    assert_eq!(s.nacp_median, 8);
    assert!((s.bound_m - 92.6).abs() < 1e-9);
}

/// 40 aircraft with a common 0.4 s latency and a common -15 m along offset (ERAM's own
/// convention), speeds from 100 to 250 m/s; two with 1.0 s and 2.0 s more, one of them at a
/// constant speed, and three with a speed range over 50 kt.
fn fleet() -> Vec<(u32, Vec<SampleRow>)> {
    let mut v = Vec::new();
    for k in 0..40u32 {
        let v0 = 100.0 + 150.0 * f64::from(k) / 39.0;
        let (amp, extra) = match k {
            5 => (0.0, 1.0),
            20 => (30.0, 2.0),
            k if k % 13 == 0 => (30.0, 0.0),
            _ => (6.0, 0.0),
        };
        let p = Plane::new(v0, amp, 0.4 + extra, -15.0, 0.0, 1000 + u64::from(k));
        v.push((0x300 + k, p.rows().0));
    }
    v
}

// REQ: OA-SHD-031
#[test]
fn the_fleet_fit_recovers_the_common_offset_and_latency_and_stage_b_the_two_injected_lags() {
    let f = screen_fleet(&fleet(), true).unwrap();
    let ft = &f.fit;
    assert_eq!((ft.aircraft, ft.samples), (40, 12_000));
    // The two lagged aircraft pull the least squares a little (3 s over 40 aircraft).
    assert!((ft.l_s - 0.4).abs() < 0.15, "L {}", ft.l_s);
    assert!((ft.l_ts_s - 0.4).abs() < 0.1, "L_ts {}", ft.l_ts_s);
    assert!(ft.l_ci.0 < ft.l_s && ft.l_s < ft.l_ci.1);
    // c and L are collinear across a fleet whose speeds span 100 to 250 m/s: an error of 0.1 s in L
    // is 17 m in c, so c is only loosely determined and stage (b) inherits that.
    assert!((ft.c_m + 15.0).abs() < 40.0, "c {}", ft.c_m);
    assert!((ft.c_ts_m + 15.0).abs() < 40.0, "c_ts {}", ft.c_ts_m);
    assert!(ft.c_ci.0 < ft.c_m && ft.c_m < ft.c_ci.1);
    let (lo, hi) = ft.l_ts_ci.unwrap();
    assert!(
        lo < ft.l_ts_s && ft.l_ts_s < hi && lo < 0.4 && 0.4 < hi,
        "{lo} {hi}"
    );
    assert!(ft.c_ts_ci.is_some());
    // The least-squares reference gives the same aircraft in the same order.
    assert_eq!(
        f.aircraft.iter().map(|s| s.stat.addr).collect::<Vec<_>>(),
        f.aircraft_ols_ref
            .iter()
            .map(|s| s.stat.addr)
            .collect::<Vec<_>>()
    );
    assert!(screen_fleet(&fleet(), false).unwrap().fit.l_ts_ci.is_none());
    let a = &f.aircraft;
    assert_eq!(a.len(), 40);
    assert!(a.windows(2).all(|w| w[0].stat.addr < w[1].stat.addr));
    for s in a {
        assert!((s.excess_s - (s.stat.l_s - ft.l_ts_s)).abs() < 1e-12);
    }
    // k = 5 flies at a constant speed: only stage (b) identifies it, and its two-parameter fit
    // does not exist. k = 20 (speed range 60 m/s) has both.
    let (c5, c20) = (&a[5], &a[20]);
    assert!(c5.stat.two.is_none());
    assert!(c20.stat.two.is_some());
    assert!(
        (c5.stat.l_s - ft.l_ts_s - 1.0).abs() < 0.25,
        "{}",
        c5.excess_s
    );
    assert!(
        (c20.stat.l_s - ft.l_ts_s - 2.0).abs() < 0.25,
        "{}",
        c20.excess_s
    );
    assert!(c5.lag && c20.lag);
    let l2 = c20.stat.two.as_ref().unwrap();
    assert!(l2.l_ci.0 < 2.4 && 2.4 < l2.l_ci.1, "{l2:?}");
    // Nobody else is a LAG or a BIAS.
    assert_eq!(a.iter().filter(|s| s.lag).count(), 2);
    assert!(a.iter().all(|s| !s.bias));
}

// REQ: OA-SHD-031
#[test]
fn a_constant_speed_aircraft_has_a_latency_only_through_the_fleet_intercept() {
    // At a constant speed an offset and a latency cannot be told apart: the same along offset
    // reads as a different latency under a different intercept, and the constant-speed fit has
    // no two-parameter version.
    let p = Plane::new(180.0, 0.0, 1.0, 0.0, 0.0, 9);
    let rows = p.rows().0;
    assert!(oa_shadow::screen::ols(&rows.iter().collect::<Vec<_>>()).is_none());
    let a = screen_aircraft(1, &rows, R0).unwrap();
    let b = screen_aircraft(
        1,
        &rows,
        Reference {
            c_m: -18.0,
            b_m: 0.0,
        },
    )
    .unwrap();
    assert!((a.l_s - 1.0).abs() < 0.1);
    // c = -18 m at 180 m/s is 0.1 s.
    assert!((a.l_s - b.l_s - 0.1).abs() < 0.01, "{} {}", a.l_s, b.l_s);
    assert!(a.two.is_none() && b.two.is_none());
}

// REQ: OA-SHD-032
#[test]
fn the_flags_are_strict_at_their_edges() {
    assert!(!is_lag(LAG_LOWER_S));
    assert!(is_lag(LAG_LOWER_S + 1e-9));
    assert!(!is_lag(-1.0));
    assert!(!is_early(-LAG_LOWER_S));
    assert!(is_early(-LAG_LOWER_S - 1e-9));
    // Bound 10 m: three bounds is 30 m (fleet p95 below it).
    assert!(!is_bias(30.0, 30.0, 10.0, 5.0));
    assert!(!is_bias(30.01, 30.0, 10.0, 5.0));
    assert!(!is_bias(30.0, 30.01, 10.0, 5.0));
    assert!(is_bias(30.01, 30.01, 10.0, 5.0));
    // The fleet's own p95 spread is a floor under the threshold.
    assert_eq!(bias_threshold(10.0, 40.0), 40.0);
    assert_eq!(bias_threshold(10.0, 5.0), 30.0);
    assert!(!is_bias(40.0, 40.01, 10.0, 40.0));
    assert!(is_bias(40.01, 40.01, 10.0, 40.0));
    // An unknown NACp has an infinite bound: nothing flags.
    assert!(!is_bias(1e9, 1e9, f64::INFINITY, 0.0));
}

// REQ: OA-SHD-033
#[test]
fn the_bootstrap_is_deterministic_and_seeded() {
    let rows = rows_of(1.0, 40.0, 3);
    let a = bootstrap(&rows, 5, R0, true);
    let b = bootstrap(&rows, 5, R0, true);
    assert_eq!(a, b);
    assert_eq!(a.l.len(), 1000);
    assert_eq!(a.l2.len(), 1000);
    assert_ne!(a, bootstrap(&rows, 6, R0, true));
    assert!(bootstrap(&rows, 5, R0, false).l2.is_empty());
    assert_eq!(screen_aircraft(1, &rows, R0), screen_aircraft(1, &rows, R0));
    assert_eq!(screen_fleet(&fleet(), true), screen_fleet(&fleet(), true));
}

// REQ: OA-SHD-034
#[test]
fn a_report_is_left_out_for_the_first_reason_that_applies() {
    let p = Plane::new(150.0, 10.0, 0.0, 0.0, 0.0, 1);
    let n = p.flight.samples.len() as u64;
    let with = |f: &dyn Fn(i64, &mut TruthPoint) -> bool| {
        build_samples(&p.raw, &[&p.flight], &|t| {
            let mut x = p.truth(t)?;
            if f(t, &mut x) {
                None
            } else {
                Some(x)
            }
        })
    };
    // Everything passes.
    let (rows, t) = with(&|_, _| false);
    assert_eq!((rows.len() as u64, t.used, t.accounted()), (n, n, n));
    // Each reason, at every report: a different field breaks.
    type Change = fn(&mut TruthPoint);
    let cases: [(&str, Change); 7] = [
        ("sigma", |x| x.sigma95_m = 15.01),
        ("turn", |x| x.turn_dps = Some(0.51)),
        ("turn?", |x| x.turn_dps = None),
        ("vert", |x| x.vert_fpm = Some(-500.1)),
        ("vert?", |x| x.vert_fpm = None),
        ("slow", |x| x.vel_north_mps = 0.5),
        ("edge", |x| {
            x.sigma95_m = 15.0;
            x.turn_dps = Some(0.5);
            x.vert_fpm = Some(500.0);
        }),
    ];
    for (name, change) in cases {
        let (rows, t) = with(&|_, x| {
            change(x);
            false
        });
        assert_eq!(t.accounted(), n, "{name}");
        let left_out = match name {
            "sigma" => t.sigma,
            "turn" => t.turn_over,
            "turn?" => t.turn_unknown,
            "vert" => t.vert_over,
            "vert?" => t.vert_unknown,
            "slow" => t.no_velocity,
            _ => 0,
        };
        // The edge values (sigma 15 m, 0.5 deg/s, 500 ft/min) are kept.
        let expect = if name == "edge" { 0 } else { n };
        assert_eq!(left_out, expect, "{name}");
        assert_eq!(rows.len() as u64, n - expect, "{name}");
    }
    // No smoother state at all.
    let (rows, t) = build_samples(&p.raw, &[&p.flight], &|_| None);
    assert!(rows.is_empty());
    assert_eq!((t.no_truth, t.accounted()), (n, n));
    // A late ADS-B is not a reason: interpolation is not used, so a thinned trace still counts.
    let sparse: Vec<RawReport> = p.raw.iter().step_by(30).copied().collect();
    assert_eq!(
        build_samples(&sparse, &[&p.flight], &|t| p.truth(t))
            .0
            .len() as u64,
        n
    );
}

// REQ: OA-SHD-035
#[test]
fn overlaps_contradicted_spans_and_coasts_are_left_out() {
    let p = Plane::new(150.0, 10.0, 0.0, 0.0, 0.0, 1);
    let mut f1 = p.flight.clone();
    // A second flight of the same aircraft overlapping the first: its overlap is left out.
    let mut f2 = f1.clone();
    f2.gufi = 2;
    f2.samples.retain(|s| s.t_ms >= T0 + 3_000_000);
    // Three consecutive far reports of the first flight, and a coasted one.
    for k in 10..13 {
        f1.samples[k].lat_e7 += 5_000_000;
    }
    f1.samples[20].coast = true;
    let (rows, t) = build_samples(&p.raw, &[&f1, &f2], &|t| p.truth(t));
    assert_eq!(t.contradicts, 3, "{t:?}");
    assert_eq!(t.coast, 1);
    assert!(t.dup_overlap > 0);
    assert_eq!(t.eram_reports, (f1.samples.len() + f2.samples.len()) as u64);
    assert_eq!(t.accounted(), t.eram_reports);
    assert_eq!(rows.len() as u64, t.used);
    // Two far reports are not a contradicted address: they stay (and are huge samples).
    let mut g = p.flight.clone();
    for k in 10..12 {
        g.samples[k].lat_e7 += 5_000_000;
    }
    assert_eq!(
        build_samples(&p.raw, &[&g], &|t| p.truth(t)).1.contradicts,
        0
    );
}

// REQ: OA-SHD-036
#[test]
fn too_few_samples_or_too_short_a_span_is_not_screened_and_is_counted() {
    let rows = rows_of(1.0, 0.0, 1);
    // Ten blocks of ten samples at the least.
    assert!(screen_aircraft(1, &rows[..99], R0).is_none());
    assert!(screen_aircraft(1, &rows[..100], R0).is_some());
    // 100 samples a second apart span 99 s: under 10 minutes.
    let quick: Vec<SampleRow> = rows[..100]
        .iter()
        .enumerate()
        .map(|(i, r)| SampleRow {
            t_ms: T0 + 1000 * i as i64,
            ..*r
        })
        .collect();
    assert!(screen_aircraft(1, &quick, R0).is_none());
    // The fleet fit takes aircraft from 30 samples; only ten blocks are flagged. Each reason
    // for being out of the fit is counted.
    let mut inputs = fleet();
    inputs.push((1, rows[..99].to_vec()));
    inputs.push((2, quick));
    inputs.push((3, rows[..20].to_vec()));
    let f = screen_fleet(&inputs, true).unwrap();
    assert_eq!((f.with_samples, f.in_fit, f.eligible), (43, 41, 40));
    assert_eq!(f.aircraft.len(), 40);
    assert_eq!((f.few_samples, f.short_span), (1, 1));
    assert_eq!(f.fit.aircraft, 41);
    assert!(f.aircraft.iter().all(|a| a.stat.addr > 3));
}

// REQ: OA-SHD-037
#[test]
fn the_report_carries_the_caveat_and_a_digest_over_what_precedes_it() {
    let fleet = screen_fleet(&fleet(), true).unwrap();
    let header = vec!["input x".to_string()];
    let t = Tally {
        eram_reports: 9,
        used: 9,
        ..Tally::default()
    };
    let a = render_screen(&fleet, &header, 3, &t);
    let b = render_screen(&fleet, &header, 3, &t);
    assert_eq!(a, b);
    assert!(a.summary.contains(CAVEAT));
    assert_eq!(
        CAVEAT,
        "ERAM also uses ADS-B in its own tracking, so an ADS-B error shared with ERAM is not visible here; these counts are a lower bound. Radar error grows with range and is not modelled per sample."
    );
    assert!(a.summary.contains("indistinguishable at constant speed"));
    // Addresses only in the CSV.
    assert!(a.aircraft_csv.contains("000300,"));
    assert!(!a.summary.contains("000300"));
    assert!(a
        .summary
        .contains("screened for flags: 40 of 40 in the fleet fit"));
    assert!(!a.summary.contains("too few long, gated tracks"));
    assert!(a.summary.contains("BIAS threshold = max("));
    let (body, last) = a.summary.trim_end().rsplit_once('\n').unwrap();
    let mut d = a.aircraft_csv.clone().into_bytes();
    d.extend_from_slice(body.as_bytes());
    d.push(b'\n');
    assert_eq!(
        last,
        format!(
            "digest sha256:{} (aircraft.csv and this summary above)",
            to_hex(&sha256(&d))
        )
    );
    assert_eq!(a.digest, to_hex(&sha256(&d)));
}

// REQ: OA-SHD-038
#[test]
fn raw_reports_carry_the_nacp_and_keep_only_usable_adsb_positions() {
    use oa_adsb::{reports_push, Reports, F_STALE, NONE_U16, NONE_U8, ROW_EMPTY};
    use oa_shadow::run::raw_reports;
    let mut r = Reports::default();
    let row = |t: i64, nacp: u8, source: u8, flags: u8| {
        let mut x = ROW_EMPTY;
        x.time_ns = t * 1_000_000;
        x.icao = 7;
        x.lat_e7 = LAT;
        x.lon_e7 = LON;
        x.nac_p = nacp;
        x.source = source;
        x.flags = flags;
        x.gs_dkt = 1000;
        x.track_ddeg = 900;
        x
    };
    // Out of order on purpose. 1000: NACp 9; 2000: absent (carried); 3000: stale (dropped);
    // 4000: MLAT (dropped); 5000: unknown 0 (carried); 100000: beyond the carry age.
    for x in [
        row(5000, 0, 0, 0),
        row(1000, 9, 0, 0),
        row(2000, NONE_U8, 0, 0),
        row(3000, 11, 0, F_STALE),
        row(4000, 11, 5, 0),
        row(100_000, NONE_U8, 0, 0),
    ] {
        reports_push(&mut r, &x);
    }
    let mut empty = ROW_EMPTY;
    empty.icao = 8;
    reports_push(&mut r, &empty);
    let m = raw_reports(&r, &std::iter::once(7).collect());
    let v = &m[&7];
    assert_eq!(
        v.iter().map(|x| (x.t_ms, x.nacp)).collect::<Vec<_>>(),
        vec![(1000, 9), (2000, 9), (5000, 9), (100_000, 0)]
    );
    // 100 kt (1000 dkt) at track 90 degrees is east.
    let (e, n) = v[0].vel.unwrap();
    assert!((f64::from(e) - 51.4444).abs() < 1e-3 && n.abs() < 1e-3);
    assert!(!m.contains_key(&8));
    let _ = NONE_U16;
}

// REQ: OA-SHD-034
#[test]
fn a_gross_offset_a_non_finite_offset_and_an_unknown_altitude_are_handled_apart() {
    let p = Plane::new(150.0, 10.0, 0.0, 0.0, 0.0, 1);
    let n = p.flight.samples.len() as u64;
    let mut f = p.flight.clone();
    // One report 2.5 km east of where ADS-B has the aircraft (over the limit), one 1.9 km (kept).
    let t5 = (f.samples[5].t_ms - T0) as f64 / 1000.0;
    let t6 = (f.samples[6].t_ms - T0) as f64 / 1000.0;
    let (lat, lon) = at(2500.0, north_m(150.0, 10.0, t5));
    f.samples[5].lat_e7 = lat;
    f.samples[5].lon_e7 = lon;
    let (lat, lon) = at(1900.0, north_m(150.0, 10.0, t6));
    f.samples[6].lat_e7 = lat;
    f.samples[6].lon_e7 = lon;
    f.samples[7].alt_ft = None;
    assert_eq!(MAX_OFFSET_M, 2000.0);
    let (rows, t) = build_samples(&p.raw, &[&f], &|t| p.truth(t));
    assert_eq!((t.offset_gross, t.used, t.accounted()), (1, n - 1, n));
    assert!(rows.iter().any(|r| r.t_ms == f.samples[6].t_ms));
    // The unknown altitude has its own code, apart from the band below zero.
    let r7 = rows.iter().find(|r| r.t_ms == f.samples[7].t_ms).unwrap();
    assert_eq!(r7.alt_band, ALT_UNKNOWN);
    assert!(rows.iter().filter(|r| r.alt_band == 1).count() > 100);
    // The smoother says NaN: counted, never pushed.
    let (rows, t) = build_samples(&p.raw, &[&p.flight], &|t| {
        let mut x = p.truth(t)?;
        x.vel_north_mps = f64::INFINITY;
        x.lat_deg = if t % 24_000 == 0 { f64::NAN } else { x.lat_deg };
        Some(x)
    });
    assert!(t.non_finite + t.offset_gross + t.no_velocity > 0, "{t:?}");
    assert!(rows
        .iter()
        .all(|r| r.along_m.is_finite() && r.cross_m.is_finite()));
    assert_eq!(t.accounted(), n);
    // The distance to the nearest raw report: raw every second, ERAM on whole seconds.
    let (rows, _) = build_samples(&p.raw, &[&p.flight], &|t| p.truth(t));
    assert!(rows.iter().all(|r| r.gap_ms == 0));
    let sparse: Vec<RawReport> = p.raw.iter().step_by(30).copied().collect();
    let (rows, _) = build_samples(&sparse, &[&p.flight], &|t| p.truth(t));
    assert!(rows.iter().any(|r| r.gap_ms > 5000) && rows.iter().all(|r| r.gap_ms <= 15_000));
}

// REQ: OA-SHD-035
#[test]
fn a_flight_lends_its_span_only_over_the_reports_that_survive_the_contradiction_rule() {
    let p = Plane::new(150.0, 10.0, 0.0, 0.0, 0.0, 1);
    let mut f1 = p.flight.clone();
    for k in 0..10 {
        f1.samples[k].lat_e7 += 5_000_000;
    }
    let mut f2 = p.flight.clone();
    f2.gufi = 2;
    f2.samples.drain(..2);
    let (rows, t) = build_samples(&p.raw, &[&f1, &f2], &|t| p.truth(t));
    assert_eq!(t.contradicts, 10);
    // f1 is first; its reports 0..10 are contradicted, so it covers only 10.. and f2's reports
    // 2..10 (which are f1's 2..10 at the same times) are used, those from 10 on are duplicates.
    assert_eq!(t.dup_overlap, 290);
    assert_eq!(t.used, 290 + 8);
    assert_eq!(rows.len() as u64, t.used);
    assert_eq!(t.accounted(), t.eram_reports);
}

// REQ: OA-SHD-032
#[test]
fn an_early_aircraft_is_counted_apart_and_a_flag_is_checked_for_influence() {
    let mut inputs = fleet();
    // One aircraft whose ADS-B is early by 1.2 s relative to the fleet's.
    let p = Plane::new(170.0, 6.0, 0.4 - 1.2, -15.0, 0.0, 77);
    inputs.push((0x900, p.rows().0));
    let f = screen_fleet(&inputs, true).unwrap();
    let e = f.aircraft.iter().find(|a| a.stat.addr == 0x900).unwrap();
    assert!(
        e.early && !e.lag && e.excess_ci.1 < -LAG_LOWER_S,
        "{:?}",
        e.excess_ci
    );
    assert_eq!(f.aircraft.iter().filter(|a| a.early).count(), 1);
    // A flag that stands is the same before and after the influence check.
    let lag: Vec<_> = f.aircraft.iter().filter(|a| a.lag).collect();
    assert!(!lag.is_empty() && lag.iter().all(|a| a.lag_pre));
    assert!(f.aircraft.iter().all(|a| !a.lag || a.lag_pre));
    // The sensitivity table: Theil-Sen c, the ends of its interval, least squares.
    assert_eq!(f.variants.len(), 4);
    assert_eq!(
        f.variants[0].lag,
        f.aircraft.iter().filter(|a| a.lag).count()
    );
    let c = f.fit.c_ts_ci.unwrap();
    assert_eq!((f.variants[1].c_m, f.variants[2].c_m), (c.0, c.1));
    // A higher intercept reads as more lag: the low end gives the fewest LAGs.
    assert!(f.variants[1].lag <= f.variants[0].lag && f.variants[0].lag <= f.variants[2].lag);
    // The fleet cross reference and the spread that floors the BIAS threshold.
    assert!(f.b_ref_m.abs() < 5.0 && f.bias_p95_m >= 0.0);
}

// REQ: OA-SHD-032
#[test]
fn the_most_supporting_sample_is_the_one_whose_removal_lowers_the_statistic_most() {
    let mut rows = rows_of(0.0, 0.0, 5);
    rows[17].along_m = -1.0e6;
    // A mean, which one outlier moves most (a median is tied across every sample above it).
    let stat = |p: &[&SampleRow]| {
        Some(p.iter().map(|r| -r.along_m / r.speed_mps).sum::<f64>() / p.len() as f64)
    };
    assert_eq!(most_supporting(&rows, &stat), Some(17));
}

// REQ: OA-SHD-036
#[test]
fn aircraft_in_the_fit_but_not_long_enough_to_flag_make_no_prevalence() {
    // 40 aircraft of 30 samples each: all in the fleet fit, none screened for flags.
    let thin: Vec<(u32, Vec<SampleRow>)> = fleet()
        .into_iter()
        .map(|(a, r)| (a, r.into_iter().step_by(10).collect()))
        .collect();
    let f = screen_fleet(&thin, true).unwrap();
    assert_eq!((f.in_fit, f.eligible, f.aircraft.len()), (40, 0, 0));
    assert_eq!(f.fit.aircraft, 40);
    // The fleet fit exists and a render does not fail on an empty screen.
    let t = Tally::default();
    let r = render_screen(&f, &[], 40, &t);
    assert!(r
        .summary
        .contains("screened for flags: 0 of 40 in the fleet fit"));
    assert!(r
        .summary
        .contains("too few long, gated tracks for a prevalence estimate (0 aircraft"));
    assert!(r.summary.contains("LAG:  0 of 0 screened"));
}
