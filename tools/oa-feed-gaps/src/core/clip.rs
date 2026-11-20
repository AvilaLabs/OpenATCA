//! Core (B1): what `oa-adsb-clip` prints, read back. The tool writes its summary to stderr: one
//! line `traces_read=N traces_refused=N ... kept=N frames=N` and one line `archive: complete` or
//! `archive: TRUNCATED at byte ...`. The log is the text of that output. `kept` counts the rows
//! the tool put in the clip (sealed-window rows are not in it), the same rows the loader decodes
//! back out of the clip.

/// What the clip tool's log says.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipLog {
    /// Lines with a `traces_refused=` field; a log of one run has exactly one.
    pub summaries: u64,
    /// The `traces_refused=N` of the first summary line.
    pub traces_refused: Option<u64>,
    /// The `kept=N` of the first summary line.
    pub kept: Option<u64>,
    /// Lines starting `archive:`; a log of one run has exactly one.
    pub archive_lines: u64,
    /// An `archive: complete` line.
    pub complete: bool,
    /// An `archive: TRUNCATED` line.
    pub truncated: bool,
}

fn field(line: &str, name: &str) -> Option<u64> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(name))
        .and_then(|n| n.parse().ok())
}

/// Reads the log text.
pub fn parse_clip_log(text: &str) -> ClipLog {
    let mut log = ClipLog {
        summaries: 0,
        traces_refused: None,
        kept: None,
        archive_lines: 0,
        complete: false,
        truncated: false,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("archive:") {
            log.archive_lines += 1;
            log.complete |= line == "archive: complete";
            log.truncated |= line.starts_with("archive: TRUNCATED");
        }
        if line
            .split_whitespace()
            .any(|w| w.starts_with("traces_refused="))
        {
            log.summaries += 1;
            if log.summaries == 1 {
                log.traces_refused = field(line, "traces_refused=");
                log.kept = field(line, "kept=");
            }
        }
    }
    log
}

/// The reasons the clip is damaged. `decoded` is the number of reports read back from the clip.
pub fn clip_reasons(log: &ClipLog, decoded: u64) -> Vec<String> {
    let mut out = Vec::new();
    if log.summaries != 1 {
        out.push(format!(
            "clip log has {} summary lines, expected one",
            log.summaries
        ));
    }
    if log.summaries >= 1 {
        match log.traces_refused {
            None => out.push("clip log summary has no readable traces_refused".to_string()),
            Some(n) if n > 0 => out.push(format!("clip tool refused {n} trace(s)")),
            Some(_) => {}
        }
        match log.kept {
            None => out.push("clip log summary has no readable kept".to_string()),
            Some(k) if k != decoded => out.push(format!(
                "clip log kept={k} but the clip holds {decoded} reports (a log of another clip?)"
            )),
            Some(_) => {}
        }
    }
    if log.archive_lines != 1 {
        out.push(format!(
            "clip log has {} archive: lines, expected one",
            log.archive_lines
        ));
    } else if log.truncated {
        out.push("clip log says the archive is TRUNCATED".to_string());
    } else if !log.complete {
        out.push("clip log does not say the archive is complete".to_string());
    }
    out
}
