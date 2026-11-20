//! What the harness reads of the machine: load, CPU, memory. Shell code (reads /proc).

/// `/proc/loadavg` as one line ("1.20 1.50 1.10 ..."), or "unknown".
pub fn loadavg() -> String {
    // seal-allow: /proc, not an input
    std::fs::read_to_string("/proc/loadavg")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// The first three load averages only.
pub fn loadavg3() -> String {
    loadavg()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first CPU model name in `/proc/cpuinfo`.
pub fn cpu_model() -> String {
    // seal-allow: /proc, not an input
    let text = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    text.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map_or_else(|| "unknown".to_string(), |(_, v)| v.trim().to_string())
}

/// Logical CPUs available to this process.
pub fn nproc() -> usize {
    std::thread::available_parallelism().map_or(0, |n| n.get())
}

/// A `kB` field of `/proc/self/status` (`VmHWM`, `VmRSS`), in kilobytes.
pub fn status_kb(field: &str) -> Option<u64> {
    // seal-allow: /proc, not an input
    status_kb_of(&std::fs::read_to_string("/proc/self/status").ok()?, field)
}

/// [`status_kb`] over given text.
pub fn status_kb_of(text: &str, field: &str) -> Option<u64> {
    let line = text
        .lines()
        .find(|l| l.strip_prefix(field).is_some_and(|r| r.starts_with(':')))?;
    line.split_whitespace().nth(1)?.parse().ok()
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

    // REQ: OA-PRF-005
    #[test]
    fn status_fields_are_read_in_kilobytes() {
        let t = "Name:\tx\nVmHWM:\t  1234 kB\nVmRSS:\t   99 kB\nVmHWMX:\t5 kB\n";
        assert_eq!(status_kb_of(t, "VmHWM"), Some(1234));
        assert_eq!(status_kb_of(t, "VmRSS"), Some(99));
        assert_eq!(status_kb_of(t, "VmSwap"), None);
        // The live process has a resident size.
        assert!(status_kb("VmRSS").unwrap_or(0) > 0);
    }
}
