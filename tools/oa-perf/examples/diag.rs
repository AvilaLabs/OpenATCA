//! Diagnosis run: the stress traffic through the fusion layer with the assignment log on, then
//! which aircraft hold more than one track, how each extra track began, and the age of the data
//! by whether its aircraft has a duplicate. Not part of the harness proper.
//!
//! usage: diag N ADSB_FRAC RADARS SECS SEED [trace K]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use oa_perf::capacity::parse_radars;
use oa_perf::capacity::tracker_cfg;
use oa_perf::gen::{Layout, Traffic, TrafficCfg, ADDR_BASE};
use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_assign_log, fusion_config, fusion_new, fusion_plots,
    fusion_stats, fusion_tracker, fusion_tracker_config, fusion_tracker_stats,
};
use oa_track::fusion_params::KEY_RADAR_BASE;
use oa_track::fusion_score::EPOCH_NS;
use oa_track::{output_clear, output_new, tracker_live_iter, tracker_predict, O_COAST, O_TICK};

const NS: i64 = 1_000_000_000;

/// One extra track: label, first plot s, confirmation s, velocity changes, first-plot minus nearest
/// change s, first change s.
type ReflRow = (u32, f64, f64, usize, Option<f64>, Option<f64>);

/// Times (s) within the first `secs` s at which the aircraft's velocity changes, found at 0.1 s steps.
fn reflections(a: &oa_perf::gen::Aircraft, secs: i64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut prev = a.vel(0.0);
    for i in 1..secs * 10 {
        let t = i as f64 * 0.1;
        let v = a.vel(t);
        if (v.0 - prev.0).abs() > 1e-9 || (v.1 - prev.1).abs() > 1e-9 {
            out.push(t);
        }
        prev = v;
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a[1].parse().unwrap();
    let frac: f64 = a[2].parse().unwrap();
    let radars = parse_radars(&a[3]).unwrap();
    let secs: i64 = a[4].parse().unwrap();
    let seed: u64 = a[5].parse().unwrap();
    let trace: usize = a.get(7).map_or(0, |s| s.parse().unwrap());
    let tcfg = TrafficCfg {
        seed,
        n,
        layout: Layout::Stress,
        adsb_frac: frac,
        radar_only: false,
        radars,
        horizon_s: 120.0,
    };
    let mut traffic = Traffic::new(&tcfg).unwrap();
    let sites = traffic.sites();
    let base = tracker_cfg(1 << 16, n);
    let mut fc = fusion_config(fusion_tracker_config(&base, &sites));
    fc.log_assign = true;
    let mut f = fusion_new(&fc, &sites).unwrap();
    let mut out = output_new(base.out_capacity);
    let (mut adsb, mut plots) = (Vec::new(), Vec::new());
    // label -> every plot (time, radar)
    let mut all_plots: HashMap<u32, Vec<(i64, u8)>> = HashMap::new();
    // tick rows: (t_s, key, age_ms, coast) after warm-up
    let mut rows: Vec<(i64, u32, f64, bool)> = Vec::new();
    let warm = 60;
    let mut newest: HashMap<u32, i64> = HashMap::new();
    let mut wall = Vec::new();
    let mut snap = None;
    let mut capped_secs: HashMap<i64, u64> = HashMap::new();
    let mut prev_ung = 0u64;
    let mut timeline: Vec<String> = Vec::new();
    for k in 0..secs {
        adsb.clear();
        traffic.adsb_window(k, &mut adsb);
        traffic.plot_windows(k, &mut plots);
        for (r, ps) in plots.iter().enumerate() {
            for p in ps {
                all_plots
                    .entry(p.label)
                    .or_default()
                    .push((p.time_ns, r as u8));
            }
        }
        let until = EPOCH_NS + (k + 1) * NS;
        let before: HashMap<u32, i64> = tracker_live_iter(fusion_tracker(&f)).collect();
        for m in &adsb {
            fusion_adsb(&mut f, m);
        }
        for (r, p) in plots.iter().enumerate() {
            fusion_plots(&mut f, r, p);
        }
        // Only the whole `fusion_advance` call is timed (the layer reads no clock).
        let t0 = Instant::now();
        fusion_advance(&mut f, until, &mut out);
        wall.push(t0.elapsed().as_secs_f64() * 1e3);
        if k % 10 == 9 {
            let s = fusion_stats(&f);
            let (tt, cc) = oa_track::tracker_counts(fusion_tracker(&f));
            timeline.push(format!("t={:3} adv {:6.0} ms tracks {:5}+{:4}tent capped_cum {:3} ungated_cum {:5} cand_started_cum {:5} confirmed_cum {:5} cands_live_max {}", k + 1, wall[k as usize], cc, tt, s.bucket_work_capped, s.anon_plots_ungated, s.cand_started, s.cand_confirmed, s.cands_live_max));
        }
        {
            let u = fusion_stats(&f).anon_plots_ungated;
            if u > prev_ung {
                capped_secs.insert(k, u - prev_ung);
            }
            prev_ung = u;
        }
        if k + 1 == warm {
            snap = Some(*fusion_stats(&f));
        }
        if k >= warm {
            newest.clear();
            newest.extend(tracker_live_iter(fusion_tracker(&f)));
            for i in 0..out.t_ns.len() {
                let (key, t, fl) = (out.addr[i], out.t_ns[i], out.flags[i]);
                if fl & (O_TICK | O_COAST) == 0 {
                    continue;
                }
                let r = match (newest.get(&key), before.get(&key)) {
                    (Some(&x), _) if x <= t => x,
                    (_, Some(&x)) if x <= t => x,
                    _ => continue,
                };
                rows.push((k, key, (t - r) as f64 * 1e-6, fl & O_COAST != 0));
            }
        }
        output_clear(&mut out);
    }
    let end_ns = EPOCH_NS + secs * NS;
    let log = fusion_assign_log(&f);
    // per key: label votes, init info
    let mut votes: HashMap<u32, HashMap<u32, u32>> = HashMap::new();
    #[derive(Default, Clone)]
    struct Init {
        first: i64,
        confirm: i64,
        radars: Vec<u8>,
        label: u32,
    }
    let mut inits: HashMap<u32, Init> = HashMap::new();
    let mut by_key_plots: HashMap<u32, Vec<(i64, u8, u32, u8)>> = HashMap::new();
    for i in 0..log.t_ns.len() {
        *votes
            .entry(log.key[i])
            .or_default()
            .entry(log.label[i])
            .or_insert(0) += 1;
        by_key_plots.entry(log.key[i]).or_default().push((
            log.t_ns[i],
            log.radar[i],
            log.label[i],
            log.how[i],
        ));
        if log.how[i] == 2 {
            let e = inits.entry(log.key[i]).or_default();
            if e.radars.is_empty() {
                e.first = log.t_ns[i];
            }
            e.confirm = e.confirm.max(log.t_ns[i]);
            e.radars.push(log.radar[i]);
            e.label = log.label[i];
        }
    }
    let live: Vec<(u32, i64)> = tracker_live_iter(fusion_tracker(&f)).collect();
    // label of live key
    let label_of = |key: u32| -> Option<u32> {
        if key < KEY_RADAR_BASE {
            return Some(key);
        }
        votes
            .get(&key)
            .and_then(|v| v.iter().max_by_key(|(_, c)| **c).map(|(l, _)| *l))
    };
    let mut tracks_of: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut orphan = 0;
    for &(key, _) in &live {
        match label_of(key) {
            Some(l) => tracks_of.entry(l).or_default().push(key),
            None => orphan += 1,
        }
    }
    {
        let mut sg: Vec<(f64, u32, f64, u8)> = Vec::new();
        for &(key, last) in &live {
            if let Some((row, _, _)) = tracker_predict(fusion_tracker(&f), key, end_ns - 1) {
                let c = row.pos_cov_m2;
                sg.push((
                    f64::from(c[0].max(c[2])).sqrt(),
                    key,
                    (end_ns - last) as f64 * 1e-9,
                    0,
                ));
            }
        }
        sg.sort_by(|a, b| oa_math::cmp_canon_f64(b.0, a.0));
        let all: Vec<f64> = sg.iter().map(|x| x.0).collect();
        let mut sorted = all.clone();
        sorted.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
        println!(
            "track sigma m at end: median {:.0} p99 {:.0} max {:.0}; top: {:?}",
            sorted[sorted.len() / 2],
            sorted[sorted.len() * 99 / 100],
            sorted[sorted.len() - 1],
            sg.iter()
                .take(6)
                .map(|x| format!("sigma {:.0} key {:#x} since_update {:.1}s", x.0, x.1, x.2))
                .collect::<Vec<_>>()
        );
        let big = sg.iter().filter(|x| x.0 > 1000.0).count();
        println!("tracks with sigma > 1 km: {big}");
    }
    let st = fusion_stats(&f);
    println!(
        "== n={n} adsb_frac={frac} radars={} secs={secs} seed={seed}",
        a[3]
    );
    let ws = &wall[warm as usize..];
    let mean = ws.iter().sum::<f64>() / ws.len().max(1) as f64;
    println!(
        "live tracks {} (orphan {orphan}) aircraft with a track {} ; extra tracks {}",
        live.len(),
        tracks_of.len(),
        live.len() as i64 - n as i64
    );
    println!("advance ms mean (post-warm-up) {mean:.1}");
    let dup_labels: Vec<u32> = tracks_of
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(l, _)| *l)
        .collect();
    println!(
        "aircraft with >1 live track: {} of {} ({:.1}%)",
        dup_labels.len(),
        n,
        100.0 * dup_labels.len() as f64 / n as f64
    );
    let is_adsb_ac = |l: u32| -> bool {
        ((l - ADDR_BASE) as usize) < n && traffic.aircraft()[(l - ADDR_BASE) as usize].adsb
    };
    // classify each extra track: all radar-keyed tracks of a dup label except the oldest radar key (or all radar keys if an address key exists)
    let mut cls: BTreeMap<String, u32> = BTreeMap::new();
    let mut examples: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut late_frac = [0u32; 10];
    let mut late_capped: BTreeMap<usize, u32> = BTreeMap::new();
    let mut bucket_delta: BTreeMap<i64, u32> = BTreeMap::new();
    let mut pair_radars: BTreeMap<String, u32> = BTreeMap::new();
    let mut refl_rows: Vec<ReflRow> = Vec::new();
    let mut dup_age_labels: HashSet<u32> = HashSet::new();
    for &l in &dup_labels {
        dup_age_labels.insert(l);
        let keys = &tracks_of[&l];
        let addr_key = keys.iter().copied().find(|k| *k < KEY_RADAR_BASE);
        let mut rk: Vec<u32> = keys
            .iter()
            .copied()
            .filter(|k| *k >= KEY_RADAR_BASE)
            .collect();
        rk.sort_by_key(|k| inits.get(k).map_or(i64::MAX, |i| i.confirm));
        let extras: Vec<u32> = if addr_key.is_some() {
            rk.clone()
        } else {
            rk.iter().skip(1).copied().collect()
        };
        for &x in &extras {
            let Some(xi) = inits.get(&x) else {
                *cls.entry("no init record".into()).or_default() += 1;
                continue;
            };
            // reference track: the addr key or the earliest radar key
            let (rname, rconf) = if let Some(ak) = addr_key {
                let _ = ak;
                ("adsb", i64::MIN)
            } else {
                let r0 = rk[0];
                ("radar", inits.get(&r0).map_or(i64::MAX, |i| i.confirm))
            };
            {
                let ac = traffic.aircraft()[(l - ADDR_BASE) as usize];
                let refl = reflections(&ac, secs);
                let first_s = (xi.first - EPOCH_NS) as f64 * 1e-9;
                let conf_s = (xi.confirm - EPOCH_NS) as f64 * 1e-9;
                // nearest reflection at or before the confirmation, and how long before the first plot
                let near = refl
                    .iter()
                    .copied()
                    .filter(|r| *r <= conf_s)
                    .map(|r| first_s - r)
                    .min_by(|a, b| oa_math::cmp_canon_f64(a.abs(), b.abs()));
                refl_rows.push((l, first_s, conf_s, refl.len(), near, refl.first().copied()));
            }
            let all_r: HashSet<u8> = xi.radars.iter().copied().collect();
            let rad_tag = if all_r.len() > 1 {
                "mixed-radar".to_string()
            } else {
                format!("radar{}", xi.radars[0])
            };
            let timing = if rconf <= xi.first {
                "ref existed before first plot"
            } else if rconf < xi.confirm {
                "ref confirmed while candidate accumulating"
            } else {
                "ref confirmed at/after this one"
            };
            let refc = if rname == "radar" {
                inits.get(&rk[0]).map_or(0, |i| i.confirm)
            } else {
                0
            };
            if rname == "radar" {
                let d = xi.confirm.div_euclid(NS) - refc.div_euclid(NS);
                *bucket_delta.entry(d).or_default() += 1;
                let rr = inits
                    .get(&rk[0])
                    .map_or(9, |i| *i.radars.last().unwrap_or(&9));
                *pair_radars
                    .entry(format!(
                        "ref confirmed by radar{rr}, dup by radar{}",
                        xi.radars.last().unwrap_or(&9)
                    ))
                    .or_default() += 1;
            }
            let mindelta: i64 = a.get(8).map_or(0, |s| s.parse().unwrap());
            let dd = xi.confirm.div_euclid(NS) - refc.div_euclid(NS);
            if rname == "radar" && dd >= 20 {
                let secs3: Vec<i64> = by_key_plots[&x]
                    .iter()
                    .filter(|p| p.3 == 2)
                    .map(|p| (p.0 - EPOCH_NS).div_euclid(NS))
                    .collect();
                let nc = secs3.iter().filter(|s| capped_secs.contains_key(s)).count();
                *late_capped.entry(nc).or_default() += 1;
                for p in by_key_plots[&x].iter().filter(|p| p.3 == 2) {
                    let fr = ((p.0 - EPOCH_NS).rem_euclid(NS) / 100_000_000) as usize;
                    late_frac[fr.min(9)] += 1;
                }
            }
            let key = format!("{rname}-ref | {timing} | cand {rad_tag}");
            if dd < mindelta {
                continue;
            }
            *cls.entry(key.clone()).or_default() += 1;
            examples.entry(key).or_default().push(l);
        }
    }
    {
        // reflection attribution: an extra track is explained when a reflection of its aircraft
        // lies within WIN s before its first plot (to its confirmation)
        const WIN: f64 = 20.0;
        let mut hit = 0;
        let mut unexplained: Vec<String> = Vec::new();
        for (l, f, c, nr, near, r0) in &refl_rows {
            if near.is_some_and(|d| (-(c - f)..=WIN).contains(&d)) {
                hit += 1;
            } else {
                unexplained.push(format!("{l:#x} first {f:.0}s confirm {c:.0}s reflections {nr} first at {r0:?} nearest(first-refl) {near:?}"));
            }
        }
        let ac_refl = traffic
            .aircraft()
            .iter()
            .filter(|a| !reflections(a, secs).is_empty())
            .count();
        println!("-- reflection attribution: {hit} of {} extra tracks start within {WIN} s after a reflection of their aircraft; aircraft that reflect at all in {secs} s: {ac_refl} of {n}; extra-track aircraft that reflect: {} of {}", refl_rows.len(), refl_rows.iter().filter(|r| r.3 > 0).count(), refl_rows.len());
        for u in &unexplained {
            println!("   not explained: {u}");
        }
    }
    println!("-- extra-track classification (live extras at end) --");
    let tot: u32 = cls.values().sum();
    for (k, v) in &cls {
        println!("{v:5} {:5.1}%  {k}", 100.0 * *v as f64 / tot.max(1) as f64);
    }
    println!("confirm-bucket delta (dup minus ref, seconds): {bucket_delta:?}");
    println!("{pair_radars:?}");
    println!("late dups: initiating-plot position within its second, tenths: {late_frac:?}");
    for l in &timeline {
        println!("{l}");
    }
    println!("late dups (>=20 s after ref): how many of their 3 initiating plots fell in a second with ungated plots: {late_capped:?}; seconds with ungated plots {} of {secs}", capped_secs.len());
    // overall confirmations split
    let radar_keys_created = inits.len();
    println!("radar-key tracks ever confirmed: {radar_keys_created}; aircraft count {n}; radar-only aircraft {}", n - traffic.adsb_count());
    // by ADS-B status of dup aircraft
    let (mut d_ads, mut d_rad) = (0, 0);
    for &l in &dup_labels {
        if is_adsb_ac(l) {
            d_ads += 1
        } else {
            d_rad += 1
        }
    }
    println!("dup aircraft: ADS-B equipped {d_ads} of {} equipped ({:.1}%), radar-only {d_rad} of {} ({:.1}%)", traffic.adsb_count(), 100.0*d_ads as f64/traffic.adsb_count().max(1) as f64, n - traffic.adsb_count(), 100.0*d_rad as f64/(n - traffic.adsb_count()).max(1) as f64);
    // All confirmations (ever): per label how many radar keys were created
    let mut created: HashMap<u32, Vec<u32>> = HashMap::new();
    for (k, i) in &inits {
        created.entry(i.label).or_default().push(*k);
    }
    let multi = created.values().filter(|v| v.len() > 1).count();
    let adsb_with_radar = created.keys().filter(|l| is_adsb_ac(**l)).count();
    println!("aircraft that ever had >1 radar key created: {multi}; ADS-B aircraft that ever got a radar key: {adsb_with_radar}; total radar keys ever {} for {} labels", inits.len(), created.len());
    // creation-time distribution of ever-created radar keys (seconds since epoch)
    let mut hist: BTreeMap<i64, u32> = BTreeMap::new();
    for i in inits.values() {
        *hist
            .entry((i.confirm - EPOCH_NS) / NS / 10 * 10)
            .or_default() += 1;
    }
    println!("radar keys confirmed per 10 s: {:?}", hist);
    // plots not applied
    println!("stats: plots_in {} applied_assoc {} applied_hard {} held {} held_forced {} unassigned {} cand_started {} cand_joined {} cand_confirmed {} cand_expired {} cand_capacity {} bucket_work_capped {} init_capped {} anon_plots_ungated {} cands_live_max {}",
        st.plots_in, st.applied_assoc, st.applied_hard, st.held, st.held_forced, st.unassigned, st.cand_started, st.cand_joined, st.cand_confirmed, st.cand_expired, st.cand_capacity, st.bucket_work_capped, st.init_capped, st.anon_plots_ungated, st.cands_live_max);
    println!("stats: pairs_tested {} pairs_gated {} coarse_visits {} tracks_at_sum {} init_tests {} gnn_largest {}", st.pairs_tested, st.pairs_gated, st.coarse_visits, st.tracks_at_sum, st.init_tests, st.gnn.largest);
    let ts = fusion_tracker_stats(&f);
    println!(
        "tracker: started {} confirmed {} jump_rejected {} resyncs {} ended {:?}",
        ts.started, ts.confirmed, ts.jump_rejected, ts.resyncs, ts.ended
    );
    let s0 = snap.unwrap_or_default();
    let ds = |x: u64, y: u64| x - y;
    let post = (secs - warm).max(1) as f64;
    println!("POST-WARM-UP ({post} s) per second: plots {:.0} tracks_at {:.0} coarse_visits {:.0} pairs_tested {:.0} pairs_gated {:.0} init_tests {:.0} cand_started {:.1} cand_confirmed {:.1} cand_expired {:.1} held {:.1} held_forced {:.1} unassigned {:.1} ungated {:.1} work_capped {:.2} init_capped {:.2}",
        ds(st.plots_in, s0.plots_in) as f64 / post, ds(st.tracks_at_sum, s0.tracks_at_sum) as f64 / post, ds(st.coarse_visits, s0.coarse_visits) as f64 / post, ds(st.pairs_tested, s0.pairs_tested) as f64 / post, ds(st.pairs_gated, s0.pairs_gated) as f64 / post, ds(st.init_tests, s0.init_tests) as f64 / post,
        ds(st.cand_started, s0.cand_started) as f64 / post, ds(st.cand_confirmed, s0.cand_confirmed) as f64 / post, ds(st.cand_expired, s0.cand_expired) as f64 / post, ds(st.held, s0.held) as f64 / post, ds(st.held_forced, s0.held_forced) as f64 / post, ds(st.unassigned, s0.unassigned) as f64 / post, ds(st.anon_plots_ungated, s0.anon_plots_ungated) as f64 / post, ds(st.bucket_work_capped, s0.bucket_work_capped) as f64 / post, ds(st.init_capped, s0.init_capped) as f64 / post);
    let runs = ds(st.assoc_runs, s0.assoc_runs).max(1) as f64;
    println!("POST-WARM-UP coarse width: mean {:.0} m, max-ever {} m; coarse visits per plot {:.0}; live cands at end {}", ds(st.coarse_width_sum_m, s0.coarse_width_sum_m) as f64 / runs, st.coarse_width_max_m, ds(st.coarse_visits, s0.coarse_visits) as f64 / ds(st.plots_in, s0.plots_in).max(1) as f64, st.cand_started - st.cand_confirmed - st.cand_expired);
    println!("POST-WARM-UP work units per traffic second: coarse_visits {:.0} exact_tests {:.0} init_tests {:.0} applied {:.0} confirmed {:.1} buckets {:.1}",
        ds(st.coarse_visits, s0.coarse_visits) as f64 / post, ds(st.pairs_tested, s0.pairs_tested) as f64 / post, ds(st.init_tests, s0.init_tests) as f64 / post, (ds(st.applied_assoc, s0.applied_assoc) + ds(st.applied_hard, s0.applied_hard)) as f64 / post, ds(st.cand_confirmed, s0.cand_confirmed) as f64 / post, ds(st.buckets, s0.buckets) as f64 / post);
    // seal-allow: /proc, not an input
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!("shell-timed fusion_advance, mean ms per traffic second, post-warm-up (loaded machine, load average {}): {:.1}", load.split_whitespace().take(3).collect::<Vec<_>>().join(" "), mean);
    // ages (radar-only tick rows, non-coast) by dup
    let mut ages: [Vec<f64>; 3] = [vec![], vec![], vec![]];
    let mut coast_rows = [0u32; 2];
    for &(_, key, age, coast) in &rows {
        if key < KEY_RADAR_BASE {
            continue;
        }
        let Some(l) = label_of(key) else { continue };
        let d = dup_age_labels.contains(&l) as usize;
        if coast {
            coast_rows[d] += 1;
            continue;
        }
        ages[d].push(age * 1e-3);
        ages[2].push(age * 1e-3);
    }
    for (name, v) in ["no-dup aircraft", "dup aircraft", "all"]
        .iter()
        .zip(ages.iter_mut())
    {
        v.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
        let p = |q: f64| {
            v.get(((v.len() as f64 - 1.0) * q) as usize)
                .copied()
                .unwrap_or(f64::NAN)
        };
        println!(
            "radar-only tick age s [{name}]: rows {} p50 {:.1} p90 {:.1} p99 {:.1} max {:.1}",
            v.len(),
            p(0.5),
            p(0.9),
            p(0.99),
            v.last().copied().unwrap_or(f64::NAN)
        );
    }
    println!(
        "radar-only coast rows: no-dup {} dup {}",
        coast_rows[0], coast_rows[1]
    );
    // update gaps per radar track: time between applied plots of that key (post-warm-up)
    let mut gaps: [Vec<f64>; 2] = [vec![], vec![]];
    for (key, v) in &by_key_plots {
        if *key < KEY_RADAR_BASE {
            continue;
        }
        let Some(l) = label_of(*key) else { continue };
        let d = dup_age_labels.contains(&l) as usize;
        let mut ts: Vec<i64> = v.iter().filter(|x| x.3 != 2).map(|x| x.0).collect();
        ts.sort();
        for w in ts.windows(2) {
            if w[0] >= EPOCH_NS + warm * NS {
                gaps[d].push((w[1] - w[0]) as f64 * 1e-9);
            }
        }
    }
    for (name, v) in ["no-dup", "dup"].iter().zip(gaps.iter_mut()) {
        v.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
        let p = |q: f64| {
            v.get(((v.len() as f64 - 1.0) * q) as usize)
                .copied()
                .unwrap_or(f64::NAN)
        };
        println!("gap between applied plots of one radar track s [{name}]: n {} p50 {:.1} p90 {:.1} p99 {:.1} max {:.1}", v.len(), p(0.5), p(0.9), p(0.99), v.last().copied().unwrap_or(f64::NAN));
    }
    // traces
    let mut shown = 0;
    for (k, ls) in &examples {
        for &l in ls.iter().take(trace) {
            if shown >= trace * 4 {
                break;
            }
            shown += 1;
            let ac = traffic.aircraft()[(l - ADDR_BASE) as usize];
            println!("\n--- trace: class [{k}] aircraft {l:#x} adsb={} pos0=({:.0},{:.0}) v=({:.0},{:.0})", ac.adsb, ac.e0, ac.n0, ac.ve, ac.vn);
            let keys = &tracks_of[&l];
            println!(
                "live keys {:?}",
                keys.iter().map(|k| format!("{k:#x}")).collect::<Vec<_>>()
            );
            let mut applied: HashMap<(i64, u8), (u32, u8)> = HashMap::new();
            for (kk, v) in &by_key_plots {
                for x in v {
                    if x.2 == l {
                        applied.insert((x.0, x.1), (*kk, x.3));
                    }
                }
            }
            let mut pl = all_plots[&l].clone();
            pl.sort();
            for (t, r) in pl.iter().take(80) {
                let s = (*t - EPOCH_NS) as f64 * 1e-9;
                match applied.get(&(*t, *r)) {
                    Some((kk, how)) => println!("  t={s:6.1} radar{r} -> key {kk:#x} how {how}"),
                    None => println!(
                        "  t={s:6.1} radar{r} -> (not applied: unassigned/held/candidate hit)"
                    ),
                }
            }
            for kk in keys {
                if let Some(i) = inits.get(kk) {
                    println!(
                        "  key {kk:#x}: init radars {:?} first {:.1} confirm {:.1}",
                        i.radars,
                        (i.first - EPOCH_NS) as f64 * 1e-9,
                        (i.confirm - EPOCH_NS) as f64 * 1e-9
                    );
                }
            }
        }
    }
    let _ = end_ns;
}
