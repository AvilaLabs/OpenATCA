//! Core (B1): what `oa-adsb-clip` prints, read back. The tool writes its summary to stderr:
//! `traces_read=N traces_refused=N ...` and a last line `archive: complete` or `archive: TRUNCATED
//! at byte ...`. The log is the text of that output.

/// What the clip tool's log says.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipLog {
    /// The `traces_refused=N` of the summary line, `None` when the log has none.
    pub traces_refused: Option<u64>,
    /// An `archive: complete` line.
    pub complete: bool,
    /// An `archive: TRUNCATED` line.
    pub truncated: bool,
}

/// Reads the log text.
pub fn parse_clip_log(text: &str) -> ClipLog {
    let mut log = ClipLog {
        traces_refused: None,
        complete: false,
        truncated: false,
    };
    for line in text.lines() {
        let line = line.trim();
        if line == "archive: complete" {
            log.complete = true;
        } else if line.starts_with("archive: TRUNCATED") {
            log.truncated = true;
        }
        for w in line.split_whitespace() {
            if let Some(n) = w.strip_prefix("traces_refused=") {
                if let Ok(n) = n.parse::<u64>() {
                    log.traces_refused = Some(log.traces_refused.unwrap_or(0).saturating_add(n));
                }
            }
        }
    }
    log
}

/// The reasons the clip is damaged.
pub fn clip_reasons(log: &ClipLog) -> Vec<String> {
    let mut out = Vec::new();
    match log.traces_refused {
        None => out.push("clip log has no traces_refused summary".to_string()),
        Some(n) if n > 0 => out.push(format!("clip tool refused {n} trace(s)")),
        Some(_) => {}
    }
    if log.truncated {
        out.push("clip log says the archive is TRUNCATED".to_string());
    } else if !log.complete {
        out.push("clip log does not say the archive is complete".to_string());
    }
    out
}
