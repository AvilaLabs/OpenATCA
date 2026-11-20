//! Every bin and example target of the workspace uses the sealed guard or is listed, with a
//! reason, in `exempt_targets.txt`. The targets are found from the manifests and the source
//! layout, not from a list kept here, so a new target with neither fails this test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The members of the root workspace.
fn members() -> Vec<PathBuf> {
    let text = fs::read_to_string(repo().join("Cargo.toml")).unwrap();
    let start = text.find("members = [").unwrap();
    let body = &text[start..];
    let body = &body[..body.find(']').unwrap()];
    body.split('"')
        .skip(1)
        .step_by(2)
        .map(|m| repo().join(m))
        .collect()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.map(|e| e.unwrap().path()) {
        if e.is_dir() {
            rs_files(&e, out);
        } else if e.extension().is_some_and(|x| x == "rs") {
            out.push(e);
        }
    }
}

struct Target {
    key: String,
    entry: PathBuf,
    /// The crate directory.
    krate: PathBuf,
    is_bin: bool,
}

fn targets() -> Vec<Target> {
    let mut out = Vec::new();
    for dir in members() {
        let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        let pkg = manifest
            .lines()
            .find_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))
            .unwrap()
            .to_string();
        let mut add = |key: String, entry: PathBuf, is_bin: bool| {
            out.push(Target {
                key,
                entry,
                krate: dir.clone(),
                is_bin,
            });
        };
        // [[bin]] and [[example]] blocks: name and path.
        let mut declared = BTreeSet::new();
        for kind in ["bin", "example"] {
            let head = format!("[[{kind}]]");
            for block in manifest.split(&head).skip(1) {
                let block = block.split("\n[").next().unwrap();
                let get = |k: &str| {
                    block.lines().find_map(|l| {
                        l.trim()
                            .strip_prefix(k)?
                            .trim_start()
                            .strip_prefix("= \"")?
                            .strip_suffix('"')
                            .map(str::to_string)
                    })
                };
                let name = get("name").unwrap();
                let path = get("path").unwrap_or_else(|| format!("src/bin/{name}.rs"));
                declared.insert(dir.join(&path));
                add(format!("{kind}:{name}"), dir.join(path), kind == "bin");
            }
        }
        // Implicit targets.
        let main = dir.join("src/main.rs");
        if main.exists() && !declared.contains(&main) {
            add(format!("bin:{pkg}"), main, true);
        }
        for (sub, is_bin, kind) in [("src/bin", true, "bin"), ("examples", false, "example")] {
            let Ok(rd) = fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for e in rd.map(|e| e.unwrap().path()) {
                let (name, entry) = if e.is_dir() {
                    (
                        e.file_name().unwrap().to_string_lossy().into_owned(),
                        e.join("main.rs"),
                    )
                } else {
                    (
                        e.file_stem().unwrap().to_string_lossy().into_owned(),
                        e.clone(),
                    )
                };
                if entry.exists() && !declared.contains(&entry) {
                    add(format!("{kind}:{name}"), entry, is_bin);
                }
            }
        }
    }
    out
}

/// The entry points of the guard that open, check or walk paths: a free function of `oa_sealed`
/// (or one imported from it), or the `Sealed` guard type and its methods that open or walk. A
/// `DropReport`, a window test or a drop line does not count: those never look at a path.
const GUARD_FNS: [&str; 7] = [
    "open",
    "read",
    "read_to_string",
    "refuse",
    "refuse_io",
    "walk_unsealed",
    "verify_open",
];

/// Whether the file calls a guard entry point (comments and strings do not count).
fn calls_guard(text: &str) -> bool {
    let lines = lex(text);
    let code: String = lines.iter().map(|l| format!("{}\n", l.code)).collect();
    // `oa_sealed::open(..)`, also as a value, and `Sealed::production()` and the like.
    if GUARD_FNS
        .iter()
        .any(|f| !token_at(&code, &format!("oa_sealed::{f}")).is_empty())
        || !token_at(&code, "Sealed").is_empty()
    {
        return true;
    }
    // `use oa_sealed::{open, refuse as r}`: the imported name is then called bare.
    use_paths(&code).iter().any(|(p, _)| {
        matches!(
            p.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
            ["oa_sealed", f] if GUARD_FNS.contains(f)
        )
    })
}

fn uses_guard(t: &Target, all: &[Target]) -> bool {
    let mentions = |p: &Path| calls_guard(&fs::read_to_string(p).unwrap());
    if !t.is_bin {
        return mentions(&t.entry);
    }
    // A bin: its entry file, and the crate's other source files that are not the entry of a
    // target (those are other programs of the crate).
    let entries: BTreeSet<&PathBuf> = all
        .iter()
        .filter(|o| o.krate == t.krate)
        .map(|o| &o.entry)
        .collect();
    let mut files = Vec::new();
    rs_files(&t.krate.join("src"), &mut files);
    files
        .iter()
        .filter(|f| **f == t.entry || !entries.contains(f))
        .any(|f| mentions(f))
}

fn exempt() -> BTreeMap<String, String> {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("exempt_targets.txt"))
        .unwrap();
    let mut m = BTreeMap::new();
    for l in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (k, reason) = l.split_once(' ').expect("`kind:name reason`");
        assert!(!reason.trim().is_empty(), "{k}: a reason is required");
        assert!(
            m.insert(k.to_string(), reason.to_string()).is_none(),
            "{k} twice"
        );
    }
    m
}

// REQ: OA-SEAL-006
#[test]
fn every_bin_and_example_target_uses_the_guard_or_is_exempt_with_a_reason() {
    let all = targets();
    assert!(all.len() > 20, "found only {} targets", all.len());
    let exempt = exempt();
    let mut bad = Vec::new();
    let mut seen = BTreeSet::new();
    for t in &all {
        let used = uses_guard(t, &all);
        seen.insert(t.key.clone());
        match (used, exempt.contains_key(&t.key)) {
            (false, false) => bad.push(format!("{}: neither uses oa_sealed nor is exempt", t.key)),
            (true, true) => bad.push(format!(
                "{}: exempt but uses oa_sealed (stale entry)",
                t.key
            )),
            _ => {}
        }
    }
    for k in exempt.keys() {
        if !seen.contains(k) {
            bad.push(format!("{k}: exempt but no such target"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// The calls that open, read, copy or run something from a path without the guard, as names: a
/// name counts when it is used at all (a call, or the function passed as a value), not only
/// when it is followed by `(`.
const BARE: [&str; 14] = [
    "File::open",
    "File::open_buffered",
    "File::options",
    "OpenOptions",
    "fs::read",
    "fs::read_to_string",
    "fs::read_dir",
    "fs::metadata",
    "fs::symlink_metadata",
    "std::io::stdin",
    "io::stdin",
    "fs::copy",
    "fs::hard_link",
    "Command::new",
];

/// The `std::fs` functions whose bare use is a read, copy or link, and the types whose `open`,
/// `options` and `new` start one.
const FS_FUNCS: [&str; 7] = [
    "read",
    "read_to_string",
    "read_dir",
    "metadata",
    "symlink_metadata",
    "copy",
    "hard_link",
];
const FS_TYPES: [&str; 3] = ["File", "OpenOptions", "Command"];

/// The comment that allows a bare call on the next code line (or its own line, when it trails
/// code): `// seal-allow: <why>`.
const ALLOW: &str = "seal-allow:";

/// The allow comments in the sources the rule covers, pinned exactly: adding one is a decision
/// made here, not slack in a budget.
const ALLOW_COUNT: usize = 111;

/// Text that names the real data root: a file that holds one must read through `oa_sealed`.
/// Written in two pieces so this file does not name the root itself.
/// The prefix covers every variable of the project (the raw root, the data and scds roots, the
/// clip, snapshot, recording, sensor clip, NASR and CIFP variables, and any other).
const ROOT_MARKERS: [&str; 2] = [concat!("OPENATCA", "_"), concat!("openatca", "-data")];

/// One source line after lexing: the code with strings emptied and comments cut, the text of the
/// string literals (never the comments), and the allow comment on the line, if any.
#[derive(Default, Debug)]
struct Line {
    code: String,
    strs: String,
    allow: Option<String>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Reads a string literal whose body starts at `i` (`hashes` is `Some(n)` for a raw string with
/// `n` `#`) and returns the index after it. The state is carried over lines: a newline inside
/// the string ends the current line and the string goes on.
fn read_string(
    ch: &[char],
    mut i: usize,
    hashes: Option<usize>,
    out: &mut Vec<Line>,
    cur: &mut Line,
) -> usize {
    while i < ch.len() {
        let c = ch[i];
        if c == '\n' {
            out.push(std::mem::take(cur));
            i += 1;
        } else if hashes.is_none() && c == '\\' {
            if ch.get(i + 1) == Some(&'\n') {
                out.push(std::mem::take(cur));
            } else if let Some(n) = ch.get(i + 1) {
                cur.strs.push(*n);
            }
            i += 2;
        } else if c == '"' && hashes.is_none_or(|h| (1..=h).all(|k| ch.get(i + k) == Some(&'#'))) {
            cur.code.push_str("\"\"");
            // Two strings never read as one text.
            cur.strs.push('\u{1}');
            return i + 1 + hashes.unwrap_or(0);
        } else {
            cur.strs.push(c);
            i += 1;
        }
    }
    i
}

/// Splits Rust source into [`Line`]s, with strings (plain, raw, byte), characters, lifetimes,
/// line comments and nested block comments told apart across lines.
fn lex(text: &str) -> Vec<Line> {
    let ch: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = Line::default();
    let (mut i, mut block) = (0usize, 0usize);
    while i < ch.len() {
        let c = ch[i];
        let next = ch.get(i + 1).copied();
        if c == '\n' {
            out.push(std::mem::take(&mut cur));
            i += 1;
        } else if block > 0 {
            if c == '/' && next == Some('*') {
                block += 1;
                i += 2;
            } else if c == '*' && next == Some('/') {
                block -= 1;
                i += 2;
            } else {
                i += 1;
            }
        } else if c == '/' && next == Some('/') {
            let mut j = i + 2;
            let mut s = String::new();
            while j < ch.len() && ch[j] != '\n' {
                s.push(ch[j]);
                j += 1;
            }
            if let Some(r) = s.trim_start().strip_prefix(ALLOW) {
                cur.allow = Some(r.trim().to_string());
            }
            i = j;
        } else if c == '/' && next == Some('*') {
            block = 1;
            i += 2;
        } else if c == '"' {
            i = read_string(&ch, i + 1, None, &mut out, &mut cur);
        } else if matches!(c, 'r' | 'b' | 'c') && (i == 0 || !is_ident(ch[i - 1])) {
            // `r"`, `r#"`, `b"`, `br#"`, `c"`: a string with a prefix.
            let mut j = i + usize::from(c != 'r');
            let raw = ch.get(j) == Some(&'r');
            if raw {
                j += 1;
            }
            let mut h = 0;
            while raw && ch.get(j) == Some(&'#') {
                h += 1;
                j += 1;
            }
            if ch.get(j) == Some(&'"') && (raw || j > i) {
                i = read_string(&ch, j + 1, raw.then_some(h), &mut out, &mut cur);
            } else {
                cur.code.push(c);
                i += 1;
            }
        } else if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 3;
                while j < ch.len() && ch[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
            } else if ch.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                cur.code.push(c);
                i += 1;
            }
        } else {
            cur.code.push(c);
            i += 1;
        }
    }
    out.push(cur);
    out
}

/// What the file's imports and aliases make callable bare: functions of `std::fs` (under their
/// local names), the types `File`, `OpenOptions` and `Command` (under their local names), and
/// local names of the modules `std::fs` and `std::process`.
#[derive(Default, Debug)]
struct Imports {
    funcs: Vec<String>,
    types: Vec<String>,
    modules: Vec<String>,
}

/// A cursor over the text of a `use` tree.
struct Tree<'a> {
    s: &'a [char],
    pos: usize,
}

impl Tree<'_> {
    fn ws(&mut self) {
        while self.s.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn ident(&mut self) -> Option<String> {
        self.ws();
        let start = self.pos;
        while self.s.get(self.pos).copied().is_some_and(is_ident) {
            self.pos += 1;
        }
        (self.pos > start).then(|| self.s[start..self.pos].iter().collect())
    }

    /// Flattens one use tree below `prefix` into (path, local name) pairs.
    fn parse(&mut self, prefix: &[String], out: &mut Vec<(Vec<String>, String)>) {
        let mut segs = prefix.to_vec();
        loop {
            self.ws();
            match self.s.get(self.pos) {
                Some(':') => {
                    self.pos += 1;
                    continue;
                }
                Some('{') => {
                    self.pos += 1;
                    loop {
                        self.parse(&segs, out);
                        self.ws();
                        match self.s.get(self.pos) {
                            Some(',') => self.pos += 1,
                            Some('}') => {
                                self.pos += 1;
                                return;
                            }
                            _ => return,
                        }
                        self.ws();
                        if self.s.get(self.pos) == Some(&'}') {
                            self.pos += 1;
                            return;
                        }
                    }
                }
                Some('*') => {
                    self.pos += 1;
                    segs.push("*".into());
                    out.push((segs, "*".into()));
                    return;
                }
                _ => {}
            }
            let Some(id) = self.ident() else { return };
            segs.push(id);
            self.ws();
            if self.s.get(self.pos) == Some(&':') && self.s.get(self.pos + 1) == Some(&':') {
                self.pos += 2;
                continue;
            }
            break;
        }
        let last = segs.last().cloned().unwrap_or_default();
        let save = self.pos;
        let alias = match self.ident() {
            Some(a) if a == "as" => self.ident(),
            _ => {
                self.pos = save;
                None
            }
        };
        out.push((segs, alias.unwrap_or(last)));
    }
}

/// Every `use` statement of `code` as (path, local name) pairs.
fn use_paths(code: &str) -> Vec<(Vec<String>, String)> {
    let ch: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < ch.len() {
        let hit = ch[i..i + 3] == ['u', 's', 'e']
            && (i == 0 || !is_ident(ch[i - 1]))
            && ch[i + 3].is_whitespace();
        if hit {
            let end = ch[i..]
                .iter()
                .position(|c| *c == ';')
                .map_or(ch.len(), |e| i + e);
            let body = &ch[i + 3..end];
            Tree { s: body, pos: 0 }.parse(&[], &mut out);
            i = end;
        }
        i += 1;
    }
    out
}

fn imports(lines: &[Line]) -> Imports {
    let code: String = lines.iter().map(|l| format!("{}\n", l.code)).collect();
    let mut imp = Imports::default();
    for (segs, alias) in use_paths(&code) {
        let mut p: Vec<&str> = segs.iter().map(String::as_str).collect();
        let mut name = alias.clone();
        if p.last() == Some(&"self") {
            p.pop();
            if alias == "self" {
                name = p.last().copied().unwrap_or("").to_string();
            }
        }
        match p.as_slice() {
            ["std", "fs"] | ["std", "process"] => imp.modules.push(name),
            ["std", "fs", f] if FS_FUNCS.contains(f) => imp.funcs.push(name),
            ["std", "fs", t] | ["std", "process", t] if FS_TYPES.contains(t) => {
                imp.types.push(name)
            }
            ["std", "fs", "*"] => {
                imp.funcs.extend(FS_FUNCS.map(String::from));
                imp.types.extend(["File", "OpenOptions"].map(String::from));
            }
            ["std", "process", "*"] => imp.types.push("Command".into()),
            _ => {}
        }
    }
    // `type X = File;` (also through a path or with generics) makes `X` a bare type.
    let mut rest = code.as_str();
    while let Some(i) = rest.find("type ") {
        let before = rest[..i].chars().next_back();
        rest = &rest[i + 5..];
        if before.is_some_and(is_ident) {
            continue;
        }
        let Some((head, tail)) = rest.split_once(';') else {
            break;
        };
        let Some((lhs, value)) = head.split_once('=') else {
            continue;
        };
        let lhs: String = lhs.trim().chars().take_while(|c| is_ident(*c)).collect();
        let value = value.trim().split('<').next().unwrap_or("").trim();
        let last = value.rsplit("::").next().unwrap_or("");
        if !lhs.is_empty() && (FS_TYPES.contains(&last) || imp.types.iter().any(|t| t == last)) {
            imp.types.push(lhs);
        }
        rest = tail;
    }
    imp
}

/// Offsets in `code` where `tok` is used as a whole name (not inside a longer identifier).
fn token_at(code: &str, tok: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = code[from..].find(tok) {
        let at = from + i;
        let before = code[..at].chars().next_back();
        let after = code[at + tok.len()..].chars().next();
        if before.is_none_or(|c| !is_ident(c)) && after.is_none_or(|c| !is_ident(c)) {
            out.push(at);
        }
        from = at + tok.len();
    }
    out
}

/// Whether `name`, an imported function, is used as a value or called: not a method, a path
/// tail, a binding, a field or a parameter of that name.
fn uses_imported(code: &str, name: &str) -> bool {
    token_at(code, name).into_iter().any(|at| {
        let before = code[..at].trim_end();
        let after = code[at + name.len()..].trim_start();
        let word = before.rsplit(|c: char| !is_ident(c)).next().unwrap_or("");
        !(before.ends_with('.')
            || before.ends_with("::")
            || matches!(
                word,
                "fn" | "let"
                    | "mut"
                    | "ref"
                    | "struct"
                    | "enum"
                    | "mod"
                    | "const"
                    | "static"
                    | "type"
            )
            || (after.starts_with(':') && !after.starts_with("::")))
    })
}

/// `code` with the generic spellings of a path undone: `<File>::open` and `<std::fs::File>::open`
/// become `File::open`, and a turbofish (`File::<T>::open`) is dropped, so the names are found.
fn normalise(code: &str) -> String {
    let ch: Vec<char> = code.chars().collect();
    let close = |from: usize| -> Option<usize> {
        let mut depth = 0i32;
        for (k, c) in ch.iter().enumerate().skip(from) {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
        None
    };
    let mut out = String::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i] == ':' && ch.get(i + 1) == Some(&':') && ch.get(i + 2) == Some(&'<') {
            if let Some(e) = close(i + 2) {
                i = e + 1;
                continue;
            }
        }
        if ch[i] == '<' && (i == 0 || !is_ident(ch[i - 1])) {
            if let Some(e) = close(i) {
                if ch.get(e + 1) == Some(&':') && ch.get(e + 2) == Some(&':') {
                    let inner: String = ch[i + 1..e].iter().collect();
                    out.push_str(inner.split(" as ").next().unwrap_or("").trim());
                    i = e + 1;
                    continue;
                }
            }
        }
        out.push(ch[i]);
        i += 1;
    }
    out
}

/// Whether the line has an `include_bytes!` or `include_str!` whose argument is not a plain
/// string literal (an `env!` or `concat!` path is built from the environment).
fn is_computed_include(code: &str) -> bool {
    ["include_bytes!", "include_str!"].iter().any(|m| {
        let mut from = 0;
        while let Some(i) = code[from..].find(m) {
            let after = code[from + i + m.len()..].trim_start();
            let arg = after
                .strip_prefix('(')
                .or_else(|| after.strip_prefix('['))
                .or_else(|| after.strip_prefix('{'))
                .map_or("", str::trim_start);
            if !(arg.starts_with('"') || arg.starts_with("r\"") || arg.starts_with("r#")) {
                return true;
            }
            from += i + m.len();
        }
        false
    })
}

/// Whether the code line uses a bare open, read, copy, link or command, given the file's imports.
fn is_bare(code: &str, imp: &Imports) -> bool {
    let norm = normalise(code);
    if norm != code && is_bare_plain(&norm, imp) {
        return true;
    }
    is_computed_include(code) || is_bare_plain(code, imp)
}

/// [`is_bare`] on a line whose generic spellings are already undone.
fn is_bare_plain(code: &str, imp: &Imports) -> bool {
    if BARE.iter().any(|b| !token_at(code, b).is_empty()) {
        return true;
    }
    // The `read_dir` method, whatever it is called on.
    if token_at(code, "read_dir")
        .iter()
        .any(|&at| code[..at].ends_with('.'))
    {
        return true;
    }
    if imp.funcs.iter().any(|f| uses_imported(code, f)) {
        return true;
    }
    let methods = ["open", "open_buffered", "options", "new"];
    imp.types.iter().any(|t| {
        methods
            .iter()
            .any(|m| !token_at(code, &format!("{t}::{m}")).is_empty())
    }) || imp.modules.iter().any(|m| {
        FS_FUNCS
            .iter()
            .any(|f| !token_at(code, &format!("{m}::{f}")).is_empty())
    })
}

/// One bare call: line number, the trimmed line, and whether an allow comment covers it.
type Call = (usize, String, bool);

/// What the scan of one file found: the bare calls, the allow comments, the allows that cover
/// no bare call or give no reason, the `mod name;` declarations (with whether a `#[cfg(test)]`
/// is on them) and whether the file names the real data root.
struct Scan {
    calls: Vec<Call>,
    allows: usize,
    stale: Vec<(usize, String)>,
    file_mods: Vec<(String, bool)>,
    names_root: bool,
}

/// `t` without a leading visibility (`pub`, `pub(crate)`, `pub(in path)`).
fn no_vis(t: &str) -> &str {
    let Some(r) = t.strip_prefix("pub") else {
        return t;
    };
    if let Some(r) = r.strip_prefix('(') {
        return r.split_once(')').map_or(t, |(_, r)| r.trim_start());
    }
    if r.starts_with(' ') {
        r.trim_start()
    } else {
        t
    }
}

/// The `mod NAME;` of a code line, if it is one.
fn file_mod(t: &str) -> Option<String> {
    let t = no_vis(t);
    let name = t.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    name.chars().all(is_ident).then(|| name.to_string())
}

/// Whether a string of the line (not its comments or identifiers: `OPENATCA_SOURCE` is a constant
/// of the code) names the real data root, as an environment variable or a path.
fn line_names_root(l: &Line) -> bool {
    if ROOT_MARKERS.iter().any(|m| l.strs.contains(m)) {
        return true;
    }
    // A name assembled from pieces by `concat!` or `format!`: the string pieces of the line, run
    // together, name the root.
    // (The line that defines the markers themselves is the one exception.)
    if (l.code.contains("concat!") || l.code.contains("format!"))
        && !l.code.contains("ROOT_MARKERS")
    {
        let joined: String = l
            .strs
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        return ROOT_MARKERS.iter().any(|m| joined.contains(m));
    }
    false
}

/// Whether a `pub use` line brings in something of `std::fs` or `std::process`.
fn reexports_fs(t: &str) -> bool {
    use_paths(t).iter().any(|(p, _)| {
        matches!(
            p.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
            ["std", "fs" | "process", ..]
        )
    })
}

/// The bare calls of `text`. An allow comment covers only the call on the next code line (or on
/// its own line when it trails code); one that covers no call, or has no reason, is stale.
/// With `skip_tests`, a `#[cfg(test)]` followed by a `mod` item is skipped with its body (any
/// other `#[cfg(test)]` item is scanned), and an allow inside it does not count.
fn scan(text: &str, skip_tests: bool) -> Scan {
    let lines = lex(text);
    let imp = imports(&lines);
    let mut skip_depth: Option<i64> = None;
    let mut pending_cfg_test = false;
    let mut attr_depth = 0i64;
    let mut call_lines: Vec<usize> = Vec::new();
    let mut allows: Vec<(usize, String)> = Vec::new();
    let mut file_mods = Vec::new();
    let mut names_root = false;
    let mut test_modules: Vec<(usize, usize)> = Vec::new();
    let mut module_from = 0usize;
    for (i, l) in lines.iter().enumerate() {
        if line_names_root(l) {
            names_root = true;
        }
        let code = &l.code;
        let t = code.trim();
        if let Some(depth) = skip_depth.as_mut() {
            *depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
            if *depth <= 0 {
                skip_depth = None;
                test_modules.push((module_from, i));
            }
            continue;
        }
        if t.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            continue;
        }
        // Attributes between the cfg and the item, also over several lines.
        if pending_cfg_test && (attr_depth > 0 || t.starts_with("#[")) {
            attr_depth += code.matches('[').count() as i64 - code.matches(']').count() as i64;
            continue;
        }
        if pending_cfg_test && !t.is_empty() {
            pending_cfg_test = false;
            if let Some(m) = file_mod(t) {
                file_mods.push((m, true));
                continue;
            }
            if skip_tests && no_vis(t).starts_with("mod ") {
                let depth = code.matches('{').count() as i64 - code.matches('}').count() as i64;
                skip_depth = (depth > 0).then_some(depth);
                module_from = i;
                if skip_depth.is_none() {
                    test_modules.push((i, i));
                }
                continue;
            }
        }
        if let Some(m) = file_mod(t) {
            file_mods.push((m, false));
        }
        if let Some(why) = &l.allow {
            allows.push((i, why.clone()));
        }
        if no_vis(t).starts_with("use ") {
            // A `pub use` of `std::fs` or `std::process` names re-exports the functions to the
            // rest of the crate, where no import of its own shows them.
            if no_vis(t) != t && reexports_fs(t) {
                call_lines.push(i);
            }
            continue;
        }
        if is_bare(code, &imp) {
            call_lines.push(i);
        }
    }
    // A skipped test module that names the real data root is not skipped: what it opens is
    // scanned like any other code.
    for (from, to) in test_modules {
        if (from..=to).any(|k| line_names_root(&lines[k])) {
            for (k, l) in lines.iter().enumerate().take(to + 1).skip(from) {
                let t = l.code.trim();
                if let Some(why) = &l.allow {
                    allows.push((k, why.clone()));
                }
                if no_vis(t).starts_with("use ") {
                    continue;
                }
                if is_bare(&l.code, &imp) {
                    call_lines.push(k);
                }
            }
        }
    }
    call_lines.sort_unstable();
    // The line an allow covers: its own when it trails code, else the next line of code that is
    // not an attribute.
    let target = |a: usize| -> Option<usize> {
        if !lines[a].code.trim().is_empty() {
            return Some(a);
        }
        (a + 1..lines.len()).find(|&j| {
            let t = lines[j].code.trim();
            !t.is_empty() && !t.starts_with("#[")
        })
    };
    let covered: BTreeSet<usize> = allows.iter().filter_map(|(a, _)| target(*a)).collect();
    let src: Vec<&str> = text.lines().collect();
    let calls = call_lines
        .iter()
        .map(|&i| {
            (
                i + 1,
                src.get(i).copied().unwrap_or("").trim().to_string(),
                covered.contains(&i),
            )
        })
        .collect();
    let mut stale = Vec::new();
    for (a, why) in &allows {
        if why.is_empty() {
            stale.push((a + 1, "the allow has no reason".to_string()));
        }
        if !target(*a).is_some_and(|t| call_lines.contains(&t)) {
            stale.push((
                a + 1,
                "an allow that covers no bare call on its line or the next code line (stale)"
                    .to_string(),
            ));
        }
    }
    Scan {
        calls,
        allows: allows.len(),
        stale,
        file_mods,
        names_root,
    }
}

/// Tools that read only the repository's own source and register files, never data, with the
/// reason; their sources are not scanned.
const SOURCE_TOOLS: [(&str, &str); 1] = [(
    "oa-trace",
    "walks the repository's own sources and requirement registers, and reads no input data",
)];

/// The member crates the rule covers: all but the accepted exceptions (checked apart) and the
/// source tools.
fn covered_crates() -> Vec<PathBuf> {
    members()
        .into_iter()
        .filter(|k| {
            let name = k.file_name().unwrap().to_string_lossy().into_owned();
            !EXCEPTION_CRATES.iter().any(|(n, _)| *n == name)
                && !SOURCE_TOOLS.iter().any(|(n, _)| *n == name)
        })
        .collect()
}

/// The files `mod name;` can mean from `file`: the module file beside it, or in a directory of
/// the file's own name (the form of a file that is not a crate or module root).
fn module_files(file: &Path, name: &str) -> Vec<PathBuf> {
    let dir = file.parent().unwrap();
    let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
    let mut out = Vec::new();
    for base in [dir.to_path_buf(), dir.join(&stem)] {
        out.push(base.join(format!("{name}.rs")));
        out.push(base.join(name).join("mod.rs"));
    }
    out
}

/// The files that are declared as `#[cfg(test)] mod name;` (and so test code), with every module
/// they declare in turn. No file is skipped by what it is called.
fn test_module_files(all: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mods: Vec<(&PathBuf, Vec<(String, bool)>)> = all
        .iter()
        .map(|f| (f, scan(&fs::read_to_string(f).unwrap(), true).file_mods))
        .collect();
    let mut skipped: BTreeSet<PathBuf> = BTreeSet::new();
    loop {
        let before = skipped.len();
        for (f, ms) in &mods {
            let all_test = skipped.contains(*f);
            for (name, is_test) in ms {
                if *is_test || all_test {
                    for c in module_files(f, name) {
                        if all.contains(&c) {
                            skipped.insert(c);
                        }
                    }
                }
            }
        }
        if skipped.len() == before {
            return skipped;
        }
    }
}

/// The files the bare-call rule covers in full: `src/` and `examples/` of every covered crate
/// (core crates included), without the files that are `#[cfg(test)]` modules.
fn covered_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for k in covered_crates() {
        let mut all = Vec::new();
        for sub in ["src", "examples"] {
            rs_files(&k.join(sub), &mut all);
        }
        all.sort();
        let skipped = test_module_files(&all);
        // A test file that names the real data root is covered like any other.
        files.extend(all.into_iter().filter(|f| {
            !skipped.contains(f) || scan(&fs::read_to_string(f).unwrap(), true).names_root
        }));
    }
    files
}

/// `tests/`, `benches/` and `build.rs` of every covered crate. A file of these that names the
/// real data root must read through `oa_sealed`; bare reads of fixtures and temp dirs stay
/// allowed in the others.
fn harness_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for k in covered_crates() {
        for sub in ["tests", "benches"] {
            rs_files(&k.join(sub), &mut files);
        }
        if k.join("build.rs").exists() {
            files.push(k.join("build.rs"));
        }
    }
    files.sort();
    files
}

// REQ: OA-SEAL-006
#[test]
fn a_bare_open_or_read_in_any_crate_needs_an_allow_comment_with_a_reason_and_no_allow_is_stale() {
    let mut bad = Vec::new();
    let mut allows = 0;
    let mut check = |f: &Path, strict_only_if_root: bool| {
        let text = fs::read_to_string(f).unwrap();
        let sc = scan(&text, !strict_only_if_root);
        if strict_only_if_root && !sc.names_root {
            return;
        }
        allows += sc.allows;
        let rel = f.strip_prefix(repo()).unwrap().display().to_string();
        for (n, line, ok) in sc.calls {
            if !ok {
                bad.push(format!("{rel}:{n}: {line}"));
            }
        }
        for (n, why) in sc.stale {
            bad.push(format!("{rel}:{n}: {why}"));
        }
    };
    for f in covered_files() {
        check(&f, false);
    }
    for f in harness_files() {
        check(&f, true);
    }
    assert!(
        bad.is_empty(),
        "a bare open, read, copy, link or command needs `// {ALLOW} <why>` (a write, a temp dir, a /proc file) or the guard:\n{bad:#?}"
    );
    assert_eq!(
        allows, ALLOW_COUNT,
        "the allow comments are pinned exactly: {allows} found, {ALLOW_COUNT} pinned; change ALLOW_COUNT with the allow"
    );
}

fn bare_lines(text: &str) -> Vec<usize> {
    scan(text, true).calls.iter().map(|c| c.0).collect()
}

// REQ: OA-SEAL-006
#[test]
fn a_target_counts_as_guarded_only_with_a_call_to_a_guard_entry_point() {
    // A drop report, a window test and a mere mention in a comment or a string do not count.
    for text in [
        "fn main() { let mut d = oa_sealed::DropReport::new(); d.add(1); }",
        "fn f(t: i64) -> bool { oa_sealed::in_sealed_window(t) }",
        "// oa_sealed::open(p)\nfn main() {}",
        "fn main() { let s = \"oa_sealed::open\"; }",
        "use oa_sealed::DropReport;\nfn main() {}",
        "fn main() { oa_sealed::note_window_drops(1); let _ = oa_sealed::drop_line(0); }",
    ] {
        assert!(!calls_guard(text), "{text}");
    }
    // The entry points that open, check or walk paths do.
    for text in [
        "fn main() { oa_sealed::open(p); }",
        "fn main() { oa_sealed::refuse(p); }",
        "fn main() { oa_sealed::refuse_io(p); }",
        "fn main() { oa_sealed::read_to_string(p); }",
        "fn main() { oa_sealed::walk_unsealed(p); }",
        "use oa_sealed::{open, DropReport};\nfn main() { open(p); }",
        "use oa_sealed::Sealed;\nfn f(g: &Sealed) {}",
        "fn main() { oa_sealed::Sealed::production().check(p); }",
    ] {
        assert!(calls_guard(text), "{text}");
    }
}

// REQ: OA-SEAL-006
#[test]
fn the_scan_finds_a_metadata_probe_and_standard_input_as_bare() {
    assert_eq!(
        bare_lines("let a = std::fs::metadata(p);\nlet b = fs::symlink_metadata(q);\nlet c = std::io::stdin();\nlet d = io::stdin();\nlet e = file.metadata();\n"),
        [1, 2, 3, 4]
    );
    assert_eq!(
        bare_lines(
            "use std::fs::{metadata, symlink_metadata as sm};\nfn a() { metadata(p); sm(q); }\n"
        ),
        [2]
    );
}

// REQ: OA-SEAL-006
#[test]
fn the_bare_call_scan_finds_calls_imports_and_aliases_and_honours_allows_strings_and_test_modules()
{
    let text = "fn a() { let f = File::open(p); }\n// seal-allow: temp dir\nlet g = fs::read(q);\nlet h = fs::read_dir(d); // seal-allow: output\n/// doc: fs::read(x)\nlet i = oa_sealed::open(p);\nlet s = \"fs::read( // not code\";\n#[cfg(test)]\nmod t { fn x() { File::open(z); } }\nlet j = OpenOptions::new();\nlet k = std::process::Command::new(\"x\");\n";
    let sc = scan(text, true);
    let flags: Vec<(usize, bool)> = sc.calls.iter().map(|(n, _, ok)| (*n, *ok)).collect();
    assert_eq!(
        flags,
        [(1, false), (3, true), (4, true), (10, false), (11, false)]
    );
    assert_eq!(sc.allows, 2);
    assert!(sc.stale.is_empty(), "{:?}", sc.stale);
    // `use std::fs::{...}` imports and an alias are caught.
    let text = "use std::fs::{read_to_string as slurp, File as F};\nuse std::fs as filesystem;\nfn a() {\n let a = slurp(p);\n let b = F::open(q);\n let c = filesystem::read(r);\n let d = x.read_to_string();\n}\n";
    assert_eq!(bare_lines(text), [4, 5, 6]);
    // A cfg(test) item that is not a module is scanned; the test module after it is not.
    let text = "#[cfg(test)]\nfn helper() { fs::read(p); }\n#[cfg(test)]\nmod tests {\n fn t() { fs::read(q); }\n}\nfn live() { fs::read(r); }\n";
    assert_eq!(bare_lines(text), [2, 7]);
    // A stale allow and an allow without a reason are reported.
    let text = "// seal-allow: gone\nlet a = 1;\nlet b = 2;\nlet c = 3;\n// seal-allow:\nlet d = fs::read(p);\n";
    let sc = scan(text, true);
    assert_eq!(sc.stale.len(), 2, "{:?}", sc.stale);
}

// REQ: OA-SEAL-006
#[test]
fn the_scan_finds_generic_paths_computed_includes_re_exports_and_a_test_module_naming_the_root() {
    // `<File>::open`, `<std::fs::File>::open` and a turbofish are the same calls.
    assert_eq!(
        bare_lines("let a = <File>::open(p);\nlet b = <std::fs::File>::open(p);\nlet c = File::<X>::open(p);\nlet d = <Foo as Bar>::open(p);\nlet e = fs::read::<_>(p);\n"),
        [1, 2, 3, 5]
    );
    // An include whose argument is computed is read from the environment; a literal is a fixture.
    assert_eq!(
        bare_lines("let a = include_bytes!(\"data/x.bin\");\nlet b = include_str!(concat!(env!(\"D\"), \"/x\"));\nlet c = include_bytes!(env!(\"P\"));\nlet d = include_str!(r#\"y\"#);\n"),
        [2, 3]
    );
    // A re-export of the file system or process module to the rest of the crate.
    assert_eq!(
        bare_lines("pub use std::fs::read_to_string;\npub(crate) use std::fs;\nuse std::fs::read;\npub use std::collections::BTreeMap;\npub use std::process::{Command as C};\n"),
        [1, 2, 5]
    );
    // A cfg(test) module is skipped, unless it names the real data root.
    let root = format!("{}DATA", ROOT_MARKERS[0]);
    let text = format!(
        "#[cfg(test)]\nmod a {{\n fn t() {{ fs::read(p); }}\n}}\n#[cfg(test)]\nmod b {{\n fn t() {{ let r = std::env::var(\"{root}\"); fs::read(q); }}\n}}\n"
    );
    assert_eq!(bare_lines(&text), [7]);
}

// REQ: OA-SEAL-006
#[test]
fn the_scan_finds_function_values_globs_grouped_uses_aliases_methods_and_multi_line_strings() {
    // A function passed as a value, with no `(`.
    assert_eq!(
        bare_lines("let a = v.iter().map(fs::read_to_string);\nlet b = Some(File::open);\nlet c = fs::read_to_string_x(p);\nlet d = MyFile::open(p);\n"),
        [1, 2]
    );
    // A glob, a grouped use, and an alias of the module or the type.
    assert_eq!(
        bare_lines("use std::fs::*;\nfn a() { let x = read(p); let y = File::open(q); let z = std::fs::read(w); }\n"),
        [2]
    );
    assert_eq!(
        bare_lines("use std::{fs::{self, File as F, read_to_string as slurp}, io};\nfn a() {\n let a = fs::read(p);\n let b = F::open(q);\n let c = slurp(r);\n let d = io::read(s);\n}\n"),
        [3, 4, 5]
    );
    assert_eq!(
        bare_lines("use std::process::Command as C;\nuse std::process as pr;\nfn a() {\n let a = C::new(\"x\");\n let b = pr::Command::new(\"y\");\n}\n"),
        [4, 5]
    );
    assert_eq!(
        bare_lines("type Reader = std::fs::File;\ntype Cmd = Command;\nfn a() {\n let a = Reader::open(p);\n let b = Cmd::new(\"x\");\n}\n"),
        [4, 5]
    );
    // The `read_dir` method.
    assert_eq!(
        bare_lines("let a = path.read_dir();\nlet b = d.read_dir().unwrap();\n"),
        [1, 2]
    );
    // A binding of an imported name is not a call; a method of that name is not either.
    assert_eq!(
        bare_lines("use std::fs::read;\nfn a(read: bool) {\n let read = 1;\n x.read();\n}\n"),
        Vec::<usize>::new()
    );
    // A string that runs over lines, and a raw string, hide what is inside them.
    let text = "let a = \"first line\nfs::read(p) inside the string\nlast\"; let b = fs::read(q);\nlet c = r#\"\nFile::open(\"x\")\n\"#;\nlet d = br\"fs::copy\";\nlet e = '\"'; let f = fs::copy(a, b);\n/* fs::read(p)\nFile::open */ let g = File::open(r);\n";
    assert_eq!(bare_lines(text), [3, 8, 10]);
    // An allow covers only the call on the next code line.
    let text = "// seal-allow: one\nlet a = fs::read(p);\nlet b = fs::read(q);\n";
    let sc = scan(text, true);
    let flags: Vec<(usize, bool)> = sc.calls.iter().map(|(n, _, ok)| (*n, *ok)).collect();
    assert_eq!(flags, [(2, true), (3, false)]);
    // An allow inside a skipped test module is not counted.
    let sc = scan(
        "#[cfg(test)]\nmod t {\n // seal-allow: x\n fn a() { fs::read(p); }\n}\n",
        true,
    );
    assert_eq!((sc.allows, sc.calls.len()), (0, 0));
    // A file that names the data root is told from one that does not (strings count, comments not).
    let var = format!("{}CLIP", ROOT_MARKERS[0]);
    assert!(scan(&format!("let r = std::env::var(\"{var}\");"), true).names_root);
    assert!(scan(&format!("let r = \"/x/{}/raw\";", ROOT_MARKERS[1]), true).names_root);
    assert!(!scan(&format!("// {var} is read elsewhere\nlet r = 1;"), true).names_root);
    // A root name assembled by `concat!` or `format!` from pieces counts too.
    let (a, b) = (&ROOT_MARKERS[0][..4], &ROOT_MARKERS[0][4..]);
    assert!(
        scan(
            &format!("let n = concat!(\"{a}\", \"{b}\", \"CLIP\");"),
            true
        )
        .names_root
    );
    assert!(scan(&format!("let n = format!(\"{a}{b}{{}}\", 1);"), true).names_root);
    assert!(!scan("let n = format!(\"x{}\", 1);", true).names_root);
}

// REQ: OA-SEAL-006
#[test]
fn a_file_is_skipped_only_when_a_cfg_test_module_declares_it_never_by_its_name() {
    let dir = std::env::temp_dir().join(format!("oa-sealed-testmods-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("shell/inner")).unwrap();
    let w = |p: &str, t: &str| fs::write(dir.join(p), t).unwrap();
    w("lib.rs", "mod shell;\n#[cfg(test)]\nmod tests;\n");
    w(
        "shell/mod.rs",
        "mod live;\n#[cfg(test)]\n#[allow(unused)]\nmod tests;\n",
    );
    w("tests.rs", "mod nested;\nfn a() {}\n");
    w("nested.rs", "fn b() {}\n");
    w("shell/live.rs", "fn c() {}\n");
    // Named like tests, but declared without a cfg(test): scanned.
    w("shell/tests_index.rs", "fn d() {}\n");
    w("shell/tests.rs", "fn e() {}\n");
    let mut all = Vec::new();
    rs_files(&dir, &mut all);
    all.sort();
    let skipped = test_module_files(&all);
    let names: Vec<String> = skipped
        .iter()
        .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["nested.rs", "shell/tests.rs", "tests.rs"]);
    fs::remove_dir_all(&dir).ok();
}

/// The accepted exceptions: oa-arinc424 must stay free of dependencies (so it cannot use
/// oa-sealed) and oa-geo is a core crate (core crates do not depend on the shell crate oa-sealed).
/// Their opens are in tests only, on fixtures and on the env-named inputs `OPENATCA_CIFP` and
/// `OPENATCA_GEODTEST`; each file and its count of bare calls is pinned here.
const EXCEPTION_CRATES: [(&str, &[(&str, usize)]); 2] = [
    (
        "oa-arinc424",
        &[
            ("tests/cifp_full.rs", 1),
            ("tests/common/mod.rs", 1),
            ("tests/file_and_limits.rs", 3),
            ("tests/nasr_cross.rs", 2),
        ],
    ),
    (
        "oa-geo",
        &[
            ("tests/general.rs", 3),
            ("tests/geodesic.rs", 1),
            ("tests/purity.rs", 2),
        ],
    ),
];

// REQ: OA-SEAL-006
#[test]
fn the_arinc424_and_geo_exceptions_gain_no_bare_open_beyond_the_pinned_test_lines() {
    for (krate, pinned) in EXCEPTION_CRATES {
        let root = repo().join("crates").join(krate);
        let mut files = Vec::new();
        for sub in ["src", "examples", "tests", "benches"] {
            rs_files(&root.join(sub), &mut files);
        }
        let mut found: BTreeMap<String, usize> = BTreeMap::new();
        for f in files {
            // The same scan as the covered crates, with nothing skipped: a test module inside a
            // source file counts too.
            let n = scan(&fs::read_to_string(&f).unwrap(), false).calls.len();
            if n > 0 {
                let rel = f.strip_prefix(&root).unwrap().to_string_lossy().to_string();
                found.insert(rel, n);
            }
        }
        let want: BTreeMap<String, usize> =
            pinned.iter().map(|(f, n)| (f.to_string(), *n)).collect();
        assert_eq!(
            found, want,
            "{krate}: a bare open or read was added (accepted exceptions are pinned, OA-SEAL-006)"
        );
    }
}

/// The dependencies that read or map files by a path of their own, so they could bypass the
/// guard. One in a manifest needs a `# seal-note: <why>` comment on its line or the line above.
const FILE_CRATES: [&str; 4] = ["libc", "memmap", "memmap2", "walkdir"];

fn unnoted_file_crates(manifest: &str) -> Vec<String> {
    let lines: Vec<&str> = manifest.lines().collect();
    let mut bad = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        let key = t
            .split(['=', '.', ' ', ']'])
            .next()
            .unwrap_or("")
            .trim_start_matches('[');
        let table = t
            .strip_prefix('[')
            .and_then(|r| r.rsplit_once("dependencies."))
            .map(|(_, n)| n.trim_end_matches(']'));
        let name = table.unwrap_or(key);
        if !FILE_CRATES.contains(&name) {
            continue;
        }
        let noted = l.contains("# seal-note:")
            || i.checked_sub(1)
                .and_then(|j| lines.get(j))
                .is_some_and(|p| p.trim_start().starts_with("# seal-note:"));
        if !noted {
            bad.push(format!("{name} (line {})", i + 1));
        }
    }
    bad
}

// REQ: OA-SEAL-006
#[test]
fn no_manifest_gains_a_file_or_mapping_dependency_without_a_seal_note() {
    let mut manifests = vec![repo().join("Cargo.toml")];
    manifests.extend(members().into_iter().map(|m| m.join("Cargo.toml")));
    for m in manifests {
        let text = fs::read_to_string(&m).unwrap();
        let bad = unnoted_file_crates(&text);
        assert!(
            bad.is_empty(),
            "{}: needs `# seal-note: <why it cannot bypass the guard>`: {bad:?}",
            m.display()
        );
    }
    // The checker itself.
    assert_eq!(
        unnoted_file_crates("[dependencies]\nlibc = \"0.2\"\n").len(),
        1
    );
    assert_eq!(
        unnoted_file_crates("[dependencies]\nwalkdir = { version = \"2\" }\n").len(),
        1
    );
    assert_eq!(
        unnoted_file_crates("[dependencies.memmap2]\nversion = \"0.9\"\n").len(),
        1
    );
    assert_eq!(
        unnoted_file_crates("[dependencies]\n# seal-note: reads only /proc\nlibc = \"0.2\"\n")
            .len(),
        0
    );
    assert_eq!(
        unnoted_file_crates(
            "[dependencies]\nlibc = \"0.2\" # seal-note: x\noa-x = { path = \"../x\" }\n"
        )
        .len(),
        0
    );
    assert_eq!(
        unnoted_file_crates("[dependencies]\nlibcx = \"1\"\n").len(),
        0
    );
}
