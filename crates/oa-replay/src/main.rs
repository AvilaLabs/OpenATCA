#![forbid(unsafe_code)]
//! `oa-replay`: merges recorded SWIM feeds into one ordered stream, replays it through the
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
    add_errors, errors_total, frames_total, load_files, merge_order, new_pipeline,
    rows_applied_total, run_loaded, snapshot, state_hash, Checkpoint, ErrKind, Pipeline, ERR_COUNT,
    ERR_NAMES, FEED_NAMES, NS_PER_S,
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
    let mut loaded = match load_files(&cmd.files, cmd.chunk) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("oa-replay: {e}");
            return ExitCode::from(1);
        }
    };
    merge_order(&mut loaded);
    let mut p = match new_pipeline(cmd.cfg) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("oa-replay: bad settings: {e:?}");
            return ExitCode::from(2);
        }
    };
    for k in [
        ErrKind::FrameBadMagic,
        ErrKind::FrameUnknownKind,
        ErrKind::FrameOversize,
        ErrKind::FrameCrc,
        ErrKind::FrameTruncated,
        ErrKind::GzipDamaged,
        ErrKind::GzipTruncated,
    ] {
        add_errors(
            &mut p,
            k,
            loaded.errors.get(k as usize).copied().unwrap_or(0),
        );
    }
    let t0 = Instant::now();
    run_loaded(&mut p, &loaded);
    let elapsed = t0.elapsed();

    let mut out = std::io::stdout().lock();
    if !cmd.quiet {
        for c in &p.checkpoints {
            let _ = writeln!(out, "{}", checkpoint_line(c));
        }
    }
    let _ = write!(out, "{}", summary(&p, loaded.files, loaded.bytes));
    let secs = elapsed.as_secs_f64().max(1e-9);
    let rows = rows_applied_total(&p);
    let _ = writeln!(
        out,
        "timing elapsed_s={secs:.3} frames_per_s={:.0} rows_per_s={:.0}",
        frames_total(&p) as f64 / secs,
        rows as f64 / secs
    );
    ExitCode::SUCCESS
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
            "feed {name} frames={} messages={} other_frames={} rows={} applied={} out_of_order={} refused_full={}\n",
            g(&p.frames),
            g(&p.messages),
            g(&p.other_frames),
            g(&p.rows_seen),
            g(&p.rows_applied),
            g(&p.rows_out_of_order),
            g(&p.rows_refused_full),
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
        to_hex(&state_hash(&p.table))
    ));
    s
}
