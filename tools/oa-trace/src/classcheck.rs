//! Core/shell class check (OA-TRC-200 to OA-TRC-208; RULESET B7 to B9).
//!
//! Every workspace member declares `[package.metadata.openatca] class` in its Cargo.toml: `core`,
//! `shell`, `split` or `unsplit`. The check reads the class of every member, scans core code
//! (all of `src/` of a core crate, `src/core/` of a split crate; never binaries or test code) for
//! the names B8 forbids, checks that core code depends only on core code, checks the shape of a
//! split crate, and keeps the unsplit list ([`ALLOWLIST`]) from growing. Cargo.toml is read with
//! a small parser of its own (F1: no TOML dependency); source is read by a lexer that blanks
//! comments, string literals and character literals before looking for names.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The committed list of crates allowed to be `unsplit` (B9), relative to the workspace root. It
/// only shrinks: an entry for a crate that is no longer unsplit fails until it is removed.
pub const ALLOWLIST: &str = "tools/oa-trace/unsplit-allowlist.txt";

/// Names core code must not use. Paths are written with `::`; each is matched on tokens, so
/// whitespace and line breaks inside a path do not hide it.
const FORBIDDEN_STD: &[&str] = &[
    "fs", "net", "io", "env", "process", "thread", "time", "sync",
];
const FORBIDDEN_MACROS: &[&str] = &[
    "thread_local",
    "print",
    "println",
    "eprint",
    "eprintln",
    "dbg",
];
const FORBIDDEN_IDENTS: &[&str] = &["HashMap", "HashSet", "RandomState"];

/// Result of the class check.
#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
pub struct ClassReport {
    /// Crate name -> declared class, for every member whose class is valid.
    pub classes: BTreeMap<String, String>,
    /// Crates declared `unsplit`, sorted (the B9 debt).
    pub unsplit: Vec<String>,
    /// Every failure as `file:line: token: message` or `crate: message`, in a stable order.
    pub failures: Vec<String>,
}

impl ClassReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Core,
    Shell,
    Split,
    Unsplit,
}

fn parse_class(s: &str) -> Option<Class> {
    match s {
        "core" => Some(Class::Core),
        "shell" => Some(Class::Shell),
        "split" => Some(Class::Split),
        "unsplit" => Some(Class::Unsplit),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Cargo.toml, minimally

/// What the check needs from one Cargo.toml.
#[derive(Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Manifest {
    pub name: Option<String>,
    /// `Some(raw value)` if `[package.metadata.openatca]` has a `class` key.
    pub class: Option<String>,
    /// Keys of `[dependencies]` (and `[dependencies.<name>]` tables).
    pub deps: Vec<String>,
}

fn unquote(v: &str) -> Option<String> {
    let v = v.trim();
    let v = v.split('#').next().unwrap_or("").trim();
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.to_string())
}

/// Reads the package name, the openatca class and the dependency names from Cargo.toml text.
pub fn parse_manifest(text: &str) -> Manifest {
    let mut m = Manifest::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(h) = line.strip_prefix('[') {
            let h = h.trim_end_matches(']').trim().to_string();
            if let Some(dep) = h.strip_prefix("dependencies.") {
                m.deps.push(dep.trim().trim_matches('"').to_string());
            }
            section = h;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        match section.as_str() {
            "package" if key == "name" => m.name = unquote(value),
            "package.metadata.openatca" if key == "class" => {
                m.class = Some(unquote(value).unwrap_or_else(|| value.trim().to_string()));
            }
            "dependencies" => {
                let name = key.split('.').next().unwrap_or(key).trim_matches('"');
                if !name.is_empty() && !name.starts_with('#') {
                    m.deps.push(name.to_string());
                }
            }
            _ => {}
        }
    }
    m
}

/// The `members = [...]` list of the root Cargo.toml.
pub fn parse_members(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut in_workspace = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_workspace = line == "[workspace]";
            inside = false;
            continue;
        }
        if !in_workspace {
            continue;
        }
        let mut rest = line;
        if !inside {
            let Some(r) = line.strip_prefix("members") else {
                continue;
            };
            let Some((_, r)) = r.split_once('[') else {
                continue;
            };
            inside = true;
            rest = r;
        }
        let (body, done) = match rest.split_once(']') {
            Some((b, _)) => (b, true),
            None => (rest, false),
        };
        out.extend(
            body.split(',')
                .filter_map(|p| unquote(p.trim()))
                .filter(|p| !p.is_empty()),
        );
        if done {
            inside = false;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Lexer

/// `src` with comments, string literals and character literals replaced by spaces; line breaks
/// are kept so line numbers still match. Lifetimes (`'a`) are left alone.
pub fn mask(src: &str) -> String {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    let is_ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut out = String::with_capacity(src.len());
    let blank = |out: &mut String, ch: char| out.push(if ch == '\n' { '\n' } else { ' ' });
    let mut i = 0;
    while i < c.len() {
        let ch = at(i);
        let prev = if i == 0 { '\0' } else { at(i - 1) };
        if ch == '/' && at(i + 1) == '/' {
            while i < c.len() && at(i) != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == '*' {
            let mut depth = 0;
            while i < c.len() {
                if at(i) == '/' && at(i + 1) == '*' {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if at(i) == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    blank(&mut out, at(i));
                    i += 1;
                }
            }
        } else if ch == 'r'
            && (!is_ident(prev) || (matches!(prev, 'b' | 'c') && (i < 2 || !is_ident(at(i - 2)))))
            && matches!(at(i + 1), '"' | '#')
            && {
                let mut j = i + 1;
                while at(j) == '#' {
                    j += 1;
                }
                at(j) == '"'
            }
        {
            let mut hashes = 0;
            let mut j = i + 1;
            while at(j) == '#' {
                hashes += 1;
                j += 1;
            }
            // j is at the opening quote
            for _ in i..=j {
                out.push(' ');
            }
            i = j + 1;
            while i < c.len() {
                if at(i) == '"' && (1..=hashes).all(|k| at(i + k) == '#') {
                    for _ in 0..=hashes {
                        out.push(' ');
                    }
                    i += hashes + 1;
                    break;
                }
                blank(&mut out, at(i));
                i += 1;
            }
        } else if ch == '"' {
            out.push(' ');
            i += 1;
            while i < c.len() {
                let d = at(i);
                if d == '\\' {
                    out.push(' ');
                    blank(&mut out, at(i + 1));
                    i += 2;
                } else if d == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                } else {
                    blank(&mut out, d);
                    i += 1;
                }
            }
        } else if ch == '\'' {
            if at(i + 1) == '\\' {
                // '\n', '\'', '\u{..}': skip the escaped character, then up to the closing quote
                out.push_str("   ");
                i += 3;
                while i < c.len() && at(i) != '\'' && at(i) != '\n' {
                    out.push(' ');
                    i += 1;
                }
                if at(i) == '\'' {
                    out.push(' ');
                    i += 1;
                }
            } else if at(i + 2) == '\'' && at(i + 1) != '\'' {
                out.push_str("   ");
                i += 3;
            } else {
                out.push(ch);
                i += 1;
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tok {
    s: String,
    line: usize,
}

/// Words, `::`, and every other character on its own, from masked text.
fn tokenize(masked: &str) -> Vec<Tok> {
    let c: Vec<char> = masked.chars().collect();
    let mut out = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < c.len() {
        let ch = c.get(i).copied().unwrap_or(' ');
        if ch == '\n' {
            line += 1;
            i += 1;
        } else if ch.is_whitespace() {
            i += 1;
        } else if ch.is_alphanumeric() || ch == '_' {
            let start = i;
            while c.get(i).is_some_and(|d| d.is_alphanumeric() || *d == '_') {
                i += 1;
            }
            out.push(Tok {
                s: c.get(start..i).unwrap_or(&[]).iter().collect(),
                line,
            });
        } else if ch == ':' && c.get(i + 1) == Some(&':') {
            out.push(Tok {
                s: "::".to_string(),
                line,
            });
            i += 2;
        } else {
            out.push(Tok {
                s: ch.to_string(),
                line,
            });
            i += 1;
        }
    }
    out
}

fn s(toks: &[Tok], i: usize) -> &str {
    toks.get(i).map_or("", |t| t.s.as_str())
}

/// Index just past the group that opens at `toks[i]` (`(`, `[` or `{`).
fn skip_group(toks: &[Tok], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < toks.len() {
        match s(toks, j) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    toks.len()
}

/// Skips `pub`, `pub(..)` and the modifiers `const`/`unsafe`/`async`/`extern` that precede `fn`.
fn skip_modifiers(toks: &[Tok], mut i: usize) -> usize {
    loop {
        match s(toks, i) {
            "pub" => {
                i += 1;
                if s(toks, i) == "(" {
                    i = skip_group(toks, i);
                }
            }
            "const" | "unsafe" | "async" | "extern"
                if matches!(
                    s(toks, i + 1),
                    "fn" | "unsafe" | "async" | "extern" | "impl" | "trait"
                ) =>
            {
                i += 1;
            }
            _ => return i,
        }
    }
}

/// Removes `#[cfg(test)]` and `#[cfg(kani)]` items (B7: tests and proof harnesses are shell) and
/// returns the remaining tokens and the names of out-of-line modules removed (`mod tests;`).
fn strip_test_items(toks: Vec<Tok>) -> (Vec<Tok>, Vec<String>) {
    let mut out = Vec::new();
    let mut files = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let is_cfg = s(&toks, i) == "#"
            && s(&toks, i + 1) == "["
            && s(&toks, i + 2) == "cfg"
            && s(&toks, i + 3) == "("
            && matches!(s(&toks, i + 4), "test" | "kani")
            && s(&toks, i + 5) == ")"
            && s(&toks, i + 6) == "]";
        if !is_cfg {
            if let Some(t) = toks.get(i) {
                out.push(t.clone());
            }
            i += 1;
            continue;
        }
        i += 7;
        // further attributes on the same item
        while s(&toks, i) == "#" && s(&toks, i + 1) == "[" {
            i = skip_group(&toks, i + 1);
        }
        i = skip_modifiers(&toks, i);
        let kw = s(&toks, i).to_string();
        if kw == "mod" && s(&toks, i + 2) == ";" {
            files.push(s(&toks, i + 1).to_string());
        }
        let to_semicolon = matches!(kw.as_str(), "use" | "const" | "static" | "type" | "mod")
            && !(kw == "mod" && s(&toks, i + 2) == "{");
        let mut depth = 0usize;
        while i < toks.len() {
            match s(&toks, i) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" => depth = depth.saturating_sub(1),
                "}" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && !to_semicolon {
                        i += 1;
                        break;
                    }
                }
                ";" if depth == 0 => {
                    i += 1;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
    }
    (out, files)
}

// ---------------------------------------------------------------------------------------------
// Scanning

/// A name in core code that B8 forbids, or a dependency on non-core code.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Hit {
    pub line: usize,
    pub token: String,
    pub message: String,
}

fn hit(line: usize, token: &str, message: &str) -> Hit {
    Hit {
        line,
        token: token.to_string(),
        message: message.to_string(),
    }
}

/// Finds the B8 names in core tokens. `classes` maps every workspace crate to its class, for the
/// dependency check; `own` is the scanned crate (naming itself is not a dependency).
fn scan_tokens(toks: &[Tok], own: &str, classes: &BTreeMap<String, Class>) -> Vec<Hit> {
    let mut hits = Vec::new();
    let own_us = own.replace('-', "_");
    let mut in_use = false;
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        match w {
            "use" => in_use = true,
            ";" => in_use = false,
            _ => {}
        }
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let next = s(toks, i + 1);
        if w == "std" && next == "::" {
            let after = s(toks, i + 2);
            if FORBIDDEN_STD.contains(&after) {
                hits.push(hit(
                    t.line,
                    &format!("std::{after}"),
                    "B8: core code names it",
                ));
            } else if after == "{" {
                // use std::{fs, io::Read};
                let end = skip_group(toks, i + 2);
                let mut depth = 0usize;
                for j in i + 2..end {
                    match s(toks, j) {
                        "{" => depth += 1,
                        "}" => depth = depth.saturating_sub(1),
                        x if depth == 1
                            && FORBIDDEN_STD.contains(&x)
                            && matches!(s(toks, j - 1), "{" | ",") =>
                        {
                            hits.push(hit(t.line, &format!("std::{x}"), "B8: core code names it"));
                        }
                        _ => {}
                    }
                }
            }
        } else if w == "static" && next == "mut" {
            hits.push(hit(t.line, "static mut", "B8: core code names it"));
        } else if next == "!" && FORBIDDEN_MACROS.contains(&w) && s(toks, i + 2) != "=" {
            hits.push(hit(t.line, &format!("{w}!"), "B8: core code names it"));
        } else if FORBIDDEN_IDENTS.contains(&w) {
            hits.push(hit(t.line, w, "B8: core code names it"));
        } else if w == "shell"
            && (prev == "::" || next == "::" || (in_use && matches!(prev, "{" | ",")))
        {
            hits.push(hit(t.line, "shell", "B8: path into a shell module"));
        } else if next == "::" && prev != "::" && w != own_us {
            // a workspace crate named by its underscored name
            match classes.get(&w.replace('_', "-")) {
                Some(Class::Core) | None => {}
                Some(Class::Split) if s(toks, i + 2) == "core" => {}
                Some(Class::Split) => hits.push(hit(
                    t.line,
                    w,
                    "B8: names a split crate outside its `core` module",
                )),
                Some(_) => hits.push(hit(t.line, w, "B8: names a shell or unsplit crate")),
            }
        }
    }
    hits
}

/// Scans one file of core code. Returns the hits and the out-of-line test modules it declares.
pub fn scan_source(
    src: &str,
    own: &str,
    classes: &BTreeMap<String, Class>,
) -> (Vec<Hit>, Vec<String>) {
    let (toks, test_mods) = strip_test_items(tokenize(&mask(src)));
    (scan_tokens(&toks, own, classes), test_mods)
}

/// The first reason `lib.rs` is not only module declarations, re-exports and attributes.
pub fn lib_rs_problem(src: &str) -> Option<Hit> {
    let (toks, _) = strip_test_items(tokenize(&mask(src)));
    let mut i = 0;
    while i < toks.len() {
        if s(&toks, i) == "#" {
            i += 1;
            if s(&toks, i) == "!" {
                i += 1;
            }
            if s(&toks, i) != "[" {
                return Some(hit(
                    toks.get(i).map_or(0, |t| t.line),
                    s(&toks, i),
                    "stray `#`",
                ));
            }
            i = skip_group(&toks, i);
            continue;
        }
        let line = toks.get(i).map_or(0, |t| t.line);
        i = skip_modifiers(&toks, i);
        let kw = s(&toks, i);
        let end = (i..toks.len())
            .find(|&j| matches!(s(&toks, j), ";" | "{"))
            .unwrap_or(toks.len());
        let ok = match kw {
            "mod" => end == i + 2 && s(&toks, end) == ";",
            "use" => {
                // a use list may contain braces; the statement ends at the first `;`
                (i..toks.len()).any(|j| s(&toks, j) == ";")
            }
            _ => false,
        };
        if !ok {
            return Some(hit(
                line,
                kw,
                "B7: lib.rs of a split crate holds only module declarations, re-exports and attributes",
            ));
        }
        i = (i..toks.len())
            .find(|&j| s(&toks, j) == ";")
            .map_or(toks.len(), |j| j + 1);
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Workspace

fn collect_rs(dir: &Path, skip_bin: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if skip_bin && path.file_name().is_some_and(|n| n == "bin") {
                continue;
            }
            collect_rs(&path, skip_bin, out);
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

struct Member {
    name: String,
    dir: PathBuf,
    class: Class,
    deps: Vec<String>,
}

/// The core files of a member: every `.rs` under `src/` except binaries (core), or under
/// `src/core/` (split); sorted, with test files and out-of-line test modules left to the scan.
fn core_files(m: &Member) -> Vec<PathBuf> {
    let base = match m.class {
        Class::Split => m.dir.join("src/core"),
        _ => m.dir.join("src"),
    };
    let mut files = Vec::new();
    collect_rs(&base, true, &mut files);
    files.retain(|p| {
        !(is_test_file(p) || m.class == Class::Core && p == &m.dir.join("src/main.rs"))
    });
    files.sort();
    files
}

/// Checks every workspace member under `root` (B7 to B9).
pub fn check(root: &Path) -> ClassReport {
    let mut rep = ClassReport::default();
    let root_toml = root.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&root_toml) else {
        rep.failures
            .push(format!("{}: cannot read", root_toml.display()));
        return rep;
    };
    let mut members = Vec::new();
    for dir in parse_members(&text) {
        let path = root.join(&dir).join("Cargo.toml");
        let Ok(t) = fs::read_to_string(&path) else {
            rep.failures.push(format!("{dir}: cannot read Cargo.toml"));
            continue;
        };
        let man = parse_manifest(&t);
        let name = man.name.clone().unwrap_or_else(|| dir.clone());
        let class = match man.class.as_deref() {
            None => {
                rep.failures.push(format!(
                    "{name}: no [package.metadata.openatca] class (B7); declare core, shell, split or unsplit"
                ));
                continue;
            }
            Some(v) => match parse_class(v) {
                Some(c) => c,
                None => {
                    rep.failures.push(format!(
                        "{name}: unknown class {v:?} (B7); declare core, shell, split or unsplit"
                    ));
                    continue;
                }
            },
        };
        rep.classes.insert(
            name.clone(),
            match class {
                Class::Core => "core",
                Class::Shell => "shell",
                Class::Split => "split",
                Class::Unsplit => "unsplit",
            }
            .to_string(),
        );
        if class == Class::Unsplit {
            rep.unsplit.push(name.clone());
        }
        members.push(Member {
            name,
            dir: root.join(&dir),
            class,
            deps: man.deps,
        });
    }
    rep.unsplit.sort();
    let classes: BTreeMap<String, Class> =
        members.iter().map(|m| (m.name.clone(), m.class)).collect();

    for m in &members {
        if !matches!(m.class, Class::Core | Class::Split) {
            continue;
        }
        if m.class == Class::Core {
            for d in &m.deps {
                match classes.get(d) {
                    Some(Class::Core | Class::Split) | None => {}
                    Some(c) => rep.failures.push(format!(
                        "{}: {d}: B8: core crate depends on a {} crate",
                        m.name,
                        if *c == Class::Shell {
                            "shell"
                        } else {
                            "unsplit"
                        }
                    )),
                }
            }
        }
        if m.class == Class::Split {
            for sub in ["core", "shell"] {
                if !m.dir.join("src").join(sub).is_dir() {
                    rep.failures
                        .push(format!("{}: split crate has no src/{sub}/ (B7)", m.name));
                }
            }
            let lib = m.dir.join("src/lib.rs");
            match fs::read_to_string(&lib) {
                Ok(src) => {
                    if let Some(h) = lib_rs_problem(&src) {
                        rep.failures.push(format!(
                            "{}:{}: {}: {}",
                            rel(root, &lib),
                            h.line,
                            h.token,
                            h.message
                        ));
                    }
                }
                Err(e) => rep
                    .failures
                    .push(format!("{}: cannot read src/lib.rs: {e}", m.name)),
            }
        }
        let files = core_files(m);
        // out-of-line `#[cfg(test)] mod x;` files are tests, not core
        let mut skip: Vec<PathBuf> = Vec::new();
        let mut parsed = Vec::new();
        for f in &files {
            let Ok(src) = fs::read_to_string(f) else {
                rep.failures.push(format!("{}: cannot read", rel(root, f)));
                continue;
            };
            let (hits, test_mods) = scan_source(&src, &m.name, &classes);
            if let Some(dir) = f.parent() {
                for tm in test_mods {
                    skip.push(dir.join(format!("{tm}.rs")));
                    skip.push(dir.join(&tm).join("mod.rs"));
                    if f.file_name()
                        .is_some_and(|n| n != "lib.rs" && n != "mod.rs")
                    {
                        if let Some(stem) = f.file_stem() {
                            skip.push(dir.join(stem).join(format!("{tm}.rs")));
                        }
                    }
                }
            }
            parsed.push((f.clone(), hits));
        }
        for (f, hits) in parsed {
            if skip.contains(&f) {
                continue;
            }
            for h in hits {
                rep.failures.push(format!(
                    "{}:{}: {}: {}",
                    rel(root, &f),
                    h.line,
                    h.token,
                    h.message
                ));
            }
        }
    }

    // B9: the unsplit list only shrinks
    let allow: Vec<String> = fs::read_to_string(root.join(ALLOWLIST))
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if fs::metadata(root.join(ALLOWLIST)).is_err() {
        rep.failures.push(format!("{ALLOWLIST}: cannot read"));
    }
    for name in &rep.unsplit {
        if !allow.contains(name) {
            rep.failures.push(format!(
                "{name}: B9: new unsplit crate, not in {ALLOWLIST}; split it or classify it"
            ));
        }
    }
    for name in &allow {
        if !rep.unsplit.contains(name) {
            rep.failures.push(format!(
                "{ALLOWLIST}: {name}: B9: listed but not unsplit (migrated or unknown); remove the entry"
            ));
        }
    }
    rep.failures.sort();
    rep.failures.dedup();
    rep
}

/// Plain-text report, deterministic.
pub fn render_text(rep: &ClassReport) -> String {
    let mut o = String::new();
    let count = |c: &str| rep.classes.values().filter(|v| *v == c).count();
    o.push_str(&format!(
        "Crate classes (B7): {} core, {} shell, {} split, {} unsplit\n",
        count("core"),
        count("shell"),
        count("split"),
        count("unsplit")
    ));
    o.push_str(&format!(
        "Unsplit crates, migration debt (B9) ({}):\n",
        rep.unsplit.len()
    ));
    for n in &rep.unsplit {
        o.push_str(&format!("  {n}\n"));
    }
    o.push_str(&format!(
        "\nClass check failures ({}):\n",
        rep.failures.len()
    ));
    for f in &rep.failures {
        o.push_str(&format!("  {f}\n"));
    }
    o.push_str(&format!(
        "\nCore/shell class check (B7 to B9): {}\n",
        if rep.ok() { "PASS" } else { "FAIL" }
    ));
    o
}
