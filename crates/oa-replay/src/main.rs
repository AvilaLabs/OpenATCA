#![forbid(unsafe_code)]
//! `oa-replay`: merges recorded SWIM feeds and ADS-B clips into one ordered stream, replays it through the
//! flight-state pipeline and prints the checkpoints and a final summary. Shell (B1): reads
//! files, reads a clock for the throughput line, writes stdout.
//!
//! Every line except the one starting `timing` is a function of the recordings and the settings
//! alone, so two runs over the same files print identical text there (RULESET B6). Exit status:
//! 0 after a complete replay (failures inside the data are counted in the summary, not fatal),
//! 1 if a file cannot be read, 2 for a usage error.

use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use oa_replay::cli::{parse_args, USAGE};
use oa_replay::{
    errors_total, frames_total, list_recordings, new_pipeline, replay_hash, rows_applied_total,
    run_streams, snapshot, spec_from_paths, Checkpoint, FeedKind, Pipeline, ERR_COUNT, ERR_NAMES,
    FEED_NAMES, NS_PER_S,
};
use oa_sha256::to_hex;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oa-replay: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut specs = Vec::with_capacity(cmd.inputs.len());
    for i in &cmd.inputs {
        let paths = if i.dir {
            match list_recordings(&i.path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("oa-replay: {e}");
                    return ExitCode::from(1);
                }
            }
        } else {
            vec![i.path.clone()]
        };
        specs.push(spec_from_paths(i.feed, paths));
    }
    let mut p = match new_pipeline(cmd.cfg) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("oa-replay: bad settings: {e:?}");
            return ExitCode::from(2);
        }
    };
    let t0 = Instant::now();
    let mut out = std::io::stdout().lock();
    let mut printed = 0usize;
    let mut first_ns = i64::MIN;
    let mut next_progress = i64::MIN;
    let quiet = cmd.quiet;
    let progress_ns = cmd.progress_ns;
    let mut hook = |p: &Pipeline| {
        if !quiet {
            while let Some(c) = p.checkpoints.get(printed) {
                let _ = writeln!(out, "{}", checkpoint_line(c));
                printed += 1;
            }
        }
        if first_ns == i64::MIN {
            first_ns = p.replay_ns;
            next_progress = p.replay_ns.saturating_add(progress_ns);
        } else if p.replay_ns >= next_progress {
            next_progress = p.replay_ns.saturating_add(progress_ns);
            let wall = t0.elapsed().as_secs_f64().max(1e-9);
            let replayed = (p.replay_ns - first_ns) as f64 / NS_PER_S as f64;
            eprintln!(
                "progress replay_s={} frames={} frames_per_s={:.0} speed={:.0}x rss_mb={:.0}",
                p.replay_ns.div_euclid(NS_PER_S),
                frames_total(p),
                frames_total(p) as f64 / wall,
                replayed / wall,
                status_kb("VmRSS:") as f64 / 1024.0
            );
        }
    };
    let report = match run_streams(&mut p, specs, cmd.chunk, &mut hook) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("oa-replay: {e}");
            return ExitCode::from(1);
        }
    };
    let elapsed = t0.elapsed();
    let _ = write!(out, "{}", summary(&p, report.files, report.bytes));
    let secs = elapsed.as_secs_f64().max(1e-9);
    let rows = rows_applied_total(&p);
    let _ = writeln!(
        out,
        "timing elapsed_s={secs:.3} frames_per_s={:.0} rows_per_s={:.0} peak_rss_kb={}",
        frames_total(&p) as f64 / secs,
        rows as f64 / secs,
        status_kb("VmHWM:")
    );
    ExitCode::SUCCESS
}

/// A `kB` figure from `/proc/self/status` (0 where there is none): resident or peak memory.
fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn checkpoint_line(c: &Checkpoint) -> String {
    format!(
        "checkpoint t_ns={} t_s={} frames={} rows={} flights={} hash={}",
        c.replay_ns,
        c.replay_ns.div_euclid(NS_PER_S),
        c.frames,
        c.rows_applied,
        c.flights,
        to_hex(&c.hash)
    )
}

fn adsb_used(p: &Pipeline) -> bool {
    p.frames.get(FeedKind::Adsb as usize).copied().unwrap_or(0) > 0
}

fn summary(p: &Pipeline, files: usize, bytes: u64) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "summary files={files} bytes={bytes} frames={} checkpoints={}\n",
        frames_total(p),
        p.checkpoints.len()
    ));
    for (i, name) in FEED_NAMES.iter().enumerate() {
        let g = |a: &[u64]| a.get(i).copied().unwrap_or(0);
        s.push_str(&format!(
            "feed {name} frames={} messages={} other_frames={} not_applied={} rows={} applied={} out_of_order={} refused_full={}\n",
            g(&p.frames),
            g(&p.messages),
            g(&p.other_frames),
            g(&p.not_applied),
            g(&p.rows_seen),
            g(&p.rows_applied),
            g(&p.rows_out_of_order),
            g(&p.rows_refused_full),
        ));
    }
    let c = &p.clock;
    if c.held > 0 || c.forward_steps > 0 || c.backward_steps > 0 {
        s.push_str(&format!(
            "clock held={} forward_steps={} backward_steps={}\n",
            c.held, c.forward_steps, c.backward_steps
        ));
    }
    let st = &p.table.stats;
    s.push_str(&format!(
        "tbfm linked_gufi={} linked_tma={} created={} unlinked={} promoted={}\n",
        st.tbfm_linked_gufi, st.tbfm_linked_tma, st.tbfm_created, st.tbfm_unlinked, st.promoted
    ));
    s.push_str(&format!(
        "table live={} capacity={} evicted={} sweeps={} refused_full={} out_of_order={} conflicts={} key_reassigned={}\n",
        p.table.live,
        p.table.capacity,
        st.evicted,
        st.sweeps,
        st.refused_full,
        st.out_of_order,
        st.conflicts,
        st.key_reassigned
    ));
    if adsb_used(p) {
        let a = &p.adsb.stats;
        s.push_str(&format!(
            "adsb aircraft={} capacity={} applied={} out_of_order={} refused_full={}\n",
            p.adsb.live, p.adsb.capacity, a.applied, a.out_of_order, a.refused_full
        ));
        if a.evicted > 0 || a.future_refused > 0 {
            s.push_str(&format!(
                "adsb evicted={} future_refused={}\n",
                a.evicted, a.future_refused
            ));
        }
    }
    let fed = |f: FeedKind| p.frames.get(f as usize).copied().unwrap_or(0) > 0;
    if fed(FeedKind::Tfms) {
        let t = &p.tfms.stats;
        s.push_str(&format!(
            "tfms flights={} capacity={} applied={} out_of_order={} refused_full={} no_ref={} \
             linked_plan={} linked_acid={} ambiguous={} conflicts={} evicted={}\n",
            p.tfms.slots.live,
            p.tfms.slots.capacity,
            t.applied,
            t.out_of_order,
            t.refused_full,
            t.no_ref,
            t.linked_plan,
            t.linked_acid,
            t.ambiguous,
            t.conflicts,
            t.evicted
        ));
        let w = &p.flows.stats;
        s.push_str(&format!(
            "tfms programmes={} capacity={} applied={} out_of_order={} refused_full={} evicted={}\n",
            p.flows.slots.live,
            p.flows.slots.capacity,
            w.applied,
            w.out_of_order,
            w.refused_full,
            w.evicted
        ));
    }
    if fed(FeedKind::Fns) {
        let n = &p.fns.stats;
        s.push_str(&format!(
            "fns notams={} capacity={} applied={} created={} replaced={} cancelled={} \
             cancel_unmatched={} out_of_order={} refused_full={} no_key={} expired={}\n",
            p.fns.slots.live,
            p.fns.slots.capacity,
            n.applied,
            n.created,
            n.replaced,
            n.cancelled,
            n.cancel_unmatched,
            n.out_of_order,
            n.refused_full,
            n.no_key,
            n.expired
        ));
    }
    if errors_total(&p.errors) == 0 {
        s.push_str("errors none\n");
    } else {
        s.push_str("errors");
        for k in 0..ERR_COUNT {
            let n = p.errors.get(k).copied().unwrap_or(0);
            if n > 0 {
                s.push_str(&format!(
                    " {}={n}",
                    ERR_NAMES.get(k).copied().unwrap_or("?")
                ));
            }
        }
        s.push('\n');
    }
    let fin = snapshot(p);
    s.push_str(&format!(
        "final t_ns={} flights={} state_hash={}\n",
        fin.replay_ns,
        fin.flights,
        to_hex(&replay_hash(p))
    ));
    s
}
