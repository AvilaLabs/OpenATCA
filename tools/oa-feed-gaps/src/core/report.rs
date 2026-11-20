//! Core (B1): the report text and the verdict.

use super::clip::{clip_reasons, ClipLog};
use super::exclude::{excluded, too_much, total_ns, LIMIT_NS, MARGIN_NS};
use super::gaps::{reasons, Gap, Stats, Window, GAP_CAP, OVER_NS};
use super::timefmt::{iso, secs};

/// One feed's result and what its reader counted.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Feed {
    /// `SFDPS` or `ADS-B`.
    pub name: &'static str,
    pub stats: Stats,
    /// Damage counts by name; a non-zero one makes the feed damaged.
    pub damage: Vec<(String, u64)>,
    /// Counts that are reported and not damage, by name.
    pub info: Vec<(String, u64)>,
}

fn gap_line(g: &Gap) -> String {
    format!(
        "{} from {} to {}",
        secs(g.len_ns()),
        iso(g.start_ns),
        iso(g.end_ns)
    )
}

fn feed_text(f: &Feed, out: &mut String) {
    let st = &f.stats;
    out.push_str(&format!("[{}]\n", f.name));
    for (k, v) in &f.info {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push_str(&format!("messages in window: {}\n", st.count));
    match (st.first_ns, st.last_ns) {
        (Some(a), Some(b)) => {
            out.push_str(&format!(
                "first message: {} ({} after the window start)\n",
                iso(a),
                secs(a - st.window.from_ns)
            ));
            out.push_str(&format!(
                "last message: {} ({} before the window end)\n",
                iso(b),
                secs(st.window.to_ns - b)
            ));
        }
        _ => out.push_str("first message: none\nlast message: none\n"),
    }
    match &st.largest {
        Some(g) => out.push_str(&format!("largest interval: {}\n", gap_line(g))),
        None => out.push_str("largest interval: none\n"),
    }
    out.push_str(&format!("intervals over 600 s: {}\n", st.over_total));
    for g in &st.over {
        out.push_str(&format!("  {}\n", gap_line(g)));
    }
    if st.overflow > 0 {
        out.push_str(&format!(
            "  ({} more over 600 s not listed; the list holds {GAP_CAP})\n",
            st.overflow
        ));
    }
    out.push_str(&format!("out-of-order times: {}\n", st.out_of_order));
    for (k, v) in &f.damage {
        out.push_str(&format!("{k}: {v}\n"));
    }
}

/// The report text and whether the day is damaged.
pub fn render(window: Window, feeds: &[Feed], clip: Option<&ClipLog>) -> (String, bool) {
    let mut out = String::new();
    out.push_str("oa-feed-gaps report (P-006 section 6)\n");
    out.push_str(&format!(
        "window: {} to {} ({})\n",
        iso(window.from_ns),
        iso(window.to_ns),
        secs(window.to_ns - window.from_ns)
    ));
    out.push_str(&format!(
        "limit: {}; an interval over it is damage\n",
        secs(OVER_NS)
    ));
    let mut why = Vec::new();
    for f in feeds {
        feed_text(f, &mut out);
        why.extend(reasons(f.name, &f.stats, &f.damage));
    }
    let stats: Vec<&Stats> = feeds.iter().map(|f| &f.stats).collect();
    let merged = excluded(window, &stats);
    out.push_str(&format!(
        "excluded intervals (each over 600 s and the {} after it, merged): {}, {} of the {} allowed\n",
        secs(MARGIN_NS),
        merged.len(),
        secs(total_ns(&merged)),
        secs(LIMIT_NS)
    ));
    for g in &merged {
        out.push_str(&format!("exclude {} {}\n", iso(g.start_ns), iso(g.end_ns)));
    }
    if too_much(&merged) {
        why.push(format!(
            "excluded intervals total {}, over {}",
            secs(total_ns(&merged)),
            secs(LIMIT_NS)
        ));
    }
    match clip {
        Some(c) => {
            out.push_str(&format!(
                "clip log: traces_refused={} archive_complete={}\n",
                c.traces_refused
                    .map_or("none".to_string(), |n| n.to_string()),
                c.complete
            ));
            why.extend(clip_reasons(c));
        }
        None => out.push_str("clip checks: not made (no --clip-log)\n"),
    }
    let damaged = !why.is_empty();
    if damaged {
        out.push_str(&format!("DAMAGED: {}\n", why.join("; ")));
    } else {
        out.push_str("CLEAN\n");
    }
    (out, damaged)
}
