//! RULESET B5 beyond the `pub fn`: every way shell code can hand behaviour to the core that is not
//! a public function signature. A `pub trait` the shell can implement, a generic parameter bounded
//! by a trait the shell can implement, and a function pointer, `Fn*` or `dyn` anywhere. Same
//! lexer as [`super`]; a hit is a [`CallbackSig`], so the reviewed allowlist covers it the same
//! way (`crate file.rs::name reason`).

use super::harden::{head_of, CallbackSig};
use super::{
    is_word, mask, s, skip_group, skip_modifiers, skip_turbofish, strip_test_items, tokenize, Ctx,
    Tok,
};
use std::collections::BTreeSet;

/// The bounds a core generic may carry besides a sealed trait: std marker and derivable traits
/// whose behaviour a derive writes, not the caller.
pub const FIXED_BOUNDS: &[&str] = &[
    "Copy",
    "Clone",
    "Debug",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Default",
    "Sized",
];
const CALLBACK_WORDS: &[&str] = &[
    "Fn",
    "FnMut",
    "FnOnce",
    "AsyncFn",
    "AsyncFnMut",
    "AsyncFnOnce",
    "dyn",
];

/// Module names the file declares, split by visibility: `pub mod` (unrestricted) and every other
/// `mod` (private, `pub(crate)`, `pub(super)`). The modules `core` and `shell` of a split crate
/// are neither: they are structure, not seals.
pub fn module_visibility(src: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let (mut private, mut public) = (BTreeSet::new(), BTreeSet::new());
    for (i, t) in toks.iter().enumerate() {
        if t.s != "mod" || !is_word(s(&toks, i + 1)) || !matches!(s(&toks, i + 2), "{" | ";") {
            continue;
        }
        let name = s(&toks, i + 1);
        if matches!(name, "core" | "shell") {
            continue;
        }
        if i >= 1 && s(&toks, i - 1) == "pub" {
            public.insert(name.to_string());
        } else {
            private.insert(name.to_string());
        }
    }
    (private, public)
}

/// Whether the path `toks[from..to]` runs through a module in `private`: a segment other than
/// the last is one (`sealed::Sealed`, `crate::sealed::Sealed`).
fn through_private(toks: &[Tok], from: usize, to: usize, private: &BTreeSet<String>) -> bool {
    (from..to).any(|k| {
        s(toks, k + 1) == "::"
            && private.contains(s(toks, k))
            && !matches!(s(toks, k), "self" | "super" | "crate")
    })
}

/// Names a file imports through a private module (`use sealed::Sealed;`,
/// `use crate::sealed::{Sealed, Other};`).
fn imported_via_private(toks: &[Tok], private: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < toks.len() {
        if s(toks, i) == "use" && super::is_path_start(toks, i) {
            let end = (i..toks.len())
                .find(|&k| s(toks, k) == ";")
                .unwrap_or(toks.len());
            let mut seen = false;
            for k in i + 1..end {
                if private.contains(s(toks, k)) && s(toks, k + 1) == "::" {
                    seen = true;
                } else if seen && is_word(s(toks, k)) && !matches!(s(toks, k + 1), "::") {
                    out.insert(s(toks, k).to_string());
                }
            }
            i = end;
        }
        i += 1;
    }
    out
}

/// Index of the token that ends the item header started at `from`: the first `{` or `;` outside
/// brackets, and the position of a `where` before it.
fn header_end(toks: &[Tok], from: usize) -> (usize, Option<usize>) {
    let mut depth = 0usize;
    let mut where_at = None;
    let mut k = from;
    while k < toks.len() {
        match s(toks, k) {
            "(" | "[" => depth += 1,
            ")" | "]" => depth = depth.saturating_sub(1),
            "<" => depth += 1,
            ">" if s(toks, k - 1) != "-" => depth = depth.saturating_sub(1),
            "{" | ";" if depth == 0 => return (k, where_at),
            "where" if depth == 0 && where_at.is_none() => where_at = Some(k),
            _ => {}
        }
        k += 1;
    }
    (toks.len(), where_at)
}

/// Index just past the `<..>` that opens at `toks[i]`.
fn angles_end(toks: &[Tok], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < toks.len() {
        match s(toks, j) {
            "<" => depth += 1,
            ">" if s(toks, j - 1) != "-" => {
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

/// Splits `toks[from..to]` at `sep` outside brackets (angle brackets included).
fn split_top(toks: &[Tok], from: usize, to: usize, sep: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = from;
    for k in from..to {
        match s(toks, k) {
            "(" | "[" | "{" | "<" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            ">" if s(toks, k - 1) != "-" => depth = depth.saturating_sub(1),
            x if x == sep && depth == 0 => {
                out.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
    }
    if start < to {
        out.push((start, to));
    }
    out
}

/// The last path segment of the bound that starts at `toks[from]`, after `for<..>` and `?`;
/// `None` for an empty bound (a lifetime was blanked) or a `?Trait`.
fn bound_name(toks: &[Tok], from: usize, to: usize) -> Option<String> {
    let mut j = from;
    if s(toks, j) == "for" && s(toks, j + 1) == "<" {
        j = angles_end(toks, j + 1);
    }
    if s(toks, j) == "?" {
        return None;
    }
    while s(toks, j) == "::" {
        j += 1;
    }
    let mut last = None;
    while j < to && is_word(s(toks, j)) {
        last = Some(s(toks, j).to_string());
        if s(toks, j + 1) == "::" {
            j += 2;
        } else {
            break;
        }
    }
    last
}

/// The bounds in `toks[from..to]` (`A + B<X> + 'a`) that are not a fixed std marker trait or a
/// sealed trait.
fn bad_bounds(toks: &[Tok], from: usize, to: usize, ctx: &Ctx<'_>) -> Vec<String> {
    split_top(toks, from, to, "+")
        .into_iter()
        .filter_map(|(a, b)| bound_name(toks, a, b))
        .filter(|n| !FIXED_BOUNDS.contains(&n.as_str()) && !ctx.sealed.contains(n))
        .collect()
}

/// What `callback_sigs` calls the self type of an impl, for the allowlist key.
fn impl_key(toks: &[Tok], from: usize, to: usize) -> String {
    let from = if s(toks, from) == "<" {
        angles_end(toks, from)
    } else {
        from
    };
    let mut angle = 0usize;
    let mut head_from = from;
    for k in from..to {
        match s(toks, k) {
            "<" => angle += 1,
            ">" if s(toks, k - 1) != "-" => angle = angle.saturating_sub(1),
            "for" if angle == 0 => head_from = k + 1,
            _ => {}
        }
    }
    format!("impl@{}", head_of(toks.get(head_from..to).unwrap_or(&[])).0)
}

fn note(why: &mut Vec<String>, bad: Vec<String>, what: &str) {
    for b in bad {
        why.push(format!(
            "{what} bound `{b}` is not a std marker trait or a sealed trait"
        ));
    }
}

/// The first `:` of `toks[from..to]` outside brackets.
fn top_colon(toks: &[Tok], from: usize, to: usize) -> Option<usize> {
    let mut depth = 0usize;
    for k in from..to {
        match s(toks, k) {
            "(" | "[" | "{" | "<" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            ">" if s(toks, k - 1) != "-" => depth = depth.saturating_sub(1),
            ":" if depth == 0 => return Some(k),
            _ => {}
        }
    }
    None
}

/// B5 for traits, generics and callback types in `src` (see the module doc).
pub fn extra_sigs(src: &str, ctx: &Ctx<'_>) -> Vec<CallbackSig> {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let mut out = Vec::new();
    seal_rules(&toks, ctx, &mut out);
    bound_rules(&toks, ctx, &mut out);
    callback_words(&toks, &mut out);
    out
}

/// The `pub trait`s of `src` that are sealed, and the traits that seal them: a supertrait reached
/// through a private module of the crate (`pub trait T: sealed::Sealed`, or `Sealed` imported
/// through one).
pub fn sealed_traits(
    src: &str,
    private: &BTreeSet<String>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let (mut sealed, mut sealing) = (BTreeSet::new(), BTreeSet::new());
    for t in pub_traits(&toks, private) {
        if t.sealed {
            sealed.insert(t.name);
            sealing.extend(t.sealing);
        }
    }
    (sealed, sealing)
}

struct PubTrait {
    name: String,
    line: usize,
    sealed: bool,
    /// The supertraits that seal it (`Sealed` of `sealed::Sealed`).
    sealing: Vec<String>,
}

/// The `pub trait` items (unrestricted `pub`) of the tokens and whether each is sealed.
fn pub_traits(toks: &[Tok], private: &BTreeSet<String>) -> Vec<PubTrait> {
    let imported = imported_via_private(toks, private);
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.s != "pub" || s(toks, i + 1) == "(" {
            continue;
        }
        let j = skip_modifiers(toks, i);
        // `unsafe trait`, `auto trait`
        let mut k = j;
        while matches!(s(toks, k), "unsafe" | "auto") {
            k += 1;
        }
        if s(toks, k) != "trait" || !is_word(s(toks, k + 1)) {
            continue;
        }
        let name = s(toks, k + 1).to_string();
        let mut m = k + 2;
        if s(toks, m) == "<" {
            m = angles_end(toks, m);
        }
        let mut sealing = Vec::new();
        if s(toks, m) == ":" {
            let (end, _) = header_end(toks, m);
            for (a, b) in split_top(toks, m + 1, end, "+") {
                let bare = bound_name(toks, a, b);
                let via_path = through_private(toks, a, b, private);
                let via_use = b == a + 1 && bare.as_ref().is_some_and(|n| imported.contains(n));
                if let (true, Some(n)) = (via_path || via_use, bare) {
                    sealing.push(n);
                }
            }
        }
        out.push(PubTrait {
            name,
            line: t.line,
            sealed: !sealing.is_empty(),
            sealing,
        });
    }
    out
}

fn seal_rules(toks: &[Tok], ctx: &Ctx<'_>, out: &mut Vec<CallbackSig>) {
    for t in pub_traits(toks, ctx.private_mods) {
        // the trait that seals the others (`mod sealed { pub trait Sealed {} }`) is not itself
        // implementable from outside: no path to it leaves the private module
        if !t.sealed && !ctx.sealing.contains(&t.name) {
            out.push(CallbackSig {
                name: t.name,
                line: t.line,
                why: "a `pub trait` that is not sealed (no supertrait from a private module): the shell can implement it".to_string(),
            });
        }
    }
    for (i, t) in toks.iter().enumerate() {
        // `pub use sealed::Sealed;` hands the seal out
        if t.s == "use" && i >= 1 && s(toks, i - 1) == "pub" {
            let end = (i..toks.len())
                .find(|&k| s(toks, k) == ";")
                .unwrap_or(toks.len());
            if let Some(k) = (i + 1..end).find(|&k| ctx.sealing.contains(s(toks, k))) {
                out.push(CallbackSig {
                    name: s(toks, k).to_string(),
                    line: t.line,
                    why: "`pub use` of a sealing trait hands the seal out".to_string(),
                });
            }
        }
        // `impl<T> sealed::Sealed for T {}` seals nothing
        if t.s == "impl"
            && matches!(
                if i == 0 { "" } else { s(toks, i - 1) },
                "" | ";" | "}" | "{" | "]" | "unsafe"
            )
        {
            let mut j = i + 1;
            let mut params: BTreeSet<&str> = BTreeSet::new();
            if s(toks, j) == "<" {
                let end = angles_end(toks, j);
                for (a, _) in split_top(toks, j + 1, end - 1, ",") {
                    if is_word(s(toks, a)) && s(toks, a) != "const" {
                        params.insert(s(toks, a));
                    }
                }
                j = end;
            }
            let (end, _) = header_end(toks, j);
            let for_at = (j..end).find(|&k| s(toks, k) == "for" && s(toks, k + 1) != "<");
            if let Some(f) = for_at {
                let trait_via = bound_name(toks, j, f).is_some_and(|n| ctx.sealing.contains(&n));
                let self_bare = f + 2 == end || s(toks, f + 2) == "where";
                if trait_via && self_bare && params.contains(s(toks, f + 1)) {
                    out.push(CallbackSig {
                        name: format!("impl@{}", s(toks, f + 1)),
                        line: t.line,
                        why: "a blanket impl of a sealing trait: any shell type with the bound is sealed in".to_string(),
                    });
                }
            }
        }
    }
}

fn bound_rules(toks: &[Tok], ctx: &Ctx<'_>, out: &mut Vec<CallbackSig>) {
    for (i, t) in toks.iter().enumerate() {
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let (kind, name) = match t.s.as_str() {
            "fn" | "struct" | "enum" | "union" | "trait" | "type" if is_word(s(toks, i + 1)) => {
                (t.s.as_str(), s(toks, i + 1).to_string())
            }
            "impl" if matches!(prev, "" | ";" | "}" | "{" | "]" | "unsafe") => {
                ("impl", String::new())
            }
            _ => continue,
        };
        let mut j = if kind == "impl" { i + 1 } else { i + 2 };
        let mut params: Vec<(String, bool)> = Vec::new();
        let mut why: Vec<String> = Vec::new();
        let gen_end = if s(toks, j) == "<" {
            let end = angles_end(toks, j);
            for (a, b) in split_top(toks, j + 1, end - 1, ",") {
                if s(toks, a) == "const" || !is_word(s(toks, a)) {
                    continue;
                }
                let pname = s(toks, a).to_string();
                let mut bounded = false;
                if s(toks, a + 1) == ":" {
                    // bounds up to a default `= Type`
                    let stop = (a + 2..b).find(|&k| s(toks, k) == "=").unwrap_or(b);
                    note(&mut why, bad_bounds(toks, a + 2, stop, ctx), "a generic");
                    bounded = split_top(toks, a + 2, stop, "+")
                        .into_iter()
                        .any(|(x, y)| bound_name(toks, x, y).is_some());
                }
                params.push((pname, bounded));
            }
            end
        } else {
            j
        };
        j = gen_end;
        // fn: arguments, then `-> ret`, then `where`
        if kind == "fn" && s(toks, j) == "(" {
            let end = skip_group(toks, j);
            for (a, b) in split_top(toks, j + 1, end - 1, ",") {
                for k in a..b {
                    if s(toks, k) == "impl" {
                        note(
                            &mut why,
                            bad_bounds(toks, k + 1, b, ctx),
                            "an `impl` argument",
                        );
                    }
                }
            }
            j = end;
        }
        let (stop, where_at) = header_end(toks, j);
        if let Some(w) = where_at {
            for (a, b) in split_top(toks, w + 1, stop, ",") {
                let Some(colon) = top_colon(toks, a, b) else {
                    continue;
                };
                note(&mut why, bad_bounds(toks, colon + 1, b, ctx), "a where");
                if colon == a + 1 {
                    let lhs = s(toks, a).to_string();
                    for p in params.iter_mut() {
                        if p.0 == lhs {
                            p.1 = true;
                        }
                    }
                }
            }
        }
        let key = if kind == "impl" {
            impl_key(toks, i + 1, where_at.unwrap_or(stop))
        } else {
            name
        };
        // an unbounded type parameter stored in the field of a `pub` struct, enum or union
        if matches!(kind, "struct" | "enum" | "union") && prev == "pub" {
            let end = if s(toks, stop) == "{" {
                skip_group(toks, stop)
            } else {
                stop
            };
            let body_from = if s(toks, gen_end) == "(" {
                gen_end
            } else {
                stop
            };
            for (pname, bounded) in &params {
                if !bounded && (body_from..end).any(|k| s(toks, k) == pname) {
                    why.push(format!(
                        "the unbounded type parameter `{pname}` is stored in a field of a `pub` type"
                    ));
                }
            }
        }
        why.sort();
        why.dedup();
        if !why.is_empty() {
            out.push(CallbackSig {
                name: key,
                line: t.line,
                why: why.join("; "),
            });
        }
    }
}

/// Every `Fn`, `FnMut`, `FnOnce`, `dyn` and `fn(..)` pointer type of the file, with the name of
/// the innermost item that holds it.
fn callback_words(toks: &[Tok], out: &mut Vec<CallbackSig>) {
    // (start, end, name) of every item that can hold one
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let next = s(toks, i + 1);
        let (name, from) = match t.s.as_str() {
            "fn" if is_word(next) => (next.to_string(), i),
            "struct" | "enum" | "union" | "trait" | "type" | "static" if is_word(next) => {
                (next.to_string(), i)
            }
            "const" if is_word(next) && !matches!(next, "fn" | "unsafe" | "async" | "extern") => {
                (next.to_string(), i)
            }
            "impl" if matches!(prev, "" | ";" | "}" | "{" | "]" | "unsafe") => {
                let (end, _) = header_end(toks, i + 1);
                (impl_key(toks, i + 1, end), i)
            }
            _ => continue,
        };
        // to the `;` or the closing brace of the item
        let mut depth = 0usize;
        let mut k = from + 1;
        let mut end = toks.len();
        while k < toks.len() {
            match s(toks, k) {
                "(" | "[" => depth += 1,
                ")" | "]" => depth = depth.saturating_sub(1),
                "{" if depth == 0 => {
                    end = skip_group(toks, k);
                    break;
                }
                ";" if depth == 0 => {
                    end = k;
                    break;
                }
                _ => {}
            }
            k += 1;
        }
        spans.push((from, end, name));
    }
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        let hit = CALLBACK_WORDS.contains(&w)
            || (w == "fn" && skip_turbofish(toks, i + 1) == i + 1 && s(toks, i + 1) == "(");
        if !hit {
            continue;
        }
        let name = spans
            .iter()
            .filter(|(a, b, _)| *a <= i && i < *b)
            .max_by_key(|(a, _, _)| *a)
            .map_or("<file>", |(_, _, n)| n.as_str());
        out.push(CallbackSig {
            name: name.to_string(),
            line: t.line,
            why: format!(
                "`{}` in core code: a callback or function pointer can carry platform behaviour",
                if w == "fn" { "fn(..)" } else { w }
            ),
        });
    }
}
