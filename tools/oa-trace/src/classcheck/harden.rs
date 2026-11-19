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
/// Every `oa_math` helper that core code must reach by qualified path (RULESET B8): the NaN
/// canonicalisers and each public function of `order.rs` (a test keeps this list whole).
pub const HELPERS: &[&str] = &[
    "canon_nan",
    "canon_nan_f32",
    "cmp_canon_f64",
    "cmp_canon_f32",
    "is_sign_negative_canon_f64",
    "is_sign_negative_canon_f32",
    "copysign_canon_f64",
    "copysign_canon_f32",
    "signum_canon_f64",
    "signum_canon_f32",
    "min_f64",
    "min_f32",
    "max_f64",
    "max_f32",
    "clamp_f64",
    "clamp_f32",
];
const BYTES: &[&str] = &["to_le_bytes", "to_be_bytes", "to_ne_bytes"];

/// Whether `toks[i]` starts a call `oa_math::canon_nan(..)` or `oa_math::canon_nan_f32(..)`, by
/// that qualified path only; returns the index just past its argument group.
fn canon_call_end(toks: &[Tok], i: usize) -> Option<usize> {
    (s(toks, i) == "oa_math"
        && s(toks, i + 1) == "::"
        && CANON.contains(&s(toks, i + 2))
        && s(toks, i + 3) == "(")
        .then(|| skip_group(toks, i + 3))
}

/// Whether the `to_bits` at `toks[i]` is the method form on the result of a qualified
/// `oa_math::canon_nan(..)` / `canon_nan_f32(..)` call, the call alone or inside extra parentheses.
fn canon_to_bits(toks: &[Tok], i: usize) -> bool {
    if i < 2 || s(toks, i - 1) != "." || s(toks, i - 2) != ")" || !call_at(toks, i + 1) {
        return false;
    }
    let mut close = i - 2;
    loop {
        let Some(open) = open_of(toks, close) else {
            return false;
        };
        // a call's own argument group: `oa_math::canon_nan(x)` directly before `.to_bits()`
        if open >= 3 && canon_call_end(toks, open - 3) == Some(close + 1) {
            return open < 4 || s(toks, open - 4) != "::";
        }
        // extra parentheses: not an argument group, so not after a name, `)`, `]`, `>` or `?`
        let before = s(toks, open.wrapping_sub(1));
        let argument = open >= 1
            && ((is_word(before)
                && !matches!(before, "return" | "in" | "if" | "match" | "else" | "while"))
                || matches!(before, ")" | "]" | ">" | "?"));
        if argument || open + 1 >= close {
            return false;
        }
        if s(toks, open + 1) == "(" && skip_group(toks, open + 1) == close {
            close = skip_group(toks, open + 1) - 1;
            continue;
        }
        return canon_call_end(toks, open + 1) == Some(close)
            && (open < 1 || s(toks, open - 1) != "::");
    }
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

/// Token indexes inside a `use ..;` or `extern ..;` statement, where `as` renames.
fn rename_statements(toks: &[Tok]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < toks.len() {
        if matches!(s(toks, i), "use" | "extern") && is_path_start(toks, i) {
            let end = (i..toks.len())
                .find(|&k| s(toks, k) == ";")
                .unwrap_or(toks.len());
            out.extend(i..end);
            i = end;
        }
        i += 1;
    }
    out
}

/// Whether the integer primitive name at `toks[i]` is declared or renamed there: `type u32 = ..`,
/// `mod i64`, `struct u8`, `enum`, `union`, `trait`, `.. as u64` in a `use` or `extern crate`, or a
/// generic parameter (`fn f<u64>()`, `struct S<T, u8>`). Then `u64::to_le_bytes(x)` or
/// `i64::signum(x)` may not mean the primitive's function.
pub(super) fn int_shadow(toks: &[Tok], i: usize, stmt: &BTreeSet<usize>) -> bool {
    if !super::INT_TYPES.contains(&s(toks, i)) {
        return false;
    }
    let prev = if i == 0 { "" } else { s(toks, i - 1) };
    if matches!(prev, "type" | "mod" | "struct" | "enum" | "union" | "trait") {
        return true;
    }
    if prev == "as" && stmt.contains(&i) {
        return true;
    }
    if matches!(prev, "<" | ",") {
        // a generic parameter list: walk back to its `<`, named by `fn f`, `struct S`, `impl`..
        let mut depth = 0usize;
        let mut j = i;
        while j > 0 {
            j -= 1;
            match s(toks, j) {
                ">" if s(toks, j.wrapping_sub(1)) != "-" => depth += 1,
                "<" if depth == 0 => {
                    // `impl<..>`, or `fn NAME<..>`, `struct NAME<..>` and the like
                    return s(toks, j.wrapping_sub(1)) == "impl"
                        || (j >= 2
                            && matches!(
                                s(toks, j - 2),
                                "fn" | "struct" | "enum" | "union" | "trait" | "type"
                            ));
                }
                "<" => depth -= 1,
                ";" | "{" | "}" | "(" | ")" => return false,
                _ => {}
            }
        }
    }
    false
}

/// The ban checks of RULESET B8 that look at one token with its neighbours: unsafe and link
/// items, addresses, platform sizes, lowercase type aliases, fn-pointer casts, and a float's
/// bits that were not canonicalised. `oa-math` itself does bit arithmetic on floats by design.
pub(super) fn token_bans(toks: &[Tok], ctx: &Ctx<'_>, hits: &mut Vec<Hit>) {
    let math = ctx.math;
    let guard = guard_mask(toks);
    let ev = Evidence::new(toks, ctx.floats);
    // only the external crate `oa_math` (the dependency key, when the crate declares one, must
    // name the oa-math package); a module, import rename or local of that name is not it
    let math_dep = ctx.aliases.get("oa_math").is_none_or(|t| t == "oa-math");
    let is_math_root = |w: &str| w == "oa_math" && math_dep;
    // oa_math's helpers are called by qualified path only: an import, a bare call or a local
    // of the same name could be anything, and the scan reads no types
    for (from, to) in use_statements(toks) {
        if math {
            break;
        }
        for t in toks.get(from..to).unwrap_or(&[]) {
            if HELPERS.contains(&t.s.as_str()) {
                hits.push(hit(
                    t.line,
                    &t.s,
                    "B8: oa_math's helpers are called as `oa_math::<name>(..)`, never imported",
                ));
            }
        }
        let body = toks.get(from..to).unwrap_or(&[]);
        let from_math = body
            .iter()
            .enumerate()
            .any(|(k, t)| is_math_root(&t.s) && is_path_start(body, k));
        if from_math && body.iter().any(|t| t.s == "*") {
            hits.push(hit(
                body.first().map_or(0, |t| t.line),
                "*",
                "B8: `use oa_math::*` hides which helper a name is; call `oa_math::<name>(..)`",
            ));
        }
    }
    let stmt = rename_statements(toks);
    let in_use: BTreeSet<usize> = use_statements(toks)
        .into_iter()
        .flat_map(|(a, b)| a..b)
        .collect();
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let next = s(toks, i + 1);
        let qual = s(toks, i.wrapping_sub(2));
        if w == "oa_math"
            && !math
            && (next != "::"
                || prev == "::"
                || matches!(
                    prev,
                    "mod"
                        | "as"
                        | "fn"
                        | "let"
                        | "mut"
                        | "const"
                        | "static"
                        | "type"
                        | "struct"
                        | "enum"
                        | "union"
                        | "trait"
                        | "ref"
                        | "crate"
                        | "impl"
                ))
        {
            hits.push(hit(
                t.line,
                w,
                "B8: `oa_math` in core code names only the external crate, as the head of a path `oa_math::..`; a module, rename, local or parameter of that name fails",
            ));
        }
        if (w == "capacity" && (prev == "::" || (prev == "." && next == "(")))
            || w == "spare_capacity_mut"
        {
            hits.push(hit(
                t.line,
                w,
                "B8: `capacity` (a call, a method or a path value such as `Vec::capacity`) is a value the allocator chooses; compare lengths or keep an explicit capacity value. `try_reserve`-style probes are left alone: their only result is success or failure",
            ));
        }
        if !math && int_shadow(toks, i, &stmt) {
            hits.push(hit(
                t.line,
                w,
                "B8: an item, rename or generic parameter named after an integer primitive would capture `u64::to_le_bytes(x)`",
            ));
        }
        if HELPERS.contains(&w)
            && !math
            && !in_use.contains(&i)
            && (prev != "::" || !is_math_root(qual))
        {
            hits.push(hit(
                t.line,
                w,
                "B8: this name is oa_math's: write `oa_math::<name>(..)`; core code defines, imports or shadows no helper of that name",
            ));
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
        "to_bits" => {
            // every `to_bits` token, method or path, any qualifier, fails unless it is the method
            // on the result of `oa_math::canon_nan(..)`: a NaN's sign and payload are not portable
            if !canon_to_bits(toks, i) {
                hits.push(hit(
                    line,
                    "to_bits",
                    "B8: `to_bits` in core code is allowed only directly after `oa_math::canon_nan(x)` or `oa_math::canon_nan_f32(x)`, a NaN's sign and payload differ between machines",
                ));
            }
        }
        _ if BYTES.contains(&w) => {
            // only `u64::to_le_bytes(x)` with an integer primitive qualifier: the compiler
            // then rejects a float argument, which the scan cannot tell from any other receiver
            let q = s(toks, i.wrapping_sub(2));
            let ok = prev == "::"
                && super::INT_TYPES.contains(&q)
                && (s(toks, i.wrapping_sub(3)) != "::"
                    || s(toks, i.wrapping_sub(4)) == "primitive");
            if !ok {
                hits.push(hit(
                    line,
                    w,
                    &format!("B8: write `u64::{w}(x)` with an integer primitive as the qualifier (the compiler then refuses a float); a float's bytes come from `oa_math::canon_nan(x).to_bits()` and then the integer's"),
                ));
            }
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

/// Sign and order reads the whole workspace leaves to `oa_math` (RULESET B8, OA-TRC-260): the
/// standard library's `total_cmp`, `is_sign_negative`, `is_sign_positive`, `copysign` and `signum`
/// depend on the sign and payload of a NaN, which differ between machines, so a sort or a
/// selection that reads them can pick differently on two of them. Returns the hits of the
/// non-test tokens of `src`, the out-of-line test modules and whether the whole file is test code.
/// `signum` is allowed only as the integer path `i64::signum(x)`, which the compiler checks.
pub(super) fn sign_order_hits(src: &str) -> (Vec<Hit>, Vec<String>, bool) {
    let st = strip_test_items(tokenize(&mask(src)));
    let toks = &st.toks;
    let mut hits = Vec::new();
    let stmt = rename_statements(toks);
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        if int_shadow(toks, i, &stmt) {
            hits.push(hit(
                t.line,
                w,
                "B8: an item, rename or generic parameter named after an integer primitive would capture `i64::signum(x)`",
            ));
        }
        let instead = match w {
            "total_cmp" => "oa_math::cmp_canon_f64 or cmp_canon_f32",
            "copysign" => "oa_math::copysign_canon_f64 or copysign_canon_f32",
            "is_sign_negative" | "is_sign_positive" => {
                "oa_math::is_sign_negative_canon_f64 or is_sign_negative_canon_f32 (is_sign_positive is its negation)"
            }
            "signum" => "oa_math::signum_canon_f64 or signum_canon_f32, or the integer path `i64::signum(x)`",
            _ => continue,
        };
        if w == "signum"
            && s(toks, i.wrapping_sub(1)) == "::"
            && super::INT_TYPES.contains(&s(toks, i.wrapping_sub(2)))
        {
            continue;
        }
        hits.push(hit(
            t.line,
            w,
            &format!("B8: `{w}` reads the sign and payload of a NaN, which differ between machines; use {instead}"),
        ));
    }
    (hits, st.test_mods, st.whole_file)
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
