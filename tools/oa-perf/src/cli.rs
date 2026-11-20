//! The command line and the orchestration: `all`, `clip`, `sweep`, `one`.
//!
//! `one` runs a single capacity run in this process (so its `VmHWM` is that run's alone).
//! `sweep` starts `one` as a child process for each size, reads the key=value file the child
//! wrote, finds the knee and runs the overload. `all` is the clip run and then the sweep.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::capacity::{
    cap_kv, cap_text, loss_counters, parse_radars, run_capacity, seconds_csv, CapCfg,
};
use crate::clip::{clip_kv, clip_text, run_clip, ClipCfg};
use crate::gen::{Layout, TrafficCfg};
use crate::stats::{fmt_f, Kv};
use crate::sys::{cpu_model, loadavg, nproc};
use oa_track::fusion_params::MAX_ASSOC_TRACKS;
use oa_track::fusion_score::EPOCH_NS;

/// Seconds past the end of the run that the traffic is generated for.
const HORIZON_PAD_S: i64 = 60;

/// The usage text.
pub const USAGE: &str = "usage: oa-perf <all|clip|sweep|one> [flags]
  all     the clip latency run, then the capacity sweep, knee and overload
  clip    per-update latency on the real clip (OPENATCA_ADSB_CLIP or --clip)
  sweep   capacity runs at each size, the knee, and a run at twice the knee
  one     a single capacity run in this process (--n)
flags (defaults in brackets):
  --out DIR          results directory [oa-perf-out]
  --clip FILE        the clip [$OPENATCA_ADSB_CLIP]      --clip-secs S  [600]
  --sizes A,B,...    aircraft counts [1000,5000,10000,20000,38000]
  --n N              aircraft count for `one` [1000]
  --layout nas|stress  traffic layout [stress]: stress puts every aircraft under every radar,
                     nas spreads them at the whole-NAS density
  --radars e,t,t     radars: e en-route (12 s), t terminal (4.8 s) [e,t,t]
  --adsb-frac F      fraction of aircraft sending ADS-B [0.5]
  --warmup S --duration S   seconds fed before the statistics, and seconds measured [60, 60]
  --max-wall S       stop a run's feeding after this many wall seconds, positive [540]
  --max-tracks N     tracker capacity [65536]
  --refine K         bisection steps between the knee and the first failing size [2]
  --seed N           traffic seed [1]
  --no-clip --no-overload   leave a part out
  --no-early-stop    feed the whole run even when it is far slower than real time";

/// Parsed flags.
#[derive(Debug, Clone)]
pub struct Opts {
    pub cmd: String,
    pub out: PathBuf,
    pub clip: Option<String>,
    pub clip_secs: i64,
    pub sizes: Vec<usize>,
    pub n: usize,
    pub layout: Layout,
    pub radars: String,
    pub adsb_frac: f64,
    pub warmup: i64,
    pub duration: i64,
    pub max_wall: f64,
    pub max_tracks: usize,
    pub refine: usize,
    pub seed: u64,
    pub no_clip: bool,
    pub no_overload: bool,
    pub no_early_stop: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            cmd: String::new(),
            out: PathBuf::from("oa-perf-out"),
            clip: std::env::var("OPENATCA_ADSB_CLIP").ok(),
            clip_secs: 600,
            sizes: vec![1_000, 5_000, 10_000, 20_000, 38_000],
            n: 1_000,
            layout: Layout::Stress,
            radars: "e,t,t".to_string(),
            adsb_frac: 0.5,
            warmup: 60,
            duration: 60,
            max_wall: 540.0,
            max_tracks: MAX_ASSOC_TRACKS,
            refine: 2,
            seed: 1,
            no_clip: false,
            no_overload: false,
            no_early_stop: false,
        }
    }
}

fn num<T: std::str::FromStr>(flag: &str, v: Option<&String>) -> Result<T, String> {
    v.ok_or_else(|| format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag}: not a valid number"))
}

/// Refuses the timing and fraction values that cannot work: a wall limit that is not a positive
/// finite number, a fraction outside 0..=1, and traffic times whose sum (with the 60 s the
/// traffic is generated past the run) or whose nanoseconds would overflow or leave the range the
/// layer accepts.
fn check_times(o: &Opts) -> Result<(), String> {
    if !(o.max_wall.is_finite() && o.max_wall > 0.0) {
        return Err(format!(
            "--max-wall: {} is not a positive finite number of seconds",
            o.max_wall
        ));
    }
    if !(0.0..=1.0).contains(&o.adsb_frac) {
        return Err(format!(
            "--adsb-frac: {} is not between 0 and 1",
            o.adsb_frac
        ));
    }
    let total = o
        .warmup
        .checked_add(o.duration)
        .and_then(|t| t.checked_add(HORIZON_PAD_S))
        .ok_or("--warmup plus --duration is too large")?;
    let limit_s = (oa_track::TIME_LIMIT_NS - EPOCH_NS) / 1_000_000_000;
    if total > limit_s || o.clip_secs > limit_s {
        return Err(format!(
            "--warmup plus --duration (or --clip-secs) is more than {limit_s} s, the longest time the layer accepts"
        ));
    }
    Ok(())
}

/// Parses the command line (without the program name).
pub fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    o.cmd = it.next().ok_or("no command")?.clone();
    if !["all", "clip", "sweep", "one"].contains(&o.cmd.as_str()) {
        return Err(format!("unknown command '{}'", o.cmd));
    }
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => o.out = PathBuf::from(it.next().ok_or("--out needs a value")?),
            "--clip" => o.clip = Some(it.next().ok_or("--clip needs a value")?.clone()),
            "--clip-secs" => o.clip_secs = num(a, it.next())?,
            "--sizes" => {
                let v = it.next().ok_or("--sizes needs a value")?;
                o.sizes = v
                    .split(',')
                    .map(|x| {
                        x.trim()
                            .parse()
                            .map_err(|_| format!("--sizes: bad size '{x}'"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--n" => o.n = num(a, it.next())?,
            "--layout" => {
                let v = it.next().ok_or("--layout needs a value")?;
                o.layout = Layout::parse(v).ok_or("--layout: nas or stress")?;
            }
            "--radars" => o.radars = it.next().ok_or("--radars needs a value")?.clone(),
            "--adsb-frac" => o.adsb_frac = num(a, it.next())?,
            "--warmup" => o.warmup = num(a, it.next())?,
            "--duration" => o.duration = num(a, it.next())?,
            "--max-wall" => o.max_wall = num(a, it.next())?,
            "--max-tracks" => o.max_tracks = num(a, it.next())?,
            "--refine" => o.refine = num(a, it.next())?,
            "--seed" => o.seed = num(a, it.next())?,
            "--no-clip" => o.no_clip = true,
            "--no-overload" => o.no_overload = true,
            "--no-early-stop" => o.no_early_stop = true,
            other => return Err(format!("unknown flag '{other}'")),
        }
    }
    if o.sizes.is_empty() || o.sizes.contains(&0) {
        return Err("--sizes: sizes must be positive".to_string());
    }
    if o.warmup < 0 || o.duration < 1 || o.clip_secs < 1 {
        return Err("--warmup, --duration or --clip-secs out of range".to_string());
    }
    check_times(&o)?;
    parse_radars(&o.radars)?;
    Ok(o)
}

/// The capacity configuration for `n` aircraft.
pub fn cap_cfg(o: &Opts, n: usize) -> Result<CapCfg, String> {
    Ok(CapCfg {
        traffic: TrafficCfg {
            seed: o.seed,
            n,
            layout: o.layout,
            adsb_frac: o.adsb_frac,
            radars: parse_radars(&o.radars)?,
            horizon_s: o.warmup.saturating_add(o.duration) as f64 + HORIZON_PAD_S as f64,
        },
        warmup_s: o.warmup,
        measure_s: o.duration,
        max_tracks: o.max_tracks,
        max_wall_s: o.max_wall,
        early_stop: !o.no_early_stop,
    })
}

fn tag(o: &Opts, n: usize) -> String {
    format!("{}_n{n}", o.layout.name())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One capacity run in this process; writes `cap_<layout>_n<N>.{txt,kv}` and the per-second CSV.
fn one(o: &Opts, n: usize) -> Result<(), String> {
    let r = run_capacity(&cap_cfg(o, n)?)?;
    let text = cap_text(&r);
    print!("{text}");
    let t = tag(o, n);
    write(&o.out.join(format!("cap_{t}.txt")), &text)?;
    write(&o.out.join(format!("cap_{t}.kv")), &cap_kv(&r).render())?;
    write(&o.out.join(format!("seconds_{t}.csv")), &seconds_csv(&r))
}

/// Starts this binary as `one` for `n` and returns the result file it wrote.
fn child(o: &Opts, n: usize) -> Result<Kv, String> {
    let mut o = o.clone();
    o.no_early_stop = o.no_early_stop || o.cmd == "overload";
    let o = &o;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let st = Command::new(exe)
        .arg("one")
        .args(["--n", &n.to_string()])
        .args(["--out", &o.out.to_string_lossy()])
        .args(["--layout", o.layout.name()])
        .args(["--radars", &o.radars])
        .args(["--adsb-frac", &o.adsb_frac.to_string()])
        .args(["--warmup", &o.warmup.to_string()])
        .args(["--duration", &o.duration.to_string()])
        .args(["--max-wall", &o.max_wall.to_string()])
        .args(["--max-tracks", &o.max_tracks.to_string()])
        .args(["--seed", &o.seed.to_string()])
        .args(if o.no_early_stop {
            vec!["--no-early-stop"]
        } else {
            vec![]
        })
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!("the run at n={n} failed: {st}"));
    }
    let p = o.out.join(format!("cap_{}.kv", tag(o, n)));
    Ok(Kv::parse(
        &std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
    ))
}

fn flag(kv: &Kv, k: &str) -> bool {
    kv.get(k) == Some("1")
}

fn row_line(n: usize, kv: &Kv) -> String {
    let f = |k: &str| kv.get_f64(k).unwrap_or(f64::NAN);
    let moved: Vec<String> = kv
        .rows
        .iter()
        .filter_map(|(k, v)| {
            let name = k.strip_prefix("loss.")?;
            (v.parse::<u64>().ok()? > 0).then(|| format!("{name}={v}"))
        })
        .collect();
    format!(
        "{n:>6} {:>7.3} {:>9} {:>9} {:>10} {:>9.0} {:>6} {:>6}  {}\n",
        f("realtime_ratio"),
        fmt_f(f("advance.p99_ms")),
        fmt_f(f("advance.max_ms")),
        fmt_f(f("age_tick_rows.p99_ms")),
        f("vmhwm_kb") / 1024.0,
        if flag(kv, "keeps_up") {
            "yes"
        } else if flag(kv, "in_time") {
            "LOST"
        } else {
            "NO"
        },
        if flag(kv, "truncated") {
            "cut"
        } else if flag(kv, "stopped_early") {
            "early"
        } else {
            ""
        },
        if moved.is_empty() {
            "-".to_string()
        } else {
            moved.join(" ")
        }
    )
}

const TABLE_HEAD: &str =
    "     N  x-real  adv-p99ms adv-max-ms  age-p99ms(tick) HWM-MB keeps  note  work-loss counters moved\n";

/// The sweep, knee and overload. Returns the summary text.
fn sweep(o: &Opts, kvs: &mut Kv) -> Result<String, String> {
    let mut results: Vec<(usize, Kv)> = Vec::new();
    for &n in &o.sizes {
        let kv = child(o, n)?;
        results.push((n, kv));
    }
    // Bisect between the largest size that keeps up and the next failing one above it.
    for _ in 0..o.refine {
        results.sort_by_key(|r| r.0);
        let ok = results
            .iter()
            .filter(|r| flag(&r.1, "keeps_up"))
            .map(|r| r.0)
            .max();
        let Some(ok) = ok else { break };
        let Some(fail) = results.iter().filter(|r| r.0 > ok).map(|r| r.0).min() else {
            break;
        };
        let mid = ok / 2 + fail / 2;
        let mid = mid / 100 * 100;
        if mid <= ok || mid >= fail {
            break;
        }
        let kv = child(o, mid)?;
        results.push((mid, kv));
    }
    results.sort_by_key(|r| r.0);
    let knee = results
        .iter()
        .filter(|r| flag(&r.1, "keeps_up"))
        .map(|r| r.0)
        .max();
    let knee_strict = results
        .iter()
        .filter(|r| flag(&r.1, "keeps_up_strict"))
        .map(|r| r.0)
        .max();
    let mut s = format!(
        "== capacity sweep, layout {} [loaded machine, preliminary; load now {}] ==\n{TABLE_HEAD}",
        o.layout.name(),
        loadavg()
    );
    for (n, kv) in &results {
        s.push_str(&row_line(*n, kv));
        for (k, v) in &kv.rows {
            kvs.put(&format!("sweep.{}.n{n}.{k}", o.layout.name()), v);
        }
    }
    s.push_str(&format!(
        "knee (largest size where every measured second's advance took under 1 s and no work was lost): {}\n",
        knee.map_or("none of the sizes keeps up".to_string(), |k| k.to_string())
    ));
    s.push_str(&format!(
        "strict knee (same test, kept for result files): {}\n",
        knee_strict.map_or("none".to_string(), |k| k.to_string())
    ));
    kvs.put(&format!("knee.{}", o.layout.name()), knee.unwrap_or(0));
    kvs.put(
        &format!("knee_strict.{}", o.layout.name()),
        knee_strict.unwrap_or(0),
    );
    if !o.no_overload {
        if let Some(k) = knee {
            let n2 = k.saturating_mul(2);
            s.push_str(&format!("\n== overload: twice the knee, n={n2} ==\n"));
            let mut oo = o.clone();
            oo.cmd = "overload".to_string();
            let kv2 = child(&oo, n2)?;
            s.push_str(TABLE_HEAD);
            if let Some((_, kv1)) = results.iter().find(|r| r.0 == k) {
                s.push_str(&row_line(k, kv1));
                s.push_str(&compare(kv1, &kv2));
            }
            s.push_str(&row_line(n2, &kv2));
            for (kk, v) in &kv2.rows {
                kvs.put(&format!("overload.{}.n{n2}.{kk}", o.layout.name()), v);
            }
        } else {
            s.push_str("\nno knee, so no overload run\n");
        }
    }
    Ok(s)
}

/// What changed between the knee run and the overload run, in plain words.
fn compare(a: &Kv, b: &Kv) -> String {
    let mut s = String::from("  overload against the knee run:\n");
    let g = |kv: &Kv, k: &str| kv.get_f64(k).unwrap_or(f64::NAN);
    s.push_str(&format!(
        "    time per traffic second {:.3} -> {:.3} x real; p99 advance {} -> {} ms; max {} -> {} ms\n",
        g(a, "realtime_ratio"),
        g(b, "realtime_ratio"),
        fmt_f(g(a, "advance.p99_ms")),
        fmt_f(g(b, "advance.p99_ms")),
        fmt_f(g(a, "advance.max_ms")),
        fmt_f(g(b, "advance.max_ms"))
    ));
    s.push_str(&format!(
        "    memory: peak {:.0} -> {:.0} MB; buffers unchanged: {} -> {}\n",
        g(a, "vmhwm_kb") / 1024.0,
        g(b, "vmhwm_kb") / 1024.0,
        flag(a, "buffers_stable"),
        flag(b, "buffers_stable")
    ));
    let mut any = false;
    for (name, _) in loss_counters(&Default::default(), &Default::default(), 0) {
        // The output rows have their own line below.
        if name.starts_with("out.") {
            continue;
        }
        let (x, y) = (g(a, &format!("loss.{name}")), g(b, &format!("loss.{name}")));
        if x > 0.0 || y > 0.0 {
            s.push_str(&format!("    {name}: {x:.0} -> {y:.0}\n"));
            any = true;
        }
    }
    let z = |kv: &Kv, k: &str| kv.get_f64(k).unwrap_or(0.0);
    if z(a, "out_overflow") > 0.0 || z(b, "out_overflow") > 0.0 {
        s.push_str(&format!(
            "    output rows lost to a full table: {:.0} -> {:.0}\n",
            z(a, "out_overflow"),
            z(b, "out_overflow")
        ));
        any = true;
    }
    if !any {
        s.push_str("    no refusal or capacity counter moved\n");
    }
    s.push_str(&format!(
        "    tracks held at the end: {:.0} -> {:.0}; data age p99 of tick rows {} -> {} ms\n",
        g(a, "tracks_confirmed"),
        g(b, "tracks_confirmed"),
        fmt_f(g(a, "age_tick_rows.p99_ms")),
        fmt_f(g(b, "age_tick_rows.p99_ms"))
    ));
    s
}

fn clip_part(o: &Opts, kvs: &mut Kv) -> Result<String, String> {
    let Some(path) = o.clip.clone() else {
        return Ok("clip latency skipped: OPENATCA_ADSB_CLIP (or --clip) is not set\n".to_string());
    };
    // The clip may come from the environment: it is checked here, before it is read, as is the output.
    oa_sealed::refuse(std::path::Path::new(&path))?;
    oa_sealed::refuse(&o.out)?;
    let r = run_clip(&ClipCfg {
        path,
        secs: o.clip_secs,
        seed: 500 + o.seed,
    })?;
    let text = clip_text(&r);
    write(&o.out.join("clip.txt"), &text)?;
    let kv = clip_kv(&r);
    write(&o.out.join("clip.kv"), &kv.render())?;
    for (k, v) in &kv.rows {
        kvs.put(&format!("clip.{k}"), v);
    }
    Ok(text)
}

fn header(o: &Opts) -> String {
    format!(
        "oa-perf {} | cpu: {} | nproc {} | load before: {}\nlayout {} radars {} adsb-frac {} warm-up {} s, measured {} s per run, seed {}\n",
        o.cmd,
        cpu_model(),
        nproc(),
        loadavg(),
        o.layout.name(),
        o.radars,
        o.adsb_frac,
        o.warmup,
        o.duration,
        o.seed
    )
}

/// Runs the command line.
pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let o = parse_args(args).map_err(|e| format!("{e}\n{USAGE}"))?;
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    if o.cmd == "one" {
        return one(&o, o.n);
    }
    let mut kvs = Kv::default();
    let mut summary = header(&o);
    print!("{summary}");
    kvs.put("cpu_model", cpu_model());
    kvs.put("nproc", nproc());
    kvs.put("load_before", loadavg());
    if (o.cmd == "all" || o.cmd == "clip") && !(o.no_clip && o.cmd == "all") {
        let t = clip_part(&o, &mut kvs)?;
        print!("{t}");
        summary.push_str(&t);
    }
    if o.cmd == "all" || o.cmd == "sweep" {
        let t = sweep(&o, &mut kvs)?;
        print!("{t}");
        summary.push_str(&t);
    }
    let tail = format!("load after: {}\n", loadavg());
    print!("{tail}");
    summary.push_str(&tail);
    kvs.put("load_after", loadavg());
    write(&o.out.join("summary.txt"), &summary)?;
    write(&o.out.join("results.kv"), &kvs.render())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    // REQ: OA-PRF-007
    #[test]
    fn flags_are_parsed_and_bad_ones_refused() {
        let o = parse_args(&a(
            "sweep --sizes 10,20 --layout nas --duration 5 --warmup 0",
        ))
        .unwrap();
        assert_eq!(
            (o.sizes.clone(), o.layout, o.duration, o.warmup),
            (vec![10, 20], Layout::Nas, 5, 0)
        );
        assert!(parse_args(&a("sweep --sizes 0")).is_err());
        assert!(parse_args(&a("one --no-early-stop")).unwrap().no_early_stop);
        assert!(parse_args(&a("sweep --bogus")).is_err());
        assert!(parse_args(&a("nothing")).is_err());
        assert!(parse_args(&a("one --radars e,x")).is_err());
        assert!(parse_args(&a("one --n")).is_err());
        assert_eq!(cap_cfg(&o, 20).unwrap().traffic.n, 20);
    }

    // REQ: OA-PRF-007
    #[test]
    fn the_knee_comparison_reads_the_counters() {
        let a = Kv::parse(
            "realtime_ratio=0.5\nloss.fusion.bucket_work_capped=0\nvmhwm_kb=2048\nbuffers_stable=1\n",
        );
        let b = Kv::parse("realtime_ratio=1.5\nloss.fusion.bucket_work_capped=7\nvmhwm_kb=4096\nbuffers_stable=1\nout_overflow=3\n");
        let t = compare(&a, &b);
        assert!(t.contains("0.500 -> 1.500"), "{t}");
        assert!(t.contains("fusion.bucket_work_capped: 0 -> 7"), "{t}");
        assert!(
            t.contains("output rows lost to a full table: 0 -> 3"),
            "{t}"
        );
        assert!(t.contains("2 -> 4 MB"), "{t}");
    }

    // REQ: OA-PRF-010
    #[test]
    fn timing_and_float_values_that_cannot_work_are_refused() {
        let bad = [
            "one --max-wall NaN",
            "one --max-wall inf",
            "one --max-wall -inf",
            "one --max-wall 0",
            "one --max-wall -5",
            "one --adsb-frac NaN",
            "one --adsb-frac 1.5",
            "one --adsb-frac -0.1",
            "one --warmup 9223372036854775807 --duration 1",
            "one --warmup 9223372036854775807 --duration 9223372036854775807",
            "one --warmup 0 --duration 9223372036854775807",
            "one --warmup 3000000000 --duration 1",
            "one --clip-secs 9223372036854775807",
            "one --warmup -1",
            "one --duration 0",
        ];
        for b in bad {
            let e = parse_args(&a(b)).expect_err(b);
            assert!(!e.is_empty() && e.starts_with("--"), "{b}: {e}");
        }
        assert!(parse_args(&a("one --max-wall 0.5 --adsb-frac 0 --warmup 0")).is_ok());
        assert!(parse_args(&a("one --adsb-frac 1")).is_ok());
    }
}
