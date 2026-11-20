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
//! Requirements: OA-SEAL-001 and the following, in REQUIREMENTS.md.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The list file, compiled in.
pub const LIST: &str = include_str!("../sealed_paths.txt");

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

/// A sealed span of time, half-open: `from_ns <= t < to_ns`, in nanoseconds since the Unix epoch.
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
                if root.is_some() || !value.starts_with('/') {
                    return Err(format!("line {}: one absolute root only", i + 1));
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
                windows.push(Window {
                    text: value.to_owned(),
                    from_ns,
                    to_ns,
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
        PRODUCTION.get_or_init(|| Sealed::with_list(LIST, None))
    }

    /// The guard for the compiled-in entries under `root` (tests build a fake root).
    pub fn with_root(root: &Path) -> Sealed {
        Sealed::with_list(LIST, Some(root))
    }

    /// The guard for list text (and, if given, another data root). A list that does not parse
    /// refuses every path: the guard fails closed.
    pub fn with_list(text: &str, root: Option<&Path>) -> Sealed {
        match parse_list(text) {
            Ok(mut list) => {
                if let Some(r) = root {
                    list.root = r.to_path_buf();
                }
                let root = resolve(&list.root, 0).unwrap_or_else(|_| list.root.clone());
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

    /// The windows of the list as one span of Unix milliseconds for the decoders. A list with no
    /// window, or one that does not parse, gives the window that holds all time (fails closed).
    pub fn drop_window(&self) -> DropWindow {
        let from = self.list.windows.iter().map(|x| x.from_ns).min();
        let to = self.list.windows.iter().map(|x| x.to_ns).max();
        match (self.broken, from, to) {
            (false, Some(a), Some(b)) => DropWindow::from_ns_span(a, b),
            _ => DropWindow::all_time(),
        }
    }

    /// Whether `unix_ns` (nanoseconds since the Unix epoch, UTC) is in a sealed window. A guard
    /// whose list does not parse, or has no window line, seals every instant (fails closed, like
    /// [`Sealed::drop_window`]).
    pub fn in_window(&self, unix_ns: i64) -> bool {
        self.broken
            || self.list.windows.is_empty()
            || self.list.windows.iter().any(|w| w.contains(unix_ns))
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

    /// Asks the kernel for the real path of an open file and refuses a sealed one. A platform
    /// without `/proc/self/fd` skips this (the check before the open stands).
    pub fn verify_open(&self, path: &Path, file: &File) -> io::Result<()> {
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
        Ok(())
    }
}

/// Production check: `path` against the compiled-in list and the real data root.
pub fn refuse(path: &Path) -> Result<(), String> {
    Sealed::production().check(path).map_err(|e| e.to_string())
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

impl DropReport {
    /// A report that prints its line on standard error.
    pub fn new() -> DropReport {
        DropReport {
            n: 0,
            stdout: false,
        }
    }

    /// A report that prints its line on standard output (a tool whose output is that stream).
    pub fn to_stdout() -> DropReport {
        DropReport { n: 0, stdout: true }
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

/// The span of Unix milliseconds a decoder drops rows in, `from` inclusive and `to` exclusive.
/// A decoder takes it as a required argument, so a caller cannot forget it: the shells pass
/// [`DropWindow::sealed`], and [`DropWindow::empty_for_tests_and_fuzz`] is the only way to drop nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropWindow {
    from_ms: i64,
    to_ms: i64,
}

impl DropWindow {
    /// The span `from_ms` (inclusive) to `to_ms` (exclusive); empty when `from_ms >= to_ms`.
    pub const fn new(from_ms: i64, to_ms: i64) -> DropWindow {
        DropWindow { from_ms, to_ms }
    }

    /// The span of the nanosecond window `from_ns` to `to_ns`, in milliseconds. The start is
    /// rounded down and the end up, so a window that is not a whole number of milliseconds never
    /// leaves a gap at either edge: a millisecond that holds any instant of the window is in it.
    pub const fn from_ns_span(from_ns: i64, to_ns: i64) -> DropWindow {
        let up = to_ns.div_euclid(1_000_000) + (to_ns.rem_euclid(1_000_000) != 0) as i64;
        DropWindow::new(from_ns.div_euclid(1_000_000), up)
    }

    /// A window that holds no time: nothing is dropped. For tests and fuzz targets only; a tool
    /// passes [`DropWindow::sealed`].
    pub const fn empty_for_tests_and_fuzz() -> DropWindow {
        DropWindow {
            from_ms: 0,
            to_ms: 0,
        }
    }

    /// The sealed window of the compiled-in list ([`sealed_window_ms`]). It fails closed, like
    /// the path guard: if the list has no window or does not parse, the window covers all time
    /// and every row is dropped.
    pub fn sealed() -> DropWindow {
        Sealed::production().drop_window()
    }

    /// A window that holds all time (the closed failure of [`DropWindow::sealed`]).
    pub const fn all_time() -> DropWindow {
        DropWindow {
            from_ms: i64::MIN,
            to_ms: i64::MAX,
        }
    }

    /// Whether `t_ms` is in the window.
    pub const fn contains(&self, t_ms: i64) -> bool {
        self.from_ms <= t_ms && t_ms < self.to_ms
    }

    /// The span as `(from, to)`, or `None` when it is empty.
    pub const fn span(&self) -> Option<(i64, i64)> {
        if self.from_ms < self.to_ms {
            Some((self.from_ms, self.to_ms))
        } else {
            None
        }
    }
}

/// The sealed windows of the list as one span of Unix milliseconds, `from` inclusive and `to`
/// exclusive (`from` rounded down and `to` up, so a non-aligned window leaves no gap); with
/// several windows it is the span from the first start to the last end, which also holds the
/// gaps (the check fails closed). It fails closed like [`DropWindow::sealed`]: a list with no
/// window, or one that does not parse, gives the span of all time. It is the span of
/// [`DropWindow::sealed`], and always `Some`.
pub fn sealed_window_ms() -> Option<(i64, i64)> {
    DropWindow::sealed().span()
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
        assert_eq!((a, b), (1_795_132_800_000, 1_795_393_020_000));
        let mut r = DropReport::new();
        r.add(2);
        r.add(3);
        assert_eq!(r.count(), 5);
    }

    #[test]
    fn a_window_is_half_open_and_the_sealed_one_is_the_listed_span() {
        let w = DropWindow::new(10, 20);
        assert!(!w.contains(9) && w.contains(10) && w.contains(19) && !w.contains(20));
        assert_eq!(w.span(), Some((10, 20)));
        let e = DropWindow::empty_for_tests_and_fuzz();
        assert!(!e.contains(0) && !e.contains(i64::MIN) && e.span().is_none());
        assert_eq!(DropWindow::sealed().span(), sealed_window_ms());
        assert_eq!(
            DropWindow::sealed().span(),
            Some((1_795_132_800_000, 1_795_393_020_000))
        );
    }

    #[test]
    fn the_drop_window_fails_closed_when_the_list_has_no_window_or_does_not_parse() {
        let all = DropWindow::all_time();
        assert!(all.contains(0) && all.contains(1_795_200_000_000) && all.contains(i64::MIN));
        // The compiled-in list: its own span.
        assert_eq!(
            Sealed::with_list(LIST, None).drop_window().span(),
            Some((1_795_132_800_000, 1_795_393_020_000))
        );
        // A list that does not parse, and one with no window line: all time.
        assert_eq!(Sealed::with_list("not a list\n", None).drop_window(), all);
        let none: String = LIST
            .lines()
            .filter(|l| !l.starts_with("window"))
            .map(|l| format!("{l}\n"))
            .collect();
        let g = Sealed::with_list(&none, None);
        assert!(g.windows().is_empty());
        assert_eq!(g.drop_window(), all);
    }

    #[test]
    fn the_time_gate_fails_closed_without_a_window_and_a_window_not_aligned_to_a_millisecond_has_no_gap(
    ) {
        let none: String = LIST
            .lines()
            .filter(|l| !l.starts_with("window"))
            .map(|l| format!("{l}\n"))
            .collect();
        let g = Sealed::with_list(&none, None);
        assert!(g.in_window(0) && g.in_window(1_795_200_000_000_000_000));
        // A window from 0.4 ms after 2026-11-20T00:00:00Z to 0.6 ms after 2026-11-23T00:17:00Z
        // (the list has whole seconds, so this is the conversion itself).
        let w = DropWindow::from_ns_span(1_795_132_800_000_400_000, 1_795_393_020_000_600_000);
        // The start is rounded down and the end up: the millisecond that holds the first instant
        // and the one that holds the last are both in the span.
        assert_eq!(w.span(), Some((1_795_132_800_000, 1_795_393_020_001)));
        assert!(w.contains(1_795_132_800_000) && w.contains(1_795_393_020_000));
        assert!(!w.contains(1_795_132_799_999) && !w.contains(1_795_393_020_001));
        // A window of whole milliseconds is exact, and the compiled-in list agrees.
        assert_eq!(
            DropWindow::from_ns_span(1_000_000, 3_000_000).span(),
            Some((1, 3))
        );
        assert_eq!(DropWindow::sealed().span(), sealed_window_ms());
    }
}
