#![forbid(unsafe_code)]
//! `oa-sealed` (shell, B1): the one guard for the sealed hold-out data. The list of sealed paths
//! is `crates/oa-sealed/sealed_paths.txt`, compiled in; `tools/sealed_guard.py` reads the same file, so the
//! Rust and Python tools cannot drift. There is no environment variable, flag or file that
//! switches the guard off.
//!
//! A path is resolved before it is checked: relative paths, `..`, and every symlink on the way
//! (a symlinked parent directory included) are followed, and a path that does not exist yet is
//! resolved through its nearest existing ancestor. A symlink that cannot be resolved (loop,
//! dangling to nowhere readable) is refused. The check is made on the resolved path under the
//! data root, and by name on the path as given and as resolved (`scds` followed by a sealed
//! name), so a copy of the data under another root is refused as well. A directory that holds a
//! sealed entry (the data root, `scds`) is refused too: a tool must name something below it.
//! Nothing inside a sealed directory is listed or read to decide this.
//!
//! The in-root and holding-directory rules need the real data root to exist (they compare the
//! resolved path with the resolved root); the name rule needs nothing, so it still works in CI,
//! where there is no data root.
//!
//! [`Sealed::open`] checks, opens, and then asks the kernel which file the descriptor is
//! (`/proc/self/fd/N`) and checks that path too, so a link swapped in between the check and the
//! open is caught and the file closed. Where `/proc` is missing, only the check before the open
//! is made.
//!
//! The path rule is the list's `seal` lines. A last component `X*` is a name prefix; `*X*` matches
//! any component at or below that point that contains `X` (so `derived/*2026.11.21*` seals any
//! derived clip of that day, at any depth). The content rule is the list's `window` lines: a
//! half-open UTC span in which every row or frame is sealed, whatever file holds it. The guard
//! owns the windows ([`in_sealed_window`]); each decoder pre-scans a whole document (a whole
//! message for stdds) for a message time in one, before it touches any counter or row, and drops
//! the document whole, counting only its messages.
//!
//! The list on disk is compared with the compiled-in one when a *reader* starts (see
//! [`Sealed::production`]): a build whose list differs from `sealed_paths.txt` fails closed, and the
//! fix is to rebuild. Capture and write-only paths (the `oa-feed` recorder, which records and
//! never computes on what it reads) use [`Sealed::production_embedded`] and [`refuse_io_embedded`]
//! instead: they check against the compiled-in list and skip the comparison, so recording never
//! stops because the list file changed. A running recorder keeps the list it was built with until it
//! is restarted on a rebuilt binary; the readers of what it recorded refuse until they are rebuilt.
//!
//! Requirements: OA-SEAL-001 and the following, in REQUIREMENTS.md.

pub use oa_time_window::DropWindow;
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The list file, compiled in.
pub const LIST: &str = include_str!("../sealed_paths.txt");

/// Where [`LIST`] was read from at build time. At run time the guard compares the file there, if
/// there is one, with the compiled-in text ([`production_guard`]).
const LIST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/sealed_paths.txt");
const DEFAULT_DATA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../openatca-data");
const DATA_ROOT_SENTINEL: &str = ".openatca-data-root";
const DATA_ROOT_SENTINEL_BYTES: &[u8] = b"openatca-data-root-v1\n";

fn configured_root(
    root: &Path,
    data_root: Option<std::ffi::OsString>,
    default_data_root: &Path,
) -> Result<PathBuf, ()> {
    if root == Path::new("$OPENATCA_DATA") {
        return match data_root {
            Some(value) if !value.is_empty() => {
                let path = PathBuf::from(value);
                path.is_absolute().then_some(path).ok_or(())
            }
            Some(_) => Err(()),
            None => Ok(default_data_root.to_path_buf()),
        };
    }
    if root.is_absolute() {
        Ok(root.to_path_buf())
    } else {
        Err(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod data_root_tests {
    use super::{Sealed, DATA_ROOT_SENTINEL, DEFAULT_DATA_ROOT, LIST};
    use std::path::Path;
    #[path = "../tests/common/test_support.rs"]
    mod test_support;

    fn list_without_window() -> String {
        LIST.lines()
            .filter(|line| !line.starts_with("window"))
            .map(|line| format!("{line}\n"))
            .collect()
    }

    fn assert_fail_closed(guard: &Sealed, outside: &Path) {
        assert!(guard.broken);
        assert!(guard.check(outside).is_err());
        assert!(guard.in_window(i64::MIN) && guard.in_window(i64::MAX));
        assert_eq!(guard.drop_window(), oa_time_window::DropWindow::all_time());
    }

    #[test]
    fn default_root_is_relative_to_the_repository_sibling() {
        assert_eq!(
            std::path::PathBuf::from(DEFAULT_DATA_ROOT),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../openatca-data")
        );
        let configured = super::configured_root(
            Path::new("$OPENATCA_DATA"),
            None,
            Path::new(DEFAULT_DATA_ROOT),
        )
        .unwrap();
        assert_eq!(configured, Path::new(DEFAULT_DATA_ROOT));
    }

    #[test]
    fn configured_root_without_sentinel_keeps_embedded_guard_closed() {
        let parent = test_support::scratch("invalid-root");
        let root = parent.join("sentinelless");
        std::fs::create_dir(&root).unwrap();
        let guard = Sealed::with_list_using_data_root(LIST, None, Some(root.into_os_string()));
        assert_fail_closed(&guard, &parent.join("outside-unsealed-path"));
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn an_unset_root_uses_the_supplied_fallback_through_full_guard_validation() {
        let parent = test_support::scratch("fallback-root");

        let missing = parent.join("missing-default");
        let markerless = parent.join("wrong-default");
        std::fs::create_dir_all(&markerless).unwrap();
        let outside = parent.join("ordinary-outside-path");
        for fallback in [missing.as_path(), markerless.as_path(), Path::new("/")] {
            let guard = Sealed::with_list_using_data_root_and_default(LIST, None, None, fallback);
            assert_fail_closed(&guard, &outside);
        }

        let valid = parent.join("valid-default");
        std::fs::create_dir_all(valid.join("scds/rec-20261119/day2/sfdps")).unwrap();
        std::fs::write(valid.join(DATA_ROOT_SENTINEL), b"openatca-data-root-v1\n").unwrap();
        std::fs::write(
            valid.join("scds/rec-20261119/day2/sfdps/a.bin"),
            b"synthetic",
        )
        .unwrap();
        let guard = Sealed::with_list_using_data_root_and_default(LIST, None, None, &valid);
        assert!(!guard.broken);
        assert!(guard.check(&outside).is_ok());
        assert!(guard
            .check(&valid.join("scds/rec-20261119/day2/sfdps/a.bin"))
            .is_err());
        assert_ne!(guard.drop_window(), oa_time_window::DropWindow::all_time());
        std::fs::remove_dir_all(parent).unwrap();
    }
}

fn resolve_data_root(path: &Path) -> Result<PathBuf, ()> {
    let resolved = fs::canonicalize(path).map_err(|_| ())?;
    if resolved.parent().is_none() {
        return Err(());
    }
    // seal-allow: confirms the configured root itself is an existing directory, not a data input.
    if !fs::metadata(&resolved).map_err(|_| ())?.is_dir() {
        return Err(());
    }
    let sentinel = resolved.join(DATA_ROOT_SENTINEL);
    // seal-allow: checks only the fixed sentinel's filesystem type and length before opening it.
    let metadata = fs::symlink_metadata(&sentinel).map_err(|_| ())?;
    if !metadata.file_type().is_file() || metadata.len() != DATA_ROOT_SENTINEL_BYTES.len() as u64 {
        return Err(());
    }
    // seal-allow: the guard reads only its fixed root-identity sentinel before any data access.
    let file = File::open(sentinel).map_err(|_| ())?;
    if !file.metadata().map_err(|_| ())?.is_file() {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(DATA_ROOT_SENTINEL_BYTES.len() + 1);
    file.take((DATA_ROOT_SENTINEL_BYTES.len() + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    (bytes == DATA_ROOT_SENTINEL_BYTES)
        .then_some(resolved)
        .ok_or(())
}

/// FNV-1a 64 of `bytes`, in hex: a short name for a list in a warning (not a security hash).
fn list_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The guard for the compiled-in `embedded` list, checked against the list file at `path`: if the
/// file exists and its bytes differ from the embedded text (or it exists and cannot be read), the
/// list the binary was built with is not the list on disk, and the guard fails closed (every path
/// refused, every instant sealed) with a warning on `warn` naming both hashes. A file that is
/// absent (an installed binary, a copied one) leaves the embedded list in force.
fn production_guard(embedded: &str, path: &Path, warn: &mut dyn io::Write) -> Sealed {
    // seal-allow: the guard's own list file, compared with the compiled-in copy
    match fs::read(path) {
        Ok(disk) if disk != embedded.as_bytes() => {
            let _ = writeln!(
                warn,
                "seal list on disk ({}) differs from the list compiled in ({}) [{}]: every path refused and every timed row dropped; rebuild",
                list_hash(&disk),
                list_hash(embedded.as_bytes()),
                path.display()
            );
            Sealed::with_list("", None)
        }
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            let _ = writeln!(
                warn,
                "seal list on disk cannot be read ({e}) [{}]: every path refused and every timed row dropped",
                path.display()
            );
            Sealed::with_list("", None)
        }
        _ => Sealed::with_list(embedded, None),
    }
}

/// A deepest chain of links followed by hand before a path is refused as unresolvable.
const MAX_LINKS: usize = 40;

/// How the last component of an entry is matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Exact,
    /// `X*`: a name prefix.
    Prefix,
    /// `*X*`: any component at or below the directory part that contains `X`.
    Contains,
}

/// One sealed entry: directory components and a final name, exact, a prefix or a substring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The entry as written in the list.
    pub text: String,
    parts: Vec<String>,
    kind: Kind,
}

impl Entry {
    fn matches_at(&self, names: &[String]) -> bool {
        let n = self.parts.len();
        if names.len() < n {
            return false;
        }
        let Some((last, dirs)) = self.parts.split_last() else {
            return false;
        };
        if names.iter().zip(dirs).any(|(a, b)| a != b) {
            return false;
        }
        let from = n - 1;
        match self.kind {
            Kind::Exact => names.get(from).is_some_and(|a| a == last),
            Kind::Prefix => names
                .get(from)
                .is_some_and(|a| a.starts_with(last.as_str())),
            Kind::Contains => names
                .get(from..)
                .is_some_and(|t| t.iter().any(|a| a.contains(last.as_str()))),
        }
    }

    /// Whether `names` has the entry as consecutive components anywhere.
    fn named_in(&self, names: &[String]) -> bool {
        (0..names.len()).any(|i| names.get(i..).is_some_and(|t| self.matches_at(t)))
    }

    /// Whether `names` (below the root) is a proper ancestor directory of the entry.
    fn is_ancestor(&self, names: &[String]) -> bool {
        // A substring entry seals names, not a folder: its directory part is an ordinary folder.
        self.kind != Kind::Contains
            && names.len() < self.parts.len()
            && self.parts.iter().zip(names).all(|(a, b)| a == b)
    }
}

/// How far each listed `window` is widened on both sides, in nanoseconds: 15 minutes. It covers
/// rounding of a fraction, differences between a scan and a parser, and the lag between a
/// feed's own time and the time its frame was received (the SFDPS lag floor is 58 s). Data
/// within the margin of an edge is dropped and counted (OA-SEAL-011, OA-SEAL-014). The Python
/// guard (`tools/sealed_guard.py`) uses the same value.
pub const WINDOW_MARGIN_NS: i64 = 15 * 60 * 1_000_000_000;

/// A sealed span of time, half-open (the listed span widened by [`WINDOW_MARGIN_NS`] on each
/// side): `from_ns <= t < to_ns`, in nanoseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The span as written in the list (`<from> <to>`).
    pub text: String,
    pub from_ns: i64,
    pub to_ns: i64,
}

impl Window {
    /// Whether `unix_ns` is in the window.
    pub fn contains(&self, unix_ns: i64) -> bool {
        self.from_ns <= unix_ns && unix_ns < self.to_ns
    }
}

/// The sealed days: a path below (or anywhere as consecutive components) a scope directory is
/// sealed when a component below the scope names a sealed day, in any spelling. A component is
/// normalised first (every `.`, `-` and `_` deleted), so `2026.11.20`, `2026-11-20`, `2026_11_20`
/// and `20261120` are one name, and the day is matched as a substring of it; the consecutive
/// components `2026`, `11`, `20` (and `202611`, `20`) name it too. A directory under an exempt
/// path (the weather inputs) is left out. A match is on the digits only: `120261120` is sealed
/// (it fails closed), a day-first or worded date is not matched at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Days {
    /// The days as written (`YYYY-MM-DD`).
    pub days: Vec<String>,
    /// Scope directories, as written, and as components.
    pub scopes: Vec<(String, Vec<String>)>,
    /// Directories left out of every scope, as written and as components.
    pub exempt: Vec<(String, Vec<String>)>,
}

/// `name` with every `.`, `-` and `_` deleted.
fn compact(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '.' | '-' | '_'))
        .collect()
}

impl Days {
    /// Whether the components below a scope (`rest`, already below the scope directory) name a
    /// sealed day; the day found.
    fn day_in(&self, rest: &[String]) -> Option<&str> {
        let norm: Vec<String> = rest.iter().map(|c| compact(c)).collect();
        for d in &self.days {
            let c = compact(d);
            let (y, m, dd) = (c.get(..4)?, c.get(4..6)?, c.get(6..)?);
            let ym = c.get(..6)?;
            let hit = norm.iter().any(|n| n.contains(c.as_str()))
                || norm
                    .windows(3)
                    .any(|w| matches!(w, [a, b, c] if a == y && b == m && c == dd))
                || norm
                    .windows(2)
                    .any(|w| matches!(w, [a, b] if a == ym && b == dd));
            if hit {
                return Some(d);
            }
        }
        None
    }

    /// The scope and day when `names` has a scope as consecutive components (at the start only
    /// when `anchored`, else anywhere) and a component below it names a sealed day.
    fn hit(&self, names: &[String], anchored: bool) -> Option<(&str, &str)> {
        let starts: Vec<usize> = if anchored {
            vec![0]
        } else {
            (0..names.len()).collect()
        };
        for i in starts {
            let t = names.get(i..)?;
            for (text, parts) in &self.scopes {
                if !t.starts_with(parts) || t.len() <= parts.len() {
                    continue;
                }
                if self.exempt.iter().any(|(_, e)| t.starts_with(e)) {
                    continue;
                }
                if let Some(d) = t.get(parts.len()..).and_then(|r| self.day_in(r)) {
                    return Some((text, d));
                }
            }
        }
        None
    }
}

/// The parsed list: the data root, the sealed entries, the sealed days and the sealed windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub days: Days,
    pub windows: Vec<Window>,
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDTHH:MM:SSZ` (exactly that shape) as nanoseconds since the Unix epoch.
fn parse_utc_ns(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    if b.len() != 20 {
        return None;
    }
    let sep = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    if sep.iter().any(|(i, c)| b.get(*i) != Some(c)) {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> {
        let s = t.get(a..z)?;
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1970..=2200).contains(&y) || d < 1 || d > dim || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s;
    secs.checked_mul(1_000_000_000)
}

impl List {
    /// The list as lines (`root <path>`, then `seal <entry>` and then `window <from> <to>` in file order): what the Python
    /// guard prints for `--dump`, so a test can compare the two parsers.
    pub fn dump(&self) -> String {
        let mut out = format!("root {}\n", self.root.display());
        for e in &self.entries {
            out.push_str(&format!("seal {}\n", e.text));
        }
        for d in &self.days.days {
            out.push_str(&format!("day {d}\n"));
        }
        for (t, _) in &self.days.scopes {
            out.push_str(&format!("dayscope {t}\n"));
        }
        for (t, _) in &self.days.exempt {
            out.push_str(&format!("dayexempt {t}\n"));
        }
        for w in &self.windows {
            out.push_str(&format!("window {}\n", w.text));
        }
        out
    }
}

/// Parses the list file text. Anything it does not understand is an error, never skipped.
/// Only printable ASCII and `\n` are allowed anywhere in the file, and a line is exactly
/// `word`, one space, `value` (no other white space), so the Python parser cannot read it
/// differently.
pub fn parse_list(text: &str) -> Result<List, String> {
    if let Some(b) = text
        .bytes()
        .find(|b| *b != b'\n' && !(0x20..=0x7e).contains(b))
    {
        return Err(format!(
            "byte 0x{b:02x}: only printable ASCII and newline are allowed"
        ));
    }
    let mut root: Option<PathBuf> = None;
    let mut entries = Vec::new();
    let mut windows = Vec::new();
    let mut days = Days::default();
    for (i, line) in text.split('\n').enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (word, value) = line.split_once(' ').ok_or_else(|| {
            format!(
                "line {}: expected `root <path>`, `seal <path>` or `window <from> <to>`",
                i + 1
            )
        })?;
        if value.is_empty() || value.starts_with(' ') || value.ends_with(' ') {
            return Err(format!(
                "line {}: extra white space around the value",
                i + 1
            ));
        }
        match word {
            "root" => {
                if root.is_some() || !(value.starts_with('/') || value == "$OPENATCA_DATA") {
                    return Err(format!(
                        "line {}: one absolute root or `$OPENATCA_DATA` marker only",
                        i + 1
                    ));
                }
                root = Some(PathBuf::from(value));
            }
            "seal" => {
                let mut parts: Vec<String> = value.split('/').map(str::to_owned).collect();
                if parts.iter().any(|p| p.is_empty() || p == "." || p == "..") {
                    return Err(format!("line {}: bad sealed path", i + 1));
                }
                let mut kind = Kind::Exact;
                if let Some(l) = parts.last_mut() {
                    if l.ends_with('*') {
                        l.pop();
                        kind = Kind::Prefix;
                        if l.starts_with('*') {
                            l.remove(0);
                            kind = Kind::Contains;
                        }
                        if l.is_empty() || l.contains('*') {
                            return Err(format!("line {}: bare or inner `*`", i + 1));
                        }
                    } else if l.contains('*') {
                        return Err(format!("line {}: `*` only as `X*` or `*X*`", i + 1));
                    }
                }
                if parts.iter().rev().skip(1).any(|p| p.contains('*')) {
                    return Err(format!("line {}: `*` only in the last component", i + 1));
                }
                entries.push(Entry {
                    text: value.to_owned(),
                    parts,
                    kind,
                });
            }
            "day" => {
                let b = value.as_bytes();
                let digits = |r: std::ops::Range<usize>| {
                    b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit))
                };
                let num = |r: std::ops::Range<usize>| {
                    value
                        .get(r)
                        .and_then(|x| x.parse::<u32>().ok())
                        .unwrap_or(0)
                };
                if value.len() != 10
                    || !(digits(0..4) && digits(5..7) && digits(8..10))
                    || b.get(4) != Some(&b'-')
                    || b.get(7) != Some(&b'-')
                    || !(1..=12).contains(&num(5..7))
                    || !(1..=31).contains(&num(8..10))
                {
                    return Err(format!("line {}: expected `day YYYY-MM-DD`", i + 1));
                }
                days.days.push(value.to_owned());
            }
            "dayscope" | "dayexempt" => {
                let parts: Vec<String> = value.split('/').map(str::to_owned).collect();
                if parts
                    .iter()
                    .any(|p| p.is_empty() || p == "." || p == ".." || p.contains('*'))
                {
                    return Err(format!("line {}: bad day scope path", i + 1));
                }
                let t = (value.to_owned(), parts);
                if word == "dayscope" {
                    days.scopes.push(t);
                } else {
                    days.exempt.push(t);
                }
            }
            "window" => {
                let bad = || format!("line {}: expected `window <from-UTC> <to-UTC>`", i + 1);
                let (a, b) = value.split_once(' ').ok_or_else(bad)?;
                let (from_ns, to_ns) = (
                    parse_utc_ns(a).ok_or_else(bad)?,
                    parse_utc_ns(b).ok_or_else(bad)?,
                );
                if from_ns >= to_ns {
                    return Err(format!("line {}: the window is empty or reversed", i + 1));
                }
                // The span is widened by the margin on each side (the listed text is kept).
                windows.push(Window {
                    text: value.to_owned(),
                    from_ns: from_ns.saturating_sub(WINDOW_MARGIN_NS),
                    to_ns: to_ns.saturating_add(WINDOW_MARGIN_NS),
                });
            }
            _ => return Err(format!("line {}: unknown word `{word}`", i + 1)),
        }
    }
    let root = root.ok_or("no root line")?;
    if entries.is_empty() {
        return Err("no seal lines".into());
    }
    if days.days.is_empty() != days.scopes.is_empty() {
        return Err("`day` and `dayscope` lines come together".into());
    }
    Ok(List {
        root,
        entries,
        days,
        windows,
    })
}

/// A refusal: names the rule and the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub path: PathBuf,
    pub why: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.why)
    }
}

impl std::error::Error for Refusal {}

/// A deepest directory nesting [`Sealed::walk_unsealed`] descends to.
const MAX_WALK_DEPTH: usize = 64;

/// What [`Sealed::walk_unsealed`] found: the unsealed files, sorted bytewise by the string form of
/// the path, the count of entries (files or directories) skipped because the guard refuses them,
/// and the count of directories already visited (a link to a directory the walk is in or has been
/// in, or two names of one directory: the first in bytewise order of the walk is the one walked).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    pub files: Vec<PathBuf>,
    pub skipped: usize,
    pub already_visited: usize,
}

/// The guard for one data root.
#[derive(Clone, Debug)]
pub struct Sealed {
    list: List,
    root: PathBuf,
    /// The compiled-in list did not parse: every path is refused.
    broken: bool,
}

fn names(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// `path` with every link followed. `Err` for a link that cannot be followed.
fn resolve(path: &Path, depth: usize) -> Result<PathBuf, ()> {
    if depth > MAX_LINKS {
        return Err(());
    }
    let mut out = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().map_err(|_| ())?
    };
    for c in path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => {
                out.push(n);
                if let Ok(real) = fs::canonicalize(&out) {
                    out = real;
                // seal-allow: the guard's own link resolution; it reads no file, only whether a name is a link
                } else if fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink()) {
                    // A link canonicalize could not follow: follow it by hand.
                    let target = fs::read_link(&out).map_err(|_| ())?;
                    let base = out.parent().map(Path::to_path_buf).unwrap_or_default();
                    out = resolve(&base.join(target), depth + 1)?;
                }
            }
        }
    }
    Ok(out)
}

impl Sealed {
    /// The guard for the compiled-in list and the real data root, built once (production).
    pub fn production() -> &'static Sealed {
        static PRODUCTION: OnceLock<Sealed> = OnceLock::new();
        PRODUCTION.get_or_init(|| production_guard(LIST, Path::new(LIST_PATH), &mut io::stderr()))
    }

    /// The guard for the compiled-in list alone, with no comparison against the list file on disk.
    /// For capture and write-only paths, which must keep recording when the list file changes
    /// (the readers fail closed instead, see [`Sealed::production`]). Built once.
    pub fn production_embedded() -> &'static Sealed {
        static EMBEDDED: OnceLock<Sealed> = OnceLock::new();
        EMBEDDED.get_or_init(|| Sealed::with_list(LIST, None))
    }

    /// The guard for the compiled-in entries under `root` (tests build a fake root).
    pub fn with_root(root: &Path) -> Sealed {
        Sealed::with_list(LIST, Some(root))
    }

    /// The guard for list text (and, if given, another data root). A list that does not parse
    /// refuses every path: the guard fails closed.
    pub fn with_list(text: &str, root: Option<&Path>) -> Sealed {
        Self::with_list_using_data_root(text, root, std::env::var_os("OPENATCA_DATA"))
    }

    fn with_list_using_data_root(
        text: &str,
        root: Option<&Path>,
        data_root: Option<std::ffi::OsString>,
    ) -> Sealed {
        Self::with_list_using_data_root_and_default(
            text,
            root,
            data_root,
            Path::new(DEFAULT_DATA_ROOT),
        )
    }

    fn with_list_using_data_root_and_default(
        text: &str,
        root: Option<&Path>,
        data_root: Option<std::ffi::OsString>,
        default_data_root: &Path,
    ) -> Sealed {
        match parse_list(text) {
            Ok(mut list) => {
                if let Some(r) = root {
                    list.root = r.to_path_buf();
                }
                let configured =
                    configured_root(&list.root, data_root, default_data_root).unwrap_or_default();
                let root = if configured.as_os_str().is_empty() {
                    PathBuf::new()
                } else {
                    resolve_data_root(&configured).unwrap_or_default()
                };
                if root.as_os_str().is_empty() {
                    return Sealed {
                        list,
                        root,
                        broken: true,
                    };
                }
                Sealed {
                    list,
                    root,
                    broken: false,
                }
            }
            Err(_) => Sealed {
                list: List {
                    root: PathBuf::new(),
                    entries: Vec::new(),
                    days: Days::default(),
                    windows: Vec::new(),
                },
                root: PathBuf::new(),
                broken: true,
            },
        }
    }

    /// Refuses `path` if it is, is inside, resolves into, or holds a sealed entry.
    pub fn check(&self, path: &Path) -> Result<(), Refusal> {
        self.check_with(path, false)
    }

    /// [`Sealed::check`]; with `walking`, a directory that merely holds a sealed entry is
    /// accepted (a walk descends into it and skips the sealed names).
    fn check_with(&self, path: &Path, walking: bool) -> Result<(), Refusal> {
        if self.broken {
            return Err(Refusal {
                path: path.to_path_buf(),
                why: "refused by OA-SEAL-001: the sealed list does not parse, so nothing is read"
                    .into(),
            });
        }
        match resolve(path, 0) {
            Ok(real) => self.check_resolved(path, &real, walking),
            Err(()) => Err(Refusal {
                path: path.to_path_buf(),
                why: "refused by OA-SEAL-001: the path cannot be resolved (a link loop or an unreadable link)"
                    .into(),
            }),
        }
    }

    /// The rule on a path as given and its resolved form.
    fn check_resolved(&self, path: &Path, real: &Path, walking: bool) -> Result<(), Refusal> {
        let (given, resolved) = (names(path), names(real));
        let below = real.strip_prefix(&self.root).ok().map(names);
        for e in &self.list.entries {
            let how = if e.named_in(&given) {
                "the path names"
            } else if e.named_in(&resolved) {
                "the path resolves to"
            } else if below.as_ref().is_some_and(|b| e.matches_at(b)) {
                "the path is inside"
            } else if !walking && below.as_ref().is_some_and(|b| e.is_ancestor(b)) {
                "the path is a directory that holds (name something below it)"
            } else {
                continue;
            };
            return Err(Refusal {
                path: path.to_path_buf(),
                why: format!(
                    "refused by OA-SEAL-001: {how} the sealed hold-out entry `{}` (crates/oa-sealed/sealed_paths.txt); sealed data is never read",
                    e.text
                ),
            });
        }
        let days = &self.list.days;
        let hit = days
            .hit(&given, false)
            .map(|h| ("the path names", h))
            .or_else(|| {
                days.hit(&resolved, false)
                    .map(|h| ("the path resolves to", h))
            })
            .or_else(|| {
                below
                    .as_ref()
                    .and_then(|b| days.hit(b, true))
                    .map(|h| ("the path is inside", h))
            });
        if let Some((how, (scope, day))) = hit {
            return Err(Refusal {
                path: path.to_path_buf(),
                why: format!(
                    "refused by OA-SEAL-001: {how} the sealed hold-out entry `day {day} below {scope}` (crates/oa-sealed/sealed_paths.txt); sealed data is never read"
                ),
            });
        }
        Ok(())
    }

    /// The sealed windows of the list.
    pub fn windows(&self) -> &[Window] {
        &self.list.windows
    }

    /// The windows of the list as one span of Unix milliseconds for the decoders: the hull, from
    /// the earliest start to the latest end, with any gap between windows inside it (fails
    /// closed). A list with no window, or one that does not parse, gives the window that holds
    /// all time.
    pub fn drop_window(&self) -> DropWindow {
        let from = self.list.windows.iter().map(|x| x.from_ns).min();
        let to = self.list.windows.iter().map(|x| x.to_ns).max();
        match (self.broken, from, to) {
            (false, Some(a), Some(b)) => oa_time_window::from_ns_span(a, b),
            _ => DropWindow::all_time(),
        }
    }

    /// Whether `unix_ns` (nanoseconds since the Unix epoch, UTC) is in a sealed window. A guard
    /// whose list does not parse, or has no window line, seals every instant (fails closed, like
    /// [`Sealed::drop_window`]).
    pub fn in_window(&self, unix_ns: i64) -> bool {
        self.broken
            || self.list.windows.is_empty()
            || self
                .list
                .windows
                .iter()
                .any(|w| unix_ns >= w.from_ns && unix_ns < w.to_ns)
    }

    /// Walks `root` for files: a directory or file the guard refuses is skipped and counted,
    /// never descended into or opened (a directory that only holds a sealed entry, such as the data
    /// root or `raw/adsblol`, is walked); the files are returned in sorted order. `root` itself
    /// being refused is an error (the caller names the path). A name that is a file is returned
    /// as the only file.
    pub fn walk_unsealed(&self, root: &Path) -> io::Result<Walk> {
        self.check_with(root, true)
            .map_err(|r| io::Error::other(r.why))?;
        // A root that does not exist is an error, not an empty walk.
        // seal-allow: the guard's own walk; root was checked above
        fs::metadata(root)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", root.display())))?;
        let mut walk = Walk::default();
        let mut visited = BTreeSet::new();
        self.walk_into(root, 0, &mut walk, &mut visited)?;
        walk.files
            .sort_by(|a, b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
        Ok(walk)
    }

    fn walk_into(
        &self,
        dir: &Path,
        depth: usize,
        walk: &mut Walk,
        visited: &mut BTreeSet<PathBuf>,
    ) -> io::Result<()> {
        if !dir.is_dir() {
            walk.files.push(dir.to_path_buf());
            return Ok(());
        }
        if depth > MAX_WALK_DEPTH {
            return Err(io::Error::other(format!(
                "{}: directories nested deeper than {MAX_WALK_DEPTH}; the walk stops with an error, not silently",
                dir.display()
            )));
        }
        // Each canonical directory is walked once, so a link back to a directory already seen is
        // skipped and counted rather than followed round again.
        let real = fs::canonicalize(dir)?;
        if !visited.insert(real) {
            walk.already_visited += 1;
            return Ok(());
        }
        // The entries are sorted bytewise before any is walked, so which of two names of one
        // directory is walked does not depend on the order the file system lists them in.
        let mut entries = Vec::new();
        // seal-allow: the guard's own walk; every entry is checked below before it is used
        for e in fs::read_dir(dir)? {
            entries.push(e?.path());
        }
        entries.sort_by(|a, b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
        for p in entries {
            if self.check_with(&p, true).is_err() {
                walk.skipped += 1;
            } else if p.is_dir() {
                self.walk_into(&p, depth + 1, walk, visited)?;
            } else {
                walk.files.push(p);
            }
        }
        Ok(())
    }

    /// Checks `path`, opens it for reading, then checks what the descriptor really is: the
    /// opener every file a tool reads goes through. The error of a refusal carries the rule and
    /// no path (the caller names the path).
    pub fn open(&self, path: &Path) -> io::Result<File> {
        self.check(path).map_err(|r| io::Error::other(r.why))?;
        // seal-allow: the guard's own opener; the path was checked above and the file is verified below
        let file = File::open(path)?;
        self.verify_open(path, &file)?;
        Ok(file)
    }

    /// Opens a regular input file and returns the size of the verified descriptor. On supported
    /// Linux targets, opening a FIFO is nonblocking so it can be refused before a read.
    pub fn open_regular(&self, path: &Path) -> io::Result<(File, u64)> {
        self.check(path).map_err(|r| io::Error::other(r.why))?;
        #[cfg(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            // asm-generic/fcntl.h defines O_NONBLOCK this way on x86-64 and AArch64.
            const O_NONBLOCK: i32 = 0o4000;
            // seal-allow: guard checks the path above and verifies the nonblocking descriptor below
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(O_NONBLOCK)
                .open(path)?
        };
        #[cfg(not(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )))]
        let file = {
            // seal-allow: precheck the type of the already guarded path before a platform open
            if !fs::metadata(path)?.is_file() {
                return Err(io::Error::other("input is not a regular file"));
            }
            self.open(path)?
        };
        let meta = self.verify_open_metadata(path, &file)?;
        if !meta.is_file() {
            return Err(io::Error::other("input is not a regular file"));
        }
        Ok((file, meta.len()))
    }

    /// Asks the kernel for the real path of an open file and refuses a sealed one. A platform
    /// without `/proc/self/fd` skips this (the check before the open stands).
    pub fn verify_open(&self, path: &Path, file: &File) -> io::Result<()> {
        self.verify_open_metadata(path, file).map(|_| ())
    }

    fn verify_open_metadata(&self, path: &Path, file: &File) -> io::Result<fs::Metadata> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        // A regular file with more than one name is refused: a hard link under a neutral name
        // to a sealed file would pass the name check, and the real path of the descriptor is
        // only one of its names.
        // A file whose metadata cannot be read is refused: the check fails closed.
        let m = file.metadata().map_err(|e| {
            io::Error::other(format!(
                "refused by OA-SEAL-002: the open file's metadata cannot be read ({e}), so it is not shown to have one name"
            ))
        })?;
        if m.is_file() && m.nlink() > 1 {
            return Err(io::Error::other(format!(
                "refused by OA-SEAL-002: the file has {} hard links, so its other names could be sealed data under a neutral name; copy it instead",
                m.nlink()
            )));
        }
        let link = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        if let Ok(real) = fs::read_link(&link) {
            self.check_resolved(path, &real, false)
                .map_err(|r| io::Error::other(r.why))?;
        }
        Ok(m)
    }
}

/// Production check: `path` against the compiled-in list and the real data root.
pub fn refuse(path: &Path) -> Result<(), String> {
    Sealed::production().check(path).map_err(|e| e.to_string())
}

/// [`refuse_io`] against [`Sealed::production_embedded`]: for a capture or write-only path.
pub fn refuse_io_embedded(path: &Path) -> io::Result<()> {
    Sealed::production_embedded()
        .check(path)
        .map_err(|r| io::Error::other(r.why))
}

/// Production check as an I/O error (the rule, no path) for a loader that adds the path itself.
pub fn refuse_io(path: &Path) -> io::Result<()> {
    Sealed::production()
        .check(path)
        .map_err(|r| io::Error::other(r.why))
}

/// Production opener: refuses a sealed path, opens the file for reading and verifies the
/// opened file.
pub fn open(path: &Path) -> io::Result<File> {
    Sealed::production().open(path)
}

/// Production opener for a bounded regular-file input. Returns the descriptor and its size.
pub fn open_regular(path: &Path) -> io::Result<(File, u64)> {
    Sealed::production().open_regular(path)
}

/// Production read of a whole file through the guard's opener (`fs::read`, guarded).
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = open(path.as_ref())?;
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

/// Production read of a whole UTF-8 file through the guard's opener (`fs::read_to_string`,
/// guarded).
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let v = read(path)?;
    String::from_utf8(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Production walk: the unsealed files below `root` and the count of sealed entries skipped.
pub fn walk_unsealed(root: &Path) -> io::Result<Walk> {
    Sealed::production().walk_unsealed(root)
}

/// The sealed windows of the compiled-in list.
pub fn sealed_windows() -> &'static [Window] {
    Sealed::production().windows()
}

/// Whether `unix_ns` is in a sealed window of the compiled-in list: a reader drops the row or
/// frame and counts it.
pub fn in_sealed_window(unix_ns: i64) -> bool {
    Sealed::production().in_window(unix_ns)
}

static WINDOW_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Adds `n` to the process-wide count of rows or frames a reader dropped because their time is
/// in a sealed window (OA-SEAL-014). The readers call this; a tool prints [`window_drop_line`]
/// when it ends.
pub fn note_window_drops(n: u64) {
    WINDOW_DROPPED.fetch_add(n, Ordering::Relaxed);
}

/// The process-wide count of rows or frames dropped in a sealed window so far. A cross-check for
/// tests only: a tool prints the sum of its readers' own counts ([`drop_line`]), so an input
/// read twice is counted once.
pub fn window_dropped_total() -> u64 {
    WINDOW_DROPPED.load(Ordering::Relaxed)
}

/// The line every tool that reads rows prints once, after its last read, also with a count of 0:
/// `sealed-window rows dropped: N`, `n` being the sum of the per-reader counts (each input
/// counted once).
pub fn drop_line(n: u64) -> String {
    format!("sealed-window rows dropped: {n}")
}

/// What one dropped unit is: a row, a record or a message counts as itself; a frame dropped because
/// it was received in a window counts as the rows it holds where its payload decodes (at least 1),
/// and as 1 where it does not (a frame of another kind, or a payload that is no rows). A damaged
/// frame, which has no receive time, counts as 1 when [`DamagedGate`] drops it.
///
/// The count of rows a tool dropped in a sealed window, which prints [`drop_line`] when it is
/// dropped. A tool makes one at the top of its run and adds each reader's count as the reader
/// returns, so the line is printed on every exit that comes back through `main`: a `?` error, an
/// early return and a refusal included. (`std::process::exit` does not run it: a tool does not
/// call it after reading has started.)
#[derive(Debug)]
pub struct DropReport {
    n: u64,
    stdout: bool,
}

/// Decides what a damaged frame is, in a stream of frames. A damaged frame carries no receive
/// time, so it is gated by the receive times of the nearest good frame before it and the nearest
/// good frame after it in the same file: if either is in a sealed window, or either is missing
/// (the damage is at the start or the end of the file), the frame is dropped and counted in the
/// drop line, not counted as damaged. Every good frame (in the window or not) is shown to the
/// gate with [`DamagedGate::good`]; each damaged one with [`DamagedGate::damaged`]; at the end of
/// the file [`DamagedGate::finish`] settles what is left. One gate per file.
#[derive(Clone, Copy, Debug, Default)]
pub struct DamagedGate {
    prev: Option<i64>,
    pending: u64,
}

impl DamagedGate {
    /// A gate at the start of a file.
    pub const fn new() -> DamagedGate {
        DamagedGate {
            prev: None,
            pending: 0,
        }
    }

    /// A damaged frame: settled by the next good frame, or by [`DamagedGate::finish`].
    pub fn damaged(&mut self) {
        self.pending = self.pending.saturating_add(1);
    }

    /// A good frame received at `recv_unix_ns`. Returns `(kept, dropped)`: the damaged frames
    /// since the last good frame that are counted as damaged, and the ones dropped (counted in
    /// the drop line; the count is also added to the process-wide one).
    pub fn good(&mut self, recv_unix_ns: i64) -> (u64, u64) {
        let n = std::mem::take(&mut self.pending);
        let before = self.prev.replace(recv_unix_ns);
        if n == 0 {
            return (0, 0);
        }
        let open = before.is_some_and(|b| !in_sealed_window(b)) && !in_sealed_window(recv_unix_ns);
        if open {
            (n, 0)
        } else {
            note_window_drops(n);
            (0, n)
        }
    }

    /// The end of the file: the damaged frames with no good frame after them are dropped. Returns
    /// their count (added to the process-wide one), and the gate is reset for the next file.
    pub fn finish(&mut self) -> u64 {
        let n = std::mem::take(&mut self.pending);
        self.prev = None;
        note_window_drops(n);
        n
    }
}

/// The warning a tool prints at start when the list seals every instant: a list with no `window`
/// line, or one that does not parse (which also refuses every sealed path); `None` otherwise.
pub fn start_warning(guard: &Sealed) -> Option<&'static str> {
    if guard.broken {
        Some("seal list does not parse: every timed row is dropped and every sealed path refused")
    } else if guard.windows().is_empty() {
        Some("seal list has no window: every timed row is dropped")
    } else {
        None
    }
}

/// Writes [`start_warning`] of `guard` to `out`, once per `once`. A failed write is ignored.
fn announce(guard: &Sealed, once: &std::sync::Once, out: &mut dyn io::Write) {
    once.call_once(|| {
        if let Some(w) = start_warning(guard) {
            let _ = writeln!(out, "{w}");
        }
    });
}

/// The once-per-process guard of the start warning.
static START: std::sync::Once = std::sync::Once::new();

impl DropReport {
    /// A report that prints its line on standard error. A tool makes its report first thing, so
    /// the start warning ([`start_warning`]) is printed, once per process, at start.
    pub fn new() -> DropReport {
        DropReport::begin(Sealed::production(), &START, &mut io::stderr(), false)
    }

    /// A report that prints its line on standard output (a tool whose output is that stream); the
    /// start warning goes to standard error.
    pub fn to_stdout() -> DropReport {
        DropReport::begin(Sealed::production(), &START, &mut io::stderr(), true)
    }

    /// Makes a report after announcing the start warning of `guard` once per `once` on `out`.
    fn begin(
        guard: &Sealed,
        once: &std::sync::Once,
        out: &mut dyn io::Write,
        stdout: bool,
    ) -> DropReport {
        announce(guard, once, out);
        DropReport { n: 0, stdout }
    }

    /// Adds the count of one reader (each input is added once).
    pub fn add(&mut self, n: u64) {
        self.n = self.n.saturating_add(n);
    }

    /// The count so far.
    pub fn count(&self) -> u64 {
        self.n
    }
}

impl Default for DropReport {
    fn default() -> DropReport {
        DropReport::new()
    }
}

impl Drop for DropReport {
    fn drop(&mut self) {
        use std::io::Write;
        // A failed write (a closed pipe) is ignored: this runs on the way out and must not panic.
        let line = drop_line(self.n);
        if self.stdout {
            let _ = writeln!(io::stdout(), "{line}");
        } else {
            let _ = writeln!(io::stderr(), "{line}");
        }
    }
}

/// The sealed window of the compiled-in list. It fails closed, like the path guard: if the list
/// has no window or does not parse, the window covers all time and every row is dropped.
pub fn sealed_drop_window() -> DropWindow {
    Sealed::production().drop_window()
}

/// The sealed windows of the list as one span of Unix milliseconds, `from` inclusive and `to`
/// exclusive (`from` rounded down and `to` up, so a non-aligned window leaves no gap); with
/// several windows it is the span from the first start to the last end, which also holds the
/// gaps (the check fails closed). It fails closed like [`sealed_drop_window`]: a list with no
/// window, or one that does not parse, gives the span of all time. It is the span of
/// [`sealed_drop_window`], and always `Some`.
pub fn sealed_window_ms() -> Option<(i64, i64)> {
    oa_time_window::span(sealed_drop_window())
}

/// The line for the process-wide count (see [`window_dropped_total`]): a cross-check for tests.
pub fn window_drop_line() -> String {
    format!("sealed-window rows dropped: {}", window_dropped_total())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn configured_guard(text: &str) -> Sealed {
        Sealed::with_list(text, None)
    }

    #[test]
    fn the_compiled_in_list_parses() {
        let l = parse_list(LIST).unwrap();
        assert_eq!(l.entries.len(), 6);
        assert_eq!(l.days.days.len(), 3);
        assert_eq!(l.windows.len(), 1);
    }

    #[test]
    fn the_window_in_milliseconds_is_the_listed_span_and_the_report_adds() {
        let (a, b) = sealed_window_ms().unwrap();
        // The listed span widened by 15 minutes on each side.
        assert_eq!((a, b), (1_795_131_900_000, 1_795_393_920_000));
        let mut r = DropReport::new();
        r.add(2);
        r.add(3);
        assert_eq!(r.count(), 5);
    }

    #[test]
    fn a_window_is_half_open_and_the_sealed_one_is_the_listed_span() {
        let w = DropWindow::new(10, 20);
        assert!(
            !oa_time_window::contains(w, 9)
                && oa_time_window::contains(w, 10)
                && oa_time_window::contains(w, 19)
                && !oa_time_window::contains(w, 20)
        );
        assert_eq!(oa_time_window::span(w), Some((10, 20)));
        let e = DropWindow::empty_for_tests_and_fuzz();
        assert!(
            !oa_time_window::contains(e, 0)
                && !oa_time_window::contains(e, i64::MIN)
                && oa_time_window::span(e).is_none()
        );
        assert_eq!(
            oa_time_window::span(sealed_drop_window()),
            sealed_window_ms()
        );
        assert_eq!(
            oa_time_window::span(sealed_drop_window()),
            Some((1_795_131_900_000, 1_795_393_920_000))
        );
    }

    #[test]
    fn the_drop_window_fails_closed_when_the_list_has_no_window_or_does_not_parse() {
        let all = DropWindow::all_time();
        assert!(
            oa_time_window::contains(all, 0)
                && oa_time_window::contains(all, 1_795_200_000_000)
                && oa_time_window::contains(all, i64::MIN)
        );
        // The compiled-in list: its own span.
        assert_eq!(
            oa_time_window::span(sealed_drop_window()),
            Some((1_795_131_900_000, 1_795_393_920_000))
        );
        // A list that does not parse, and one with no window line: all time.
        assert_eq!(configured_guard("not a list\n").drop_window(), all);
        let none = list_without_window();
        let g = configured_guard(&none);
        assert!(g.windows().is_empty());
        assert_eq!(g.drop_window(), all);
    }

    #[test]
    fn a_list_that_seals_every_instant_gives_a_start_warning_and_a_good_one_does_not() {
        let none = list_without_window();
        assert_eq!(
            start_warning(&configured_guard(&none)),
            Some("seal list has no window: every timed row is dropped")
        );
        assert_eq!(
            start_warning(&configured_guard("not a list\n")),
            Some(
                "seal list does not parse: every timed row is dropped and every sealed path refused"
            )
        );
        assert_eq!(start_warning(Sealed::production()), None);
    }

    #[test]
    fn a_drop_report_made_on_a_list_that_seals_every_instant_prints_the_warning_once() {
        let none = list_without_window();
        for (list, line) in [
            (
                none.as_str(),
                "seal list has no window: every timed row is dropped\n",
            ),
            (
                "not a list\n",
                "seal list does not parse: every timed row is dropped and every sealed path refused\n",
            ),
        ] {
            let g = configured_guard(list);
            let once = std::sync::Once::new();
            let mut out = Vec::new();
            let r = DropReport::begin(&g, &once, &mut out, false);
            assert_eq!(String::from_utf8(out.clone()).unwrap(), line);
            // A second report of the process does not repeat it.
            let r2 = DropReport::begin(&g, &once, &mut out, true);
            assert_eq!(String::from_utf8(out).unwrap(), line);
            assert_eq!((r.count(), r2.count()), (0, 0));
        }
        // A good list prints nothing.
        let mut out = Vec::new();
        let g = configured_guard(LIST);
        DropReport::begin(&g, &std::sync::Once::new(), &mut out, false);
        assert!(out.is_empty());
    }

    #[test]
    fn the_time_gate_fails_closed_without_a_window_and_a_window_not_aligned_to_a_millisecond_has_no_gap(
    ) {
        let none = list_without_window();
        let g = configured_guard(&none);
        assert!(g.in_window(0) && g.in_window(1_795_200_000_000_000_000));
        // A window from 0.4 ms after 2026-11-20T00:00:00Z to 0.6 ms after 2026-11-23T00:17:00Z
        // (the list has whole seconds, so this is the conversion itself).
        let w = oa_time_window::from_ns_span(1_795_132_800_000_400_000, 1_795_393_020_000_600_000);
        // The start is rounded down and the end up: the millisecond that holds the first instant
        // and the one that holds the last are both in the span.
        assert_eq!(
            oa_time_window::span(w),
            Some((1_795_132_800_000, 1_795_393_020_001))
        );
        assert!(
            oa_time_window::contains(w, 1_795_132_800_000)
                && oa_time_window::contains(w, 1_795_393_020_000)
        );
        assert!(
            !oa_time_window::contains(w, 1_795_132_799_999)
                && !oa_time_window::contains(w, 1_795_393_020_001)
        );
        // A window of whole milliseconds is exact, and the compiled-in list agrees.
        assert_eq!(
            oa_time_window::span(oa_time_window::from_ns_span(1_000_000, 3_000_000)),
            Some((1, 3))
        );
        assert_eq!(
            oa_time_window::span(sealed_drop_window()),
            sealed_window_ms()
        );
    }

    #[test]
    fn the_sealed_window_is_the_listed_one_widened_by_fifteen_minutes() {
        let w = sealed_drop_window();
        // Dropped: a time that rounds to the listed end, one inside the end margin, and the
        // first instant of the start margin.
        for ms in [1_795_393_020_000, 1_795_393_919_000, 1_795_131_900_000] {
            assert!(oa_time_window::contains(w, ms), "{ms}");
            assert!(in_sealed_window(ms * 1_000_000), "{ms}");
        }
        // Kept: one second before the start margin and the end of the end margin.
        for ms in [1_795_131_899_000, 1_795_393_920_000] {
            assert!(!oa_time_window::contains(w, ms), "{ms}");
            assert!(!in_sealed_window(ms * 1_000_000), "{ms}");
        }
        assert_eq!(WINDOW_MARGIN_NS, 15 * 60 * 1_000_000_000);
    }

    // REQ: OA-SEAL-014
    #[test]
    fn a_list_on_disk_that_differs_from_the_compiled_in_one_fails_closed() {
        let dir = std::env::temp_dir().join(format!("oa_sealed_list_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sealed_paths.txt");
        let mut warn = Vec::new();
        // The same bytes: the embedded list is in force and nothing is said.
        fs::write(&file, LIST).unwrap();
        let g = production_guard(LIST, &file, &mut warn);
        assert!(!g.broken && g.drop_window() != DropWindow::all_time() && warn.is_empty());
        // One byte more: closed, with both hashes named.
        fs::write(&file, format!("{LIST}\n")).unwrap();
        let g = production_guard(LIST, &file, &mut warn);
        assert!(g.broken && g.drop_window() == DropWindow::all_time());
        assert!(g.in_window(0));
        let said = String::from_utf8(warn.clone()).unwrap();
        assert!(said.contains(&list_hash(LIST.as_bytes())), "{said}");
        assert!(
            said.contains(&list_hash(format!("{LIST}\n").as_bytes())),
            "{said}"
        );
        // A file that is absent (an installed binary): the embedded list stays.
        fs::remove_file(&file).unwrap();
        let mut warn = Vec::new();
        let g = production_guard(LIST, &file, &mut warn);
        assert!(!g.broken && warn.is_empty());
        // A differing list file: a reader's guard refuses every path, the capture guard (the
        // compiled-in list alone) still accepts an ordinary one.
        fs::write(&file, format!("{LIST}\n")).unwrap();
        let reader = production_guard(LIST, &file, &mut Vec::new());
        let ordinary = dir.join("rec/stdds/a.oaf");
        assert!(reader.check(&ordinary).is_err());
        assert!(refuse_io_embedded(&ordinary).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    // REQ: OA-SEAL-014
    #[test]
    fn a_damaged_frame_is_gated_by_the_good_frames_on_both_sides() {
        let (from, _) = sealed_window_ms().unwrap();
        let inside = from * 1_000_000 + 5;
        let outside = from * 1_000_000 - 3_600_000_000_000;
        // Good, damaged, good: both outside the window, so the damage is damage.
        let mut g = DamagedGate::new();
        assert_eq!(g.good(outside), (0, 0));
        g.damaged();
        assert_eq!(g.good(outside + 1), (1, 0));
        // A good frame on either side in the window drops it.
        g.damaged();
        g.damaged();
        assert_eq!(g.good(inside), (0, 2));
        g.damaged();
        assert_eq!(g.good(outside), (0, 1));
        // At the end there is no frame after: dropped. The next file starts with none before.
        g.damaged();
        assert_eq!(g.finish(), 1);
        g.damaged();
        assert_eq!(g.good(outside), (0, 1));
        assert_eq!(g.finish(), 0);
    }
}
