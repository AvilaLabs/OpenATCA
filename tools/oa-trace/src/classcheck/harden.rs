//! Second-round hardening of the class check (RULESET B7, B8, B10, B5; amended 2026-11-19 after a
//! red-team pass proved bypasses of the first round): what a split crate's shell may do to the
//! core, the unsafe and address bans, the callback allowlist and the unsplit float budget.
//! Same lexer as [`super`]; same rule: anything unreadable is a failure.

use super::{
    hit, is_word, mask, s, skip_group, skip_modifiers, strip_test_items, tokenize, FileScan, Hit,
    Tok, FLOAT_METHODS,
};
use std::collections::{BTreeMap, BTreeSet};

/// The committed list of core `pub fn` signatures that take a callback or a trait-bound generic,
/// relative to the workspace root (RULESET B5). Entries `crate path::fn reason`.
pub const CALLBACK_ALLOWLIST: &str = "tools/oa-trace/core-callbacks-allowlist.txt";
/// Float-method calls per unsplit crate, a ratchet (RULESET B9).
pub const FLOAT_BUDGET: &str = "tools/oa-trace/unsplit-float-budget.txt";

/// Idents banned in core code anywhere: they leak an address, link or platform fact, or open the
/// way to code the scan cannot read (RULESET B8).
const BANNED_IDENTS: &[(&str, &str)] = &[
    ("unsafe", "B8: core code has no `unsafe`"),
    ("no_mangle", "B8: core code exports no symbol"),
    ("export_name", "B8: core code exports no symbol"),
    ("link_section", "B8: core code places nothing in a link section"),
    ("link_name", "B8: core code links nothing by name"),
    ("as_ptr", "B8: core code takes no address"),
    ("as_mut_ptr", "B8: core code takes no address"),
    ("expose_provenance", "B8: core code takes no address"),
    ("expose_addr", "B8: core code takes no address"),
    ("with_exposed_provenance", "B8: core code takes no address"),
    ("ptr_eq", "B8: core code compares no addresses"),
    ("addr_eq", "B8: core code compares no addresses"),
    ("non_camel_case_types", "B8: core code does not silence the type-name lint, a lowercase type alias could hide a float method"),
    ("nonstandard_style", "B8: core code does not silence the type-name lint, a lowercase type alias could hide a float method"),
    ("bad_style", "B8: core code does not silence the type-name lint, a lowercase type alias could hide a float method"),
    ("macro_use", "B8: core code imports no macro"),
    ("macro_export", "B8: core code exports no macro"),
];
/// `size_of` and friends: their value is a platform fact, allowed only inside a `const _: () =
/// assert!(..)` size guard (RULESET A7).
const SIZE_IDENTS: &[&str] = &["size_of", "align_of", "size_of_val", "align_of_val"];

/// Whether `name` is CamelCase for the type-name rule: an ASCII capital first and no underscore.
fn camel(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase()) && !name.contains('_')
}

/// Per token, whether it sits inside a `const _: () = assert!(..);` size guard.
fn guard_mask(toks: &[Tok]) -> Vec<bool> {
    let mut m = vec![false; toks.len()];
    for i in 0..toks.len() {
        let head = ["const", "_", ":", "(", ")", "=", "assert", "!", "("];
        if head.iter().enumerate().all(|(k, h)| s(toks, i + k) == *h) {
            let end = skip_group(toks, i + 8);
            if s(toks, end) == ";" {
                for slot in m.iter_mut().take(end).skip(i + 6) {
                    *slot = true;
                }
            }
        }
    }
    m
}

/// Index of the `(` that matches the `)` at `toks[close]`.
fn open_of(toks: &[Tok], close: usize) -> Option<usize> {
    let mut depth = 0usize;
    for j in (0..=close).rev() {
        match s(toks, j) {
            ")" | "]" | "}" => depth += 1,
            "(" | "[" | "{" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// The ban checks of RULESET B8 that look at one token with its neighbours: unsafe and link
/// items, addresses, platform sizes, lowercase type aliases, fn-pointer casts, and a float's
/// bits that were not canonicalised. `math` is oa-math itself, which does bit arithmetic on
/// floats by design.
pub(super) fn token_bans(toks: &[Tok], math: bool, hits: &mut Vec<Hit>) {
    let guard = guard_mask(toks);
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let next = s(toks, i + 1);
        if let Some((_, msg)) = BANNED_IDENTS.iter().find(|(n, _)| *n == w) {
            hits.push(hit(t.line, w, msg));
        } else if w == "extern" && next != "crate" {
            hits.push(hit(
                t.line,
                "extern",
                "B8: core code has no foreign-function ABI",
            ));
        } else if w == "link" && prev == "[" && i >= 2 && s(toks, i - 2) == "#" {
            hits.push(hit(
                t.line,
                "#[link",
                "B8: core code links no native library",
            ));
        } else if w == "addr" && prev == "." && next == "(" {
            hits.push(hit(t.line, ".addr(", "B8: core code takes no address"));
        } else if w == "eq" && prev == "::" && s(toks, i.wrapping_sub(2)) == "ptr" {
            hits.push(hit(
                t.line,
                "ptr::eq",
                "B8: core code compares no addresses",
            ));
        } else if matches!(w, "MAX" | "MIN" | "BITS")
            && prev == "::"
            && matches!(s(toks, i.wrapping_sub(2)), "usize" | "isize")
        {
            hits.push(hit(
                t.line,
                &format!("{}::{w}", s(toks, i - 2)),
                "B8: the width of usize is a platform fact; use u32 or u64 limits",
            ));
        } else if SIZE_IDENTS.contains(&w) && !guard.get(i).copied().unwrap_or(false) {
            hits.push(hit(
                t.line,
                w,
                "B8: a platform size only inside `const _: () = assert!(..)`; state the number as a constant and assert it",
            ));
        } else if w == "type"
            && is_word(next)
            && matches!(s(toks, i + 2), "=" | "<")
            && !camel(next)
        {
            hits.push(hit(
                t.line,
                &format!("type {next}"),
                "B8: a type alias in core code is CamelCase (a lowercase alias could stand for f64 and hide a float method)",
            ));
        } else if w == "as" && matches!(next, "fn" | "unsafe" | "extern") {
            hits.push(hit(
                t.line,
                "as fn",
                "B8: no cast to a fn pointer, its Debug and Pointer text is an address",
            ));
        } else if w == "to_bits" && prev == "." && next == "(" && !math {
            // the receiver must be `canon_nan(..)`: a NaN's sign and payload are not portable
            let canon = i >= 2
                && s(toks, i - 2) == ")"
                && open_of(toks, i - 2).is_some_and(|o| o >= 1 && s(toks, o - 1) == "canon_nan");
            if !canon {
                hits.push(hit(
                    t.line,
                    "to_bits",
                    "B8: a float's bits in core code come from `oa_math::canon_nan(x).to_bits()`, a NaN's sign and payload differ between machines",
                ));
            }
        }
    }
}

/// Item names a file defines with `struct`, `enum`, `union`, `trait` or `type` (the type and
/// trait names code can `impl`).
pub(super) fn defined_types(src: &str) -> BTreeSet<String> {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let mut out = BTreeSet::new();
    for (i, t) in toks.iter().enumerate() {
        if matches!(t.s.as_str(), "struct" | "enum" | "union" | "trait" | "type")
            && is_word(s(&toks, i + 1))
        {
            out.insert(s(&toks, i + 1).to_string());
        }
    }
    out
}

/// Names a shell file defines as new items (not aliases).
fn local_items(toks: &[Tok]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, t) in toks.iter().enumerate() {
        if matches!(t.s.as_str(), "struct" | "enum" | "union" | "trait") && is_word(s(toks, i + 1))
        {
            out.insert(s(toks, i + 1).to_string());
        }
    }
    out
}

/// The core names a shell file can reach under another name: `use ..Name as Alias;` and
/// `type Alias = Name<..>;`, to a fixed point.
fn taint(toks: &[Tok], core: &BTreeSet<String>) -> BTreeSet<String> {
    let mut set = core.clone();
    for _ in 0..4 {
        let before = set.len();
        for (i, t) in toks.iter().enumerate() {
            if t.s == "as" && set.contains(s(toks, i.wrapping_sub(1))) && is_word(s(toks, i + 1)) {
                set.insert(s(toks, i + 1).to_string());
            }
            if t.s == "type" && is_word(s(toks, i + 1)) && s(toks, i + 2) == "=" {
                let mut j = i + 3;
                while matches!(
                    s(toks, j),
                    "&" | "mut" | "dyn" | "crate" | "super" | "self" | "::"
                ) {
                    j += 1;
                }
                let mut head = s(toks, j);
                while s(toks, j + 1) == "::" && is_word(s(toks, j + 2)) {
                    j += 2;
                    head = s(toks, j);
                }
                if set.contains(head) {
                    set.insert(s(toks, i + 1).to_string());
                }
            }
        }
        if set.len() == before {
            break;
        }
    }
    set
}

/// The last path segment that leads `toks` (`&mut a::b::T<X>` gives `T`) and whether the path
/// runs through `crate::core`, `super::core` or `self::core`.
fn head_of(toks: &[Tok]) -> (String, bool) {
    let mut j = 0;
    while matches!(
        s(toks, j),
        "&" | "*" | "mut" | "const" | "dyn" | "(" | "[" | "!" | "::"
    ) {
        j += 1;
    }
    let mut head = s(toks, j).to_string();
    let mut core = false;
    while s(toks, j + 1) == "::" && is_word(s(toks, j + 2)) {
        if s(toks, j + 2) == "core" && matches!(s(toks, j), "crate" | "super" | "self") {
            core = true;
        }
        j += 2;
        head = s(toks, j).to_string();
    }
    (head, core)
}

/// Scans one file of a split crate's shell: no `impl` of a core type or core trait (the core
/// scan cannot see the behaviour such an impl adds), and no `#[macro_export]`. `core` holds the
/// type and trait names the crate's core files define.
pub fn scan_shell(src: &str, core: &BTreeSet<String>) -> FileScan {
    let st = strip_test_items(tokenize(&mask(src)));
    let toks = &st.toks;
    let tainted = taint(toks, core);
    let local = local_items(toks);
    let mut hits = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.s == "macro_export" {
            hits.push(hit(
                t.line,
                "macro_export",
                "B7: a shell macro cannot be exported, core code could expand it",
            ));
        }
        let item_start = matches!(
            if i == 0 { "" } else { s(toks, i - 1) },
            "" | ";" | "}" | "{" | "]" | "unsafe"
        );
        if t.s != "impl" || !item_start {
            continue;
        }
        // header: optional generics, then tokens up to `{`, `;` or `where`
        let mut j = i + 1;
        if s(toks, j) == "<" {
            let mut depth = 0usize;
            while j < toks.len() {
                match s(toks, j) {
                    "<" => depth += 1,
                    ">" if s(toks, j - 1) != "-" => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        let from = j;
        let mut depth = 0usize;
        while j < toks.len() {
            match s(toks, j) {
                "(" | "[" => depth += 1,
                ")" | "]" => depth = depth.saturating_sub(1),
                "{" | ";" | "where" if depth == 0 => break,
                _ => {}
            }
            j += 1;
        }
        let header = toks.get(from..j).unwrap_or(&[]);
        let mut angle = 0usize;
        let mut for_at = None;
        for (k, h) in header.iter().enumerate() {
            match h.s.as_str() {
                "<" => angle += 1,
                ">" if k > 0 && s(header, k - 1) != "-" => angle = angle.saturating_sub(1),
                "for" if angle == 0 && s(header, k + 1) != "<" => {
                    for_at = Some(k);
                    break;
                }
                _ => {}
            }
        }
        let (trait_toks, self_toks) = match for_at {
            Some(k) => (header.get(..k), header.get(k + 1..).unwrap_or(&[])),
            None => (None, header),
        };
        let mut sides = vec![("type", self_toks)];
        if let Some(tt) = trait_toks {
            sides.push(("trait", tt));
        }
        for (what, side) in sides {
            let (head, via_core) = head_of(side);
            let bad = via_core || (tainted.contains(&head) && !local.contains(&head));
            if bad {
                hits.push(hit(
                    t.line,
                    &format!("impl {head}"),
                    &format!(
                        "B7: shell code `impl`s the core {what} `{head}`; behaviour on core data belongs in `src/core/`, where the scan reads it"
                    ),
                ));
            }
        }
    }
    FileScan {
        hits,
        test_mods: st.test_mods,
        whole_file: st.whole_file,
        callbacks: Vec::new(),
    }
}

/// A core `pub fn` whose signature takes a callback or a trait-bound generic.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct CallbackSig {
    pub name: String,
    pub line: usize,
    /// What makes it one: `Fn`, `FnMut`, `FnOnce`, `dyn`, `impl` in a parameter, a bound, `where`.
    pub why: String,
}

/// The `pub fn`s of a core file (any visibility `pub`, `pub(crate)` included) whose signature
/// names `Fn`, `FnMut`, `FnOnce` or `dyn`, takes `impl Trait` as a parameter, or has a bound
/// (`<T: Bound>` or a `where` clause): a way for shell code to inject behaviour into the core
/// (RULESET B5).
pub fn callback_sigs(src: &str) -> Vec<CallbackSig> {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.s != "pub" {
            continue;
        }
        let mut j = i + 1;
        if s(&toks, j) == "(" {
            j = skip_group(&toks, j);
        }
        let j = skip_modifiers(&toks, j);
        if s(&toks, j) != "fn" || !is_word(s(&toks, j + 1)) {
            continue;
        }
        let name = s(&toks, j + 1).to_string();
        let line = toks.get(j).map_or(0, |t| t.line);
        let mut k = j + 2;
        let mut why: Vec<&str> = Vec::new();
        if s(&toks, k) == "<" {
            let mut depth = 0usize;
            while k < toks.len() {
                match s(&toks, k) {
                    "<" => depth += 1,
                    ">" if s(&toks, k - 1) != "-" => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            k += 1;
                            break;
                        }
                    }
                    ":" if depth == 1 => why.push("a bound"),
                    _ => {}
                }
                k += 1;
            }
        }
        let mut depth = 0usize;
        let mut in_params = true;
        let mut seen_params = false;
        while k < toks.len() {
            let w = s(&toks, k);
            match w {
                "(" | "[" => {
                    depth += 1;
                    seen_params = true;
                }
                ")" | "]" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && seen_params {
                        in_params = false;
                    }
                }
                "{" | ";" if depth == 0 => break,
                "where" if depth == 0 => why.push("where"),
                "Fn" | "FnMut" | "FnOnce" | "dyn" => why.push(w),
                "impl" if in_params => why.push("impl"),
                _ => {}
            }
            k += 1;
        }
        why.sort_unstable();
        why.dedup();
        if !why.is_empty() {
            out.push(CallbackSig {
                name,
                line,
                why: why.join(", "),
            });
        }
    }
    out
}

/// One line of the callback allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct CallbackEntry {
    pub krate: String,
    /// `file::fn`, file relative to the crate's `src/`.
    pub path: String,
    pub reason: String,
}

/// Parses the allowlist: `crate path::fn reason`, `#` comments, blank lines. A line without a
/// reason or with a path that is not `file::fn` is a failure.
pub fn parse_callbacks(text: &str) -> (Vec<CallbackEntry>, Vec<String>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let mut parts = l.splitn(3, char::is_whitespace);
        let (krate, path, reason) = (
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("").trim(),
        );
        if krate.is_empty() || !path.contains(".rs::") || reason.len() < 8 {
            bad.push(l.to_string());
        } else {
            ok.push(CallbackEntry {
                krate: krate.to_string(),
                path: path.to_string(),
                reason: reason.to_string(),
            });
        }
    }
    (ok, bad)
}

/// Whether the float method at `toks[i]` is a call of the platform's: `.name(` on any receiver,
/// or `Type::name` where the qualifier is a float type, `Self`, a generic or an upper-case name.
/// A call through `oa_math`/`oa_ieee`, an integer `rem_euclid` and a lowercase module path are
/// not.
fn platform_float_call(toks: &[Tok], i: usize) -> bool {
    let w = s(toks, i);
    let prev = if i == 0 { "" } else { s(toks, i - 1) };
    if !FLOAT_METHODS.contains(&w) {
        return false;
    }
    if prev == "." {
        return s(toks, i + 1) == "(";
    }
    if prev != "::" || i < 2 {
        return false;
    }
    let qual = s(toks, i - 2);
    let blessed = matches!(qual, "oa_math" | "oa_ieee");
    let int_euclid = matches!(w, "rem_euclid" | "div_euclid") && super::INT_TYPES.contains(&qual);
    !blessed
        && !int_euclid
        && (matches!(qual, "f32" | "f64" | "Self" | ">")
            || qual.starts_with(|c: char| c.is_ascii_uppercase()))
}

/// Platform float-method calls in non-test code of `src`.
pub fn count_float_calls(src: &str) -> (usize, FileScan) {
    let st = strip_test_items(tokenize(&mask(src)));
    let n = (0..st.toks.len())
        .filter(|&i| platform_float_call(&st.toks, i))
        .count();
    (
        n,
        FileScan {
            hits: Vec::new(),
            test_mods: st.test_mods,
            whole_file: st.whole_file,
            callbacks: Vec::new(),
        },
    )
}

/// Parses the float budget: `crate count`, `#` comments. Returns the entries and the bad lines.
pub fn parse_budget(text: &str) -> (BTreeMap<String, usize>, Vec<String>) {
    let mut ok = BTreeMap::new();
    let mut bad = Vec::new();
    for l in text.lines() {
        let l = l.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        let mut p = l.split_whitespace();
        match (
            p.next(),
            p.next().and_then(|n| n.parse::<usize>().ok()),
            p.next(),
        ) {
            (Some(k), Some(n), None) => {
                if ok.insert(k.to_string(), n).is_some() {
                    bad.push(format!("{l} (listed twice)"));
                }
            }
            _ => bad.push(l.to_string()),
        }
    }
    (ok, bad)
}
