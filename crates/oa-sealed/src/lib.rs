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
//! owns the windows ([`in_sealed_window`]); each reader, which has the parser, drops and counts
//! the rows that fall in one.
//!
//! Requirements: OA-SEAL-001 and the following, in REQUIREMENTS.md.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
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

/// The parsed list: the data root, the sealed entries and the sealed windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
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
    Ok(List {
        root,
        entries,
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

/// What [`Sealed::walk_unsealed`] found: the unsealed files, sorted, and the count of sealed
/// entries (files or directories) that were skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    pub files: Vec<PathBuf>,
    pub skipped: usize,
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
        Ok(())
    }

    /// The sealed windows of the list.
    pub fn windows(&self) -> &[Window] {
        &self.list.windows
    }

    /// Whether `unix_ns` (nanoseconds since the Unix epoch, UTC) is in a sealed window. A guard
    /// whose list does not parse seals every instant.
    pub fn in_window(&self, unix_ns: i64) -> bool {
        self.broken || self.list.windows.iter().any(|w| w.contains(unix_ns))
    }

    /// Walks `root` for files: a directory or file the guard refuses is skipped and counted,
    /// never descended into or opened (a directory that only holds a sealed entry, such as the data
    /// root or `raw/adsblol`, is walked); the files are returned in sorted order. `root` itself
    /// being refused is an error (the caller names the path). A name that is a file is returned
    /// as the only file.
    pub fn walk_unsealed(&self, root: &Path) -> io::Result<Walk> {
        self.check_with(root, true)
            .map_err(|r| io::Error::other(r.why))?;
        let mut walk = Walk::default();
        self.walk_into(root, 0, &mut walk)?;
        walk.files.sort();
        Ok(walk)
    }

    fn walk_into(&self, dir: &Path, depth: usize, walk: &mut Walk) -> io::Result<()> {
        if !dir.is_dir() {
            walk.files.push(dir.to_path_buf());
            return Ok(());
        }
        if depth > MAX_WALK_DEPTH {
            return Ok(());
        }
        for e in fs::read_dir(dir)? {
            let p = e?.path();
            if self.check_with(&p, true).is_err() {
                walk.skipped += 1;
            } else if p.is_dir() {
                self.walk_into(&p, depth + 1, walk)?;
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
        let file = File::open(path)?;
        self.verify_open(path, &file)?;
        Ok(file)
    }

    /// Asks the kernel for the real path of an open file and refuses a sealed one. A platform
    /// without `/proc/self/fd` skips this (the check before the open stands).
    pub fn verify_open(&self, path: &Path, file: &File) -> io::Result<()> {
        use std::os::fd::AsRawFd;
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
        assert_eq!(l.entries.len(), 15);
        assert_eq!(l.windows.len(), 1);
    }
}
