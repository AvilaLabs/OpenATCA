//! Second-round hardening of the class check (RULESET B7, B8, B10, B5; amended 2026-11-19 after a
//! red-team pass proved bypasses of the first round): what a split crate's shell may do to the
//! core, the unsafe and address bans, the callback allowlist and the unsplit float budget.
//! Same lexer as [`super`]; same rule: anything unreadable is a failure.

use super::floats::Evidence;
use super::{
    call_at, hit, is_path_start, is_word, mask, s, skip_group, skip_turbofish, strip_test_items,
    tokenize, Ctx, FileScan, Hit, Tok, FLOAT_METHODS,
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
    ("into_raw", "B8: core code takes no address"),
    ("into_raw_parts", "B8: core code takes no address"),
    ("from_raw", "B8: core code takes no address"),
    ("leak", "B8: core code takes no address, a leaked allocation has one"),
    ("as_ptr_range", "B8: core code takes no address"),
    ("as_mut_ptr_range", "B8: core code takes no address"),
    ("addr_of", "B8: core code takes no address"),
    ("addr_of_mut", "B8: core code takes no address"),
    ("ptr_eq", "B8: core code compares no addresses"),
    ("addr_eq", "B8: core code compares no addresses"),
    ("non_camel_case_types", "B8: core code does not silence the type-name lint, a lowercase type alias could hide a float method"),
    ("nonstandard_style", "B8: core code does not silence the type-name lint, a lowercase type alias could hide a float method"),
    ("bad_style", "B8: core code does not silence the type-name lint, a lowercase type alias could hide a float method"),
    ("Pointer", "B8: core code does not format pointers (fmt::Pointer prints an address)"),
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
pub(super) fn open_of(toks: &[Tok], close: usize) -> Option<usize> {
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

/// The functions that make a float's bits canonical (RULESET B8).
const CANON: &[&str] = &["canon_nan", "canon_nan_f32"];
const BYTES: &[&str] = &["to_le_bytes", "to_be_bytes", "to_ne_bytes"];

/// Whether `toks[i]` starts a call of `canon_nan(..)` or `canon_nan_f32(..)`, bare or through
/// `oa_math::`; returns the index just past its argument group.
fn canon_call_end(toks: &[Tok], i: usize) -> Option<usize> {
    let mut j = i;
    if matches!(s(toks, j), "oa_math") && s(toks, j + 1) == "::" {
        j += 2;
    }
    (CANON.contains(&s(toks, j)) && s(toks, j + 1) == "(").then(|| skip_group(toks, j + 1))
}

/// `use` statements of the file: token ranges from after `use` up to `;`.
fn use_statements(toks: &[Tok]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if s(toks, i) == "use" && is_path_start(toks, i) {
            let end = (i..toks.len())
                .find(|&k| s(toks, k) == ";")
                .unwrap_or(toks.len());
            out.push((i + 1, end));
            i = end;
        }
        i += 1;
    }
    out
}

/// The ban checks of RULESET B8 that look at one token with its neighbours: unsafe and link
/// items, addresses, platform sizes, lowercase type aliases, fn-pointer casts, and a float's
/// bits that were not canonicalised. `oa-math` itself does bit arithmetic on floats by design.
pub(super) fn token_bans(toks: &[Tok], ctx: &Ctx<'_>, hits: &mut Vec<Hit>) {
    let math = ctx.math;
    let guard = guard_mask(toks);
    let ev = Evidence::new(toks, ctx.floats);
    let is_math_root =
        |w: &str| w == "oa_math" || ctx.aliases.get(w).is_some_and(|t| t == "oa-math");
    // `canon_nan` names must be oa_math's: imported from it, or called through it
    let mut imported: BTreeSet<&str> = BTreeSet::new();
    for (from, to) in use_statements(toks) {
        let body = toks.get(from..to).unwrap_or(&[]);
        let from_math = body
            .iter()
            .enumerate()
            .any(|(k, t)| is_math_root(&t.s) && is_path_start(body, k));
        for t in body.iter().filter(|t| CANON.contains(&t.s.as_str())) {
            if from_math {
                imported.insert(t.s.as_str());
            } else if !math {
                hits.push(hit(
                    t.line,
                    &t.s,
                    "B8: `canon_nan` is imported from `oa_math` only",
                ));
            }
        }
    }
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let next = s(toks, i + 1);
        let qual = s(toks, i.wrapping_sub(2));
        if CANON.contains(&w) && !math {
            if matches!(
                prev,
                "fn" | "let"
                    | "mut"
                    | "const"
                    | "static"
                    | "type"
                    | "struct"
                    | "enum"
                    | "trait"
                    | "mod"
                    | "impl"
                    | "as"
                    | "ref"
                    | "."
            ) {
                hits.push(hit(
                    t.line,
                    w,
                    "B8: core code defines or shadows nothing named `canon_nan`; it is oa_math's",
                ));
            } else if next == "(" {
                let ours = if prev == "::" {
                    is_math_root(qual)
                } else {
                    imported.contains(w)
                };
                if !ours {
                    hits.push(hit(
                        t.line,
                        w,
                        "B8: this `canon_nan` is not oa_math's: call `oa_math::canon_nan` or import it with `use oa_math::canon_nan;`",
                    ));
                }
            }
        }
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
        } else if w == "raw" && prev == "&" && matches!(next, "const" | "mut") {
            hits.push(hit(t.line, "&raw", "B8: core code takes no address"));
        } else if w == "addr" && prev == "." && call_at(toks, i + 1) {
            hits.push(hit(t.line, ".addr(", "B8: core code takes no address"));
        } else if w == "eq" && prev == "::" && qual == "ptr" {
            hits.push(hit(
                t.line,
                "ptr::eq",
                "B8: core code compares no addresses",
            ));
        } else if matches!(w, "MAX" | "MIN" | "BITS")
            && prev == "::"
            && (matches!(qual, "usize" | "isize")
                || (qual == ">" && matches!(s(toks, i.wrapping_sub(3)), "usize" | "isize")))
        {
            hits.push(hit(
                t.line,
                &format!(
                    "{}::{w}",
                    if qual == ">" {
                        s(toks, i.wrapping_sub(3))
                    } else {
                        qual
                    }
                ),
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
        } else if w == "type"
            && is_word(next)
            && s(toks, i + 2) == "="
            && matches!(s(toks, i + 3), "usize" | "isize")
            && s(toks, i + 4) == ";"
        {
            hits.push(hit(
                t.line,
                &format!("type {next}"),
                "B8: no alias of usize or isize in core code (`Alias::MAX` would hide the platform width)",
            ));
        } else if w == "as" && matches!(next, "fn" | "unsafe" | "extern") {
            hits.push(hit(
                t.line,
                "as fn",
                "B8: no cast to a fn pointer, its Debug and Pointer text is an address",
            ));
        }
        if !math {
            float_rules(toks, i, &ev, hits);
        }
    }
}

/// The `Name::` qualifier of the method or path at `toks[i]` is a float type: `f64::x`, `<f32>::x`.
fn float_path(toks: &[Tok], i: usize) -> bool {
    let prev = if i == 0 { "" } else { s(toks, i - 1) };
    let qual = s(toks, i.wrapping_sub(2));
    prev == "::"
        && (matches!(qual, "f32" | "f64")
            || (qual == ">"
                && matches!(s(toks, i.wrapping_sub(3)), "f32" | "f64")
                && s(toks, i.wrapping_sub(4)) == "<"))
}

/// The rules of RULESET B8 about float bits, sign and order, for the token at `toks[i]`: what
/// feeds a hash or a written output is `canon_nan(x).to_bits()`, and the functions whose result
/// depends on the sign or payload of a NaN, or on the sign of a zero, are `oa_math`'s.
fn float_rules(toks: &[Tok], i: usize, ev: &Evidence<'_>, hits: &mut Vec<Hit>) {
    let w = s(toks, i);
    let prev = if i == 0 { "" } else { s(toks, i - 1) };
    let line = toks.get(i).map_or(0, |t| t.line);
    let path = float_path(toks, i);
    let method = prev == "." && call_at(toks, i + 1);
    match w {
        "to_bits" if method || path => {
            // the receiver (or the UFCS argument) must be `canon_nan(..)`: a NaN's sign and
            // payload are not portable; a path used as a value (`.map(f64::to_bits)`) is not
            let canon = if prev == "." {
                i >= 2
                    && s(toks, i - 2) == ")"
                    && open_of(toks, i - 2)
                        .is_some_and(|o| o >= 1 && CANON.contains(&s(toks, o - 1)))
            } else if call_at(toks, i + 1) {
                let open = skip_turbofish(toks, i + 1);
                canon_call_end(toks, open + 1) == Some(skip_group(toks, open) - 1)
            } else {
                false
            };
            if !canon {
                hits.push(hit(
                    line,
                    "to_bits",
                    "B8: a float's bits in core code come from `oa_math::canon_nan(x).to_bits()`, a NaN's sign and payload differ between machines",
                ));
            }
        }
        _ if BYTES.contains(&w) && (path || (method && i >= 2 && ev.recv_is_float(i - 2))) => {
            hits.push(hit(
                line,
                w,
                "B8: a float's bytes in core code come from `oa_math::canon_nan(x).to_bits()`, then the integer's bytes",
            ));
        }
        "total_cmp" | "is_sign_negative" | "is_sign_positive" | "copysign" => {
            let instead = match w {
                "total_cmp" => "oa_math::cmp_canon_f64 or cmp_canon_f32",
                "copysign" => "oa_math::copysign_canon_f64 or copysign_canon_f32",
                _ => "oa_math::is_sign_negative_canon_f64 or is_sign_negative_canon_f32 (is_sign_positive is its negation)",
            };
            hits.push(hit(
                line,
                w,
                &format!("B8: `{w}` reads the sign and payload of a NaN, which differ between machines; use {instead}"),
            ));
        }
        "signum" | "min" | "max" | "clamp" => {
            let float = path
                || (method && {
                    let open = skip_turbofish(toks, i + 1);
                    let close = skip_group(toks, open);
                    (i >= 2 && ev.recv_is_float(i - 2)) || ev.mentions(open + 1, close - 1)
                });
            if float {
                let instead = match w {
                    "signum" => "compare with 0.0 and write the sign out, or use oa_math::copysign_canon_f64(1.0, x)",
                    "min" => "oa_math::min_f64 or min_f32",
                    "max" => "oa_math::max_f64 or max_f32",
                    _ => "oa_math::clamp_f64 or clamp_f32",
                };
                hits.push(hit(
                    line,
                    w,
                    &format!("B8: a float `{w}` does not say which zero it returns for -0.0 and 0.0 or which NaN it keeps; use {instead}"),
                ));
            }
        }
        _ => {}
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
pub(super) fn head_of(toks: &[Tok]) -> (String, bool) {
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
        return call_at(toks, i + 1);
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
