//! The text `oa-replay` prints: checkpoint lines, the summary, the timing line, and the writes
//! that put them on stdout. Shell (B1): the functions here write to whatever [`Write`] they are
//! given, and a failed write is returned, never ignored, so a run whose output is lost (a closed
//! pipe, a full disk) does not end as a success.

use std::io::{self, Write};

use oa_sha256::to_hex;

use crate::{
    errors_total, frames_total, pipeline_try_drain_checkpoints, replay_hash, replay_state_digest,
    snapshot, Checkpoint, FeedKind, Pipeline, ERR_COUNT, ERR_NAMES, FEED_NAMES, NS_PER_S,
};

/// Writes the held checkpoints to `out`, one line each, oldest first, taking each out of the
/// ring only after its line was written: on a write error the failed checkpoint stays held and
/// the error is returned. With `quiet` the checkpoints are taken out and not printed.
pub fn drain_checkpoint_lines(
    p: &mut Pipeline,
    out: &mut dyn Write,
    quiet: bool,
) -> io::Result<()> {
    pipeline_try_drain_checkpoints(p, |c| {
        if quiet {
            Ok(())
        } else {
            writeln!(out, "{}", checkpoint_line(c))
        }
    })
}

/// Writes the summary and the timing line, and flushes.
pub fn write_summary(
    p: &Pipeline,
    out: &mut dyn Write,
    files: usize,
    bytes: u64,
    elapsed_s: f64,
    peak_rss_kb: u64,
) -> io::Result<()> {
    write!(out, "{}", summary(p, files, bytes))?;
    let secs = elapsed_s.max(1e-9);
    writeln!(
        out,
        "timing elapsed_s={secs:.3} frames_per_s={:.0} rows_per_s={:.0} peak_rss_kb={peak_rss_kb}",
        frames_total(p) as f64 / secs,
        crate::rows_applied_total(p) as f64 / secs,
    )?;
    out.flush()
}

/// The message for a failed write to stdout.
pub fn write_failure_message(e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::BrokenPipe {
        "oa-replay: output closed (broken pipe), run stopped".to_string()
    } else {
        format!("oa-replay: cannot write output: {e}")
    }
}

pub fn checkpoint_line(c: &Checkpoint) -> String {
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

pub fn summary(p: &Pipeline, files: usize, bytes: u64) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "summary files={files} bytes={bytes} frames={} checkpoints={} checkpoints_dropped={}\n",
        frames_total(p),
        p.checkpoints.recorded(),
        p.checkpoints.dropped()
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
            "clock held={} forward_steps={} backward_steps={} stale_marked={} stale_now={}\n",
            c.held,
            c.forward_steps,
            c.backward_steps,
            p.stale_marked,
            crate::pipeline_stale_count(p)
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
    s.push_str(&format!(
        "final state_digest={}\n",
        to_hex(&replay_state_digest(p))
    ));
    s
}
