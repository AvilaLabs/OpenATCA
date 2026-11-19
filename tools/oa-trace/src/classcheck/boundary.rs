//! RULESET B5 for everything the shell can reach: a `pub trait` it can implement, and, in an item
//! it can name (plain `pub`; methods of a `pub trait`; `pub` fields), a generic bound that is not
//! `Copy`, `Sized` or a sealed trait, an `impl Trait` argument, `Fn*`, `dyn`, a `fn(..)` pointer
//! or a private type alias that holds one. Private items may use any of them. Same lexer as
//! [`super`]; a hit is a [`CallbackSig`], so the reviewed allowlist covers it the same way
//! (`crate file.rs::name reason`).

use super::harden::CallbackSig;
use super::{
    is_word, mask, s, skip_group, skip_modifiers, skip_turbofish, strip_test_items, tokenize, Ctx,
    Tok,
};
use std::collections::{BTreeMap, BTreeSet};

/// The bounds a reachable generic may carry besides a sealed trait: `Sized`. Every other std
/// trait has methods a shell type can implement, `Copy` included (`Copy: Clone`, and `vec![x; n]`,
/// `resize`, `to_vec` and `fill` run a hand-written `Clone`), and `Eq` counts as `PartialEq`.
pub const FIXED_BOUNDS: &[&str] = &["Sized"];
const CALLBACK_WORDS: &[&str] = &[
    "Fn",
    "FnMut",
    "FnOnce",
    "AsyncFn",
    "AsyncFnMut",
    "AsyncFnOnce",
    "dyn",
];

/// Every `mod name {` or `mod name;` of the file, outside test code: the inline modules that
/// enclose it (outermost first), its name, and whether it is plain `pub`. The modules `core` and
/// `shell` of a split crate are structure, not seals, and are left out.
pub fn module_decls(src: &str) -> Vec<(Vec<String>, String, bool)> {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let mut out = Vec::new();
    // inline modules still open: (name, brace depth inside it)
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate() {
        match t.s.as_str() {
            "{" => depth += 1,
            "}" => {
                depth = depth.saturating_sub(1);
                while stack.last().is_some_and(|(_, d)| *d > depth) {
                    stack.pop();
                }
            }
            "mod" if is_word(s(&toks, i + 1)) && matches!(s(&toks, i + 2), "{" | ";") => {
                let name = s(&toks, i + 1);
                if matches!(name, "core" | "shell") {
                    continue;
                }
                let public = i >= 1 && s(&toks, i - 1) == "pub";
                let outer = stack.iter().map(|(n, _)| n.clone()).collect();
                out.push((outer, name.to_string(), public));
                if s(&toks, i + 2) == "{" {
                    stack.push((name.to_string(), depth + 1));
                }
            }
            _ => {}
        }
    }
    out
}

/// The modules of one crate by name, split by whether the shell can reach them: reachable only if
/// the module and every module that encloses it is plain `pub`. `files` are the crate's sources
/// with the module path of each file (`src/a/b.rs` is `["a", "b"]`; `lib.rs`, and the `core` root
/// of a split crate, is empty). A module whose enclosing file module is not declared anywhere is
/// taken as reachable (it cannot seal anything). A name declared reachable anywhere is reachable.
pub fn module_visibility(files: &[(Vec<String>, &str)]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut decls: BTreeMap<Vec<String>, bool> = BTreeMap::new();
    for (base, text) in files {
        for (inline, name, public) in module_decls(text) {
            let mut path = base.clone();
            path.extend(inline);
            path.push(name);
            let e = decls.entry(path).or_insert(false);
            *e |= public;
        }
    }
    let (mut private, mut public) = (BTreeSet::new(), BTreeSet::new());
    for path in decls.keys() {
        let reachable = (1..=path.len()).all(|n| {
            path.get(..n)
                .and_then(|p| decls.get(p).copied())
                .unwrap_or(true)
        });
        if let Some(name) = path.last() {
            if reachable {
                public.insert(name.clone());
            } else {
                private.insert(name.clone());
            }
        }
    }
    (private, public)
}

/// The module that holds the trait a bound names, when the path that starts at `toks[from]` runs
/// through a module in `private`: one of its leading segments, before any `<..>`, other than the
/// last (`sealed::Sealed`, `crate::sealed::Sealed`, `inner::sealed::Sealed` -> `sealed`). A path
/// inside the generic arguments (`Iterator<Item = sealed::Private>`) is not the bound's own path.
fn seal_module(toks: &[Tok], from: usize, to: usize, private: &BTreeSet<String>) -> Option<String> {
    let mut j = from;
    while matches!(s(toks, j), "::" | "?" | "(") || (s(toks, j) == "for" && s(toks, j + 1) == "<") {
        j = if s(toks, j) == "for" {
            angles_end(toks, j + 1)
        } else {
            j + 1
        };
    }
    let mut segs: Vec<&str> = Vec::new();
    while j < to && is_word(s(toks, j)) {
        segs.push(s(toks, j));
        if s(toks, j + 1) == "::" {
            j += 2;
        } else {
            break;
        }
    }
    let n = segs.len().checked_sub(1)?;
    let via = segs
        .iter()
        .take(n)
        .any(|g| private.contains(*g) && !matches!(*g, "self" | "super" | "crate"));
    if via {
        segs.get(n.checked_sub(1)?).map(|g| g.to_string())
    } else {
        None
    }
}

/// One name a `use` statement brings in: the path to it (the last segment is the item, or `*`)
/// and the name it is bound to.
struct UseLeaf {
    path: Vec<String>,
    name: String,
}

/// Expands the use tree at `toks[i..]` (up to `end`) into its leaves; returns the index after it.
fn use_leaves(
    toks: &[Tok],
    mut i: usize,
    end: usize,
    prefix: &[String],
    out: &mut Vec<UseLeaf>,
) -> usize {
    let mut path: Vec<String> = prefix.to_vec();
    while i < end {
        match s(toks, i) {
            "::" => i += 1,
            "{" => {
                i += 1;
                while i < end && s(toks, i) != "}" {
                    i = use_leaves(toks, i, end, &path, out);
                    if s(toks, i) == "," {
                        i += 1;
                    }
                }
                return i + 1;
            }
            "*" => {
                path.push("*".to_string());
                out.push(UseLeaf {
                    name: "*".to_string(),
                    path,
                });
                return i + 1;
            }
            // `crate::sealed::{self}` names the module `sealed`
            "self" if !path.is_empty() && !matches!(s(toks, i + 1), "::") => {
                let name = path.last().cloned().unwrap_or_default();
                out.push(UseLeaf { path, name });
                return i + 1;
            }
            w if is_word(w) => {
                path.push(w.to_string());
                i += 1;
                if s(toks, i) == "::" {
                    continue;
                }
                let mut name = w.to_string();
                if s(toks, i) == "as" {
                    name = s(toks, i + 1).to_string();
                    i += 2;
                }
                out.push(UseLeaf { path, name });
                return i;
            }
            _ => return i + 1,
        }
    }
    i
}

/// Every leaf of every `use` statement in the tokens, with the line of the statement and whether
/// it is a plain `pub use`.
fn use_statements(toks: &[Tok]) -> Vec<(usize, bool, Vec<UseLeaf>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if s(toks, i) == "use" && super::is_path_start(toks, i) {
            let end = (i..toks.len())
                .find(|&k| s(toks, k) == ";")
                .unwrap_or(toks.len());
            let mut leaves = Vec::new();
            use_leaves(toks, i + 1, end, &[], &mut leaves);
            out.push((
                toks.get(i).map_or(0, |t| t.line),
                i >= 1 && s(toks, i - 1) == "pub",
                leaves,
            ));
            i = end;
        }
        i += 1;
    }
    out
}

/// Whether a leaf's path runs through a module in `private`, other than by its last segment.
fn leaf_via_private(leaf: &UseLeaf, private: &BTreeSet<String>) -> bool {
    let n = leaf.path.len().saturating_sub(1);
    leaf.path
        .iter()
        .take(n)
        .any(|seg| private.contains(seg) && !matches!(seg.as_str(), "self" | "super" | "crate"))
}

/// Names a file imports through a private module, each with the module that holds it
/// (`use sealed::Sealed;`, `use crate::sealed::{Sealed, Other};`,
/// `use crate::{sealed::Sealed, public::Other};`), per leaf of the use tree.
fn imported_via_private(toks: &[Tok], private: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (_, _, leaves) in use_statements(toks) {
        for leaf in &leaves {
            if leaf_via_private(leaf, private) && leaf.name != "*" {
                let n = leaf.path.len();
                let module = n
                    .checked_sub(2)
                    .and_then(|k| leaf.path.get(k))
                    .cloned()
                    .unwrap_or_default();
                out.insert(leaf.name.clone(), module.clone());
                if let Some(l) = leaf.path.last() {
                    out.insert(l.clone(), module);
                }
            }
        }
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

/// What a bound is.
enum Bound {
    /// A blanked lifetime, `'a`, or `?Sized`: carries no behaviour.
    Skip,
    /// A trait, by the last segment of its path.
    Name(String),
    /// Anything the reader cannot name: judged as a bad bound (unknown is flagged).
    Unknown,
}

/// Reads the bound in `toks[from..to]`: parentheses, `?` and `for<..>` are peeled off first.
fn bound_kind(toks: &[Tok], from: usize, to: usize) -> Bound {
    if from >= to {
        return Bound::Skip;
    }
    let mut j = from;
    let mut question = false;
    loop {
        match s(toks, j) {
            "(" => j += 1,
            "?" => {
                question = true;
                j += 1;
            }
            "for" if s(toks, j + 1) == "<" => j = angles_end(toks, j + 1),
            _ => break,
        }
    }
    if s(toks, j).starts_with('\'') {
        return Bound::Skip;
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
    match last {
        Some(n) if question => {
            if n == "Sized" {
                Bound::Skip
            } else {
                Bound::Unknown
            }
        }
        Some(n) => Bound::Name(n),
        None => Bound::Unknown,
    }
}

/// The last path segment of the bound, when it has one (see [`bound_kind`]).
fn bound_name(toks: &[Tok], from: usize, to: usize) -> Option<String> {
    match bound_kind(toks, from, to) {
        Bound::Name(n) => Some(n),
        _ => None,
    }
}

/// The bounds in `toks[from..to]` (`A + B<X> + 'a`) that are not `Sized` or a sealed trait; one
/// that cannot be read is reported as such.
fn bad_bounds(toks: &[Tok], from: usize, to: usize, ctx: &Ctx<'_>) -> Vec<String> {
    split_top(toks, from, to, "+")
        .into_iter()
        .filter_map(|(a, b)| match bound_kind(toks, a, b) {
            Bound::Skip => None,
            Bound::Unknown => Some("<unreadable>".to_string()),
            Bound::Name(n) => Some(n),
        })
        .filter(|n| {
            !FIXED_BOUNDS.contains(&n.as_str())
                && !ctx.sealed.contains(n)
                && !CALLBACK_WORDS.contains(&n.as_str())
        })
        .collect()
}

/// The tokens `toks[from..to]` as one string without spaces; two words in a row are joined by `_`
/// so `&mut X` and `dyn Fn` stay readable.
fn squash(toks: &[Tok], from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut prev_word = false;
    for k in from..to {
        let w = s(toks, k);
        let word = is_word(w);
        if word && prev_word {
            out.push('_');
        }
        out.push_str(w);
        prev_word = word;
    }
    out
}

/// What the allowlist calls an `impl` block, precisely: `impl@Wrap<T>` for an inherent impl,
/// `impl@Default/for/Wrap<T>` for a trait impl (the self type with its generic arguments, and the
/// trait with its own; the `impl<..>` parameters and the `where` clause are left out, so the key
/// has no spaces). `to` is the end of the header, before any `where`.
fn impl_key(toks: &[Tok], from: usize, to: usize) -> String {
    let from = if s(toks, from) == "<" {
        angles_end(toks, from)
    } else {
        from
    };
    let mut angle = 0usize;
    let mut for_at = None;
    for k in from..to {
        match s(toks, k) {
            "<" => angle += 1,
            ">" if s(toks, k - 1) != "-" => angle = angle.saturating_sub(1),
            "for" if angle == 0 => for_at = Some(k),
            _ => {}
        }
    }
    match for_at {
        Some(f) => format!(
            "impl@{}/for/{}",
            squash(toks, from, f),
            squash(toks, f + 1, to)
        ),
        None => format!("impl@{}", squash(toks, from, to)),
    }
}

fn note(why: &mut Vec<String>, bad: Vec<String>, what: &str) {
    for b in bad {
        why.push(format!(
            "{what} bound `{b}` is not `Sized` or a sealed trait (a shell type can implement its methods)"
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
    reach_rules(&toks, ctx, &mut out);
    out
}

/// The `pub trait`s of `src` that are sealed, the traits that seal them, and the modules that hold
/// those: a supertrait reached through a private module of the crate (`pub trait T:
/// sealed::Sealed`, or `Sealed` imported through one).
pub fn sealed_traits(
    src: &str,
    private: &BTreeSet<String>,
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let (mut sealed, mut sealing, mut mods) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for t in pub_traits(&toks, private) {
        if t.sealed {
            sealed.insert(t.name);
            sealing.extend(t.sealing);
            mods.extend(t.modules);
        }
    }
    (sealed, sealing, mods)
}

struct PubTrait {
    name: String,
    line: usize,
    sealed: bool,
    /// The supertraits that seal it (`Sealed` of `sealed::Sealed`).
    sealing: Vec<String>,
    /// The modules that hold them (`sealed`).
    modules: Vec<String>,
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
        let (mut sealing, mut modules) = (Vec::new(), Vec::new());
        if s(toks, m) == ":" {
            let (end, _) = header_end(toks, m);
            for (a, b) in split_top(toks, m + 1, end, "+") {
                let bare = bound_name(toks, a, b);
                let via_path = seal_module(toks, a, b, private);
                let via_use = bare.as_ref().and_then(|n| imported.get(n));
                let module = via_path.or_else(|| {
                    // a bare imported name: the whole bound is that one word (maybe in parens)
                    (b <= a + 3).then(|| via_use.cloned()).flatten()
                });
                if let (Some(module), Some(n)) = (module, bare) {
                    sealing.push(n);
                    modules.push(module);
                }
            }
        }
        out.push(PubTrait {
            name,
            line: t.line,
            sealed: !sealing.is_empty(),
            sealing,
            modules,
        });
    }
    out
}

fn seal_rules(toks: &[Tok], ctx: &Ctx<'_>, out: &mut Vec<CallbackSig>) {
    // a `pub use` hands out what it names: a sealing trait (by its own name; `as` is banned), and
    // any glob, which cannot be told from one
    for (line, public, leaves) in use_statements(toks) {
        if !public {
            continue;
        }
        for leaf in &leaves {
            let why = if leaf.name == "*" {
                Some(
                    "a `pub use` glob re-exports whatever the module holds, a seal included"
                        .to_string(),
                )
            } else if ctx.sealing.contains(&leaf.name)
                || leaf.path.last().is_some_and(|l| ctx.sealing.contains(l))
            {
                Some("`pub use` of a sealing trait hands the seal out".to_string())
            } else if leaf
                .path
                .last()
                .is_some_and(|l| ctx.sealing_mods.contains(l))
            {
                Some("a `pub use` of a module that holds a sealing trait publishes the seal without naming it".to_string())
            } else if leaf
                .path
                .last()
                .is_some_and(|l| ctx.private_mods.contains(l))
                // `atan::atan` names the item inside the module, not the module
                && !leaf
                    .path
                    .len()
                    .checked_sub(2)
                    .and_then(|n| leaf.path.get(n))
                    .is_some_and(|p| {
                        ctx.private_mods.contains(p) && !matches!(p.as_str(), "self" | "super" | "crate")
                    })
            {
                Some("a `pub use` of a private module publishes whatever seal it holds without naming it".to_string())
            } else {
                None
            };
            if let Some(why) = why {
                out.push(CallbackSig {
                    name: leaf.name.clone(),
                    line,
                    why,
                });
            }
        }
    }
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
            let (end, wh) = header_end(toks, j);
            let end = wh.unwrap_or(end);
            let for_at = (j..end).find(|&k| s(toks, k) == "for" && s(toks, k + 1) != "<");
            if let Some(f) = for_at {
                // a trait that cannot be read might be a seal: unknown is flagged
                let trait_via = match bound_kind(toks, j, f) {
                    Bound::Name(n) => ctx.sealing.contains(&n),
                    Bound::Unknown => true,
                    Bound::Skip => false,
                };
                // any type parameter anywhere in the self type: `X`, `&X`, `Box<X>`, `[X]`, `(X, u8)`
                let generic_self = (f + 1..end).any(|k| params.contains(s(toks, k)));
                if trait_via && generic_self {
                    out.push(CallbackSig {
                        name: impl_key(toks, i + 1, end),
                        line: t.line,
                        why: "a blanket impl of a sealing trait: any shell type that fits the generic self type is sealed in".to_string(),
                    });
                }
            }
        }
    }
}

/// Names of the type aliases of the sources (one crate's files, private or not) whose body holds
/// an `Fn*`, `dyn` or `fn(..)`, directly or through another alias of the crate (`type B =
/// Vec<A>;` with `A` one).
pub fn callback_aliases<'a>(srcs: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    // (name, the words of the body, whether the body holds a callback itself)
    let mut aliases: Vec<(String, Vec<String>, bool)> = Vec::new();
    for src in srcs {
        let toks = strip_test_items(tokenize(&mask(src))).toks;
        for (i, t) in toks.iter().enumerate() {
            if t.s == "type" && is_word(s(&toks, i + 1)) {
                let (end, _) = header_end(&toks, i + 2);
                let direct =
                    (i + 2..end).any(|k| callback_at(&toks, k) || s(&toks, k) == "Arguments");
                let words = (i + 2..end).map(|k| s(&toks, k).to_string()).collect();
                aliases.push((s(&toks, i + 1).to_string(), words, direct));
            }
        }
    }
    let mut out: BTreeSet<String> = aliases
        .iter()
        .filter(|a| a.2)
        .map(|a| a.0.clone())
        .collect();
    loop {
        let before = out.len();
        for (name, words, _) in &aliases {
            if words.iter().any(|w| out.contains(w)) {
                out.insert(name.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Whether `toks[k]` is `Fn`, `FnMut`, `FnOnce`, `dyn` or the `fn` of a `fn(..)` pointer type.
fn callback_at(toks: &[Tok], k: usize) -> bool {
    CALLBACK_WORDS.contains(&s(toks, k))
        || (s(toks, k) == "fn" && skip_turbofish(toks, k + 1) == k + 1 && s(toks, k + 1) == "(")
}

/// Reasons the tokens `toks[from..to]` hand behaviour in: a callback word, or a private callback
/// alias named.
fn callback_whys(toks: &[Tok], from: usize, to: usize, ctx: &Ctx<'_>, why: &mut Vec<String>) {
    for k in from..to {
        if callback_at(toks, k) {
            let w = s(toks, k);
            why.push(format!(
                "`{}` in a signature the shell can reach: a callback or function pointer can carry platform behaviour",
                if w == "fn" { "fn(..)" } else { w }
            ));
        } else if s(toks, k) == "Arguments" && (k == 0 || s(toks, k - 1) != ".") {
            why.push(
                "`fmt::Arguments` in a signature the shell can reach carries a shell `Display` impl"
                    .to_string(),
            );
        } else if ctx.cb_types.contains(s(toks, k)) && (k == 0 || s(toks, k - 1) != ".") {
            why.push(format!(
                "the type alias `{}` holds an `Fn`, `dyn` or `fn(..)` type",
                s(toks, k)
            ));
        }
    }
}

/// Bound reasons of the generics `<..>` at `toks[i]` (if any); returns the index after them.
fn generics_whys(toks: &[Tok], i: usize, ctx: &Ctx<'_>, why: &mut Vec<String>) -> usize {
    if s(toks, i) != "<" {
        return i;
    }
    let end = angles_end(toks, i);
    for (a, b) in split_top(toks, i + 1, end - 1, ",") {
        if s(toks, a) == "const" || !is_word(s(toks, a)) || s(toks, a + 1) != ":" {
            continue;
        }
        // bounds up to a default `= Type`
        let stop = (a + 2..b).find(|&k| s(toks, k) == "=").unwrap_or(b);
        note(why, bad_bounds(toks, a + 2, stop, ctx), "a generic");
    }
    end
}

/// Reasons for the `where` clause of the header ending at `stop`.
fn where_whys(toks: &[Tok], w: Option<usize>, stop: usize, ctx: &Ctx<'_>, why: &mut Vec<String>) {
    if let Some(w) = w {
        for (a, b) in split_top(toks, w + 1, stop, ",") {
            if let Some(colon) = top_colon(toks, a, b) {
                note(why, bad_bounds(toks, colon + 1, b, ctx), "a where");
            }
        }
    }
}

/// Reasons a `fn` whose keyword is at `toks[j]` lets the shell hand in behaviour.
fn fn_whys(toks: &[Tok], j: usize, ctx: &Ctx<'_>) -> Vec<String> {
    let mut why = Vec::new();
    let mut k = generics_whys(toks, j + 2, ctx, &mut why);
    if s(toks, k) == "(" {
        let end = skip_group(toks, k);
        for (a, b) in split_top(toks, k + 1, end - 1, ",") {
            for m in a..b {
                if s(toks, m) == "impl" {
                    note(
                        &mut why,
                        bad_bounds(toks, m + 1, b, ctx),
                        "an `impl` argument",
                    );
                }
            }
        }
        k = end;
    }
    let (stop, w) = header_end(toks, k);
    where_whys(toks, w, stop, ctx, &mut why);
    callback_whys(toks, j + 2, stop, ctx, &mut why);
    why
}

/// The plain-`pub` fields of the record or tuple body at `toks[from]` (`{..}` or `(..)`).
fn pub_field_ranges(toks: &[Tok], from: usize) -> Vec<(usize, usize)> {
    let end = skip_group(toks, from);
    split_top(toks, from + 1, end - 1, ",")
        .into_iter()
        .filter(|&(a, _)| {
            let mut a = a;
            while s(toks, a) == "#" && s(toks, a + 1) == "[" {
                a = skip_group(toks, a + 1);
            }
            s(toks, a) == "pub" && s(toks, a + 1) != "("
        })
        .collect()
}

/// B5 for the items the shell can reach (see the module doc). Reachability is lexical: an item
/// with plain `pub`, a method of a `pub trait`, a `pub` field; the module path is not followed.
fn reach_rules(toks: &[Tok], ctx: &Ctx<'_>, out: &mut Vec<CallbackSig>) {
    // inherent `impl` blocks whose header carries a bound or a callback: (body span, reasons)
    let mut impls: Vec<(usize, usize, Vec<String>)> = Vec::new();
    // impls of a trait whose header has a problem: (key, line, reasons)
    let mut headers: Vec<(String, usize, Vec<String>)> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        if t.s != "impl" || !matches!(prev, "" | ";" | "}" | "{" | "]" | "unsafe") {
            continue;
        }
        let mut why = Vec::new();
        let k = generics_whys(toks, i + 1, ctx, &mut why);
        let (stop, w) = header_end(toks, k);
        if s(toks, stop) != "{" {
            continue;
        }
        where_whys(toks, w, stop, ctx, &mut why);
        callback_whys(toks, i + 1, stop, ctx, &mut why);
        // the body of an impl is core's code, its header is a boundary: whatever the shell can
        // put in a generic of the header (`impl<F: Fn()> Hook for &F`) is called by the core
        let is_trait_impl = (k..stop).any(|m| s(toks, m) == "for" && s(toks, m + 1) != "<");
        if is_trait_impl {
            if !why.is_empty() {
                headers.push((impl_key(toks, i + 1, w.unwrap_or(stop)), t.line, why));
            }
        } else if !why.is_empty() {
            impls.push((stop, skip_group(toks, stop), why));
        }
    }
    let mut push = |name: &str, line: usize, mut why: Vec<String>| {
        why.sort();
        why.dedup();
        if !why.is_empty() {
            out.push(CallbackSig {
                name: name.to_string(),
                line,
                why: why.join("; "),
            });
        }
    };
    for (key, line, why) in headers {
        push(&key, line, why);
    }
    for (i, t) in toks.iter().enumerate() {
        if t.s != "pub" || s(toks, i + 1) == "(" {
            continue;
        }
        let j = skip_modifiers(toks, i);
        let kw = s(toks, j);
        let name = s(toks, j + 1);
        if !is_word(name) {
            continue;
        }
        match kw {
            "fn" => {
                let mut why = fn_whys(toks, j, ctx);
                for (from, to, ws) in &impls {
                    if *from < j && j < *to {
                        why.extend(ws.iter().cloned());
                    }
                }
                push(name, toks.get(j).map_or(0, |t| t.line), why);
            }
            "struct" | "enum" | "union" | "type" => {
                let mut why = Vec::new();
                let k = generics_whys(toks, j + 2, ctx, &mut why);
                let (stop, w) = header_end(toks, k);
                where_whys(toks, w, stop, ctx, &mut why);
                match kw {
                    // the whole body of a `pub type` or `pub enum` is reachable
                    "type" => callback_whys(toks, j + 2, stop, ctx, &mut why),
                    "enum" => {
                        let end = if s(toks, stop) == "{" {
                            skip_group(toks, stop)
                        } else {
                            stop
                        };
                        callback_whys(toks, j + 2, end, ctx, &mut why);
                    }
                    _ => {
                        // the header: the generics and the `where` clause, not the fields
                        callback_whys(toks, j + 2, k, ctx, &mut why);
                        if let Some(w) = w {
                            callback_whys(toks, w, stop, ctx, &mut why);
                        }
                        let body = if s(toks, k) == "(" {
                            Some(k)
                        } else if s(toks, stop) == "{" {
                            Some(stop)
                        } else {
                            None
                        };
                        if let Some(b) = body {
                            for (a, z) in pub_field_ranges(toks, b) {
                                callback_whys(toks, a, z, ctx, &mut why);
                            }
                        }
                    }
                }
                push(name, toks.get(j).map_or(0, |t| t.line), why);
            }
            "trait" => {
                let mut head = Vec::new();
                let k = generics_whys(toks, j + 2, ctx, &mut head);
                let (stop, w) = header_end(toks, k);
                where_whys(toks, w, stop, ctx, &mut head);
                callback_whys(toks, j + 2, stop, ctx, &mut head);
                if s(toks, stop) != "{" {
                    continue;
                }
                let end = skip_group(toks, stop);
                push(name, toks.get(j).map_or(0, |t| t.line), head.clone());
                // every method declared in the body, at the top level of the body
                let mut depth = 0usize;
                for m in stop..end {
                    match s(toks, m) {
                        "{" | "(" | "[" => depth += 1,
                        "}" | ")" | "]" => depth = depth.saturating_sub(1),
                        "fn" if depth == 1 && is_word(s(toks, m + 1)) => {
                            let why = fn_whys(toks, m, ctx);
                            push(s(toks, m + 1), toks.get(m).map_or(0, |t| t.line), why);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
