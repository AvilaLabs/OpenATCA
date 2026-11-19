//! Which expressions are floats, as far as a token reader can tell (RULESET B8). The class check
//! reads tokens and resolves no names, so every judgement here is evidence: a float literal, an
//! `as f64` cast, a name declared `f32`/`f64` in the same `fn`, a struct field, tuple-struct field
//! or `fn` return type that some core crate declares as a float, a float method. Whatever this
//! cannot prove stays in the "Known lexical limits" of the RULESET.

use super::harden::open_of;
use super::{
    is_word, mask, s, skip_group, strip_test_items, tokenize, Tok, FLOAT_METHODS, INT_TYPES,
};
use std::collections::{BTreeMap, BTreeSet};

/// What the core crates of the whole workspace declare as floats (RULESET B8): a name that is a
/// float field in one crate is one in every crate.
#[derive(Debug, Default, Clone)]
// A7.2: cold struct, size guard waived
pub struct FloatFacts {
    /// Named fields of structs declared `name: f64` (arrays, slices, `Vec` and `Option` of floats
    /// too).
    pub fields: BTreeSet<String>,
    /// Tuple structs: name -> indexes of the fields that are floats (`struct P(f64, u8)` -> {0}).
    pub tuples: BTreeMap<String, BTreeSet<usize>>,
    /// What each function name returns, over every function of that name (see [`Agg`]).
    pub rets: BTreeMap<String, Vec<Agg>>,
    /// Type aliases whose body mentions a float: `type V3 = [f64; 3];`, `type P = (f64, f64);`,
    /// `type Real = f64;`, `type W = V3;` (resolved by name, through other aliases).
    pub aliases: BTreeMap<String, Agg>,
    /// Named struct fields declared with an alias-looking type: field -> the type names.
    pub field_types: BTreeMap<String, BTreeSet<String>>,
    /// `const` and `static` names declared `f64` or `f32`.
    pub consts: BTreeSet<String>,
}

impl FloatFacts {
    /// Adds what the non-test code of one source file declares.
    pub fn add_source(&mut self, src: &str) {
        let toks = strip_test_items(tokenize(&mask(src))).toks;
        for (i, t) in toks.iter().enumerate() {
            match t.s.as_str() {
                "struct" | "union" if is_word(s(&toks, i + 1)) => self.add_struct(&toks, i),
                "fn" if is_word(s(&toks, i + 1)) => self.add_fn(&toks, i),
                "type" if is_word(s(&toks, i + 1)) => self.add_alias(&toks, i),
                "const" | "static" if is_word(s(&toks, i + 1)) && s(&toks, i + 2) == ":" => {
                    if declared_float(&toks, i + 2) == Some(Shape::Scalar) {
                        self.consts.insert(s(&toks, i + 1).to_string());
                    }
                }
                _ => {}
            }
        }
    }

    fn add_struct(&mut self, toks: &[Tok], i: usize) {
        let name = s(toks, i + 1).to_string();
        let mut open = i + 2;
        if s(toks, open) == "<" {
            open = skip_angles(toks, open);
        }
        match s(toks, open) {
            "{" => {
                for k in open..skip_group(toks, open) {
                    if declared_float(toks, k).is_some() && is_word(s(toks, k - 1)) {
                        self.fields.insert(s(toks, k - 1).to_string());
                    } else if s(toks, k) == ":" && is_word(s(toks, k - 1)) {
                        let end = (k + 1..toks.len())
                            .find(|&m| matches!(s(toks, m), "," | "}"))
                            .unwrap_or(toks.len());
                        if let Agg::Alias(n) = parse_type(toks, k + 1, end) {
                            self.field_types
                                .entry(s(toks, k - 1).to_string())
                                .or_default()
                                .insert(n);
                        }
                    }
                }
            }
            "(" => {
                let end = skip_group(toks, open);
                let idx = tuple_float_indexes(toks, open + 1, end.saturating_sub(1));
                if !idx.is_empty() {
                    self.tuples.entry(name).or_default().extend(idx);
                }
            }
            _ => {}
        }
    }

    fn add_fn(&mut self, toks: &[Tok], i: usize) {
        let mut k = i + 2;
        if s(toks, k) == "<" {
            k = skip_angles(toks, k);
        }
        if s(toks, k) != "(" {
            return;
        }
        k = skip_group(toks, k);
        let mut ret = Agg::Other;
        if s(toks, k) == "-" && s(toks, k + 1) == ">" {
            let from = k + 2;
            let mut depth = 0usize;
            let mut to = from;
            while to < toks.len() {
                match s(toks, to) {
                    "(" | "[" | "<" => depth += 1,
                    ")" | "]" => depth = depth.saturating_sub(1),
                    ">" if s(toks, to - 1) != "-" => depth = depth.saturating_sub(1),
                    "{" | ";" | "where" if depth == 0 => break,
                    _ => {}
                }
                to += 1;
            }
            ret = parse_type(toks, from, to);
        }
        self.rets
            .entry(s(toks, i + 1).to_string())
            .or_default()
            .push(ret);
    }

    /// `type Name<..> = body;`
    fn add_alias(&mut self, toks: &[Tok], i: usize) {
        let mut k = i + 2;
        if s(toks, k) == "<" {
            k = skip_angles(toks, k);
        }
        if s(toks, k) != "=" {
            return;
        }
        let from = k + 1;
        let to = (from..toks.len())
            .find(|&m| s(toks, m) == ";")
            .unwrap_or(toks.len());
        let shape = parse_type(toks, from, to);
        if shape != Agg::Other {
            self.aliases.insert(s(toks, i + 1).to_string(), shape);
        }
    }

    /// What the alias, or the type word, resolves to: a float, an aggregate that holds floats, or
    /// nothing (`depth` stops an alias that names itself).
    fn resolve(&self, a: &Agg, depth: usize) -> Option<Agg> {
        match a {
            Agg::Other => None,
            Agg::Alias(n) if depth < 8 => {
                self.aliases.get(n).and_then(|b| self.resolve(b, depth + 1))
            }
            Agg::Alias(_) => None,
            _ => Some(a.clone()),
        }
    }

    /// The float aggregate a type name stands for (a bare float alias is a `Scalar`).
    pub(super) fn alias_shape(&self, name: &str) -> Option<Agg> {
        self.resolve(&Agg::Alias(name.to_string()), 0)
    }

    /// Whether every function of this name that the core crates declare returns a float.
    pub fn float_fn(&self, name: &str) -> bool {
        self.rets.get(name).is_some_and(|v| {
            !v.is_empty() && v.iter().all(|a| self.resolve(a, 0) == Some(Agg::Scalar))
        })
    }

    /// What every function of this name returns when it is a float aggregate: an array, `Vec` or
    /// `Option` of floats, or a tuple (the indexes that are floats in all of them).
    pub(super) fn agg_ret(&self, name: &str) -> Option<Agg> {
        let rets: Vec<Agg> = self
            .rets
            .get(name)?
            .iter()
            .map(|a| self.resolve(a, 0))
            .collect::<Option<Vec<_>>>()?;
        let first = rets.first()?.clone();
        match first {
            Agg::Container if rets.iter().all(|r| *r == Agg::Container) => Some(Agg::Container),
            Agg::Tuple(mut idx) => {
                for r in &rets {
                    match r {
                        Agg::Tuple(o) => idx.retain(|x| o.contains(x)),
                        _ => return None,
                    }
                }
                (!idx.is_empty()).then_some(Agg::Tuple(idx))
            }
            _ => None,
        }
    }

    /// Whether a field of this name is a float: declared so, or with an alias type that is.
    pub(super) fn field_float(&self, name: &str) -> bool {
        self.fields.contains(name)
            || self.field_types.get(name).is_some_and(|ts| {
                ts.iter()
                    .any(|t| matches!(self.alias_shape(t), Some(Agg::Scalar | Agg::Container)))
            })
    }
}

/// What a type is, as far as floats go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agg {
    /// `f32`, `f64`, `&f64`.
    Scalar,
    /// `[f64; N]`, `&[f64]`, `Vec<f64>`, `Option<f64>`: indexing gives a float.
    Container,
    /// `(f64, u8)`: the indexes that are floats.
    Tuple(BTreeSet<usize>),
    /// A path type that may be an alias (`V3`, `crate::P`): the last segment.
    Alias(String),
    /// Anything else, `()` included.
    Other,
}

/// The type in `toks[from..to]`.
fn parse_type(toks: &[Tok], from: usize, to: usize) -> Agg {
    let mut j = from;
    while j < to && matches!(s(toks, j), "&" | "mut") {
        j += 1;
    }
    let w = s(toks, j);
    if matches!(w, "f32" | "f64") && s(toks, j + 1) != "::" {
        return Agg::Scalar;
    }
    if w == "(" {
        let end = skip_group(toks, j);
        let idx = tuple_float_indexes(toks, j + 1, end.saturating_sub(1));
        return if idx.is_empty() {
            Agg::Other
        } else {
            Agg::Tuple(idx)
        };
    }
    let has_float = (j..to).any(|k| matches!(s(toks, k), "f32" | "f64") && s(toks, k + 1) != "::");
    if (w == "[" || (matches!(w, "Vec" | "Option") && s(toks, j + 1) == "<")) && has_float {
        return Agg::Container;
    }
    // a path: the last segment before any generic arguments
    let mut last = None;
    let mut m = j;
    while m < to && is_word(s(toks, m)) {
        last = Some(s(toks, m));
        if s(toks, m + 1) == "::" {
            m += 2;
        } else {
            break;
        }
    }
    match last {
        Some(n) if n.starts_with(|c: char| c.is_ascii_uppercase()) => Agg::Alias(n.to_string()),
        _ => Agg::Other,
    }
}

/// How a name was declared float.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// `f64`, `&f64`, `&mut f64`.
    Scalar,
    /// `[f64; N]`, `&[f64]`, `Vec<f64>`, `Option<f64>`.
    Container,
}

/// Index just past the `<..>` that opens at `toks[i]`.
fn skip_angles(toks: &[Tok], i: usize) -> usize {
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

/// Whether `toks[i]` is the `:` of a declaration `name: f64` (or `&f32`, `[f64; N]`, `Vec<f64>`),
/// and which shape.
fn declared_float(toks: &[Tok], i: usize) -> Option<Shape> {
    if s(toks, i) != ":" || i == 0 || !is_word(s(toks, i - 1)) {
        return None;
    }
    let mut j = i + 1;
    let mut container = false;
    while matches!(s(toks, j), "&" | "mut" | "[" | "Vec" | "<" | "Option") {
        container |= matches!(s(toks, j), "[" | "Vec" | "<" | "Option");
        j += 1;
    }
    // `x: f64::MAX` in a struct literal is a value, not a type
    if matches!(s(toks, j), "f32" | "f64") && s(toks, j + 1) != "::" {
        Some(if container {
            Shape::Container
        } else {
            Shape::Scalar
        })
    } else {
        None
    }
}

/// Top-level comma-separated chunks of `toks[from..to]` (nesting of `()[]{}<>` respected).
fn chunks(toks: &[Tok], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = from;
    for k in from..to {
        match s(toks, k) {
            "(" | "[" | "{" | "<" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            ">" if s(toks, k - 1) != "-" => depth = depth.saturating_sub(1),
            "," if depth == 0 => {
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

/// Whether the type that starts the chunk `toks[from..to]` is a plain float (`f64`, `&f64`,
/// `pub f64`).
fn chunk_is_float(toks: &[Tok], from: usize, to: usize) -> bool {
    let mut j = from;
    while j < to && matches!(s(toks, j), "pub" | "&" | "mut") {
        if s(toks, j) == "pub" && s(toks, j + 1) == "(" {
            j = skip_group(toks, j + 1) - 1;
        }
        j += 1;
    }
    j < to && matches!(s(toks, j), "f32" | "f64") && s(toks, j + 1) != "::"
}

/// Indexes of the float chunks of the tuple type or tuple-struct body `toks[from..to]`.
fn tuple_float_indexes(toks: &[Tok], from: usize, to: usize) -> BTreeSet<usize> {
    chunks(toks, from, to)
        .into_iter()
        .enumerate()
        .filter(|(_, (a, b))| chunk_is_float(toks, *a, *b))
        .map(|(n, _)| n)
        .collect()
}

/// What a name was bound to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bind {
    Scalar,
    Container,
    /// Bound to something that is not a float (an untyped `let` whose value shows no float, a
    /// `for` variable): it hides an earlier float of the same name.
    Other,
}

/// What one `fn` declares, token by token.
#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
struct FnScope {
    from: usize,
    to: usize,
    /// Token index from which the name is bound, the name, what it is bound to; the last binding
    /// before a use decides (a later `let` of the same name shadows an earlier one).
    binds: Vec<(usize, String, Bind)>,
    /// `name: (f64, u8)`, `let (a, b): ..` and `let p = (1.0, 2)`: name -> float indexes.
    tuples: BTreeMap<String, BTreeSet<usize>>,
    /// `name: Type` with a path type: name -> last segment.
    types: BTreeMap<String, String>,
}

/// The float evidence of one file's tokens: its `fn` scopes and `impl` blocks, with the
/// workspace-wide facts.
pub(super) struct Evidence<'a> {
    toks: &'a [Tok],
    facts: &'a FloatFacts,
    fns: Vec<FnScope>,
    /// Token span and self-type name of every `impl` block.
    impls: Vec<(usize, usize, String)>,
}

/// Methods that exist only on floats (an integer has no `sqrt`).
const FLOAT_ONLY: &[&str] = &[
    "recip",
    "to_degrees",
    "to_radians",
    "copysign",
    "is_nan",
    "is_finite",
    "is_infinite",
    "total_cmp",
    "next_up",
    "next_down",
];
/// Methods an integer has too: the result is a float only if the receiver is.
const AMBIGUOUS: &[&str] = &[
    "abs",
    "signum",
    "min",
    "max",
    "clamp",
    "midpoint",
    "pow",
    "rem_euclid",
    "div_euclid",
];
/// Methods that hand the receiver's type through.
const PASSTHROUGH: &[&str] = &[
    "unwrap",
    "expect",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "clone",
    "copied",
    "cloned",
    "to_owned",
    "get_or_insert",
];

fn float_only_method(m: &str) -> bool {
    FLOAT_ONLY.contains(&m) || (FLOAT_METHODS.contains(&m) && !AMBIGUOUS.contains(&m))
}

/// Whether `toks[k]` begins a float literal: `1.0`, `1.`, `1e3`, `1f64`, `1_f32`, `1.5e-3`.
pub(super) fn float_literal_at(toks: &[Tok], k: usize) -> bool {
    let t = s(toks, k);
    if !t.starts_with(|c: char| c.is_ascii_digit())
        || t.starts_with("0x")
        || t.starts_with("0b")
        || t.starts_with("0o")
        || dot_access(toks, k)
    {
        return false;
    }
    if t.ends_with("f32") || t.ends_with("f64") {
        return true;
    }
    let exp = t.find(['e', 'E']).is_some_and(|p| {
        t.get(..p)
            .is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_digit() || c == '_'))
            && t.get(p + 1..)
                .is_some_and(|r| r.chars().all(|c| c.is_ascii_digit() || c == '_'))
    });
    if exp {
        return true;
    }
    s(toks, k + 1) == "."
        && (s(toks, k + 2).starts_with(|c: char| c.is_ascii_digit())
            || !(is_word(s(toks, k + 2)) || s(toks, k + 2) == "."))
}

/// Whether the number token at `end` is the last token of a float literal.
fn float_literal_ends_at(toks: &[Tok], end: usize) -> bool {
    if float_literal_at(toks, end) {
        return true;
    }
    s(toks, end.wrapping_sub(1)) == "."
        && end >= 2
        && float_literal_at(toks, end - 2)
        && s(toks, end).starts_with(|c: char| c.is_ascii_digit())
}

const NOT_CALLEE: &[&str] = &[
    "if", "while", "match", "in", "return", "else", "as", "mut", "let", "for", "loop", "break",
    "move", "where", "impl", "dyn", "fn", "ref",
];

impl<'a> Evidence<'a> {
    pub(super) fn new(toks: &'a [Tok], facts: &'a FloatFacts) -> Evidence<'a> {
        let mut ev = Evidence {
            toks,
            facts,
            fns: Vec::new(),
            impls: Vec::new(),
        };
        ev.find_impls();
        ev.find_fns();
        ev
    }

    fn find_impls(&mut self) {
        let toks = self.toks;
        for (i, t) in toks.iter().enumerate() {
            let prev = if i == 0 { "" } else { s(toks, i - 1) };
            if t.s != "impl" || !matches!(prev, "" | ";" | "}" | "{" | "]" | "unsafe") {
                continue;
            }
            let mut j = i + 1;
            if s(toks, j) == "<" {
                j = skip_angles(toks, j);
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
            let mut head_from = from;
            let mut angle = 0usize;
            for k in from..j {
                match s(toks, k) {
                    "<" => angle += 1,
                    ">" if s(toks, k - 1) != "-" => angle = angle.saturating_sub(1),
                    "for" if angle == 0 => head_from = k + 1,
                    _ => {}
                }
            }
            let (head, _) = super::harden::head_of(toks.get(head_from..j).unwrap_or(&[]));
            let mut end = j;
            while end < toks.len() && s(toks, end) != "{" && s(toks, end) != ";" {
                end += 1;
            }
            let end = if s(toks, end) == "{" {
                skip_group(toks, end)
            } else {
                end
            };
            self.impls.push((i, end, head));
        }
    }

    fn find_fns(&mut self) {
        let toks = self.toks;
        for (i, t) in toks.iter().enumerate() {
            if t.s != "fn" || !is_word(s(toks, i + 1)) {
                continue;
            }
            let mut depth = 0usize;
            let mut k = i + 2;
            while k < toks.len() {
                match s(toks, k) {
                    "(" | "[" => depth += 1,
                    ")" | "]" => depth = depth.saturating_sub(1),
                    "{" | ";" if depth == 0 => break,
                    _ => {}
                }
                k += 1;
            }
            let end = if s(toks, k) == "{" {
                skip_group(toks, k)
            } else {
                k
            };
            let mut sc = FnScope {
                from: i,
                to: end,
                ..FnScope::default()
            };
            self.declare(&mut sc);
            self.fns.push(sc);
        }
        // untyped lets and fold accumulators, in source order: a name takes its evidence from
        // the bindings that come before it
        for n in 0..self.fns.len() {
            self.propagate(n);
        }
    }

    /// Typed names: parameters, typed `let`s, closure parameters, tuple types and patterns.
    fn declare(&self, sc: &mut FnScope) {
        let toks = self.toks;
        for m in sc.from..sc.to {
            if let Some(shape) = declared_float(toks, m) {
                let name = s(toks, m - 1).to_string();
                let bind = match shape {
                    Shape::Scalar => Bind::Scalar,
                    Shape::Container => Bind::Container,
                };
                sc.binds.push((m - 1, name, bind));
            } else if s(toks, m) == ":" && m > 0 && is_word(s(toks, m - 1)) {
                let mut j = m + 1;
                while matches!(s(toks, j), "&" | "mut") {
                    j += 1;
                }
                let name = s(toks, m - 1).to_string();
                if s(toks, j) == "(" {
                    let end = skip_group(toks, j);
                    let idx = tuple_float_indexes(toks, j + 1, end.saturating_sub(1));
                    if !idx.is_empty() {
                        sc.tuples.entry(name).or_default().extend(idx);
                    }
                } else {
                    // `a::b::Type<..>`: the last segment
                    let mut head = s(toks, j);
                    while s(toks, j + 1) == "::" && is_word(s(toks, j + 2)) {
                        j += 2;
                        head = s(toks, j);
                    }
                    if head.starts_with(|c: char| c.is_ascii_uppercase()) {
                        match self.facts.alias_shape(head) {
                            Some(Agg::Scalar) => sc.binds.push((m - 1, name.clone(), Bind::Scalar)),
                            Some(Agg::Container) => {
                                sc.binds.push((m - 1, name.clone(), Bind::Container));
                            }
                            Some(Agg::Tuple(idx)) => {
                                sc.tuples.entry(name.clone()).or_default().extend(idx);
                            }
                            _ => {}
                        }
                        sc.types.insert(name, head.to_string());
                    }
                }
            }
            // `(a, b): (f64, f64)`: a tuple pattern with a tuple type
            if s(toks, m) == ")" && s(toks, m + 1) == ":" && s(toks, m + 2) == "(" {
                if let Some(open) = open_of(toks, m) {
                    let ty_end = skip_group(toks, m + 2);
                    let types = chunks(toks, m + 3, ty_end.saturating_sub(1));
                    let names = chunks(toks, open + 1, m);
                    for (k, (a, b)) in names.iter().enumerate() {
                        if let Some((ta, tb)) = types.get(k) {
                            if chunk_is_float(toks, *ta, *tb) {
                                if let Some(n) = binding_name(toks, *a, *b) {
                                    sc.binds.push((m, n, Bind::Scalar));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// One pass, in source order, over the untyped `let`s, `for` variables and `fold` closures of
    /// function `n`.
    fn propagate(&mut self, n: usize) {
        let toks = self.toks;
        let Some((from, to)) = self.fns.get(n).map(|f| (f.from, f.to)) else {
            return;
        };
        for i in from..to {
            let mut add: Vec<(usize, String, Bind)> = Vec::new();
            let mut tuple: Option<(String, BTreeSet<usize>)> = None;
            if s(toks, i) == "let" {
                // pattern up to a top-level `=` (a `:` type means it was declared already)
                let mut j = i + 1;
                let mut depth = 0usize;
                let mut typed = false;
                while j < to {
                    match s(toks, j) {
                        "(" | "[" | "{" => depth += 1,
                        ")" | "]" | "}" => depth = depth.saturating_sub(1),
                        ":" if depth == 0 => typed = true,
                        "=" | ";" if depth == 0 => break,
                        _ => {}
                    }
                    j += 1;
                }
                if typed || s(toks, j) != "=" {
                    continue;
                }
                let init_from = j + 1;
                let mut k = init_from;
                let mut d = 0usize;
                while k < to {
                    match s(toks, k) {
                        "(" | "[" | "{" => d += 1,
                        ")" | "]" | "}" => {
                            if d == 0 {
                                break;
                            }
                            d -= 1;
                        }
                        ";" | "else" if d == 0 => break,
                        _ => {}
                    }
                    k += 1;
                }
                let pat = (i + 1, j);
                let init_is_tuple = s(toks, init_from) == "(" && skip_group(toks, init_from) == k;
                if s(toks, pat.0) == "(" && skip_group(toks, pat.0) == pat.1 {
                    // `let (a, b) = (e1, e2);`: element by element
                    if init_is_tuple {
                        let names = chunks(toks, pat.0 + 1, pat.1 - 1);
                        let exprs = chunks(toks, init_from + 1, k - 1);
                        if names.len() == exprs.len() {
                            for ((na, nb), (ea, eb)) in names.iter().zip(exprs.iter()) {
                                if let Some(name) = binding_name(toks, *na, *nb) {
                                    let bind = if self.mentions_in(n, *ea, *eb) {
                                        Bind::Scalar
                                    } else {
                                        Bind::Other
                                    };
                                    add.push((k, name, bind));
                                }
                            }
                        }
                    }
                } else if let Some(name) = binding_name(toks, pat.0, pat.1) {
                    if let Some(shape) = self.init_agg(init_from, k) {
                        // `let p = make_v3();`: an array or tuple of floats
                        match shape {
                            Agg::Tuple(idx) => {
                                tuple = Some((name.clone(), idx));
                                add.push((k, name, Bind::Other));
                            }
                            _ => add.push((k, name, Bind::Container)),
                        }
                    } else if init_is_tuple && chunks(toks, init_from + 1, k - 1).len() > 1 {
                        // `let best = (0, f64::MAX);`: a tuple, `best.1` is the float
                        let idx = chunks(toks, init_from + 1, k - 1)
                            .iter()
                            .enumerate()
                            .filter(|(_, (a, b))| self.mentions_in(n, *a, *b))
                            .map(|(m, _)| m)
                            .collect();
                        tuple = Some((name.clone(), idx));
                        add.push((k, name, Bind::Other));
                    } else {
                        let bind = if self.mentions_in(n, init_from, k) {
                            Bind::Scalar
                        } else {
                            Bind::Other
                        };
                        add.push((k, name, bind));
                    }
                }
            } else if s(toks, i) == "for" && s(toks, i + 1) != "<" {
                // `for x in ..`: `x` is not a float we know of; the body starts at the `{`
                if let Some(at_in) = (i + 1..to).find(|&m| s(toks, m) == "in") {
                    if let Some(name) = binding_name(toks, i + 1, at_in) {
                        let body = (at_in..to).find(|&m| s(toks, m) == "{").unwrap_or(to);
                        add.push((body, name, Bind::Other));
                    }
                }
            } else if s(toks, i) == "fold"
                && s(toks, i + 1) == "("
                && s(toks, i.wrapping_sub(1)) == "."
            {
                // `.fold(<float init>, |acc, x| ..)`: `acc` is a float
                let end = skip_group(toks, i + 1);
                let parts = chunks(toks, i + 2, end.saturating_sub(1));
                if let (Some((ia, ib)), Some((ca, _))) = (parts.first(), parts.get(1)) {
                    if s(toks, *ca) == "|"
                        && self.mentions_in(n, *ia, *ib)
                        && is_word(s(toks, ca + 1))
                    {
                        add.push((*ca, s(toks, ca + 1).to_string(), Bind::Scalar));
                    }
                }
            }
            if let Some(f) = self.fns.get_mut(n) {
                f.binds.extend(add);
                if let Some((name, idx)) = tuple {
                    f.tuples.entry(name).or_default().extend(idx);
                }
            }
        }
    }

    fn mentions_in(&self, n: usize, lo: usize, hi: usize) -> bool {
        self.mentions_with(lo, hi, Some(n))
    }

    /// The function scopes that contain token `i`.
    fn scopes_at(&self, i: usize) -> impl Iterator<Item = &FnScope> {
        self.fns.iter().filter(move |f| f.from < i && i < f.to)
    }

    /// Whether `name` at token `i` is bound to a float (a scalar; a container too unless
    /// `scalar_only`) by the last binding before `i`.
    fn declared_name(&self, i: usize, name: &str, scalar_only: bool, only: Option<usize>) -> bool {
        let hit = |f: &FnScope| {
            f.binds
                .iter()
                .filter(|(p, n, _)| n == name && *p <= i)
                .max_by_key(|(p, _, _)| *p)
                .is_some_and(|(_, _, b)| match b {
                    Bind::Scalar => true,
                    Bind::Container => !scalar_only,
                    Bind::Other => false,
                })
        };
        match only {
            Some(n) => self.fns.get(n).is_some_and(hit),
            None => self.scopes_at(i).any(hit),
        }
    }

    /// The self type of the innermost `impl` that contains token `i`.
    fn impl_head(&self, i: usize) -> Option<&str> {
        self.impls
            .iter()
            .filter(|(a, b, _)| *a < i && i < *b)
            .max_by_key(|(a, _, _)| *a)
            .map(|(_, _, h)| h.as_str())
    }

    /// Whether `toks[k]`, a number after a `.`, indexes a float field of a tuple struct or tuple
    /// variable: `self.0` in an `impl` of `struct P(f64)`, `p.0` with `p: P`, `t.1` with
    /// `t: (u8, f64)`.
    fn tuple_field_float(&self, k: usize, only: Option<usize>) -> bool {
        let Ok(idx) = s(self.toks, k).parse::<usize>() else {
            return false;
        };
        let base = s(self.toks, k.wrapping_sub(2));
        if !is_word(base) || s(self.toks, k.wrapping_sub(1)) != "." {
            return false;
        }
        let in_struct = |ty: &str| self.facts.tuples.get(ty).is_some_and(|t| t.contains(&idx));
        if base == "self" {
            return self.impl_head(k).is_some_and(in_struct);
        }
        let scopes: Vec<&FnScope> = match only {
            Some(n) => self.fns.get(n).into_iter().collect(),
            None => self.scopes_at(k).collect(),
        };
        scopes.iter().any(|f| {
            f.tuples.get(base).is_some_and(|t| t.contains(&idx))
                || f.types.get(base).is_some_and(|ty| in_struct(ty))
        })
    }

    /// The float aggregate the call whose `)` is at `toks[close]` returns, by the name it calls.
    fn call_agg(&self, close: usize) -> Option<Agg> {
        let open = open_of(self.toks, close)?;
        let name = s(self.toks, open.checked_sub(1)?);
        (is_word(name) && !NOT_CALLEE.contains(&name))
            .then(|| self.facts.agg_ret(name))
            .flatten()
    }

    /// Whether `toks[k]`, a number after `call(..).`, is a float field of a tuple a function
    /// returns: `foo().0`.
    fn call_tuple_field(&self, k: usize) -> bool {
        let Ok(idx) = s(self.toks, k).parse::<usize>() else {
            return false;
        };
        k >= 2
            && s(self.toks, k - 1) == "."
            && s(self.toks, k - 2) == ")"
            && matches!(self.call_agg(k - 2), Some(Agg::Tuple(t)) if t.contains(&idx))
    }

    /// Whether the initializer `toks[from..to]` is exactly one call of a function that returns a
    /// float aggregate (`let p = make_v3();`, `let p = geom::make();`).
    fn init_agg(&self, from: usize, to: usize) -> Option<Agg> {
        let mut j = from;
        while is_word(s(self.toks, j)) && s(self.toks, j + 1) == "::" {
            j += 2;
        }
        if is_word(s(self.toks, j))
            && s(self.toks, j + 1) == "("
            && skip_group(self.toks, j + 1) == to
        {
            self.facts.agg_ret(s(self.toks, j))
        } else {
            None
        }
    }

    /// Whether the outermost expression of `toks[from..to]` is an integer by its own shape,
    /// whatever floats it mentions inside: `.count()`, `.len()`, `usize::try_from(..)` (with a
    /// trailing `?`, `.unwrap()`, `.expect(..)`, `.unwrap_or(..)`).
    fn integer_shape(&self, from: usize, to: usize) -> bool {
        let toks = self.toks;
        if to >= from + 4
            && s(toks, to - 1) == ")"
            && s(toks, to - 2) == "("
            && matches!(s(toks, to - 3), "count" | "len")
            && s(toks, to - 4) == "."
        {
            return true;
        }
        if INT_TYPES.contains(&s(toks, from))
            && s(toks, from + 1) == "::"
            && matches!(s(toks, from + 2), "try_from" | "from")
            && s(toks, from + 3) == "("
        {
            let mut j = skip_group(toks, from + 3);
            while j < to {
                if s(toks, j) == "?" {
                    j += 1;
                } else if s(toks, j) == "."
                    && matches!(
                        s(toks, j + 1),
                        "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default"
                    )
                    && s(toks, j + 2) == "("
                {
                    j = skip_group(toks, j + 2);
                } else {
                    return false;
                }
            }
            return true;
        }
        false
    }

    /// Whether the tokens `toks[lo..hi]` mention a float: a literal, an `as f64` cast, `f64::`,
    /// a float-typed name, a float field, a float method or function. An expression that ends in
    /// an integer cast is not a float whatever it mentions.
    pub(super) fn mentions(&self, lo: usize, hi: usize) -> bool {
        self.mentions_with(lo, hi, None)
    }

    fn mentions_with(&self, lo: usize, hi: usize, only: Option<usize>) -> bool {
        let toks = self.toks;
        if self.integer_shape(lo, hi) {
            return false;
        }
        // `if c { a } else { b }` and `match x { .. }` have the type of their branches, not of the
        // condition: read from the first `{`, and any cast inside counts
        let mut lo = lo;
        let mut block = false;
        if matches!(s(toks, lo), "if" | "match") {
            block = true;
            while lo < hi && s(toks, lo) != "{" {
                lo = skip_one(toks, lo);
            }
        }
        // the last top-level `as T` decides the type of the whole expression
        let mut depth = 0usize;
        let mut last_cast: Option<&str> = None;
        for k in lo..hi {
            match s(toks, k) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                "as" if depth == 0 || block => last_cast = Some(s(toks, k + 1)),
                _ => {}
            }
        }
        if let Some(t) = last_cast {
            if INT_TYPES.contains(&t) || matches!(t, "bool" | "char") {
                return false;
            }
        }
        for k in lo..hi {
            let w = s(toks, k);
            let prev = if k == 0 { "" } else { s(toks, k - 1) };
            let next = s(toks, k + 1);
            if float_literal_at(toks, k) {
                return true;
            }
            if w == "as" && matches!(next, "f32" | "f64") {
                return true;
            }
            if matches!(w, "f32" | "f64")
                && (next == "::"
                    || (prev == "<"
                        && next == ">"
                        && (s(toks, k.wrapping_sub(2)) == "::" || s(toks, k + 2) == "::")))
            {
                return true;
            }
            if w.starts_with(|c: char| c.is_ascii_digit())
                && (self.tuple_field_float(k, only) || self.call_tuple_field(k))
            {
                return true;
            }
            if !is_word(w) || w.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let call = super::call_at(toks, k + 1);
            if dot_access(toks, k) {
                if call {
                    if float_only_method(w) || (self.facts.float_fn(w) && !AMBIGUOUS.contains(&w)) {
                        return true;
                    }
                } else if self.facts.field_float(w) {
                    return true;
                }
            } else if prev == "::" {
                if (call && self.facts.float_fn(w) && module_qualifier(s(toks, k.wrapping_sub(2))))
                    || (!call && self.facts.consts.contains(w))
                {
                    return true;
                }
            } else if call {
                let close = skip_group(toks, k + 1);
                if self.facts.float_fn(w)
                    || (s(toks, close) == "[" && self.facts.agg_ret(w) == Some(Agg::Container))
                {
                    return true;
                }
            } else if next == "."
                && s(toks, k + 2).starts_with(|c: char| c.is_ascii_digit())
                && !float_literal_at(toks, k + 2)
            {
                // `best.0`: a field of a tuple, judged by `tuple_field_float`
            } else if self.declared_name(k, w, next != "[", only) || self.facts.consts.contains(w) {
                return true;
            }
        }
        false
    }

    /// Whether the receiver expression that ends at `toks[end]` (the token before a `.` or
    /// `::`) is a float, by shape: the last step of the chain decides.
    pub(super) fn recv_is_float(&self, end: usize) -> bool {
        self.recv(end, 0)
    }

    fn recv(&self, end: usize, depth: usize) -> bool {
        let toks = self.toks;
        if depth > 8 {
            return false;
        }
        let t = s(toks, end);
        match t {
            ")" => {
                let Some(open) = open_of(toks, end) else {
                    return false;
                };
                let name = s(toks, open.wrapping_sub(1));
                if open >= 1 && is_word(name) && !NOT_CALLEE.contains(&name) {
                    let before = s(toks, open.wrapping_sub(2));
                    if before == "." {
                        if float_only_method(name) {
                            return true;
                        }
                        if AMBIGUOUS.contains(&name) {
                            return open >= 3 && self.recv(open - 3, depth + 1);
                        }
                        if PASSTHROUGH.contains(&name) {
                            return (open >= 3 && self.recv(open - 3, depth + 1))
                                || self.turbofish_float(open - 1);
                        }
                        self.facts.float_fn(name)
                    } else if before == "::" {
                        let qual = s(toks, open.wrapping_sub(3));
                        matches!(qual, "f32" | "f64")
                            || (self.facts.float_fn(name) && module_qualifier(qual))
                    } else {
                        self.facts.float_fn(name)
                    }
                } else if open >= 1 && s(toks, open - 1) == ">" {
                    // `x.sum::<f64>()`
                    open >= 4
                        && matches!(s(toks, open - 2), "f32" | "f64")
                        && s(toks, open - 3) == "<"
                        && s(toks, open - 4) == "::"
                } else if open >= 1 && matches!(s(toks, open - 1), "]" | ")") {
                    // `a[i](..)`, `f(x)(y)`: a call result of a call: unknown
                    false
                } else {
                    self.mentions(open + 1, end)
                }
            }
            "]" => match open_of(toks, end) {
                Some(open) if open >= 1 && s(toks, open - 1) == ")" => {
                    self.call_agg(open - 1) == Some(Agg::Container)
                        || self.recv(open - 1, depth + 1)
                }
                Some(open) if open >= 1 => self.recv(open - 1, depth + 1),
                _ => false,
            },
            _ if t.starts_with(|c: char| c.is_ascii_digit()) => {
                float_literal_ends_at(toks, end)
                    || self.tuple_field_float(end, None)
                    || self.call_tuple_field(end)
            }
            _ if is_word(t) => {
                let prev = s(toks, end.wrapping_sub(1));
                if dot_access(toks, end) {
                    self.facts.field_float(t)
                } else if prev == "::" {
                    matches!(s(toks, end.wrapping_sub(2)), "f32" | "f64")
                        || (s(toks, end.wrapping_sub(2)) == ">"
                            && matches!(s(toks, end.wrapping_sub(3)), "f32" | "f64"))
                        || self.facts.consts.contains(t)
                } else {
                    self.declared_name(end, t, false, None) || self.facts.consts.contains(t)
                }
            }
            _ => false,
        }
    }

    /// Whether the turbofish of the method name at `toks[name]` is `::<f64>`.
    fn turbofish_float(&self, name: usize) -> bool {
        let toks = self.toks;
        s(toks, name + 1) == "::"
            && s(toks, name + 2) == "<"
            && matches!(s(toks, name + 3), "f32" | "f64")
    }
}

/// Whether `toks[k]` follows a field or method `.` (not the second dot of a range `a..b`).
fn dot_access(toks: &[Tok], k: usize) -> bool {
    k >= 1 && s(toks, k - 1) == "." && !(k >= 2 && s(toks, k - 2) == ".")
}

/// Index of the token after the one at `i`, or after the whole group when it opens one.
fn skip_one(toks: &[Tok], i: usize) -> usize {
    if matches!(s(toks, i), "(" | "[") {
        skip_group(toks, i)
    } else {
        i + 1
    }
}

/// Whether the qualifier of a path call is a module (`oa_math::x`), not a type: a lowercase
/// name that is not a primitive type.
fn module_qualifier(q: &str) -> bool {
    q.starts_with(|c: char| c.is_ascii_lowercase())
        && !INT_TYPES.contains(&q)
        && !matches!(q, "bool" | "char" | "str" | "f32" | "f64")
}

/// The name a `let` or tuple pattern chunk binds: `x`, `mut x`, `&x` (anything else is none).
fn binding_name(toks: &[Tok], from: usize, to: usize) -> Option<String> {
    let mut j = from;
    while j < to && matches!(s(toks, j), "mut" | "&" | "ref") {
        j += 1;
    }
    (j + 1 == to && is_word(s(toks, j)) && s(toks, j) != "_").then(|| s(toks, j).to_string())
}
