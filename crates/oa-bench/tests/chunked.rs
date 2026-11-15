//! The chunked benchmark's exact histogram: the statistics of whole-millisecond values agree
//! with `stats_of` over the same values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_bench::assoc::astatus;
use oa_bench::chunked::{benchmark_chunk, chunk_add, chunk_finish, truth_index, ChunkAcc, MsHist};
use oa_bench::legacy::{flight_ix, new_legacy, push_th, set_addr, Legacy, NO_ALT, NO_TIME};
use oa_bench::report::{code_version, render_csv, render_text, Header};
use oa_bench::run::{find, run_benchmark, RunConfig, RunOutput};
use oa_bench::stats::{stats_of, MetricResult};
use oa_bench::synth::{build_world, env, truth_at, Injected, World};
use oa_bench::truth::Truth;

fn lcg(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

// REQ: OA-TRK-043
#[test]
fn the_millisecond_histogram_gives_the_statistics_of_the_values() {
    let mut s = 7u64;
    for (n, whole_seconds) in [(1usize, true), (2, false), (1000, false), (50_000, true)] {
        let ms: Vec<i64> = (0..n)
            .map(|_| {
                let v = (lcg(&mut s) % 1_200_000) as i64 - 100_000;
                if whole_seconds {
                    (v / 1000) * 1000
                } else {
                    v
                }
            })
            .collect();
        let vals: Vec<f64> = ms.iter().map(|&m| m as f64 / 1000.0).collect();
        let mut h = MsHist::default();
        for &m in &ms {
            h.add(m);
        }
        let (a, b) = (h.stats(), stats_of(&vals));
        // Order statistics are exact; the sums are exact integers here against a running floating
        // point sum there, so the mean and RMS agree to rounding.
        assert_eq!(
            (a.n, a.median, a.p95, a.p99, a.max),
            (b.n, b.median, b.p95, b.p99, b.max)
        );
        let close = |x: f64, y: f64| (x - y).abs() <= 1e-12 * y.abs().max(1.0);
        assert!(close(a.mean, b.mean) && close(a.rms, b.rms), "{a:?} {b:?}");
        if whole_seconds {
            // Whole seconds add exactly in floating point: bit for bit.
            assert_eq!(
                (a.mean.to_bits(), a.rms.to_bits()),
                (b.mean.to_bits(), b.rms.to_bits())
            );
        }
        // Adding two halves gives the whole.
        let (mut h1, mut h2) = (MsHist::default(), MsHist::default());
        for (i, &m) in ms.iter().enumerate() {
            if i % 2 == 0 {
                h1.add(m);
            } else {
                h2.add(m);
            }
        }
        h1.merge(&h2);
        assert_eq!(h1, h);
    }
    assert_eq!(MsHist::default().stats(), oa_bench::stats::Stats::default());
}

// ---- a tracker's output in address ranges, against one piece ---------------------------------

/// One output row.
#[derive(Clone, Copy)]
struct Row {
    addr: u32,
    t_ms: i64,
    lat_e7: i32,
    lon_e7: i32,
    alt_ft: i32,
    /// The aircraft is moved far from its truth.
    far: bool,
}

/// The synthetic truth sampled every 2 s as a tracker's output, each aircraft with its own
/// error (so the sums of the statistics are not round numbers); every fourth aircraft is moved
/// 5 degrees north, far enough for its address to be flagged as implausible.
fn rows_of(w: &World) -> Vec<Row> {
    let mut v = Vec::new();
    for (k, l) in w.legs.iter().enumerate() {
        let mut t = l.start_ms + 2000;
        let mut i = 0i64;
        while t <= l.end_ms - 2000 {
            if let Ok(s) = truth_at(&w.aircraft, l.addr, t) {
                let north = if k % 4 == 3 {
                    5.0
                } else {
                    (20.0 + (k as f64) * 13.37 + ((i * 7919) % 1000) as f64 * 0.0123) / 111_195.0
                };
                v.push(Row {
                    addr: l.addr,
                    t_ms: t,
                    lat_e7: ((s.lat + north) * 1e7).round() as i32,
                    lon_e7: ((s.lon + ((i * 31) % 17) as f64 * 1e-5) * 1e7).round() as i32,
                    alt_ft: s.baro_alt_ft.map_or(NO_ALT, |a| a + 10 + (i % 13) as i32),
                    far: k % 4 == 3,
                });
            }
            t += 2000;
            i += 1;
        }
    }
    v
}

/// The legacy table of `rows`, one flight per address, the flights created from the highest
/// address down (so the flight order is not the address order).
fn legacy_of(rows: &[Row]) -> Legacy {
    let mut leg = new_legacy();
    let mut addrs: Vec<u32> = rows.iter().map(|r| r.addr).collect();
    addrs.sort_unstable();
    addrs.dedup();
    let mut at = std::collections::BTreeMap::new();
    for &a in addrs.iter().rev() {
        let f = flight_ix(&mut leg, u128::from(a) | (1u128 << 100)).unwrap();
        set_addr(&mut leg, f, a);
        // A callsign of its own on the far flights (it disagrees with the truth's, which flags
        // them), so that the case lines of two flights differ.
        if rows.iter().any(|r| r.addr == a && r.far) {
            let cs = format!("TRK{:04X}", a & 0xFFFF);
            leg.flights.callsign[f as usize][..cs.len()].copy_from_slice(cs.as_bytes());
        }
        at.insert(a, f);
    }
    for r in rows {
        push_th(
            &mut leg,
            at[&r.addr],
            r.t_ms,
            NO_TIME,
            r.lat_e7,
            r.lon_e7,
            r.alt_ft,
            None,
            false,
        );
    }
    leg
}

fn header() -> Header {
    Header {
        mode: "synthetic".into(),
        code_version: code_version(),
        inputs: vec![],
        replay_hash: "none".into(),
        counts: vec![],
        notes: vec![],
        truth: vec![],
    }
}

fn texts(o: &RunOutput) -> (String, String) {
    (render_text(&header(), o), render_csv(&header(), o))
}

/// Asserts two reports are the same text, naming the first line that differs.
fn same_text(got: &str, want: &str, what: &str) {
    if let Some((i, (g, w))) = got
        .lines()
        .zip(want.lines())
        .enumerate()
        .find(|(_, (g, w))| g != w)
    {
        panic!("{what}: line {} differs\n  got:  {g}\n  want: {w}", i + 1);
    }
    assert_eq!(
        got.lines().count(),
        want.lines().count(),
        "{what}: line count"
    );
}

/// Scores `legs_by_range` in the order given with the benchmark's chunk functions.
fn chunked(
    w: &World,
    parts: &[((u32, u32), Vec<Row>)],
) -> Result<(RunOutput, oa_bench::pair::PairTable), oa_bench::chunked::ChunkError> {
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let tix = truth_index(&truth);
    let cfg = RunConfig::default();
    let mut acc = ChunkAcc::default();
    for (range, rows) in parts {
        let leg = legacy_of(rows);
        let c = benchmark_chunk(&leg, &truth, &tix, &env(), &cfg, *range);
        chunk_add(&mut acc, c)?;
    }
    chunk_finish(acc, &cfg)
}

fn split(rows: Vec<Row>, cuts: &[u32]) -> Vec<((u32, u32), Vec<Row>)> {
    let mut bounds = vec![0u32];
    bounds.extend_from_slice(cuts);
    let mut out = Vec::new();
    for (i, &lo) in bounds.iter().enumerate() {
        let hi = bounds.get(i + 1).map_or(u32::MAX, |&b| b - 1);
        out.push(((lo, hi), Vec::new()));
    }
    for r in rows {
        let k = out
            .iter()
            .position(|(rg, _)| r.addr >= rg.0 && r.addr <= rg.1);
        out[k.unwrap()].1.push(r);
    }
    out
}

fn world_cuts(w: &World) -> Vec<u32> {
    let mut a: Vec<u32> = w.legs.iter().map(|l| l.addr).collect();
    a.sort_unstable();
    a.dedup();
    vec![a[a.len() / 3], a[2 * a.len() / 3]]
}

// REQ: OA-BENCH-040
#[test]
fn chunked_and_one_shot_reports_are_byte_identical_for_flights_out_of_address_order() {
    let w = build_world(&Injected::default());
    let rows = rows_of(&w);
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let one = run_benchmark(&legacy_of(&rows), &truth, &env(), &RunConfig::default());
    let flagged = one.assoc.counts[usize::from(astatus::ADDRESS_IMPLAUSIBLE)];
    assert!(flagged >= 2, "the fixture must flag flights: {flagged}");
    let want = texts(&one);
    let cuts = world_cuts(&w);
    let parts = split(rows, &cuts);
    assert_eq!(parts.len(), 3);
    assert!(parts.iter().all(|p| !p.1.is_empty()));
    // The ranges in address order, in reverse order, and in the order middle, last, first.
    for order in [[0usize, 1, 2], [2, 1, 0], [1, 2, 0]] {
        let ordered: Vec<_> = order
            .iter()
            .map(|&i| {
                let (rg, rows) = &parts[i];
                (
                    *rg,
                    rows.iter()
                        .map(|r| Row { addr: r.addr, ..*r })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let (out, pairs) = chunked(&w, &ordered).unwrap();
        assert_eq!(pairs.t_ms.len(), one.pairs);
        let got = texts(&out);
        same_text(
            &got.0,
            &want.0,
            &format!("text report, range order {order:?}"),
        );
        same_text(
            &got.1,
            &want.1,
            &format!("csv report, range order {order:?}"),
        );
    }
}

// REQ: OA-BENCH-040
#[test]
fn a_sum_of_the_statistics_does_not_depend_on_the_order_of_the_values() {
    // Floating point sums of these differ by order (1e16 absorbs the 1 unless it comes last).
    let base = [1e16, 1.0, -1e16, 3.0, 2.5e-3, -7.0, 1e15, -1e15];
    let want = stats_of(&base);
    let mut v = base.to_vec();
    for k in 0..base.len() {
        v.rotate_left(k);
        let s = stats_of(&v);
        assert_eq!(s.mean.to_bits(), want.mean.to_bits(), "rotation {k}");
        assert_eq!(s.rms.to_bits(), want.rms.to_bits(), "rotation {k}");
        v.reverse();
        let s = stats_of(&v);
        assert_eq!(s.mean.to_bits(), want.mean.to_bits(), "reversed {k}");
    }
    // The sum is the exactly rounded one: 1 + 3 + 2.5e-3 - 7 = -2.9975 over 8 values.
    assert!((want.mean - (-2.9975 / 8.0)).abs() < 1e-12, "{}", want.mean);
}

// REQ: OA-BENCH-041
#[test]
fn ranges_that_overlap_leave_a_gap_or_miss_a_key_are_refused() {
    let w = build_world(&Injected::default());
    let rows = rows_of(&w);
    let cuts = world_cuts(&w);
    let parts = split(rows.clone(), &cuts);
    // The right tiling is accepted.
    assert!(chunked(&w, &parts).is_ok());
    // Overlap: the middle range starts one address early (rows are not duplicated, but the
    // ranges claim the same address).
    let mut p = parts.clone();
    p[1].0 .0 -= 1;
    let e = chunked(&w, &p).err().unwrap();
    assert!(e.0.contains("overlap"), "{e}");
    // A gap: the lowest key is in no range (no aircraft has it, so no flight is stranded).
    let mut p = parts.clone();
    p[0].0 .0 = 1;
    let e = chunked(&w, &p).err().unwrap();
    assert!(e.0.contains("cover") || e.0.contains("gap"), "{e}");
    // The top of the key space is missing (radar-only tracks are keyed at 0x4000_0000).
    let mut p = parts.clone();
    p[2].0 .1 = 0x3FFF_FFFF;
    let e = chunked(&w, &p).err().unwrap();
    assert!(e.0.contains("cover") || e.0.contains("gap"), "{e}");
    // A flight keyed outside its range is refused, not scored against the wrong legs.
    let mut p = parts.clone();
    let moved = p[2].1.remove(0);
    p[0].1.push(moved);
    let e = chunked(&w, &p).err().unwrap();
    assert!(e.0.contains("outside"), "{e}");
    // One range holding everything is the one-piece case.
    let all = vec![((0u32, u32::MAX), rows)];
    assert!(chunked(&w, &all).is_ok());
}

/// The headline T1 row named `label`, if any.
fn row_of<'a>(m: &'a MetricResult, label: &str) -> Option<&'a oa_bench::stats::Row> {
    m.rows.iter().find(|r| r.label == label)
}

fn scalar_of(m: &MetricResult, k: &str) -> Option<f64> {
    m.scalars.iter().find(|(n, _)| n == k).map(|(_, v)| *v)
}

// REQ: OA-BENCH-042
#[test]
fn the_headline_tables_say_what_the_flagged_flights_took_out_and_show_the_total_with_them() {
    let w = build_world(&Injected::default());
    let rows = rows_of(&w);
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let one = run_benchmark(&legacy_of(&rows), &truth, &env(), &RunConfig::default());
    let parts = split(rows, &world_cuts(&w));
    let (ch, _) = chunked(&w, &parts).unwrap();
    for out in [&one, &ch] {
        let flagged_flights = out.assoc.counts[usize::from(astatus::ADDRESS_IMPLAUSIBLE)] as f64;
        assert!(flagged_flights >= 2.0);
        for (id, fid) in [
            ("T1", "T1-flagged"),
            ("T2-raw", "T2-raw-flagged"),
            ("T4", "T4-flagged"),
        ] {
            let h = find(out, id).unwrap();
            let f = find(out, fid).unwrap();
            let kept = row_of(h, "all").unwrap().stats;
            let flagged = row_of(f, "all").unwrap().stats;
            let all = row_of(h, "all associated including flagged").unwrap().stats;
            assert_eq!(all.n, kept.n + flagged.n, "{id}");
            assert!(all.max >= kept.max && all.max >= flagged.max, "{id}");
            assert_eq!(
                scalar_of(h, "flagged_flights_excluded"),
                Some(flagged_flights),
                "{id}"
            );
            assert_eq!(
                scalar_of(h, "flagged_points_excluded"),
                Some(flagged.n as f64),
                "{id}"
            );
        }
    }
    // The text shows it in the headline table itself.
    let t = render_text(&header(), &one);
    let t1 = t
        .split("\n[T1]")
        .nth(1)
        .unwrap()
        .split("\n[")
        .next()
        .unwrap();
    assert!(t1.contains("all associated including flagged"), "{t1}");
    assert!(t1.contains("flagged_flights_excluded"), "{t1}");
}

// REQ: OA-BENCH-043
#[test]
fn a_flow_result_that_is_not_what_no_data_gives_is_refused_even_when_it_is_only_nan() {
    let w = build_world(&Injected::default());
    let rows = rows_of(&w);
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let tix = truth_index(&truth);
    let cfg = RunConfig::default();
    let leg = legacy_of(&rows);
    let range = (0u32, u32::MAX);
    // The untouched chunk is accepted.
    let ok = benchmark_chunk(&leg, &truth, &tix, &env(), &cfg, range);
    assert!(chunk_add(&mut ChunkAcc::default(), ok).is_ok());
    // A zero scalar of a no-data result that reads NaN is data the metric has no right to have.
    let mut tried = 0;
    for k in 0..6 {
        let mut c = benchmark_chunk(&leg, &truth, &tix, &env(), &cfg, range);
        let r = &mut c.flows[k];
        let i = r.scalars.iter().position(|(_, v)| *v == 0.0);
        let Some(i) = i else { continue };
        r.scalars[i].1 = f64::NAN;
        tried += 1;
        let e = chunk_add(&mut ChunkAcc::default(), c).err();
        assert!(
            e.is_some(),
            "flow result {k} with a NaN where no data gives 0 was accepted"
        );
    }
    assert!(
        tried >= 1,
        "the fixture must have zero scalars to turn into NaN"
    );
}

// REQ: OA-BENCH-044
#[test]
fn the_association_of_a_chunked_run_is_the_real_one_or_absent() {
    let w = build_world(&Injected::default());
    let rows = rows_of(&w);
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let one = run_benchmark(&legacy_of(&rows), &truth, &env(), &RunConfig::default());
    let (ch, _) = chunked(&w, &split(rows, &world_cuts(&w))).unwrap();
    let n = |a: &oa_bench::assoc::Assoc| a.addr.len();
    assert_eq!(
        n(&ch.assoc),
        n(&one.assoc),
        "one association row per flight"
    );
    assert_eq!(ch.assoc.counts, one.assoc.counts);
    // Per flight, by truth address (the flight order is the ranges' own).
    let by_addr = |a: &oa_bench::assoc::Assoc| {
        let mut v: Vec<(u32, u8, u8, u32)> = (0..a.addr.len())
            .map(|i| {
                (
                    a.addr[i].min(a.flag_addr[i]),
                    a.method[i],
                    a.status[i],
                    a.shared[i],
                )
            })
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(by_addr(&ch.assoc), by_addr(&one.assoc));
}
