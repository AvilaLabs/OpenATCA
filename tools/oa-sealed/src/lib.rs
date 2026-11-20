#![forbid(unsafe_code)]
//! `oa-sealed` (shell, B1): the one guard for the sealed hold-out data. The list of sealed paths
//! is `tools/sealed_paths.txt`, compiled in; `tools/sealed_guard.py` reads the same file, so the
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
//! The shared guard owns the path part of the rule only. A recording or row timestamped on UTC
//! day 2026-11-19 is a content rule: it needs the tool's own parser, so each reader keeps it.
//!
//! Requirements: OA-SEAL-001 and the following, in REQUIREMENTS.md.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

/// The list file, compiled in.
pub const LIST: &str = include_str!("../../sealed_paths.txt");

/// A deepest chain of links followed by hand before a path is refused as unresolvable.
const MAX_LINKS: usize = 40;

/// One sealed entry: directory components and a final name, exact or a prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The entry as written in the list.
    pub text: String,
    parts: Vec<String>,
    prefix: bool,
}

impl Entry {
    fn matches_at(&self, names: &[String]) -> bool {
        let n = self.parts.len();
        if names.len() < n {
            return false;
        }
        names
            .iter()
            .zip(&self.parts)
            .enumerate()
            .all(|(i, (a, b))| {
                if i + 1 == n && self.prefix {
                    a.starts_with(b.as_str())
                } else {
                    a == b
                }
            })
    }

    /// Whether `names` has the entry as consecutive components anywhere.
    fn named_in(&self, names: &[String]) -> bool {
        (0..names.len()).any(|i| names.get(i..).is_some_and(|t| self.matches_at(t)))
    }

    /// Whether `names` (below the root) is a proper ancestor directory of the entry.
    fn is_ancestor(&self, names: &[String]) -> bool {
        names.len() < self.parts.len() && self.parts.iter().zip(names).all(|(a, b)| a == b)
    }
}

/// The parsed list: the data root and the sealed entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
}

impl List {
    /// The list as lines (`root <path>`, then `seal <entry>` in file order): what the Python
    /// guard prints for `--dump`, so a test can compare the two parsers.
    pub fn dump(&self) -> String {
        let mut out = format!("root {}\n", self.root.display());
        for e in &self.entries {
            out.push_str(&format!("seal {}\n", e.text));
        }
        out
    }
}

/// Parses the list file text. Anything it does not understand is an error, never skipped.
pub fn parse_list(text: &str) -> Result<List, String> {
    let mut root: Option<PathBuf> = None;
    let mut entries = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (word, value) = line
            .split_once(' ')
            .ok_or_else(|| format!("line {}: expected `root <path>` or `seal <path>`", i + 1))?;
        let value = value.trim();
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
                let prefix = parts.last().is_some_and(|l| l.ends_with('*'));
                if prefix {
                    if let Some(l) = parts.last_mut() {
                        l.pop();
                        if l.is_empty() {
                            return Err(format!("line {}: bare `*`", i + 1));
                        }
                    }
                }
                entries.push(Entry {
                    text: value.to_owned(),
                    parts,
                    prefix,
                });
            }
            _ => return Err(format!("line {}: unknown word `{word}`", i + 1)),
        }
    }
    let root = root.ok_or("no root line")?;
    if entries.is_empty() {
        return Err("no seal lines".into());
    }
    Ok(List { root, entries })
}

/// A refusal: names the rule and the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub path: PathBuf,
    pub why: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: refused by OA-SEAL-001: {}",
            self.path.display(),
            self.why
        )
    }
}

impl std::error::Error for Refusal {}

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
    /// The guard for the list compiled in and the real data root (production).
    pub fn production() -> Sealed {
        Sealed::with_list(LIST, None)
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
                },
                root: PathBuf::new(),
                broken: true,
            },
        }
    }

    /// Refuses `path` if it is, is inside, resolves into, or holds a sealed entry.
    pub fn check(&self, path: &Path) -> Result<(), Refusal> {
        let refuse = |why: String| Refusal {
            path: path.to_path_buf(),
            why,
        };
        if self.broken {
            return Err(refuse(
                "the sealed list does not parse, so nothing is read".into(),
            ));
        }
        let real = match resolve(path, 0) {
            Ok(r) => r,
            Err(()) => {
                return Err(refuse(
                    "the path cannot be resolved (a link loop or an unreadable link)".into(),
                ))
            }
        };
        for e in &self.list.entries {
            let rule = |how: &str| {
                refuse(format!(
                    "{how} the sealed hold-out entry `{}` (tools/sealed_paths.txt); sealed data is never read",
                    e.text
                ))
            };
            if e.named_in(&names(path)) {
                return Err(rule("the path names"));
            }
            if e.named_in(&names(&real)) {
                return Err(rule("the path resolves to"));
            }
            {
                if let Ok(below) = real.strip_prefix(&self.root) {
                    let below = names(below);
                    if e.matches_at(&below) {
                        return Err(rule("the path is inside"));
                    }
                    if e.is_ancestor(&below) {
                        return Err(rule(
                            "the path is a directory that holds (name something below it)",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks `path`, then opens it for reading: the opener every file a tool reads goes through.
    pub fn open(&self, path: &Path) -> io::Result<File> {
        self.check(path).map_err(io::Error::other)?;
        File::open(path)
    }
}

/// Production check: `path` against the compiled-in list and the real data root.
pub fn refuse(path: &Path) -> Result<(), String> {
    Sealed::production().check(path).map_err(|e| e.to_string())
}

/// Production check as an I/O error, for an opener a loader calls on every file it reads.
pub fn refuse_io(path: &Path) -> io::Result<()> {
    Sealed::production().check(path).map_err(io::Error::other)
}

/// Production opener: refuses a sealed path, then opens the file for reading.
pub fn open(path: &Path) -> io::Result<File> {
    Sealed::production().open(path)
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
        assert_eq!(l.entries.len(), 3);
    }
}
