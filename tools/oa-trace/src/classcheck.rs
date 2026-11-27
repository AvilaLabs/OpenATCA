//! Core/shell class check (OA-TRC-200 to OA-TRC-2xx; RULESET B7 to B9).
//!
//! Every workspace member declares `[package.metadata.openatca] class` in its Cargo.toml: `core`,
//! `shell`, `split` or `unsplit`. The check reads the class of every member, scans core code
//! (all of `src/` of a core crate, `src/core/` and `src/core.rs` of a split crate; never
//! binaries or test code) against the B8 allowlist, checks that core code depends only on core
//! code, checks the shape of a split crate, and holds the unsplit list to [`UNSPLIT_ALLOWED`].
//! Cargo.toml is read with a small parser of its own (F1: no TOML dependency); source is read by
//! a lexer that blanks comments, string literals and character literals before looking at it.
//! Anything the check cannot read, parse or resolve is a failure, never a silent skip.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const FFI_PACKAGE: &str = "openatca-core-ffi";
const FFI_PACKAGE_DIR: &str = "crates/openatca-core-ffi";
const FFI_EXPORTS: &[&str] = &[
    "oa_asterix_decode_block",
    "oa_notam_fns_decode",
    "oa_geo_wgs84_inverse",
    "oa_geo_wgs84_direct",
    "oa_core_error_message",
    "oa_core_abi_version",
    "oa_math_f64_unary",
    "oa_math_f64_binary",
    "oa_math_f64_ternary",
    "oa_math_f64_powi",
    "oa_math_f64_sin_cos",
    "oa_math_f64_compare",
    "oa_math_f64_is_sign_negative",
    "oa_math_f32_unary",
    "oa_math_f32_binary",
    "oa_math_f32_ternary",
    "oa_math_f32_compare",
    "oa_math_f32_is_sign_negative",
    "oa_sha256_digest",
    "oa_sha256_hex",
    "oa_geo_wgs84_rhumb_inverse",
    "oa_geo_wgs84_rhumb_direct",
    "oa_geo_wgs84_rhumb_point_at_fraction",
    "oa_geo_wgs84_geodesic_rhumb_separation",
    "oa_geo_polygon_new_wgs84",
    "oa_geo_polygon_contains_wgs84",
    "oa_geo_polygon_edge_count",
    "oa_geo_polygon_free",
    "oa_json_parse",
    "oa_xml_parse",
    "oa_xml_decode_span",
    "oa_csv_parse",
    "oa_deflate_raw_inflate",
    "oa_deflate_raw_compress",
    "oa_tar_parse",
    "oa_zip_list",
    "oa_zip_extract_entry",
    "oa_grib2_inspect_message",
    "oa_grib2_decode_message",
    "oa_cifp_parse",
];
const FFI_DEPENDENCIES: &[(&str, &str)] = &[
    ("oa-geo", "../oa-geo"),
    ("oa-ieee", "../oa-ieee"),
    ("oa-math", "../oa-math"),
    ("oa-sha256", "../oa-sha256"),
    ("oa-csv", "../oa-csv"),
    ("oa-json", "../oa-json"),
    ("oa-xml", "../oa-xml"),
    ("oa-zip", "../oa-zip"),
    ("oa-deflate", "../oa-deflate"),
    ("oa-tar", "../oa-tar"),
    ("oa-asterix", "../oa-asterix"),
    ("oa-grib2", "../oa-grib2"),
    ("oa-notam", "../oa-notam"),
    ("oa-time-window", "../oa-time-window"),
    ("oa-arinc424", "../oa-arinc424"),
];

/// Reviewed unsafe blocks in the provisional FFI module, grouped by their owning function.
/// This intentionally pins both the function boundary and the number of unsafe blocks inside it;
/// every block also needs its local SAFETY rationale below. Change this list only with a reviewed
/// boundary change and matching negative-fixture coverage.
pub const FFI_UNSAFE_BLOCKS: &[(&str, usize)] = &[
    ("write_error", 1),
    ("oa_geo_wgs84_inverse", 1),
    ("oa_geo_wgs84_direct", 1),
    ("oa_math_f64_unary", 1),
    ("oa_math_f64_binary", 1),
    ("oa_math_f64_ternary", 1),
    ("oa_math_f64_powi", 1),
    ("oa_math_f64_sin_cos", 1),
    ("oa_math_f64_compare", 1),
    ("oa_math_f64_is_sign_negative", 1),
    ("oa_math_f32_unary", 1),
    ("oa_math_f32_binary", 1),
    ("oa_math_f32_ternary", 1),
    ("oa_math_f32_compare", 1),
    ("oa_math_f32_is_sign_negative", 1),
    ("oa_sha256_digest", 2),
    ("oa_sha256_hex", 2),
    ("oa_geo_wgs84_rhumb_inverse", 1),
    ("oa_geo_wgs84_rhumb_direct", 1),
    ("oa_geo_wgs84_rhumb_point_at_fraction", 1),
    ("oa_geo_wgs84_geodesic_rhumb_separation", 1),
    ("oa_geo_polygon_new_wgs84", 3),
    ("oa_geo_polygon_contains_wgs84", 2),
    ("oa_geo_polygon_edge_count", 2),
    ("oa_geo_polygon_free", 1),
    ("oa_core_error_message", 2),
    ("oa_json_parse", 5),
    ("oa_xml_parse", 5),
    ("oa_xml_decode_span", 4),
    ("oa_csv_parse", 5),
    ("oa_deflate_raw_inflate", 3),
    ("oa_deflate_raw_compress", 3),
    ("oa_tar_parse", 3),
    ("write_asterix_error", 1),
    ("write_asterix_counts", 1),
    ("write_cifp_error", 1),
    ("oa_asterix_decode_block", 2),
    ("write_notam_summary", 1),
    ("oa_notam_fns_decode", 3),
    ("oa_zip_list", 3),
    ("oa_zip_extract_entry", 3),
    ("oa_grib2_inspect_message", 2),
    ("oa_grib2_decode_message", 4),
    ("oa_cifp_parse", 27),
    ("ffi_l0_matches_the_shared_synthetic_vectors", 3),
    ("ffi_polygon_matches_the_shared_synthetic_vectors", 4),
    ("ffi_matches_the_shared_synthetic_geodesy_vectors", 1),
    ("null_output_is_refused_and_message_copy_is_bounded", 2),
    (
        "error_message_capacity_is_all_or_nothing_and_includes_nul",
        4,
    ),
];

mod boundary;
mod floats;
mod harden;
pub use boundary::{callback_aliases, module_visibility, sealed_traits, FIXED_BOUNDS};
pub use floats::FloatFacts;
pub use harden::{
    count_float_calls, parse_budget, parse_callbacks, scan_shell, CallbackEntry, CallbackSig,
    CALLBACK_ALLOWLIST, FLOAT_BUDGET, HELPERS,
};

/// The committed list of crates allowed to be `unsplit`, relative to the workspace root. It must
/// be a subset of [`UNSPLIT_ALLOWED`] and only shrinks.
pub const ALLOWLIST: &str = "tools/oa-trace/unsplit-allowlist.txt";

/// The twelve crates that were unsplit on 2026-11-19 (RULESET B9). A crate may leave this list
/// by being split or reclassified; no crate may join it. Only a reviewed change to this array
/// can change it.
pub const UNSPLIT_ALLOWED: [&str; 12] = [
    "oa-airspace",
    "oa-bench",
    "oa-display",
    "oa-feed",
    "oa-nam",
    "oa-replay",
    "oa-replay-diff",
    "oa-scope-replay",
    "oa-sha256",
    "oa-track",
    "oa-truth",
    "oa-wind",
];

/// Exact runtime/compilation packages whose wasm32-unknown-unknown source and width assumptions
/// are reviewed for the F2 demo. Includes named split packages whose unused shell modules compile
/// in the target closure; it does not authorize other split packages or general 32-bit builds.
pub const WASM32_PACKAGE_ALLOWLIST: [&str; 14] = [
    "oa-adsb-model",
    "oa-airspace",
    "oa-ca",
    "oa-demo-engine",
    "oa-demo-scenario",
    "oa-display",
    "oa-flight",
    "oa-geo",
    "oa-ieee",
    "oa-math",
    "oa-safety",
    "oa-sha256",
    "oa-track-core",
    "oa-workstation",
];

fn wasm32_package_allowed(name: &str) -> bool {
    WASM32_PACKAGE_ALLOWLIST.contains(&name)
}

/// Modules of `std`, `core` and `alloc` core code may name (RULESET B8). `collections` and
/// `time` are allowed only for the names in [`ALLOWED_COLLECTIONS`] and `core::time::Duration`.
const ALLOWED_MODULES: &[&str] = &[
    "fmt", "ops", "cmp", "mem", "num", "convert", "iter", "slice", "array", "str", "char", "ascii",
    "option", "result", "marker", "default", "clone", "borrow", "hint", "error", "f32", "f64",
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "vec",
    "boxed", "string", "format",
];
const ALLOWED_COLLECTIONS: &[&str] = &["BTreeMap", "BTreeSet", "VecDeque", "BinaryHeap"];
const FORBIDDEN_MACROS: &[&str] = &[
    "thread_local",
    "print",
    "println",
    "eprint",
    "eprintln",
    "dbg",
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
    "file",
    "concat",
    "stringify",
    "format_args",
];
/// Inherent `f32`/`f64` methods absent from `core` on the pinned toolchain (1.89.0). Derived by
/// compiling a throwaway `#![no_std]` probe that called every method listed in the standard
/// library documentation for `f64` (and `f32`, which gave the same set) and keeping each that
/// rustc reported as "no method named ..." (E0599). `abs`, `signum`, `copysign`, `recip`,
/// `to_degrees`, `to_radians`, `min`, `max`, `clamp`, `midpoint`, `total_cmp` and the
/// classification methods compile in `no_std`, so they are not here; `lerp`, `gamma`, `ln_gamma`,
/// `erf` and `erfc` are unstable in `std` and also absent from `core`. The standard library's
/// versions call the platform maths library, whose last bit differs between machines.
pub const FLOAT_METHODS: &[&str] = &[
    "abs_sub",
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "cbrt",
    "ceil",
    "cos",
    "cosh",
    "div_euclid",
    "erf",
    "erfc",
    "exp",
    "exp2",
    "exp_m1",
    "floor",
    "fract",
    "gamma",
    "hypot",
    "lerp",
    "ln",
    "ln_1p",
    "ln_gamma",
    "log",
    "log10",
    "log2",
    "mul_add",
    "powf",
    "powi",
    "rem_euclid",
    "round",
    "round_ties_even",
    "sin",
    "sin_cos",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "trunc",
];
/// The one crate that wraps the standard library's exactly specified float operations (B10).
const IEEE_CRATE: &str = "oa-ieee";
/// The public API of [`IEEE_CRATE`], exactly.
const IEEE_API: [&str; 5] = ["sqrt", "floor", "ceil", "trunc", "round"];
/// Integer types: `i64::rem_euclid(a, b)` is `core`'s and is allowed in path form.
const INT_TYPES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
];

/// What to write instead of the float method `w` that `core` lacks: a real `oa_ieee` or `oa_math`
/// function, or how to build it from them.
pub fn float_method_advice(w: &str) -> String {
    let text = match w {
        _ if IEEE_API.contains(&w) => return format!("use oa_ieee::{w}"),
        "rem_euclid" => {
            "for an integer write it in path form, e.g. `i64::rem_euclid(a, b)`; for a float use oa_math::rem_euclid"
        }
        "div_euclid" => {
            "for an integer write it in path form, e.g. `i64::div_euclid(a, b)`; for a float take oa_ieee::trunc of the quotient and adjust by the sign of oa_math::rem_euclid, as std does"
        }
        "powf" => "use oa_math::pow",
        "exp2" => "write oa_math::pow(2.0, x)",
        "log" => "write oa_math::ln(x) / oa_math::ln(base)",
        "log2" | "log10" => "write oa_math::ln(x) divided by oa_math::ln of the base",
        "mul_add" => "write the multiply and the add separately (a fused multiply-add is not portable)",
        "fract" => "write x - oa_ieee::trunc(x)",
        "exp_m1" => "write oa_math::exp(x) - 1.0, or a series where x is small",
        "ln_1p" => "write oa_math::ln(1.0 + x), or a series where x is small",
        "sinh" | "cosh" | "tanh" => "write it from oa_math::exp",
        "asinh" | "acosh" | "atanh" => "write it from oa_math::ln and oa_ieee::sqrt",
        "abs_sub" => "write (a - b).max(0.0)",
        "round_ties_even" => "write it from oa_ieee::round with a check for an exact tie",
        "lerp" => "write a + t * (b - a)",
        "gamma" | "ln_gamma" | "erf" | "erfc" => {
            "oa_math has no such function yet: write one there under its determinism rule"
        }
        _ => return format!("use oa_math::{w}"),
    };
    text.to_string()
}

/// The failure text for a call of the float method `w` that `core` lacks.
fn float_message(w: &str) -> String {
    format!(
        "B8: the standard library's float `{w}` differs between machines; {}. A user-defined method of this name is disallowed in core code too, and a `Name::{w}` path must start from oa_math, oa_ieee, a core crate or a module of this crate",
        float_method_advice(w)
    )
}
const FORBIDDEN_IDENTS: &[&str] = &["HashMap", "HashSet", "RandomState"];

/// Result of the class check.
#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
pub struct ClassReport {
    /// Crate name -> declared class, for every member whose class is valid.
    pub classes: BTreeMap<String, String>,
    /// Crates declared `unsplit`, sorted (the B9 debt).
    pub unsplit: Vec<String>,
    /// Platform float-method calls per unsplit crate, and the budget recorded for it.
    pub float_counts: BTreeMap<String, (usize, Option<usize>)>,
    /// Every failure as `file:line: token: message` or `crate: message`, in a stable order.
    pub failures: Vec<String>,
}

impl ClassReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

fn class_name(c: Class) -> &'static str {
    match c {
        Class::Core => "core",
        Class::Shell => "shell",
        Class::Split => "split",
        Class::Unsplit => "unsplit",
    }
}

// ---------------------------------------------------------------------------------------------
// Cargo.toml, minimally

/// One entry of a dependency table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Dep {
    /// The name used in source (the table key).
    pub key: String,
    /// `package = "..."` (a rename), if any.
    pub package: Option<String>,
    /// `path = "..."`, if any.
    pub path: Option<String>,
    /// `workspace = true`: the entry is the root `[workspace.dependencies]` one of that key.
    pub workspace: bool,
}

/// What the check needs from one Cargo.toml.
#[derive(Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Manifest {
    pub name: Option<String>,
    /// `Some(raw value)` if `[package.metadata.openatca]` has a `class` key.
    pub class: Option<String>,
    /// Entries of every dependency table (`dependencies`, `dev-dependencies`,
    /// `build-dependencies`, under `[target.*]` too), merged by key.
    pub deps: Vec<Dep>,
    /// Entries of the root `[workspace.dependencies]`.
    pub ws_deps: Vec<Dep>,
    /// `[lib] name`: the name the crate is used by in source, if not the package name.
    pub lib_name: Option<String>,
    /// `[lib]` has a `path`.
    pub lib_path: bool,
    /// `path` of every `[[bin]]`.
    pub bin_paths: Vec<String>,
    /// `[package]` has a `build` key.
    pub build_key: bool,
    /// `[lints]` has `workspace = true`.
    pub lints_workspace: bool,
    /// Individual `[lints.rust] unsafe_code = "deny"` override, used only by the A0 C ABI shell.
    pub lints_unsafe_deny: bool,
    /// Explicitly denied `[lints.clippy]` names.
    pub clippy_denied: BTreeSet<String>,
}

/// `line` without a trailing `# comment` (a `#` inside quotes is kept).
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, ch) in line.char_indices() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '#' => return line.get(..i).unwrap_or(line),
            None => {}
        }
    }
    line
}

/// Splits `text` at `sep` outside quotes.
fn split_outside_quotes(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == sep => {
                out.push(text.get(start..i).unwrap_or(""));
                start = i + ch.len_utf8();
            }
            None => {}
        }
    }
    out.push(text.get(start..).unwrap_or(""));
    out
}

/// The content of a `"..."` or `'...'` string, or `None` if `v` is not exactly one.
fn unquote(v: &str) -> Option<String> {
    let v = v.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            if !inner.contains(q) {
                return Some(inner.to_string());
            }
        }
    }
    None
}

/// `key = "value"` inside an inline table `{ ... }`.
fn inline_str(value: &str, key: &str) -> Option<String> {
    for (i, _) in value.match_indices(key) {
        let before = value.get(..i).unwrap_or("").chars().next_back();
        if before.is_some_and(|c| !(c == '{' || c == ',' || c.is_whitespace())) {
            continue;
        }
        let rest = value.get(i + key.len()..).unwrap_or("").trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let q = rest.chars().next()?;
        if q != '"' && q != '\'' {
            continue;
        }
        let body = rest.get(1..).unwrap_or("");
        let end = body.find(q)?;
        return Some(body.get(..end).unwrap_or("").to_string());
    }
    None
}

/// `key = true` inside an inline table `{ ... }`.
fn inline_true(value: &str, key: &str) -> bool {
    value.match_indices(key).any(|(i, _)| {
        let before = value.get(..i).unwrap_or("").chars().next_back();
        let rest = value.get(i + key.len()..).unwrap_or("").trim_start();
        before.is_none_or(|c| c == '{' || c == ',' || c.is_whitespace())
            && rest
                .strip_prefix('=')
                .is_some_and(|r| r.trim_start().starts_with("true"))
    })
}

/// Runs `f` on the entry for `key`, adding it first if the table has none yet.
fn with_dep(deps: &mut Vec<Dep>, key: &str, f: impl FnOnce(&mut Dep)) {
    let i = match deps.iter().position(|d| d.key == key) {
        Some(i) => i,
        None => {
            deps.push(Dep {
                key: key.to_string(),
                ..Dep::default()
            });
            deps.len() - 1
        }
    };
    if let Some(d) = deps.get_mut(i) {
        f(d);
    }
}

/// Segments of a section header such as `target.'cfg(unix)'.dependencies.oa-x`.
fn header_segments(h: &str) -> Vec<String> {
    split_outside_quotes(h, '.')
        .into_iter()
        .map(|p| unquote(p).unwrap_or_else(|| p.trim().to_string()))
        .collect()
}

fn is_dep_table(seg: &str) -> bool {
    matches!(
        seg,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    )
}

/// The lines of Cargo.toml text without comments, a `{ .. }` inline table that runs over several
/// lines joined into one (so its keys are not read as keys of the table around it).
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if depth > 0 {
            cur.push(' ');
        }
        cur.push_str(line);
        let mut quote: Option<char> = None;
        for ch in line.chars() {
            match quote {
                Some(q) if ch == q => quote = None,
                Some(_) => {}
                None if ch == '"' || ch == '\'' => quote = Some(ch),
                None if ch == '{' => depth += 1,
                None if ch == '}' => depth -= 1,
                None => {}
            }
        }
        if depth <= 0 {
            depth = 0;
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Reads the package name, the openatca class, the dependency tables, `[lib]`/`[[bin]]` paths and
/// a `build` key from Cargo.toml text.
pub fn parse_manifest(text: &str) -> Manifest {
    #[derive(PartialEq)]
    enum Sec {
        Other,
        Package,
        Meta,
        Lib,
        Bin,
        Lints,
        LintsRust,
        LintsClippy,
        DepTable(bool),
        DepNamed(String, bool),
    }
    let mut m = Manifest::default();
    let mut sec = Sec::Other;
    for line in logical_lines(text) {
        let line = line.as_str();
        if line.is_empty() {
            continue;
        }
        if let Some(h) = line.strip_prefix('[') {
            let array = h.starts_with('[');
            let h = h.trim_start_matches('[').trim_end_matches(']').trim();
            let segs = header_segments(h);
            let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
            sec = match segs.as_slice() {
                ["package"] => Sec::Package,
                ["package", "metadata", "openatca"] => Sec::Meta,
                ["lib"] => Sec::Lib,
                ["lints"] => Sec::Lints,
                ["lints", "rust"] => Sec::LintsRust,
                ["lints", "clippy"] => Sec::LintsClippy,
                ["bin"] if array => {
                    m.bin_paths.push(String::new());
                    Sec::Bin
                }
                ["workspace", "dependencies"] => Sec::DepTable(true),
                ["workspace", "dependencies", name] => {
                    with_dep(&mut m.ws_deps, name, |_| {});
                    Sec::DepNamed((*name).to_string(), true)
                }
                ["workspace", ..] => Sec::Other,
                _ => match segs.iter().position(|s| is_dep_table(s)) {
                    Some(i) if i + 1 == segs.len() => Sec::DepTable(false),
                    Some(i) if i + 2 == segs.len() => {
                        let name = segs.get(i + 1).copied().unwrap_or("").to_string();
                        with_dep(&mut m.deps, &name, |_| {});
                        Sec::DepNamed(name, false)
                    }
                    _ => Sec::Other,
                },
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match &sec {
            Sec::Package => {
                if key == "name" {
                    m.name = unquote(value);
                } else if key == "build" {
                    m.build_key = true;
                }
            }
            Sec::Meta if key == "class" => {
                m.class = Some(unquote(value).unwrap_or_else(|| value.to_string()));
            }
            Sec::Lints if key == "workspace" => m.lints_workspace = value == "true",
            Sec::LintsRust if key == "unsafe_code" => {
                m.lints_unsafe_deny = unquote(value).as_deref() == Some("deny");
            }
            Sec::LintsClippy if unquote(value).as_deref() == Some("deny") => {
                m.clippy_denied.insert(key.to_string());
            }
            Sec::Lib if key == "path" => m.lib_path = true,
            Sec::Lib if key == "name" => m.lib_name = unquote(value),
            Sec::Bin if key == "path" => {
                if let Some(last) = m.bin_paths.last_mut() {
                    *last = unquote(value).unwrap_or_else(|| value.to_string());
                }
            }
            Sec::DepNamed(name, ws) => {
                let deps = if *ws { &mut m.ws_deps } else { &mut m.deps };
                with_dep(deps, name, |d| match key {
                    "path" => d.path = unquote(value),
                    "package" => d.package = unquote(value),
                    "workspace" => d.workspace = value == "true",
                    _ => {}
                });
            }
            Sec::DepTable(ws) => {
                let parts = split_outside_quotes(key, '.');
                let name = parts
                    .first()
                    .map(|p| unquote(p).unwrap_or_else(|| p.trim().to_string()))
                    .unwrap_or_default();
                let sub = parts.get(1).map(|p| p.trim());
                let deps = if *ws { &mut m.ws_deps } else { &mut m.deps };
                with_dep(deps, &name, |d| match sub {
                    Some("path") => d.path = unquote(value),
                    Some("package") => d.package = unquote(value),
                    Some("workspace") => d.workspace = value == "true",
                    Some(_) => {}
                    None if value.starts_with('{') => {
                        d.path = inline_str(value, "path").or(d.path.take());
                        d.package = inline_str(value, "package").or(d.package.take());
                        d.workspace |= inline_true(value, "workspace");
                    }
                    None => {}
                });
            }
            _ => {}
        }
    }
    m
}

/// The string list `key = [ ... ]` of the `[workspace]` table in the root Cargo.toml, read
/// across lines with either quote style. Returns `None` if the key is absent, else the
/// entries and the entries that are not a plain quoted string (each is a failure).
pub fn parse_workspace_list(text: &str, key: &str) -> Option<(Vec<String>, Vec<String>)> {
    let mut in_ws = false;
    let mut body = String::new();
    let mut open = false;
    let mut depth: i32;
    let mut found = false;
    let mut seen_open = false;
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if !open {
            if line.starts_with('[') {
                in_ws = line == "[workspace]";
                continue;
            }
            if !in_ws {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            if k.trim() != key {
                continue;
            }
            found = true;
            open = true;
            body.push_str(v);
        } else {
            body.push(' ');
            body.push_str(line);
        }
        depth = 0;
        let mut q: Option<char> = None;
        for ch in body.chars() {
            match q {
                Some(c) if ch == c => q = None,
                Some(_) => {}
                None if ch == '"' || ch == '\'' => q = Some(ch),
                None if ch == '[' => {
                    depth += 1;
                    seen_open = true;
                }
                None if ch == ']' => depth -= 1,
                None => {}
            }
        }
        if seen_open && depth <= 0 {
            break;
        }
    }
    if !found {
        return None;
    }
    let inner = body.trim();
    let inner = inner.strip_prefix('[').unwrap_or(inner);
    let inner = inner.trim_end().strip_suffix(']').unwrap_or(inner);
    let mut items = Vec::new();
    let mut bad = Vec::new();
    for part in split_outside_quotes(inner, ',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match unquote(p) {
            Some(v) if !v.is_empty() && !v.contains('*') && !v.contains('?') => items.push(v),
            _ => bad.push(p.to_string()),
        }
    }
    Some((items, bad))
}

/// The `members` of the root Cargo.toml (entries and unparsed entries).
pub fn parse_members(text: &str) -> (Vec<String>, Vec<String>) {
    parse_workspace_list(text, "members").unwrap_or_default()
}

/// `path` joined to `base` with `.` and `..` resolved, `/`-separated; `None` if it leaves the root.
fn normalize(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for seg in base.split('/').chain(path.split('/')) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            x => parts.push(x),
        }
    }
    Some(parts.join("/"))
}

// ---------------------------------------------------------------------------------------------
// Lexer

/// `src` with comments, string literals, character literals and lifetimes (`'a`, `'static`)
/// replaced by spaces; line breaks are kept so line numbers still match.
pub fn mask(src: &str) -> String {
    mask_with_strings(src).0
}

/// [`mask`], and the character ranges (start, end) of every string literal in `src`.
pub fn mask_with_strings(src: &str) -> (String, Vec<(usize, usize)>) {
    let mut strings = Vec::new();
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
            let from = i;
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
            strings.push((from, i));
        } else if ch == '"' {
            let from = i;
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
            strings.push((from, i));
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
            } else if at(i + 1).is_alphabetic() || at(i + 1) == '_' {
                // a lifetime or label: blank it, so `&'static mut T` is not `static mut`
                out.push(' ');
                i += 1;
                while is_ident(at(i)) {
                    out.push(' ');
                    i += 1;
                }
            } else {
                out.push(ch);
                i += 1;
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    (out, strings)
}

#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
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

/// Index just past a turbofish `:: < .. >` that starts at `toks[i]`, or `i` when there is none.
fn skip_turbofish(toks: &[Tok], i: usize) -> usize {
    if s(toks, i) != "::" || s(toks, i + 1) != "<" {
        return i;
    }
    let mut depth = 0usize;
    let mut j = i + 1;
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

/// Whether a call's `(` starts at `toks[i]`, a turbofish (`::<T>`) allowed before it: the token
/// after a method or function name.
fn call_at(toks: &[Tok], i: usize) -> bool {
    s(toks, skip_turbofish(toks, i)) == "("
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

/// What [`strip_test_items`] found.
// A7.2: cold struct, size guard waived
struct Stripped {
    toks: Vec<Tok>,
    /// Names of out-of-line test-only modules (`#[cfg(test)] mod x;`).
    test_mods: Vec<String>,
    /// The file starts with `#![cfg(test)]`: all of it is test code.
    whole_file: bool,
}

/// Item keywords that may follow a test-only attribute for it to apply to an item.
const ITEM_KEYWORDS: &[&str] = &[
    "fn",
    "mod",
    "use",
    "impl",
    "struct",
    "enum",
    "const",
    "static",
    "type",
    "trait",
    "macro_rules",
    "union",
    "extern",
];

/// Whether the cfg predicate at `*i` holds only under test or proof builds; advances past it.
/// `test` and `kani` alone, and `all(..)` with such an argument, are; `any(..)` and `not(..)`
/// never are (`cfg(any(test, unix))` is compiled in a product build).
fn test_only(toks: &[Tok], i: &mut usize) -> bool {
    let w = s(toks, *i).to_string();
    match w.as_str() {
        "test" | "kani" => {
            *i += 1;
            true
        }
        "all" | "any" | "not" if s(toks, *i + 1) == "(" => {
            *i += 2;
            let mut any_arg = false;
            while *i < toks.len() && s(toks, *i) != ")" {
                any_arg |= test_only(toks, i);
                if s(toks, *i) == "," {
                    *i += 1;
                }
            }
            *i += 1;
            w == "all" && any_arg
        }
        _ => {
            // `feature = "x"`, `unix`, `target_os = ..`: not test-only; step over it
            *i += 1;
            while *i < toks.len() && !matches!(s(toks, *i), "," | ")") {
                if s(toks, *i) == "(" {
                    *i = skip_group(toks, *i);
                } else {
                    *i += 1;
                }
            }
            false
        }
    }
}

/// Lines containing only tokens inside an actual `#[cfg(test)]` item. This is shared with the
/// external-float inventory so test helpers in the middle of a source file do not hide later
/// production code. Lines shared with both test and non-test tokens are deliberately retained.
pub fn cfg_test_line_mask(src: &str) -> Vec<bool> {
    let masked = mask(src);
    let toks = tokenize(&masked);
    let mut removed = vec![false; toks.len()];
    let line_count = src.lines().count();
    let mut test_item_lines = vec![false; line_count];
    // An inner cfg(test) attribute at the file start applies to every item in the file.
    if s(&toks, 0) == "#"
        && s(&toks, 1) == "!"
        && s(&toks, 2) == "["
        && s(&toks, 3) == "cfg"
        && s(&toks, 4) == "("
        && s(&toks, 5) == "test"
        && s(&toks, 6) == ")"
        && s(&toks, 7) == "]"
    {
        return vec![true; line_count];
    }
    let mut i = 0;
    while i + 6 < toks.len() {
        if s(&toks, i) != "#"
            || s(&toks, i + 1) != "["
            || s(&toks, i + 2) != "cfg"
            || s(&toks, i + 3) != "("
            || s(&toks, i + 4) != "test"
            || s(&toks, i + 5) != ")"
            || s(&toks, i + 6) != "]"
        {
            i += 1;
            continue;
        }
        let mut j = i + 7;
        while s(&toks, j) == "#" && s(&toks, j + 1) == "[" {
            j = skip_group(&toks, j + 1);
        }
        j = skip_modifiers(&toks, j);
        if !ITEM_KEYWORDS.contains(&s(&toks, j)) {
            i += 1;
            continue;
        }
        let kw = s(&toks, j);
        let to_semicolon = matches!(kw, "use" | "const" | "static" | "type")
            || (kw == "mod" && s(&toks, j + 2) != "{")
            || (kw == "extern" && s(&toks, j + 1) == "crate");
        let mut depth = 0usize;
        let mut end = j;
        let mut complete = false;
        while end < toks.len() {
            match s(&toks, end) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" => depth = depth.saturating_sub(1),
                "}" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && !to_semicolon {
                        end += 1;
                        complete = true;
                        break;
                    }
                }
                ";" if depth == 0 => {
                    end += 1;
                    complete = true;
                    break;
                }
                _ => {}
            }
            end += 1;
        }
        if complete {
            if let Some(tokens) = removed.get_mut(i..end) {
                tokens.fill(true);
            }
            let first_line = toks.get(i).map_or(1, |token| token.line);
            let last_line = toks
                .get(end.saturating_sub(1))
                .map_or(first_line, |token| token.line);
            for line in first_line..=last_line {
                if let Some(test_line) = test_item_lines.get_mut(line.saturating_sub(1)) {
                    *test_line = true;
                }
            }
            i = end;
        } else {
            // An incomplete/unrecognized item is not excluded: the caller will scan it and fail
            // closed on any raw float read rather than silently dropping the rest of the file.
            i += 1;
        }
    }

    let mut has_kept_token = vec![false; line_count];
    for (index, token) in toks.iter().enumerate() {
        let line = token.line.saturating_sub(1);
        if !removed.get(index).copied().unwrap_or(false) {
            if let Some(seen) = has_kept_token.get_mut(line) {
                *seen = true;
            }
        }
    }
    test_item_lines
        .into_iter()
        .zip(has_kept_token)
        .map(|(in_test, kept)| in_test && !kept)
        .collect()
}

/// Removes test-only items (B7: tests and proof harnesses are shell). A test-only attribute
/// strips the item that follows it only if an item keyword follows; on a field, variant, match
/// arm, parameter or statement it strips nothing.
fn strip_test_items(toks: Vec<Tok>) -> Stripped {
    let mut out = Vec::new();
    let mut test_mods = Vec::new();
    let mut whole_file = false;
    // open `{` of the tokens kept so far: a test-only `mod x;` inside an inline module resolves
    // below that module's directory, which is not modelled, so it is not skipped (fail closed)
    let mut depth = 0usize;
    let mut i = 0;
    while i < toks.len() {
        let inner = s(&toks, i) == "#" && s(&toks, i + 1) == "!";
        let at = i + if inner { 2 } else { 1 };
        let mut p = at + 3;
        let is_test = s(&toks, i) == "#"
            && s(&toks, at) == "["
            && s(&toks, at + 1) == "cfg"
            && s(&toks, at + 2) == "("
            && test_only(&toks, &mut p)
            && s(&toks, p) == ")"
            && s(&toks, p + 1) == "]";
        if !is_test {
            match s(&toks, i) {
                "{" => depth += 1,
                "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
            if let Some(t) = toks.get(i) {
                out.push(t.clone());
            }
            i += 1;
            continue;
        }
        if inner {
            // `#![cfg(test)]` is a whole-file marker only before any item
            if out.is_empty()
                || out
                    .iter()
                    .all(|t| matches!(t.s.as_str(), "#" | "!" | "[" | "]"))
            {
                whole_file = true;
            }
            i = p + 2;
            continue;
        }
        let mut j = p + 2;
        while s(&toks, j) == "#" && s(&toks, j + 1) == "[" {
            j = skip_group(&toks, j + 1);
        }
        j = skip_modifiers(&toks, j);
        let kw = s(&toks, j).to_string();
        if !ITEM_KEYWORDS.contains(&kw.as_str()) {
            // not on an item: keep everything, strip nothing
            if let Some(t) = toks.get(i) {
                out.push(t.clone());
            }
            i += 1;
            continue;
        }
        if kw == "mod" && s(&toks, j + 2) == ";" && depth == 0 {
            test_mods.push(s(&toks, j + 1).to_string());
        }
        let to_semicolon = match kw.as_str() {
            "use" | "const" | "static" | "type" => true,
            "mod" => s(&toks, j + 2) != "{",
            "extern" => s(&toks, j + 1) == "crate",
            _ => false,
        };
        let mut depth = 0usize;
        i = j;
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
    Stripped {
        toks: out,
        test_mods,
        whole_file,
    }
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

/// What a scan needs to know about the crate the file belongs to.
// A7.2: cold struct, size guard waived
pub struct Ctx<'a> {
    /// The crate is split: `crate::` is allowed only under `crate::core::`, and `super::` may
    /// not climb out of the core module.
    pub split: bool,
    /// Module depth of the file below the crate root (`core.rs` and `core/mod.rs` are 1).
    pub depth: usize,
    /// The crate is `oa-ieee`, whose purpose is the float methods B8 forbids elsewhere.
    pub ieee: bool,
    /// The package is in the exact F2 wasm32 runtime allowlist. This permits only the exact
    /// native64-or-wasm32 width guard in its source; other target-dependent cfgs remain forbidden.
    pub wasm32_guard_allowed: bool,
    /// The crate is `oa-math`, which does bit arithmetic on floats by design.
    pub math: bool,
    /// Modules the crate declares (`mod x`): a lowercase path qualifier is one of these, a
    /// crate, or it fails.
    pub modules: &'a BTreeSet<String>,
    /// What the core crates of the whole workspace declare as floats: struct fields, tuple-struct
    /// fields, `fn` return types and constants (a bare name is judged inside its own `fn`).
    pub floats: &'a FloatFacts,
    /// Modules of this crate that are not public (`mod x`, `pub(crate) mod x`): a trait reached
    /// through one cannot be named by the shell, which is what seals a `pub trait` (B5).
    pub private_mods: &'a BTreeSet<String>,
    /// Names of the sealed `pub trait`s of every core crate (B5): a generic may be bounded by
    /// one.
    pub sealed: &'a BTreeSet<String>,
    /// Names of the traits that seal them (`Sealed` in `sealed::Sealed`): they are not re-exported
    /// and not implemented for a bare type parameter.
    pub sealing: &'a BTreeSet<String>,
    /// Names of the modules that hold a sealing trait: a `pub use` of one publishes the seal.
    pub sealing_mods: &'a BTreeSet<String>,
    /// Names of the type aliases of this crate whose body holds an `Fn*`, `dyn` or `fn(..)`: a
    /// reachable signature that names one hands behaviour in (B5).
    pub cb_types: &'a BTreeSet<String>,
    /// Class of every workspace crate by name.
    pub classes: &'a BTreeMap<String, Class>,
    /// Identifier used in source -> workspace crate it names (own name, dependency keys).
    pub aliases: &'a BTreeMap<String, String>,
}

fn is_word(t: &str) -> bool {
    t.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Whether `toks[i]` starts a path (not a field, method or later segment of one).
fn is_path_start(toks: &[Tok], i: usize) -> bool {
    match if i == 0 { "" } else { s(toks, i - 1) } {
        "." => false,
        "::" => {
            // a leading `::` follows a keyword or a delimiter, never a path segment
            let before = if i >= 2 { s(toks, i - 2) } else { "" };
            !is_word(before)
                || matches!(
                    before,
                    "use"
                        | "as"
                        | "return"
                        | "in"
                        | "dyn"
                        | "impl"
                        | "for"
                        | "where"
                        | "mut"
                        | "ref"
                        | "move"
                        | "else"
                        | "match"
                        | "if"
                        | "while"
                        | "loop"
                        | "break"
                )
        }
        _ => true,
    }
}

/// What to use instead of the module a core path names.
fn module_hint(seg: &str) -> &'static str {
    match seg {
        "time" => "use core::time::Duration; the clock is the shell's, time is an input",
        "io" | "error" => "use core::fmt or an error type of your own; I/O is shell",
        "collections" => "use alloc::collections::{BTreeMap, BTreeSet, VecDeque, BinaryHeap}",
        "sync" | "cell" | "thread" => "state is passed in as a value; no globals or threads in core",
        "fs" | "net" | "env" | "process" | "os" | "path" | "ffi" => "that is I/O and belongs in the shell",
        _ => "allowed: fmt, ops, cmp, mem, num, convert, iter, slice, array, str, char, ascii, option, result, marker, default, clone, borrow, hint, error, f32, f64, integer modules, vec (alloc::vec), boxed, string, format (alloc::format!), collections (BTreeMap, BTreeSet, VecDeque, BinaryHeap), core::time::Duration",
    }
}

/// Checks the module list of a path under `root` (`std`, `core` or `alloc`) as it is read.
fn check_segment(prefix: &[String], root: &str, line: usize, hits: &mut Vec<Hit>) {
    let Some(first) = prefix.first() else { return };
    let path = format!("{root}::{}", prefix.join("::"));
    match prefix.len() {
        1 => {
            let ok = ALLOWED_MODULES.contains(&first.as_str())
                || first == "collections"
                || (first == "time" && root == "core");
            if !ok {
                hits.push(hit(
                    line,
                    &path,
                    &format!("B8: not an allowed module; {}", module_hint(first)),
                ));
            }
        }
        2 => {
            let second = prefix.get(1).map_or("", String::as_str);
            let bad = (first == "collections" && !ALLOWED_COLLECTIONS.contains(&second))
                || (first == "time" && second != "Duration");
            if bad {
                hits.push(hit(
                    line,
                    &path,
                    &format!("B8: not allowed; {}", module_hint(first)),
                ));
            }
        }
        _ => {}
    }
}

/// Walks the path or use-tree that starts at `toks[j]` (just after `root ::`), reporting every
/// segment B8 does not allow. Returns the index past what it read.
fn walk_path(
    toks: &[Tok],
    mut j: usize,
    root: &str,
    prefix: &mut Vec<String>,
    hits: &mut Vec<Hit>,
) -> usize {
    let line = toks.get(j).map_or(0, |t| t.line);
    let w = s(toks, j);
    if w == "{" {
        j += 1;
        while j < toks.len() {
            match s(toks, j) {
                "}" => return j + 1,
                "," => j += 1,
                _ => {
                    let mut p = prefix.clone();
                    j = walk_path(toks, j, root, &mut p, hits).max(j + 1);
                }
            }
        }
        j
    } else if w == "*" {
        hits.push(hit(
            line,
            "*",
            "B8: no `*` glob on a std, core or alloc path",
        ));
        j + 1
    } else if is_word(w) {
        prefix.push(w.to_string());
        check_segment(prefix, root, line, hits);
        if s(toks, j + 1) == "::" && s(toks, j + 2) != "<" {
            return walk_path(toks, j + 2, root, prefix, hits);
        }
        if prefix.len() == 1 && matches!(w, "collections" | "time") {
            hits.push(hit(
                line,
                &format!("{root}::{w}"),
                &format!("B8: name the item, not the module; {}", module_hint(w)),
            ));
        }
        j + 1
    } else {
        hits.push(hit(
            line,
            w,
            "B8: a std, core or alloc path must be spelled out",
        ));
        j + 1
    }
}

/// Finds the B8 names in core tokens.
fn scan_tokens(toks: &[Tok], ctx: &Ctx<'_>) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut use_start: Option<usize> = None;
    // one entry per open `{`: true if it opens an inline `mod name {`
    let mut braces: Vec<bool> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let w = t.s.as_str();
        let prev = if i == 0 { "" } else { s(toks, i - 1) };
        let next = s(toks, i + 1);
        match w {
            "use" if is_path_start(toks, i) && use_start.is_none() => use_start = Some(i + 1),
            ";" => {
                if let Some(from) = use_start.take() {
                    check_use_statement(toks.get(from..i).unwrap_or(&[]), ctx, &mut hits);
                }
            }
            "{" => braces.push(i >= 2 && s(toks, i - 2) == "mod" && is_word(prev)),
            "}" => {
                braces.pop();
            }
            _ => {}
        }
        let in_use = use_start.is_some();
        let start = is_path_start(toks, i);
        if !ctx.ieee
            && FLOAT_METHODS.contains(&w)
            && ((prev == "." && call_at(toks, i + 1)) || prev == "::")
        {
            // after `.` (a method call) the receiver is any value; after `::` the qualifier must
            // be a path the scan knows: `oa_math`/`oa_ieee`, a core crate, a module this crate
            // declares, `self`/`super`/`crate`, or an integer type for `rem_euclid`/`div_euclid`.
            // Anything else (`f64`, `Self`, `R`, `<f64>`, a lowercase alias that slipped past the
            // type-name lint) fails closed, so no name can stand for the standard library's type.
            let qual = if prev == "::" && i >= 2 {
                s(toks, i - 2)
            } else {
                ""
            };
            let target = ctx.aliases.get(qual).and_then(|t| ctx.classes.get(t));
            let blessed = prev == "::"
                && (matches!(qual, "oa_math" | "oa_ieee")
                    || ctx
                        .aliases
                        .get(qual)
                        .is_some_and(|t| t == "oa-math" || t == "oa-ieee"));
            let int_euclid = prev == "::"
                && matches!(w, "rem_euclid" | "div_euclid")
                && INT_TYPES.contains(&qual);
            let known = prev == "::"
                && (matches!(qual, "self" | "super" | "crate")
                    || ctx.modules.contains(qual)
                    || target == Some(&Class::Core));
            if !blessed && !int_euclid && !known {
                hits.push(hit(t.line, w, &float_message(w)));
            }
        }
        if w == "macro_rules" {
            hits.push(hit(
                t.line,
                "macro_rules",
                "B8: core code defines no macro, so every name the scan reads is the name the compiler resolves",
            ));
        } else if w == "$" {
            hits.push(hit(t.line, "$", "B8: core code has no macro fragments"));
        } else if w == "r" && next == "#" && is_word(s(toks, i + 2)) {
            hits.push(hit(
                t.line,
                &format!("r#{}", s(toks, i + 2)),
                "B8: core code uses no raw identifier",
            ));
        } else if matches!(w, "cfg" | "cfg_attr")
            && (next == "(" || (next == "!" && s(toks, i + 2) == "("))
        {
            let open = if next == "(" { i + 1 } else { i + 2 };
            let end = skip_group(toks, open);
            if (open..end).any(|k| s(toks, k) == "debug_assertions") {
                hits.push(hit(
                    t.line,
                    "debug_assertions",
                    "B8: core code does not branch on `debug_assertions`, a debug and a release build must compute the same (`debug_assert!` stays: it can only panic)",
                ));
            }
            let target = (open..end)
                .find_map(|k| {
                    let x = s(toks, k);
                    (x.starts_with("target_") || matches!(x, "unix" | "windows")).then_some(x)
                })
                .filter(|_| !pointer_width_guard(toks, i, ctx.wasm32_guard_allowed));
            if let Some(x) = target {
                hits.push(hit(
                    t.line,
                    x,
                    "B8: core code has no target-dependent cfg, so it behaves the same on every machine",
                ));
            }
        } else if w == "*" && matches!(next, "const" | "mut") {
            hits.push(hit(
                t.line,
                &format!("*{next}"),
                "B8: core code has no raw pointer type",
            ));
        }
        if w == "extern" && next == "crate" {
            if !(s(toks, i + 2) == "alloc" && s(toks, i + 3) == ";") {
                hits.push(hit(
                    t.line,
                    "extern crate",
                    "B8: the only `extern crate` core code may have is `extern crate alloc;`",
                ));
            }
        } else if w == "static" && next == "mut" {
            hits.push(hit(t.line, "static mut", "B8: core code names it"));
        } else if next == "!"
            && s(toks, i + 2) != "="
            && prev != "."
            && FORBIDDEN_MACROS.contains(&w)
        {
            hits.push(hit(t.line, &format!("{w}!"), "B8: core code names it"));
        } else if FORBIDDEN_IDENTS.contains(&w) {
            hits.push(hit(t.line, w, "B8: core code names it"));
        } else if w == "shell"
            && (prev == "::" || next == "::" || (in_use && matches!(prev, "{" | ",")))
        {
            hits.push(hit(t.line, "shell", "B8: path into a shell module"));
        } else if start && matches!(w, "std" | "core" | "alloc") {
            if next == "::" {
                walk_path(toks, i + 2, w, &mut Vec::new(), &mut hits);
            } else if in_use {
                hits.push(hit(
                    t.line,
                    w,
                    "B8: name the module path, not the bare crate",
                ));
            }
        } else if start && ctx.aliases.contains_key(w) {
            let Some(target) = ctx.aliases.get(w) else {
                continue;
            };
            if next != "::" {
                if in_use {
                    hits.push(hit(
                        t.line,
                        w,
                        "B8: name the module path, not the bare crate",
                    ));
                }
                continue;
            }
            match ctx.classes.get(target) {
                Some(Class::Core) => {}
                Some(Class::Split) if s(toks, i + 2) == "core" => {}
                Some(Class::Split) => hits.push(hit(
                    t.line,
                    w,
                    "B8: names a split crate outside its `core` module",
                )),
                _ => hits.push(hit(t.line, w, "B8: names a shell or unsplit crate")),
            }
        } else if ctx.split && start && w == "crate" && next == "::" && s(toks, i + 2) != "core" {
            hits.push(hit(
                t.line,
                &format!("crate::{}", s(toks, i + 2)),
                "B8: core code of a split crate names `crate::` only under `crate::core::`",
            ));
        } else if ctx.split
            && w == "super"
            && next == "::"
            && (prev != "::" || (i >= 2 && s(toks, i - 2) == "self"))
        {
            let mut j = i;
            let mut up = 0;
            while s(toks, j) == "super" && s(toks, j + 1) == "::" {
                up += 1;
                j += 2;
            }
            let depth = ctx.depth + braces.iter().filter(|b| **b).count();
            if up >= depth && s(toks, j) != "core" {
                hits.push(hit(
                    t.line,
                    "super",
                    "B8: `super::` reaches the crate root; name `crate::core::` instead",
                ));
            }
        }
    }
    harden::token_bans(toks, ctx, &mut hits);
    hits
}

/// The tokens of one `use` statement (after `use`, before `;`): no `as` and no `*` where a
/// std, core, alloc or workspace-crate path is imported.
fn check_use_statement(stmt: &[Tok], ctx: &Ctx<'_>, hits: &mut Vec<Hit>) {
    // an `as` rename hides what a name is (a sealing trait, a float type alias): banned on every
    // `use` in core
    for t in stmt {
        if t.s == "as" {
            hits.push(hit(
                t.line,
                "as",
                "B8: core code has no `as` rename in a `use`",
            ));
        }
    }
    let has_root = stmt.iter().enumerate().any(|(k, t)| {
        (matches!(t.s.as_str(), "std" | "core" | "alloc") || ctx.aliases.contains_key(t.s.as_str()))
            && is_path_start(stmt, k)
    });
    if !has_root {
        return;
    }
    for t in stmt {
        if t.s == "*" {
            hits.push(hit(
                t.line,
                &t.s,
                "B8: no `*` glob on a std, core, alloc or workspace-crate path",
            ));
        }
    }
}

/// Whether the `cfg` at `toks[i]` is one exact target guard directly followed by `compile_error!`.
/// The second form is the narrow wasm32-unknown-unknown exception; scan context is created from
/// the exact package allowlist, and `has_pointer_width_guard` checks the root declaration.
fn pointer_width_guard(toks: &[Tok], i: usize, allow_wasm32: bool) -> bool {
    let native64 = [
        "cfg",
        "(",
        "not",
        "(",
        "target_pointer_width",
        "=",
        ")",
        ")",
        "]",
        "compile_error",
        "!",
    ];
    let wasm32 = [
        "cfg",
        "(",
        "not",
        "(",
        "any",
        "(",
        "target_pointer_width",
        "=",
        ",",
        "all",
        "(",
        "target_arch",
        "=",
        ",",
        "target_pointer_width",
        "=",
        ",",
        "target_os",
        "=",
        ",",
        "target_vendor",
        "=",
        ")",
        ")",
        ")",
        ")",
        "]",
        "compile_error",
        "!",
    ];
    i >= 2
        && s(toks, i - 1) == "["
        && s(toks, i - 2) == "#"
        && guard_position(toks, i - 2)
        && (native64
            .iter()
            .enumerate()
            .all(|(k, w)| s(toks, i + k) == *w)
            || (allow_wasm32 && wasm32.iter().enumerate().all(|(k, w)| s(toks, i + k) == *w)))
}

/// Whether the attribute that starts at the `#` at `toks[hash]` stands alone at an item
/// boundary: at the start of the file, or after the `]` of an inner attribute, a `;` or a `}`.
/// Another outer attribute directly before it (`#[cfg(any())]` stacked on the guard) does not.
fn guard_position(toks: &[Tok], hash: usize) -> bool {
    if hash == 0 {
        return true;
    }
    match s(toks, hash - 1) {
        ";" | "}" => true,
        "]" => {
            // walk back to the `[` that opens this `]`: an inner attribute is `# ! [`
            let mut depth = 0usize;
            let mut k = hash - 1;
            loop {
                match s(toks, k) {
                    "]" => depth += 1,
                    "[" => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return k >= 2 && s(toks, k - 1) == "!" && s(toks, k - 2) == "#";
                        }
                    }
                    _ => {}
                }
                if k == 0 {
                    return false;
                }
                k -= 1;
            }
        }
        _ => false,
    }
}

/// Lines of `#[path = ..]` attributes, also inside `cfg_attr`: they pull in a file the scan does
/// not read.
fn path_attr_lines(toks: &[Tok]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.s != "#" {
            continue;
        }
        let at = if s(toks, i + 1) == "!" { i + 2 } else { i + 1 };
        if s(toks, at) != "[" {
            continue;
        }
        let end = skip_group(toks, at);
        let head = s(toks, at + 1);
        let bad = (head == "path" && s(toks, at + 2) == "=")
            || (head == "cfg_attr"
                && (at..end).any(|k| s(toks, k) == "path" && s(toks, k + 1) == "="));
        if bad {
            out.push(t.line);
        }
    }
    out
}

/// `{:p}` (and `{x:p}`, `{:#18p}`) in a string literal: it prints an address, which differs
/// between runs and machines.
fn pointer_format_hits(src: &str, strings: &[(usize, usize)]) -> Vec<Hit> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    for &(from, to) in strings {
        let body = c.get(from..to).unwrap_or(&[]);
        for (k, ch) in body.iter().enumerate() {
            if *ch != '}' || k == 0 || body.get(k - 1) != Some(&'p') {
                continue;
            }
            // the `{` that opens this group, with a `:` between
            let open = body
                .get(..k)
                .and_then(|b| b.iter().rposition(|x| *x == '{'));
            let Some(open) = open else { continue };
            let spec = body.get(open + 1..k).unwrap_or(&[]);
            if spec.contains(&':') && !spec.contains(&'}') {
                let line = 1 + c
                    .get(..from + k)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|x| **x == '\n')
                    .count();
                out.push(hit(
                    line,
                    "{:p}",
                    "B8: core code does not format pointers, an address differs between runs",
                ));
            }
        }
    }
    out
}

/// Result of scanning one file of core code.
#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
pub struct FileScan {
    pub hits: Vec<Hit>,
    /// Out-of-line test-only modules the file declares.
    pub test_mods: Vec<String>,
    /// The whole file is test code (`#![cfg(test)]`).
    pub whole_file: bool,
    /// Core `pub fn`s whose signature takes a callback or a bound (B5).
    pub callbacks: Vec<CallbackSig>,
}

/// Scans one file of core code.
pub fn scan_source(src: &str, ctx: &Ctx<'_>) -> FileScan {
    let (masked, strings) = mask_with_strings(src);
    let raw = tokenize(&masked);
    let st = strip_test_items(raw);
    let mut hits: Vec<Hit> = path_attr_lines(&st.toks)
        .into_iter()
        .map(|l| hit(l, "#[path]", "B8: core code uses no `#[path]` attribute"))
        .collect();
    hits.extend(pointer_format_hits(src, &strings));
    hits.extend(scan_tokens(&st.toks, ctx));
    FileScan {
        hits,
        test_mods: st.test_mods,
        whole_file: st.whole_file,
        callbacks: boundary::extra_sigs(src, ctx),
    }
}

/// Whether `lib.rs` has `#![no_std]` or `#![cfg_attr(not(test), no_std)]` (B10).
pub fn has_no_std(src: &str) -> bool {
    // only the inner attributes at the top of the file count: before any item, in no module
    let toks = tokenize(&mask(src));
    let mut i = 0;
    while s(&toks, i) == "#" && s(&toks, i + 1) == "!" && s(&toks, i + 2) == "[" {
        let end = skip_group(&toks, i + 2);
        let body: String = toks
            .get(i + 3..end.saturating_sub(1))
            .unwrap_or(&[])
            .iter()
            .map(|t| t.s.as_str())
            .collect();
        if body == "no_std" || body == "cfg_attr(not(test),no_std)" {
            return true;
        }
        i = end;
    }
    false
}

/// `src` with comments blanked and its string literals kept, for the rules that read a literal.
fn mask_keep_strings(src: &str) -> Option<String> {
    let (masked, strings) = mask_with_strings(src);
    let mut out: Vec<char> = masked.chars().collect();
    let orig: Vec<char> = src.chars().collect();
    if out.len() != orig.len() {
        return None;
    }
    for (from, to) in strings {
        for k in from..to.min(orig.len()) {
            if let (Some(slot), Some(c)) = (out.get_mut(k), orig.get(k)) {
                *slot = *c;
            }
        }
    }
    Some(out.into_iter().collect())
}

/// Whether `lib.rs` has the required top-level width guard. The exact wasm32 form is accepted only
/// for packages in `WASM32_PACKAGE_ALLOWLIST`; all other core/split packages remain 64-bit only.
pub fn has_pointer_width_guard(src: &str, allow_wasm32: bool) -> bool {
    has_width_guard(src, true, allow_wasm32)
}

/// Whether a crate in the exact demo allowlist has the wasm32 guard, rather than only the native
/// 64-bit form. Used by the source trace for the pinned runtime closure.
pub fn has_wasm32_pointer_width_guard(src: &str) -> bool {
    has_width_guard(src, false, true)
}

fn has_width_guard(src: &str, allow_native64: bool, allow_wasm32: bool) -> bool {
    let Some(kept) = mask_keep_strings(src) else {
        return false;
    };
    let toks = strip_test_items(tokenize(&kept)).toks;
    let native64 = [
        "#",
        "[",
        "cfg",
        "(",
        "not",
        "(",
        "target_pointer_width",
        "=",
        "\"",
        "64",
        "\"",
        ")",
        ")",
        "]",
        "compile_error",
        "!",
        "(",
    ];
    let wasm32 = [
        "#",
        "[",
        "cfg",
        "(",
        "not",
        "(",
        "any",
        "(",
        "target_pointer_width",
        "=",
        "\"",
        "64",
        "\"",
        ",",
        "all",
        "(",
        "target_arch",
        "=",
        "\"",
        "wasm32",
        "\"",
        ",",
        "target_pointer_width",
        "=",
        "\"",
        "32",
        "\"",
        ",",
        "target_os",
        "=",
        "\"",
        "unknown",
        "\"",
        ",",
        "target_vendor",
        "=",
        "\"",
        "unknown",
        "\"",
        ")",
        ")",
        ")",
        ")",
        "]",
        "compile_error",
        "!",
        "(",
    ];
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate() {
        match t.s.as_str() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
        let native_match = allow_native64
            && native64
                .iter()
                .enumerate()
                .all(|(k, w)| s(&toks, i + k) == *w);
        let wasm_match = allow_wasm32
            && wasm32
                .iter()
                .enumerate()
                .all(|(k, w)| s(&toks, i + k) == *w);
        let guard_end = if native_match {
            Some(i + native64.len() - 1)
        } else if wasm_match {
            Some(i + wasm32.len() - 1)
        } else {
            None
        };
        if depth == 0
            && guard_position(&toks, i)
            && guard_end.is_some_and(|end| s(&toks, skip_group(&toks, end)) == ";")
        {
            return true;
        }
    }
    false
}

/// Whether `lib.rs` has `#![forbid(unsafe_code)]` as an inner attribute at the top, before any
/// item (RULESET A0, B8).
pub fn has_forbid_unsafe(src: &str) -> bool {
    let toks = tokenize(&mask(src));
    let mut i = 0;
    while s(&toks, i) == "#" && s(&toks, i + 1) == "!" && s(&toks, i + 2) == "[" {
        let end = skip_group(&toks, i + 2);
        let body: String = toks
            .get(i + 3..end.saturating_sub(1))
            .unwrap_or(&[])
            .iter()
            .map(|t| t.s.as_str())
            .collect();
        if body == "forbid(unsafe_code)" {
            return true;
        }
        i = end;
    }
    false
}

/// Whether `lib.rs` declares a module named `main` or `bin`, in any visibility, inline or in a
/// file: it would load `src/main.rs` or `src/bin*`, which the core scan does not read.
pub fn declares_main_or_bin(src: &str) -> Vec<Hit> {
    let toks = tokenize(&mask(src));
    (0..toks.len())
        .filter(|&i| s(&toks, i) == "mod" && matches!(s(&toks, i + 1), "main" | "bin"))
        .map(|i| {
            hit(
                toks.get(i).map_or(0, |t| t.line),
                &format!("mod {}", s(&toks, i + 1)),
                "B8: lib.rs of a core crate declares no module named `main` or `bin`",
            )
        })
        .collect()
}

/// Problems with the public API of `oa-ieee`'s `lib.rs` (B10): exactly the five `pub fn`s, and
/// no other public item, module, re-export or macro.
pub fn ieee_api_problems(src: &str) -> Vec<Hit> {
    let toks = strip_test_items(tokenize(&mask(src))).toks;
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let msg =
        "B10: the public API of oa-ieee is exactly `pub fn` sqrt, floor, ceil, trunc and round";
    let mut i = 0;
    let mut depth = 0usize;
    while i < toks.len() {
        let line = toks.get(i).map_or(0, |t| t.line);
        match s(&toks, i) {
            "{" | "(" | "[" => depth += 1,
            "}" | ")" | "]" => depth = depth.saturating_sub(1),
            "pub" if depth == 0 && s(&toks, i + 1) != "(" => {
                if s(&toks, i + 1) == "fn" && IEEE_API.contains(&s(&toks, i + 2)) {
                    let name = s(&toks, i + 2).to_string();
                    // the body is exactly `v.<name>()`, so no other float call hides in it
                    let open = (i + 3..toks.len()).find(|&j| s(&toks, j) == "{");
                    let body: Vec<&str> = open.map_or(Vec::new(), |o| {
                        let end = skip_group(&toks, o);
                        toks.get(o + 1..end.saturating_sub(1))
                            .unwrap_or(&[])
                            .iter()
                            .map(|t| t.s.as_str())
                            .collect()
                    });
                    if body != ["v", ".", name.as_str(), "(", ")"] {
                        out.push(hit(
                            line,
                            &name,
                            "B10: the body of each oa-ieee function is exactly `v.<same name>()`",
                        ));
                    }
                    seen.push(name);
                } else {
                    out.push(hit(line, s(&toks, i + 1), msg));
                }
            }
            "mod" | "macro_rules" | "macro_export" | "macro"
                if depth == 0 || s(&toks, i) == "macro_export" =>
            {
                out.push(hit(line, s(&toks, i), msg));
            }
            _ => {}
        }
        i += 1;
    }
    for api in IEEE_API {
        if seen.iter().filter(|n| n.as_str() == api).count() != 1 {
            out.push(hit(1, api, msg));
        }
    }
    out
}

/// Problems with `lib.rs` of a split crate: it declares exactly `mod core;` and `mod shell;`
/// (any visibility), plus re-exports, attributes and doc comments, and nothing else.
pub fn lib_rs_problems(src: &str) -> Vec<Hit> {
    lib_rs_problems_for_package(src, false)
}

fn attribute_matches_without_whitespace(
    src: &str,
    start_line: usize,
    end_line: usize,
    expected: &str,
) -> bool {
    let normalize = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let actual: String = src
        .lines()
        .skip(start_line.saturating_sub(1))
        .take(end_line.saturating_sub(start_line).saturating_add(1))
        .collect();
    normalize(&actual) == normalize(expected)
}

fn lib_rs_problems_for_package(src: &str, allow_display_wasm_shell_gate: bool) -> Vec<Hit> {
    let raw = tokenize(&mask(src));
    let mut out: Vec<Hit> = path_attr_lines(&raw)
        .into_iter()
        .map(|l| hit(l, "#[path]", "B7: lib.rs uses no `#[path]` attribute"))
        .collect();
    let toks = strip_test_items(raw).toks;
    let msg = "B7: lib.rs of a split crate declares exactly `mod core;` and `mod shell;`, re-exports and attributes";
    let (mut core_n, mut shell_n) = (0, 0);
    let mut shell_module_gated = false;
    let mut shell_uses_gated = 0usize;
    let mut shell_uses_ungated = 0usize;
    let mut pending_shell_gate = false;
    let mut i = 0;
    while i < toks.len() {
        if s(&toks, i) == "#" {
            let at_line = toks.get(i).map_or(0, |t| t.line);
            i += 1;
            let inner = s(&toks, i) == "!";
            if inner {
                i += 1;
            }
            if s(&toks, i) != "[" {
                out.push(hit(
                    toks.get(i).map_or(0, |t| t.line),
                    s(&toks, i),
                    "stray `#`",
                ));
                break;
            }
            let end = skip_group(&toks, i);
            let body: String = toks
                .get(i + 1..end.saturating_sub(1))
                .unwrap_or(&[])
                .iter()
                .map(|t| t.s.as_str())
                .collect();
            // only the attributes a lib.rs of a split crate needs: `forbid(unsafe_code)` and
            // `doc`; anything else (`macro_use` on a module, `allow`, `cfg_attr`, `path`) could
            // carry shell code into the core
            // One exact width guard and the `compile_error!(..);` after it (B10). The package
            // allowlist is enforced separately by `has_pointer_width_guard`.
            let native_width_guard = body == "cfg(not(target_pointer_width=))";
            let wasm32_width_guard = body
                == "cfg(not(any(target_pointer_width=,all(target_arch=,target_pointer_width=,target_os=,target_vendor=))))";
            if !inner
                && (native_width_guard || wasm32_width_guard)
                && s(&toks, end) == "compile_error"
                && s(&toks, end + 1) == "!"
                && s(&toks, end + 2) == "("
                && s(&toks, skip_group(&toks, end + 2)) == ";"
            {
                i = skip_group(&toks, end + 2) + 1;
                continue;
            }
            const DISPLAY_SHELL_CFG: &str = "#[cfg(not(all(target_arch = \"wasm32\", target_pointer_width = \"32\", target_os = \"unknown\", target_vendor = \"unknown\")))]";
            let attr_last_line = toks
                .get(end.saturating_sub(1))
                .map_or(at_line, |token| token.line);
            let wasm_shell_cfg = body
                == "cfg(not(all(target_arch=,target_pointer_width=,target_os=,target_vendor=)))"
                && attribute_matches_without_whitespace(
                    src,
                    at_line,
                    attr_last_line,
                    DISPLAY_SHELL_CFG,
                );
            let next = skip_modifiers(&toks, end);
            let gated_shell_module = wasm_shell_cfg
                && s(&toks, next) == "mod"
                && s(&toks, next + 1) == "shell"
                && s(&toks, next + 2) == ";";
            let gated_shell_use = wasm_shell_cfg
                && s(&toks, end) == "pub"
                && s(&toks, next) == "use"
                && s(&toks, next + 1) == "crate"
                && s(&toks, next + 2) == "::"
                && s(&toks, next + 3) == "shell"
                && (next..toks.len()).any(|j| s(&toks, j) == ";");
            let ok = body.starts_with("doc=")
                || body.starts_with("doc(")
                || (inner && body == "forbid(unsafe_code)")
                || (!inner
                    && allow_display_wasm_shell_gate
                    && (gated_shell_module || gated_shell_use));
            if !ok {
                out.push(hit(
                    at_line,
                    &body,
                    "B7: lib.rs of a split crate carries only `#![forbid(unsafe_code)]` and `doc` attributes, an attribute on `mod shell` or `mod core` can import shell code into the core",
                ));
            }
            pending_shell_gate =
                allow_display_wasm_shell_gate && (gated_shell_module || gated_shell_use) && ok;
            i = end;
            continue;
        }
        let line = toks.get(i).map_or(0, |t| t.line);
        i = skip_modifiers(&toks, i);
        let kw = s(&toks, i).to_string();
        let semi = (i..toks.len()).find(|&j| s(&toks, j) == ";");
        let shell_gate = std::mem::take(&mut pending_shell_gate);
        match kw.as_str() {
            "mod" if s(&toks, i + 2) == ";" && s(&toks, i + 1) == "core" => core_n += 1,
            "mod" if s(&toks, i + 2) == ";" && s(&toks, i + 1) == "shell" => {
                shell_n += 1;
                shell_module_gated |= shell_gate;
            }
            "use" if semi.is_some() => {
                if s(&toks, i + 1) == "crate"
                    && s(&toks, i + 2) == "::"
                    && s(&toks, i + 3) == "shell"
                {
                    if shell_gate {
                        shell_uses_gated += 1;
                    } else {
                        shell_uses_ungated += 1;
                    }
                }
            }
            _ => {
                out.push(hit(line, &kw, msg));
                break;
            }
        }
        i = semi.map_or(toks.len(), |j| j + 1);
    }
    if core_n != 1 || shell_n != 1 {
        out.push(hit(1, "mod", msg));
    }
    if allow_display_wasm_shell_gate
        && (!shell_module_gated || shell_uses_gated == 0 || shell_uses_ungated != 0)
    {
        out.push(hit(
            1,
            "mod shell",
            "B7: oa-display must gate its shell module and every shell re-export with the exact inverse wasm32-unknown-unknown predicate",
        ));
    }
    out
}

/// The only split manifest allowed to omit its shell for the F2 WASM target is oa-display. Its
/// Unix-only oa-sealed dependency must occur once, under the exact inverse WASM target predicate.
fn display_wasm_shell_manifest_problems(src: &str) -> Vec<String> {
    const TARGET: &str = "[target.'cfg(not(all(target_arch = \"wasm32\", target_pointer_width = \"32\", target_os = \"unknown\", target_vendor = \"unknown\")))'.dependencies]";
    const DEP: &str = "oa-sealed = { path = \"../oa-sealed\" }";
    let mut section = String::new();
    let mut dependency_lines = Vec::new();
    for raw in src.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.to_string();
        } else if line
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == "oa-sealed")
        {
            dependency_lines.push((section.clone(), line.to_string()));
        }
    }
    let manifest = parse_manifest(src);
    let related: Vec<&Dep> = manifest
        .deps
        .iter()
        .filter(|d| {
            d.key.replace('_', "-") == "oa-sealed"
                || d.package.as_deref() == Some("oa-sealed")
                || d.path.as_deref() == Some("../oa-sealed")
        })
        .collect();
    let exact_target_dep = matches!(
        dependency_lines.as_slice(),
        [(section, dep)] if section.as_str() == TARGET && dep.as_str() == DEP
    );
    if exact_target_dep
        && matches!(
            related.as_slice(),
            [d] if d.key == "oa-sealed" && d.path.as_deref() == Some("../oa-sealed")
        )
    {
        Vec::new()
    } else {
        vec!["oa-display: B7: oa-sealed must be declared exactly once under the exact non-WASM target dependency table".to_string()]
    }
}

// ---------------------------------------------------------------------------------------------
// Workspace

/// Every `.rs` file under `dir`, skipping the paths in `skip`. Unreadable entries and symlinks
/// are failures.
fn walk_rs(dir: &Path, skip: &[PathBuf], out: &mut Vec<PathBuf>, fails: &mut Vec<String>) {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            fails.push(format!("{}: cannot read directory: {e}", dir.display()));
            return;
        }
    };
    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                fails.push(format!("{}: cannot read entry: {e}", dir.display()));
                continue;
            }
        };
        let path = entry.path();
        if skip.contains(&path) {
            continue;
        }
        let md = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                fails.push(format!("{}: cannot stat: {e}", path.display()));
                continue;
            }
        };
        if md.file_type().is_symlink() {
            fails.push(format!(
                "{}: symlink (cargo follows it, the scan must not be steered by it)",
                path.display()
            ));
        } else if md.is_dir() {
            walk_rs(&path, skip, out, fails);
        } else if md.is_file() && path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// A7.2: cold struct, size guard waived
struct Member {
    name: String,
    /// Directory relative to the workspace root, normalized.
    rel_dir: String,
    dir: PathBuf,
    class: Class,
    man: Manifest,
}

/// Where rustc looks for `mod x;` declared in `file`, and the directory holding its children.
fn module_candidates(file: &Path, name: &str) -> (Vec<PathBuf>, PathBuf) {
    let parent = file.parent().unwrap_or(Path::new(""));
    let fname = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let base = if matches!(fname, "lib.rs" | "main.rs" | "mod.rs") {
        parent.to_path_buf()
    } else {
        parent.join(file.file_stem().unwrap_or_default())
    };
    (
        vec![
            base.join(format!("{name}.rs")),
            base.join(name).join("mod.rs"),
        ],
        base.join(name),
    )
}

/// Depth of `file` below the crate root for the `super::` rule (`src/core.rs` is 1).
fn module_depth(src: &Path, file: &Path) -> usize {
    let r = file.strip_prefix(src).unwrap_or(file);
    let mut n = r.components().count();
    if r.file_name().is_some_and(|f| f == "mod.rs") {
        n -= 1;
    }
    n.max(1)
}

/// Resolves a dependency to a workspace member name. `Err` if it has a path outside the members.
fn resolve_dep(
    d: &Dep,
    rel_dir: &str,
    ws_deps: &[Dep],
    by_dir: &BTreeMap<String, String>,
    classes: &BTreeMap<String, Class>,
) -> Result<String, String> {
    // `workspace = true` takes the root `[workspace.dependencies]` entry, whose paths are
    // relative to the root
    let (d, base) =
        if d.workspace {
            match ws_deps.iter().find(|w| w.key == d.key) {
                Some(w) => (w, ""),
                None => return Err(
                    "`workspace = true` but the root has no [workspace.dependencies] entry for it"
                        .to_string(),
                ),
            }
        } else {
            (d, rel_dir)
        };
    if let Some(p) = &d.path {
        return normalize(base, p)
            .and_then(|n| by_dir.get(&n).cloned())
            .ok_or_else(|| format!("path dependency {p:?} is not a workspace member"));
    }
    let name = d.package.clone().unwrap_or_else(|| d.key.clone());
    if classes.contains_key(&name) {
        Ok(name)
    } else {
        Err(format!(
            "dependency {name:?} cannot be resolved to a workspace crate (F6: no outside code)"
        ))
    }
}

/// Returns true only for the adjacent machine-readable approval convention used by the F1a
/// metadata check. The Python checker validates the reference against the dependency's exact
/// path/source; this lets the older class checker avoid rejecting an already-approved shell
/// external edge merely because it cannot resolve that package as a workspace member.
fn has_f1_approval(root: &Path, member_dir: &str, dep: &Dep) -> bool {
    let key = &dep.key;
    let package_file = root.join(member_dir).join("Cargo.toml");
    let workspace_file = root.join("Cargo.toml");
    for path in [package_file, workspace_file] {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let mut in_dependency_section = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_dependency_section = matches!(
                    trimmed,
                    "[dependencies]"
                        | "[dev-dependencies]"
                        | "[build-dependencies]"
                        | "[workspace.dependencies]"
                ) || trimmed.ends_with(".dependencies]")
                    || trimmed.ends_with(".dev-dependencies]")
                    || trimmed.ends_with(".build-dependencies]");
                continue;
            }
            if !in_dependency_section {
                continue;
            }
            let Some((name, value)) = trimmed.split_once('=') else {
                continue;
            };
            if name.trim() != key {
                continue;
            }
            let Some((_, comment)) = value.split_once('#') else {
                continue;
            };
            let Some(reference) = comment.trim().strip_prefix("F1-APPROVAL:") else {
                continue;
            };
            let reference = reference.trim();
            if !reference.is_empty()
                && ![
                    "approved",
                    "approval",
                    "justification",
                    "yes",
                    "true",
                    "external",
                ]
                .iter()
                .any(|generic| reference.eq_ignore_ascii_case(generic))
            {
                return true;
            }
        }
    }
    false
}

/// Checks every workspace member under `root` (B7 to B9) against [`UNSPLIT_ALLOWED`].
pub fn check(root: &Path) -> ClassReport {
    check_inner(root, &UNSPLIT_ALLOWED, true)
}

/// [`check`] with another fixed unsplit list (for the fixtures).
pub fn check_with(root: &Path, unsplit_allowed: &[&str]) -> ClassReport {
    check_inner(root, unsplit_allowed, false)
}

fn check_inner(
    root: &Path,
    unsplit_allowed: &[&str],
    enforce_product_exceptions: bool,
) -> ClassReport {
    let mut rep = ClassReport::default();
    let root_toml = root.join("Cargo.toml");
    let text = match fs::read_to_string(&root_toml) {
        Ok(t) => t,
        Err(e) => {
            rep.failures
                .push(format!("{}: cannot read: {e}", root_toml.display()));
            return rep;
        }
    };
    let (member_dirs, bad) = parse_members(&text);
    for b in bad {
        rep.failures.push(format!(
            "Cargo.toml: workspace members entry {b} is not a plain quoted path (globs are not supported)"
        ));
    }
    if member_dirs.is_empty() {
        rep.failures
            .push("Cargo.toml: no workspace members parsed".to_string());
    }
    let (exclude, bad_ex) = parse_workspace_list(&text, "exclude").unwrap_or_default();
    for b in bad_ex {
        rep.failures.push(format!(
            "Cargo.toml: workspace exclude entry {b} is not a plain quoted path"
        ));
    }
    let member_dirs: Vec<String> = member_dirs
        .iter()
        .filter_map(|d| {
            let n = normalize("", d);
            if n.is_none() {
                rep.failures
                    .push(format!("Cargo.toml: member {d:?} leaves the workspace"));
            }
            n
        })
        .collect();
    let exclude: Vec<String> = exclude
        .iter()
        .filter_map(|d| {
            let n = normalize("", d);
            if n.is_none() {
                rep.failures.push(format!(
                    "Cargo.toml: exclude entry {d:?} leaves the workspace"
                ));
            }
            n
        })
        .collect();
    cross_check_dirs(root, &member_dirs, &exclude, &mut rep.failures);

    let mut members = Vec::new();
    for dir in &member_dirs {
        let path = root.join(dir).join("Cargo.toml");
        let t = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                rep.failures
                    .push(format!("{dir}: cannot read Cargo.toml: {e}"));
                continue;
            }
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
        rep.classes
            .insert(name.clone(), class_name(class).to_string());
        if class == Class::Unsplit {
            rep.unsplit.push(name.clone());
        }
        members.push(Member {
            name,
            rel_dir: dir.clone(),
            dir: root.join(dir),
            class,
            man,
        });
    }
    rep.unsplit.sort();
    let classes: BTreeMap<String, Class> =
        members.iter().map(|m| (m.name.clone(), m.class)).collect();
    let libs: BTreeMap<String, String> = members
        .iter()
        .map(|m| {
            let lib = m.man.lib_name.clone().unwrap_or_else(|| m.name.clone());
            (m.name.clone(), lib.replace('-', "_"))
        })
        .collect();
    let ws_deps = parse_manifest(&text).ws_deps;
    let by_dir: BTreeMap<String, String> = members
        .iter()
        .map(|m| (m.rel_dir.clone(), m.name.clone()))
        .collect();

    let (allow, bad_allow, allow_ok) = match fs::read_to_string(root.join(CALLBACK_ALLOWLIST)) {
        Ok(text) => {
            let (entries, bad) = parse_callbacks(&text);
            (entries, bad, true)
        }
        Err(e) => {
            rep.failures
                .push(format!("{CALLBACK_ALLOWLIST}: cannot read: {e}"));
            (Vec::new(), Vec::new(), false)
        }
    };
    for b in bad_allow {
        rep.failures.push(format!(
            "{CALLBACK_ALLOWLIST}: entry {b:?} is not `crate file.rs::fn reason` with a reason"
        ));
    }
    let allow_keys: BTreeSet<(String, String)> = allow
        .iter()
        .map(|e| (e.krate.clone(), e.path.clone()))
        .collect();
    let mut cbs = Callbacks {
        allow: &allow_keys,
        used: BTreeSet::new(),
    };
    let (floats, seals) = workspace_facts(root, &members);
    let view = Workspace {
        classes: &classes,
        by_dir: &by_dir,
        ws_deps: &ws_deps,
        libs: &libs,
        floats: &floats,
        sealed: &seals.sealed,
        sealing: &seals.sealing,
        sealing_mods: &seals.sealing_mods,
        private: &seals.private,
    };
    for m in &members {
        if matches!(m.class, Class::Core | Class::Split) {
            check_member(root, m, &view, &mut cbs, &mut rep.failures);
        }
    }
    let ffi_fixture_member = members
        .iter()
        .any(|member| member.name == FFI_PACKAGE || member.rel_dir == FFI_PACKAGE_DIR);
    if enforce_product_exceptions || ffi_fixture_member {
        check_ffi_a0(root, &members, &mut rep.failures);
    }
    // Even lightweight fixture checks enforce the WASM boundary when a fixture contains that
    // package. Otherwise its positive/negative A0 regressions silently exercise no checker path.
    let wasm_abi_members: Vec<&Member> = members
        .iter()
        .filter(|member| member.name == WASM_ABI_PACKAGE || member.rel_dir == WASM_ABI_DIR)
        .collect();
    if enforce_product_exceptions || !wasm_abi_members.is_empty() {
        if wasm_abi_members.len() != 1 {
            rep.failures.push(format!(
                "A0: expected exactly one `{WASM_ABI_PACKAGE}` shell package at `{WASM_ABI_DIR}`"
            ));
        }
        for member in wasm_abi_members {
            check_wasm_abi_member(root, member, &mut rep.failures);
        }
    }
    if allow_ok {
        for (k, p) in allow_keys.difference(&cbs.used) {
            rep.failures.push(format!(
                "{CALLBACK_ALLOWLIST}: {k} {p}: B5: listed but no core `pub fn` of that name takes a callback or bound there; remove the entry"
            ));
        }
    }
    float_budget(root, &members, &mut rep);
    sign_order(root, &members, &mut rep);

    // B9: the unsplit list is fixed in this source and only shrinks
    for name in &rep.unsplit {
        if !unsplit_allowed.contains(&name.as_str()) {
            rep.failures.push(format!(
                "{name}: B9: new unsplit crate; the fixed list in classcheck.rs does not hold it. Split it or classify it"
            ));
        }
    }
    match fs::read_to_string(root.join(ALLOWLIST)) {
        Ok(text) => {
            for l in text.lines() {
                let name = l.split('#').next().unwrap_or("").trim();
                if name.is_empty() {
                    continue;
                }
                if !unsplit_allowed.contains(&name) {
                    rep.failures.push(format!(
                        "{ALLOWLIST}: {name}: B9: not in the fixed list in classcheck.rs; the list cannot grow"
                    ));
                }
                if !rep.unsplit.iter().any(|u| u == name) {
                    rep.failures.push(format!(
                        "{ALLOWLIST}: {name}: B9: listed but not unsplit (migrated or unknown); remove the entry"
                    ));
                }
            }
        }
        Err(e) => rep.failures.push(format!("{ALLOWLIST}: cannot read: {e}")),
    }
    rep.failures.sort();
    rep.failures.dedup();
    rep
}

/// RULESET A0's narrowly scoped provisional FFI exception: one exact shell package and module,
/// exact `src/` inventory, and a local `// SAFETY:` explanation for each unsafe block.
fn check_ffi_a0(root: &Path, members: &[Member], fails: &mut Vec<String>) {
    let package_dir = root.join(FFI_PACKAGE_DIR);
    let metadata = match fs::symlink_metadata(&package_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            fails.push(format!(
                "{FFI_PACKAGE_DIR}: A0: cannot inspect binding package: {error}"
            ));
            return;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}: A0: binding package path must be a real directory"
        ));
        return;
    }
    let candidates: Vec<&Member> = members
        .iter()
        .filter(|member| member.name == FFI_PACKAGE || member.rel_dir == FFI_PACKAGE_DIR)
        .collect();
    if candidates.len() != 1 {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}: A0: the provisional FFI exception must name exactly one workspace package"
        ));
        return;
    }
    let Some(member) = candidates.first().copied() else {
        return;
    };
    if member.name != FFI_PACKAGE
        || member.rel_dir != FFI_PACKAGE_DIR
        || member.class != Class::Shell
    {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}: A0: the exception is only `openatca-core-ffi` classified as shell at this exact path"
        ));
    }
    if member.man.lints_workspace || !member.man.lints_unsafe_deny {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}: A0: the binding package locally sets `[lints.rust] unsafe_code = \"deny\"` and does not inherit workspace `forbid`"
        ));
    }
    let required_clippy: BTreeSet<String> =
        ["unwrap_used", "expect_used", "panic", "indexing_slicing"]
            .into_iter()
            .map(str::to_string)
            .collect();
    if !required_clippy.is_subset(&member.man.clippy_denied) {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}: A0: all four workspace runtime Clippy lints remain denied"
        ));
    }
    let dependencies_match = member.man.deps.len() == FFI_DEPENDENCIES.len()
        && FFI_DEPENDENCIES.iter().all(|(key, path)| {
            member.man.deps.iter().any(|dependency| {
                dependency.key == *key
                    && dependency.path.as_deref() == Some(*path)
                    && dependency.package.is_none()
                    && !dependency.workspace
            })
        });
    if !dependencies_match {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}: A0/F1: the binding shell has exactly the reviewed direct dependencies"
        ));
    }

    let source_dir = package_dir.join("src");
    match fs::symlink_metadata(&source_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            fails.push(format!(
                "{FFI_PACKAGE_DIR}/src: A0: source directory must be a real directory"
            ));
            return;
        }
        Ok(_) => {}
        Err(error) => {
            fails.push(format!(
                "{FFI_PACKAGE_DIR}/src: A0: cannot inspect source directory: {error}"
            ));
            return;
        }
    }
    let entries = match fs::read_dir(&source_dir) {
        Ok(entries) => entries,
        Err(error) => {
            fails.push(format!(
                "{FFI_PACKAGE_DIR}/src: A0: cannot read source inventory: {error}"
            ));
            return;
        }
    };
    let mut names = Vec::new();
    let mut formats_dir_present = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                fails.push(format!(
                    "{FFI_PACKAGE_DIR}/src: A0: cannot read inventory entry: {error}"
                ));
                continue;
            }
        };
        let path = entry.path();
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        let entry_type = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata.file_type(),
            Err(error) => {
                fails.push(format!(
                    "{}: A0: cannot inspect source entry: {error}",
                    rel(root, &path)
                ));
                continue;
            }
        };
        if entry_name == "formats" {
            if entry_type.is_symlink() || !entry_type.is_dir() {
                fails.push(format!(
                    "{}: A0: the reviewed safe formats directory must be a real directory",
                    rel(root, &path)
                ));
            } else {
                formats_dir_present = true;
            }
        } else if entry_type.is_symlink() || !entry_type.is_file() {
            fails.push(format!(
                "{}: A0: source inventory contains a non-file or symlink",
                rel(root, &path)
            ));
        }
        names.push(entry_name);
    }
    names.sort();
    let expected_source_entries = BTreeSet::from([
        "ffi.rs".to_string(),
        "formats".to_string(),
        "lib.rs".to_string(),
        "text.rs".to_string(),
    ]);
    if names.iter().cloned().collect::<BTreeSet<_>>() != expected_source_entries
        || names.len() != expected_source_entries.len()
        || !formats_dir_present
    {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}/src: A0: Rust source inventory is exactly `ffi.rs`, `lib.rs`, `text.rs`, and the reviewed `formats/` directory"
        ));
    }

    let formats_dir = source_dir.join("formats");
    let mut format_files = Vec::new();
    let mut invalid_format_files = BTreeSet::new();
    let format_entries = if formats_dir_present {
        fs::read_dir(&formats_dir).map(Some)
    } else {
        Ok(None)
    };
    match format_entries {
        Ok(Some(entries)) => {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        fails.push(format!("{FFI_PACKAGE_DIR}/src/formats: A0: cannot read inventory entry: {error}"));
                        continue;
                    }
                };
                let path = entry.path();
                match fs::symlink_metadata(&path) {
                    Ok(metadata)
                        if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
                    }
                    Ok(_) => {
                        fails.push(format!(
                            "{}: A0: source inventory contains a non-file or symlink",
                            rel(root, &path)
                        ));
                        invalid_format_files
                            .insert(entry.file_name().to_string_lossy().into_owned());
                    }
                    Err(error) => {
                        fails.push(format!(
                            "{}: A0: cannot inspect source entry: {error}",
                            rel(root, &path)
                        ));
                        invalid_format_files
                            .insert(entry.file_name().to_string_lossy().into_owned());
                    }
                }
                format_files.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(None) => {}
        Err(error) => fails.push(format!(
            "{FFI_PACKAGE_DIR}/src/formats: A0: cannot read source inventory: {error}"
        )),
    }
    format_files.sort();
    if format_files
        != [
            "asterix.rs",
            "cifp.rs",
            "grib2.rs",
            "mod.rs",
            "notam.rs",
            "zip.rs",
        ]
    {
        fails.push(format!(
            "{FFI_PACKAGE_DIR}/src/formats: A0: safe format inventory is exactly `asterix.rs`, `cifp.rs`, `grib2.rs`, `mod.rs`, `notam.rs`, and `zip.rs`"
        ));
    }
    for (file, expected_level) in [
        ("lib.rs", "deny"),
        ("ffi.rs", "allow"),
        ("text.rs", "forbid"),
        ("formats/asterix.rs", "forbid"),
        ("formats/cifp.rs", "forbid"),
        ("formats/grib2.rs", "forbid"),
        ("formats/notam.rs", "forbid"),
        ("formats/mod.rs", "forbid"),
        ("formats/zip.rs", "forbid"),
    ] {
        if file.starts_with("formats/")
            && invalid_format_files.contains(file.trim_start_matches("formats/"))
        {
            continue;
        }
        let path = source_dir.join(file);
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                fails.push(format!(
                    "{}: A0: cannot read binding source: {error}",
                    rel(root, &path)
                ));
                continue;
            }
        };
        let tokenized = tokenize(&mask(&source));
        let source_injection = tokenized.windows(2).any(|window| {
            token_pair_is(window, "include", "!") || token_pair_is(window, "macro_rules", "!")
        }) || tokenized.windows(3).any(|window| {
            token_triple_is(window, "#", "[", &["path", "macro_use", "macro_export"])
        }) || tokenized.iter().any(|token| token.s == "path");
        if source_injection {
            fails.push(format!(
                "{}: A0: `include!`, `#[path]`, and source-injecting macros are refused in the fixed binding source inventory",
                rel(root, &path)
            ));
        }
        let expected_count =
            lint_attribute_count(&tokenized, expected_level, "unsafe_code", Some(true));
        let other_level = if expected_level == "allow" {
            "deny"
        } else {
            "allow"
        };
        let other_count = lint_attribute_count(&tokenized, other_level, "unsafe_code", None);
        if expected_count != 1 || other_count != 0 {
            fails.push(format!(
                "{}: A0: `{file}` has exactly one inner `#![{expected_level}(unsafe_code)]` and no opposite unsafe lint attribute",
                rel(root, &path)
            ));
        }
        if file == "lib.rs" {
            let has_module = tokenized
                .windows(3)
                .any(|window| token_triple_is(window, "mod", "ffi", &[";"]));
            if !has_module {
                fails.push(format!(
                    "{}: A0: crate root declares the audited `ffi` module",
                    rel(root, &path)
                ));
            }
            let has_text_module = tokenized
                .windows(3)
                .any(|window| token_triple_is(window, "mod", "text", &[";"]));
            if !has_text_module {
                fails.push(format!(
                    "{FFI_PACKAGE_DIR}: A0: crate root declares the audited safe `text` adapter module"
                ));
            }
            let has_formats_module = tokenized
                .windows(3)
                .any(|window| token_triple_is(window, "mod", "formats", &[";"]));
            if !has_formats_module {
                fails.push(format!(
                    "{FFI_PACKAGE_DIR}: A0: crate root declares the reviewed safe `formats` adapter module"
                ));
            }
            if tokenized.iter().any(|token| token.s == "unsafe") {
                fails.push(format!(
                    "{}: A0: unsafe operations and declarations stay out of the crate root",
                    rel(root, &path)
                ));
            }
        } else if file == "formats/mod.rs" {
            let declared_modules: BTreeSet<String> = tokenized
                .windows(3)
                .filter_map(|window| {
                    (window.first().is_some_and(|token| token.s == "mod")
                        && window.get(2).is_some_and(|token| token.s == ";"))
                    .then(|| window.get(1).map(|token| token.s.clone()))
                    .flatten()
                })
                .collect();
            if declared_modules
                != BTreeSet::from([
                    "asterix".to_string(),
                    "cifp".to_string(),
                    "grib2".to_string(),
                    "notam".to_string(),
                    "zip".to_string(),
                ])
            {
                fails.push(format!(
                    "{}: A0: safe formats module declares exactly the reviewed ASTERIX, CIFP, GRIB2, NOTAM, and ZIP sources",
                    rel(root, &path)
                ));
            }
            if tokenized.iter().any(|token| {
                matches!(
                    token.s.as_str(),
                    "unsafe" | "extern" | "GlobalAlloc" | "no_mangle" | "export_name"
                )
            }) {
                fails.push(format!(
                    "{}: A0: safe formats module contains no unsafe, ABI, allocator, or export tokens",
                    rel(root, &path)
                ));
            }
        } else if file == "formats/asterix.rs"
            || file == "formats/cifp.rs"
            || file == "formats/grib2.rs"
            || file == "formats/notam.rs"
            || file == "formats/zip.rs"
        {
            if tokenized.iter().any(|token| {
                matches!(
                    token.s.as_str(),
                    "unsafe" | "extern" | "GlobalAlloc" | "no_mangle" | "export_name"
                )
            }) {
                fails.push(format!(
                    "{}: A0: safe format adapter contains no unsafe operations, ABI declarations, allocators, or exports",
                    rel(root, &path)
                ));
            }
        } else if file == "ffi.rs" {
            let raw_lines: Vec<&str> = source.lines().collect();
            let expected_exports: BTreeSet<String> =
                FFI_EXPORTS.iter().map(|name| (*name).to_string()).collect();
            let export_count = tokenized.iter().filter(|token| token.s == "extern").count();
            let no_mangle_count = tokenized
                .iter()
                .filter(|token| token.s == "no_mangle")
                .count();
            let mut c_exports = BTreeSet::new();
            if let Some(kept) = mask_keep_strings(&source) {
                let c_tokens = tokenize(&kept);
                for (index, _) in c_tokens
                    .iter()
                    .enumerate()
                    .filter(|(_, token)| token.s == "extern")
                {
                    let is_c_function = c_tokens.get(index + 1).map(|next| next.s.as_str())
                        == Some("\"")
                        && c_tokens.get(index + 2).map(|next| next.s.as_str()) == Some("C")
                        && c_tokens.get(index + 3).map(|next| next.s.as_str()) == Some("\"")
                        && c_tokens.get(index + 4).map(|next| next.s.as_str()) == Some("fn");
                    let name = c_tokens.get(index + 5).map(|name| name.s.clone());
                    if is_c_function
                        && name
                            .as_deref()
                            .is_some_and(|name| expected_exports.contains(name))
                    {
                        if let Some(name) = name {
                            c_exports.insert(name);
                        }
                    }
                }
            }
            let mut mangled_exports = BTreeSet::new();
            for (index, _) in tokenized
                .iter()
                .enumerate()
                .filter(|(_, token)| token.s == "no_mangle")
            {
                let end = (index + 16).min(tokenized.len());
                let function = tokenized.get(index + 1..end).and_then(|tokens| {
                    tokens
                        .windows(2)
                        .find(|window| window.first().is_some_and(|token| token.s == "fn"))
                });
                if let Some(name) = function.and_then(|function| function.get(1)) {
                    if expected_exports.contains(&name.s) {
                        mangled_exports.insert(name.s.clone());
                    }
                }
            }
            let forbidden_injection = tokenized.windows(2).any(|window| {
                token_pair_is(window, "include", "!") || token_pair_is(window, "macro_rules", "!")
            }) || tokenized.windows(3).any(|window| {
                token_triple_is(window, "#", "[", &["path", "macro_use", "macro_export"])
            }) || tokenized.iter().any(|token| token.s == "path");
            if export_count != expected_exports.len()
                || no_mangle_count != expected_exports.len()
                || c_exports != expected_exports
                || mangled_exports != expected_exports
                || tokenized.iter().any(|token| {
                    matches!(
                        token.s.as_str(),
                        "export_name" | "link_name" | "link_section" | "link"
                    )
                })
                || forbidden_injection
            {
                fails.push(format!(
                    "{}: A0: exports, ABI strings and source injection are restricted to the exact named provisional function inventory",
                    rel(root, &path)
                ));
            }
            for (index, token) in tokenized
                .iter()
                .enumerate()
                .filter(|(_, token)| token.s == "unsafe")
            {
                let next = tokenized.get(index + 1).map(|next| next.s.as_str());
                let is_export = next == Some("extern")
                    && tokenized.get(index + 2).map(|next| next.s.as_str()) == Some("fn")
                    && tokenized.get(index + 3).is_some_and(|name| {
                        matches!(
                            name.s.as_str(),
                            name if FFI_EXPORTS.contains(&name) && name != "oa_core_abi_version"
                        )
                    });
                if is_export {
                    continue;
                }
                if next != Some("{") {
                    fails.push(format!(
                        "{}:{}: A0: only the pointer-bearing C exports and their small unsafe blocks are permitted in the audited FFI module",
                        rel(root, &path), token.line
                    ));
                    continue;
                }
                if unsafe_block_owner(&tokenized, index).is_none() {
                    fails.push(format!(
                        "{}:{}: A0: every unsafe block belongs to a named reviewed function",
                        rel(root, &path),
                        token.line
                    ));
                }
                if !has_preceding_safety_comment(&raw_lines, token.line) {
                    fails.push(format!(
                        "{}:{}: A0: every unsafe block has an immediately preceding `// SAFETY:` justification",
                        rel(root, &path), token.line
                    ));
                }
            }
            let actual_unsafe_blocks = unsafe_block_inventory(&tokenized);
            let expected_unsafe_blocks: BTreeMap<String, usize> = FFI_UNSAFE_BLOCKS
                .iter()
                .map(|(name, count)| ((*name).to_string(), *count))
                .collect();
            if actual_unsafe_blocks != expected_unsafe_blocks {
                fails.push(format!(
                    "{}: A0: unsafe block inventory by owning function differs from the reviewed list (actual {actual_unsafe_blocks:?}, expected {expected_unsafe_blocks:?})",
                    rel(root, &path),
                ));
            }
            if tokenized.iter().any(|token| token.s == "GlobalAlloc") {
                fails.push(format!(
                    "{}: A0: product source may not name `GlobalAlloc`",
                    rel(root, &path)
                ));
            }
        } else if tokenized.iter().any(|token| {
            matches!(
                token.s.as_str(),
                "unsafe"
                    | "extern"
                    | "GlobalAlloc"
                    | "no_mangle"
                    | "export_name"
                    | "link_name"
                    | "link_section"
            )
        }) {
            fails.push(format!(
                "{}: A0: safe adapter contains no unsafe, ABI, allocator, or linker tokens",
                rel(root, &path)
            ));
        }
    }
}

fn token_pair_is(tokens: &[Tok], first: &str, second: &str) -> bool {
    tokens.first().is_some_and(|token| token.s == first)
        && tokens.get(1).is_some_and(|token| token.s == second)
}

fn token_triple_is(tokens: &[Tok], first: &str, second: &str, third: &[&str]) -> bool {
    tokens.first().is_some_and(|token| token.s == first)
        && tokens.get(1).is_some_and(|token| token.s == second)
        && tokens
            .get(2)
            .is_some_and(|token| third.contains(&token.s.as_str()))
}

fn has_preceding_safety_comment(lines: &[&str], unsafe_line: usize) -> bool {
    let Some(mut index) = unsafe_line.checked_sub(2) else {
        return false;
    };
    let mut found_marker = false;
    loop {
        let Some(line) = lines.get(index) else {
            return false;
        };
        let comment = line.trim_start();
        if !comment.starts_with("//") {
            break;
        }
        found_marker |= comment.starts_with("// SAFETY:");
        if index == 0 {
            break;
        }
        index -= 1;
    }
    found_marker
}

/// Returns the owning function for an `unsafe { ... }` token. FFI source deliberately uses
/// ordinary function bodies and balanced braces; this small lexical walk does not try to parse
/// Rust, and fails closed when a block is outside a function.
fn unsafe_block_owner(tokens: &[Tok], unsafe_index: usize) -> Option<String> {
    let mut owners: Vec<Option<String>> = Vec::new();
    let mut pending_function: Option<String> = None;
    for (index, token) in tokens.iter().enumerate().take(unsafe_index + 2) {
        if token.s == "fn" {
            pending_function = tokens.get(index + 1).map(|name| name.s.clone());
        }
        match token.s.as_str() {
            "{" => owners.push(
                pending_function
                    .take()
                    .or_else(|| owners.last().cloned().flatten()),
            ),
            "}" => {
                owners.pop();
                pending_function = None;
            }
            _ => {}
        }
    }
    owners.last().cloned().flatten()
}

fn unsafe_block_inventory(tokens: &[Tok]) -> BTreeMap<String, usize> {
    let mut inventory = BTreeMap::new();
    for (index, _) in tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.s == "unsafe")
    {
        if tokens.get(index + 1).is_some_and(|next| next.s == "{") {
            if let Some(owner) = unsafe_block_owner(tokens, index) {
                *inventory.entry(owner).or_insert(0) += 1;
            }
        }
    }
    inventory
}

fn lint_attribute_count(tokens: &[Tok], level: &str, lint: &str, inner: Option<bool>) -> usize {
    let mut count = 0;
    for index in 0..tokens.len() {
        if s(tokens, index) == "#" {
            let has_inner = s(tokens, index + 1) == "!";
            let mut start = index + 1;
            if has_inner {
                start += 1;
            }
            if inner.is_none_or(|required| required == has_inner)
                && s(tokens, start) == "["
                && s(tokens, start + 1) == level
                && s(tokens, start + 2) == "("
                && s(tokens, start + 3) == lint
                && s(tokens, start + 4) == ")"
                && s(tokens, start + 5) == "]"
            {
                count += 1;
            }
        }
    }
    count
}

/// The file `crates/oa-math/src/order.rs`, the one place the standard library's sign and order
/// reads are allowed (RULESET B8).
const ORDER_RS: &str = "crates/oa-math/src/order.rs";

/// RULESET B8, OA-TRC-260: every crate of the workspace, whatever its class, reads the sign and
/// order of a float through `oa_math`. Source under `src`, `examples` and `benches`, outside test
/// code; `tests` directories are not scanned. A site the class check of core code already
/// reports is not reported twice.
fn sign_order(root: &Path, members: &[Member], rep: &mut ClassReport) {
    let seen: BTreeSet<String> = rep
        .failures
        .iter()
        .filter_map(|f| {
            let mut it = f.splitn(3, ':');
            Some(format!(
                "{}:{}:{}",
                it.next()?,
                it.next()?,
                it.next()?.trim()
            ))
        })
        .collect();
    for m in members {
        let mut files = Vec::new();
        let mut fails = Vec::new();
        for d in ["src", "examples", "benches"] {
            let dir = m.dir.join(d);
            if dir.is_dir() {
                walk_rs(&dir, &[], &mut files, &mut fails);
            }
        }
        rep.failures.extend(fails);
        files.sort();
        let mut scanned = Vec::new();
        for f in &files {
            if rel(root, f) == ORDER_RS {
                continue;
            }
            match fs::read_to_string(f) {
                Ok(t) => scanned.push((f.clone(), harden::sign_order_hits(&t))),
                Err(e) => rep
                    .failures
                    .push(format!("{}: cannot read: {e}", rel(root, f))),
            }
        }
        let skips = TestSkips::new(scanned.iter().map(|(f, (_, tm, whole))| (f, tm, *whole)));
        for (f, (hits, _, _)) in &scanned {
            if skips.skips(f) {
                continue;
            }
            for h in hits {
                let key = format!("{}:{}:{}", rel(root, f), h.line, h.token);
                if !seen.iter().any(|s| s.starts_with(&key)) {
                    rep.failures.push(format!(
                        "{}:{}: {}: {}",
                        rel(root, f),
                        h.line,
                        h.token,
                        h.message
                    ));
                }
            }
        }
    }
}

/// The ratchet on unsplit crates (RULESET B9): platform float-method calls, counted per crate
/// outside test code, may not rise above the number recorded in [`FLOAT_BUDGET`].
fn float_budget(root: &Path, members: &[Member], rep: &mut ClassReport) {
    let text = match fs::read_to_string(root.join(FLOAT_BUDGET)) {
        Ok(t) => t,
        Err(e) => {
            rep.failures
                .push(format!("{FLOAT_BUDGET}: cannot read: {e}"));
            return;
        }
    };
    let (budget, bad) = parse_budget(&text);
    for b in bad {
        rep.failures
            .push(format!("{FLOAT_BUDGET}: entry {b:?} is not `crate count`"));
    }
    for m in members.iter().filter(|m| m.class == Class::Unsplit) {
        let mut files = Vec::new();
        let mut fails = Vec::new();
        walk_rs(&m.dir.join("src"), &[], &mut files, &mut fails);
        rep.failures.extend(fails);
        files.sort();
        let mut counted = Vec::new();
        for f in &files {
            match fs::read_to_string(f) {
                Ok(t) => counted.push((f.clone(), count_float_calls(&t))),
                Err(e) => rep
                    .failures
                    .push(format!("{}: cannot read: {e}", rel(root, f))),
            }
        }
        let skips = TestSkips::new(
            counted
                .iter()
                .map(|(f, (_, sc))| (f, &sc.test_mods, sc.whole_file)),
        );
        let n: usize = counted
            .iter()
            .filter(|(f, _)| !skips.skips(f))
            .map(|(_, (n, _))| *n)
            .sum();
        let allowed = budget.get(&m.name).copied();
        match allowed {
            None => rep.failures.push(format!(
                "{}: B9: no entry in {FLOAT_BUDGET}; an unsplit crate records its float-method count ({n})",
                m.name
            )),
            Some(b) if n > b => rep.failures.push(format!(
                "{}: B9: {n} platform float-method calls, the budget is {b}; the count only falls (use oa_math or oa_ieee, or split the crate)",
                m.name
            )),
            Some(_) => {}
        }
        rep.float_counts.insert(m.name.clone(), (n, allowed));
    }
    for k in budget.keys() {
        if !members
            .iter()
            .any(|m| m.class == Class::Unsplit && &m.name == k)
        {
            rep.failures.push(format!(
                "{FLOAT_BUDGET}: {k}: B9: listed but not unsplit (migrated or unknown); remove the entry"
            ));
        }
    }
}

/// Every Cargo.toml under `crates/`, `tools/` and `validation/` belongs to a member or is excluded.
fn cross_check_dirs(root: &Path, members: &[String], exclude: &[String], fails: &mut Vec<String>) {
    for base in ["crates", "tools", "validation"] {
        let rd = match fs::read_dir(root.join(base)) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                fails.push(format!("{base}: cannot read directory: {e}"));
                continue;
            }
        };
        for entry in rd {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    fails.push(format!("{base}: cannot read entry: {e}"));
                    continue;
                }
            };
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(md) if md.is_dir() => {}
                Ok(_) => continue,
                Err(e) => {
                    fails.push(format!("{}: cannot stat: {e}", path.display()));
                    continue;
                }
            }
            if !path.join("Cargo.toml").exists() {
                continue;
            }
            let r = rel(root, &path);
            let excluded = exclude
                .iter()
                .any(|x| r == *x || r.starts_with(&format!("{x}/")));
            if !members.contains(&r) && !excluded {
                fails.push(format!(
                    "{r}: has a Cargo.toml but is neither a workspace member nor in `exclude`"
                ));
            }
        }
    }
}

/// What every member check reads of the workspace.
struct Workspace<'a> {
    classes: &'a BTreeMap<String, Class>,
    by_dir: &'a BTreeMap<String, String>,
    ws_deps: &'a [Dep],
    libs: &'a BTreeMap<String, String>,
    /// Float facts of every core file in the workspace.
    floats: &'a FloatFacts,
    /// Sealed `pub trait` names of every core crate, and the traits that seal them.
    sealed: &'a BTreeSet<String>,
    sealing: &'a BTreeSet<String>,
    /// Names of the modules that hold a sealing trait.
    sealing_mods: &'a BTreeSet<String>,
    /// Non-public module names, per crate.
    private: &'a BTreeMap<String, BTreeSet<String>>,
}

/// The reviewed core callback entries and which of them a core file used (B5).
struct Callbacks<'a> {
    allow: &'a BTreeSet<(String, String)>,
    used: BTreeSet<(String, String)>,
}

/// The core files, the shell files and the read failures of a core or split crate, sorted.
fn gather_files(root: &Path, m: &Member) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<String>) {
    let src = m.dir.join("src");
    let mut files = Vec::new();
    let mut shell_files = Vec::new();
    let mut walk_fails = Vec::new();
    if m.class == Class::Split {
        check_split_layout(root, m, &mut files, &mut shell_files, &mut walk_fails);
    } else {
        let skip = [src.join("bin"), src.join("main.rs")];
        walk_rs(&src, &skip, &mut files, &mut walk_fails);
    }
    files.sort();
    files.dedup();
    shell_files.sort();
    shell_files.dedup();
    (files, shell_files, walk_fails)
}

/// The module path of a source file: `src/a/b.rs` and `src/a/b/mod.rs` are `["a", "b"]`, `lib.rs`
/// is empty, and the `core` root of a split crate is empty too (`src/core.rs`, `src/core/mod.rs`).
fn file_module_path(src: &Path, file: &Path, split: bool) -> Vec<String> {
    let rel = file.strip_prefix(src).unwrap_or(file);
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = parts.pop() {
        let stem = last.strip_suffix(".rs").unwrap_or(&last).to_string();
        if !matches!(stem.as_str(), "lib" | "mod") {
            parts.push(stem);
        }
    }
    if split && parts.first().is_some_and(|p| p == "core") {
        parts.remove(0);
    }
    parts
}

/// The facts of the sealed `pub trait`s and non-public modules of every core crate (B5).
struct Seals {
    sealed: BTreeSet<String>,
    sealing: BTreeSet<String>,
    sealing_mods: BTreeSet<String>,
    private: BTreeMap<String, BTreeSet<String>>,
}

/// What every core file of the workspace declares that a rule about one file needs to know: float
/// fields, tuple fields, return types and constants (B8), the sealed `pub trait`s and the
/// non-public modules (B5).
fn workspace_facts(root: &Path, members: &[Member]) -> (FloatFacts, Seals) {
    let mut floats = FloatFacts::default();
    let mut seals = Seals {
        sealed: BTreeSet::new(),
        sealing: BTreeSet::new(),
        sealing_mods: BTreeSet::new(),
        private: BTreeMap::new(),
    };
    for m in members {
        if !matches!(m.class, Class::Core | Class::Split) {
            continue;
        }
        // read failures are reported by `check_member`, which reads the same files
        let (files, _, _) = gather_files(root, m);
        let src = m.dir.join("src");
        let split = m.class == Class::Split;
        let texts: Vec<(Vec<String>, String)> = files
            .iter()
            .filter_map(|f| {
                fs::read_to_string(f)
                    .ok()
                    .map(|t| (file_module_path(&src, f, split), t))
            })
            .collect();
        for (_, text) in &texts {
            floats.add_source(text);
        }
        let borrowed: Vec<(Vec<String>, &str)> =
            texts.iter().map(|(p, t)| (p.clone(), t.as_str())).collect();
        let (mut private, public) = module_visibility(&borrowed);
        private.retain(|n| !public.contains(n));
        for (_, text) in &texts {
            let (a, b, c) = sealed_traits(text, &private);
            seals.sealed.extend(a);
            seals.sealing.extend(b);
            seals.sealing_mods.extend(c);
        }
        seals.private.insert(m.name.clone(), private);
    }
    (floats, seals)
}

/// B7 to B9 checks of one core or split crate.
const WASM_ABI_PACKAGE: &str = "oa-demo-wasm";
const WASM_ABI_DIR: &str = "tools/oa-demo-wasm";
// Exact reviewed build-time I/O shape: one guarded input descriptor, no other input/process
// effects, and only the two generated outputs written under Cargo's OUT_DIR.
const EXPECTED_WASM_BUILD: &str = include_str!("classcheck/oa_demo_wasm_build.expected");
const EXPECTED_WASM_ABI_SOURCE: &str = include_str!("classcheck/oa_demo_wasm_abi.expected");
const EXPECTED_WASM_ABI_LIB: &str = r#"//! Host shell for the fixed-width F2 demo ABI. This crate supports native 64-bit parity tools and
//! the wasm32 browser target; `f2_frame` checks address conversion rather than truncating it.

#![deny(unsafe_code)]

#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!("oa-demo-wasm supports only 64-bit native hosts and wasm32-unknown-unknown");

// The native workspace builds the shell package without pretending to provide browser
// exports or embedding an arbitrary asset. The scalar ABI exists only for the exact
// browser target; its build script pins and embeds the real OASD bundle there.
#[cfg(all(
    target_arch = "wasm32",
    target_pointer_width = "32",
    target_os = "unknown",
    target_vendor = "unknown"
))]
mod abi;
"#;

/// The fixed A0 exception for the browser's scalar-only ABI. Any source or manifest expansion
/// requires a corresponding review of this inventory and the RULESET exception.
fn check_wasm_abi_member(root: &Path, member: &Member, fails: &mut Vec<String>) {
    let name = &member.name;
    if name != WASM_ABI_PACKAGE || member.rel_dir != WASM_ABI_DIR || member.class != Class::Shell {
        fails.push(format!(
            "{name}: A0: the oa-demo-wasm ABI exception is only `{WASM_ABI_PACKAGE}` at `{WASM_ABI_DIR}` classified as shell"
        ));
    }

    let expected_manifest = [
        "[package]",
        "name = \"oa-demo-wasm\"",
        "version.workspace = true",
        "edition.workspace = true",
        "license.workspace = true",
        "rust-version.workspace = true",
        "authors.workspace = true",
        "publish = false",
        "build = \"build.rs\"",
        "[package.metadata.openatca]",
        "class = \"shell\"",
        "[lib]",
        "crate-type = [\"cdylib\"]",
        "[dependencies]",
        "oa-demo-engine = { path = \"../../crates/oa-demo-engine\" }",
        "[build-dependencies]",
        "# F1: build-time only guarded input access; no oa-sealed dependency enters the WASM/runtime graph.",
        "oa-sealed = { path = \"../../crates/oa-sealed\" }",
        "oa-sha256 = { path = \"../../crates/oa-sha256\" }",
        "[lints.rust]",
        "unsafe_code = \"deny\"",
        "[lints.clippy]",
        "unwrap_used = \"deny\"",
        "expect_used = \"deny\"",
        "panic = \"deny\"",
        "indexing_slicing = \"deny\"",
    ];
    let manifest_path = member.dir.join("Cargo.toml");
    match fs::read_to_string(&manifest_path) {
        Ok(text) => {
            let lines: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            if lines.as_slice() != expected_manifest.as_slice() {
                fails.push(format!(
                    "{name}: A0: Cargo.toml differs from the exact reviewed manifest/dependency/lint inventory"
                ));
            }
            let parsed = parse_manifest(&text);
            if parsed.name.as_deref() != Some(WASM_ABI_PACKAGE)
                || parsed.class.as_deref() != Some("shell")
                || parsed.lints_workspace
                || parsed.deps.len() != 3
            {
                fails.push(format!(
                    "{name}: A0: package class, local lint policy, or dependency count changed"
                ));
            }
        }
        Err(error) => fails.push(format!(
            "{name}: A0: cannot read `{}`: {error}",
            rel(root, &manifest_path)
        )),
    }

    let mut rust_files = Vec::new();
    let mut walk_failures = Vec::new();
    walk_rs(&member.dir, &[], &mut rust_files, &mut walk_failures);
    fails.extend(walk_failures);
    let mut inventory: Vec<String> = rust_files.iter().map(|path| rel(root, path)).collect();
    inventory.sort();
    let mut expected_inventory = vec![
        format!("{WASM_ABI_DIR}/build.rs"),
        format!("{WASM_ABI_DIR}/src/abi.rs"),
        format!("{WASM_ABI_DIR}/src/lib.rs"),
    ];
    expected_inventory.sort();
    if inventory != expected_inventory {
        fails.push(format!(
            "{name}: A0: Rust source inventory must be exactly `{WASM_ABI_DIR}/src/lib.rs` and `{WASM_ABI_DIR}/src/abi.rs`"
        ));
    }

    let lib_path = member.dir.join("src/lib.rs");
    let abi_path = member.dir.join("src/abi.rs");
    let build_path = member.dir.join("build.rs");
    let lib = fs::read_to_string(&lib_path);
    let abi = fs::read_to_string(&abi_path);
    let build = fs::read_to_string(&build_path);
    match (&lib, &abi, &build) {
        (Ok(lib), Ok(abi), Ok(build)) => {
            if lib != EXPECTED_WASM_ABI_LIB {
                fails.push(format!(
                    "{name}: A0/B10: lib.rs differs from the exact native64/wasm32-unknown-unknown guard and private-module declaration"
                ));
            }
            if abi != EXPECTED_WASM_ABI_SOURCE {
                fails.push(format!(
                    "{name}: A0: abi.rs differs from the reviewed fixed scalar ABI source snapshot"
                ));
            }
            if build != EXPECTED_WASM_BUILD {
                fails.push(format!(
                    "{name}: A0: build.rs differs from the reviewed guarded two-output source snapshot"
                ));
            }
        }
        _ => {
            for (path, result) in [(&lib_path, &lib), (&abi_path, &abi), (&build_path, &build)] {
                if let Err(error) = result {
                    fails.push(format!(
                        "{name}: A0: cannot read `{}`: {error}",
                        rel(root, path)
                    ));
                }
            }
        }
    }
}

fn check_member(
    root: &Path,
    m: &Member,
    w: &Workspace<'_>,
    cbs: &mut Callbacks<'_>,
    fails: &mut Vec<String>,
) {
    let name = &m.name;
    if m.man.lib_path || !m.man.bin_paths.is_empty() {
        fails.push(format!(
            "{name}: B8: no custom [lib] or [[bin]] path in a core or split crate"
        ));
    }
    if m.man.build_key || m.dir.join("build.rs").exists() {
        fails.push(format!("{name}: B8: no build.rs in a core or split crate"));
    }
    // dependencies: resolved by path (a `workspace = true` entry through the root table), then
    // package, then key; one that cannot be resolved fails. Source names a crate by its `[lib]`
    // name, or by the dependency key when renamed.
    let mut aliases: BTreeMap<String, String> =
        w.libs.iter().map(|(n, l)| (l.clone(), n.clone())).collect();
    for d in &m.man.deps {
        match resolve_dep(d, &m.rel_dir, w.ws_deps, w.by_dir, w.classes) {
            Err(_) if m.class != Class::Core && has_f1_approval(root, &m.rel_dir, d) => {}
            Err(e) => fails.push(format!("{name}: {}: B8: {e}", d.key)),
            Ok(target) => {
                if m.class == Class::Core {
                    if let Some(c) = w.classes.get(&target).filter(|c| **c != Class::Core) {
                        fails.push(format!(
                            "{name}: {}: B8: core crate depends on {} crate {target}; a core crate depends only on core crates (F6)",
                            d.key,
                            class_name(*c)
                        ));
                    }
                }
                aliases.insert(d.key.replace('-', "_"), target);
            }
        }
    }

    let src = m.dir.join("src");
    let ieee = m.name == IEEE_CRATE && m.class == Class::Core;
    // every core and split crate takes the workspace lints and forbids `unsafe` at its root (A0)
    if !m.man.lints_workspace {
        fails.push(format!(
            "{name}: A0: a core or split crate declares `[lints] workspace = true`"
        ));
    }
    if m.class == Class::Core {
        match fs::read_to_string(src.join("lib.rs")) {
            Err(e) => fails.push(format!("{name}: cannot read src/lib.rs: {e}")),
            Ok(text) if ieee => {
                for h in declares_main_or_bin(&text) {
                    fails.push(format!(
                        "{}:{}: {}: {}",
                        rel(root, &src.join("lib.rs")),
                        h.line,
                        h.token,
                        h.message
                    ));
                }
                for h in ieee_api_problems(&text) {
                    fails.push(format!(
                        "{}:{}: {}: {}",
                        rel(root, &src.join("lib.rs")),
                        h.line,
                        h.token,
                        h.message
                    ));
                }
                if !has_forbid_unsafe(&text) {
                    fails.push(format!(
                        "{name}: A0: src/lib.rs of a core crate must have `#![forbid(unsafe_code)]` as an inner attribute at the top, before any item"
                    ));
                }
                if !has_pointer_width_guard(&text, wasm32_package_allowed(name)) {
                    fails.push(format!(
                        "{name}: B10: src/lib.rs of a core crate must have the 64-bit guard, or the exact wasm32-unknown-unknown guard if its package is in WASM32_PACKAGE_ALLOWLIST"
                    ));
                }
            }
            Ok(text) => {
                for h in declares_main_or_bin(&text) {
                    fails.push(format!(
                        "{}:{}: {}: {}",
                        rel(root, &src.join("lib.rs")),
                        h.line,
                        h.token,
                        h.message
                    ));
                }
                if !has_no_std(&text) {
                    fails.push(format!(
                        "{name}: B10: src/lib.rs of a core crate must have `#![cfg_attr(not(test), no_std)]` (or `#![no_std]`) as an inner attribute at the top, before any item"
                    ));
                }
                if !has_forbid_unsafe(&text) {
                    fails.push(format!(
                        "{name}: A0: src/lib.rs of a core crate must have `#![forbid(unsafe_code)]` as an inner attribute at the top, before any item"
                    ));
                }
                if !has_pointer_width_guard(&text, wasm32_package_allowed(name)) {
                    fails.push(format!(
                        "{name}: B10: src/lib.rs of a core crate must have the 64-bit guard, or the exact wasm32-unknown-unknown guard if its package is in WASM32_PACKAGE_ALLOWLIST"
                    ));
                }
            }
        }
    }
    let split = m.class == Class::Split;
    let (files, shell_files, walk_fails) = gather_files(root, m);
    fails.extend(walk_fails);
    if files.is_empty() {
        fails.push(format!("{name}: B8: no core files found to scan"));
    }
    let mut texts = Vec::new();
    for f in &files {
        match fs::read_to_string(f) {
            Ok(t) => texts.push((f.clone(), t)),
            Err(e) => fails.push(format!("{}: cannot read: {e}", rel(root, f))),
        }
    }
    // modules the crate declares, and the type and trait names its core files define
    let mut modules: BTreeSet<String> = BTreeSet::new();
    let mut core_types: BTreeSet<String> = BTreeSet::new();
    for (_, text) in &texts {
        let toks = tokenize(&mask(text));
        for (i, t) in toks.iter().enumerate() {
            if t.s == "mod" && is_word(s(&toks, i + 1)) {
                modules.insert(s(&toks, i + 1).to_string());
            }
        }
        core_types.extend(harden::defined_types(text));
    }
    let cb_types = boundary::callback_aliases(texts.iter().map(|(_, text)| text.as_str()));
    let mut scans = Vec::new();
    let empty = BTreeSet::new();
    let no_private = &w.private;
    for (f, text) in &texts {
        let ctx = Ctx {
            split,
            ieee,
            wasm32_guard_allowed: wasm32_package_allowed(&m.name),
            math: m.name == "oa-math",
            modules: &modules,
            floats: w.floats,
            private_mods: no_private.get(name.as_str()).unwrap_or(&empty),
            sealed: w.sealed,
            sealing: w.sealing,
            sealing_mods: w.sealing_mods,
            cb_types: &cb_types,
            depth: module_depth(&src, f),
            classes: w.classes,
            aliases: &aliases,
        };
        scans.push((f.clone(), scan_source(text, &ctx)));
    }
    let core_count = scans.len();
    for f in &shell_files {
        match fs::read_to_string(f) {
            Ok(t) => scans.push((f.clone(), scan_shell(&t, &core_types))),
            Err(e) => fails.push(format!("{}: cannot read: {e}", rel(root, f))),
        }
    }
    let skips = TestSkips::new(
        scans
            .iter()
            .map(|(f, sc)| (f, &sc.test_mods, sc.whole_file)),
    );
    for (n, (f, sc)) in scans.iter().enumerate() {
        if skips.skips(f) {
            continue;
        }
        for h in &sc.hits {
            fails.push(format!(
                "{}:{}: {}: {}",
                rel(root, f),
                h.line,
                h.token,
                h.message
            ));
        }
        if n >= core_count {
            continue;
        }
        let in_src = f.strip_prefix(&src).unwrap_or(f);
        let in_src = in_src.to_string_lossy().replace('\\', "/");
        for cb in &sc.callbacks {
            let key = (name.clone(), format!("{in_src}::{}", cb.name));
            if cbs.allow.contains(&key) {
                cbs.used.insert(key);
            } else {
                fails.push(format!(
                    "{}:{}: {}: B5: core code lets the shell hand in behaviour ({}). Remove it, or add `{name} {in_src}::{}` with the reason to {CALLBACK_ALLOWLIST}",
                    rel(root, f),
                    cb.line,
                    cb.name,
                    cb.why,
                    cb.name
                ));
            }
        }
    }
}

/// Test-only modules, resolved the way rustc does, and the files under them: they are shell.
struct TestSkips {
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

impl TestSkips {
    /// From each scanned file: its path, its out-of-line test modules, whether it is all test.
    fn new<'a>(scans: impl Iterator<Item = (&'a PathBuf, &'a Vec<String>, bool)>) -> TestSkips {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut dirs: Vec<PathBuf> = Vec::new();
        for (f, test_mods, whole_file) in scans {
            for tm in test_mods {
                let (cands, dir) = module_candidates(f, tm);
                files.extend(cands);
                dirs.push(dir);
            }
            if whole_file {
                files.push(f.clone());
                let (_, dir) = module_candidates(f, "");
                dirs.push(dir);
            }
        }
        TestSkips { files, dirs }
    }

    fn skips(&self, f: &Path) -> bool {
        self.files.iter().any(|x| x == f) || self.dirs.iter().any(|d| f.starts_with(d))
    }
}

/// A split crate: `src/` holds only `lib.rs`, `core`, `shell`, `main.rs` and `bin`; `lib.rs`
/// declares exactly the two modules; the core module (`core.rs` or `core/`) is collected.
fn check_split_layout(
    root: &Path,
    m: &Member,
    files: &mut Vec<PathBuf>,
    shell_files: &mut Vec<PathBuf>,
    fails: &mut Vec<String>,
) {
    let name = &m.name;
    let src = m.dir.join("src");
    let mut has = [false; 4]; // core.rs, core/, shell.rs, shell/
    match fs::read_dir(&src) {
        Err(e) => fails.push(format!("{name}: cannot read src/: {e}")),
        Ok(rd) => {
            for entry in rd {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        fails.push(format!("{name}: cannot read src/ entry: {e}"));
                        continue;
                    }
                };
                let fname = entry.file_name().to_string_lossy().into_owned();
                let is_dir = fs::symlink_metadata(entry.path()).is_ok_and(|md| md.is_dir());
                match (fname.as_str(), is_dir) {
                    ("lib.rs" | "main.rs", false) | ("bin", true) => {}
                    ("core.rs", false) => has[0] = true,
                    ("core", true) => has[1] = true,
                    ("shell.rs", false) => has[2] = true,
                    ("shell", true) => has[3] = true,
                    _ => fails.push(format!(
                        "{name}: B7: unexpected src/{fname} in a split crate (only lib.rs, core, shell, main.rs, bin)"
                    )),
                }
            }
        }
    }
    if !(has[0] || has[1]) {
        fails.push(format!("{name}: split crate has no src/core/ (B7)"));
    }
    if !(has[2] || has[3]) {
        fails.push(format!("{name}: split crate has no src/shell/ (B7)"));
    }
    if has[0] && has[1] && src.join("core/mod.rs").exists() {
        fails.push(format!(
            "{name}: B7: both src/core.rs and src/core/mod.rs exist"
        ));
    }
    let lib = src.join("lib.rs");
    match fs::read_to_string(&lib) {
        Ok(text) => {
            if !has_forbid_unsafe(&text) {
                fails.push(format!(
                    "{name}: A0: src/lib.rs of a split crate must have `#![forbid(unsafe_code)]` as an inner attribute at the top, before any item"
                ));
            }
            if !has_pointer_width_guard(&text, wasm32_package_allowed(name)) {
                fails.push(format!(
                    "{name}: B10: src/lib.rs of a split crate must have the 64-bit guard, or the exact wasm32-unknown-unknown guard if its package is in WASM32_PACKAGE_ALLOWLIST"
                ));
            }
            let display_wasm_shell_gate = name == "oa-display" && wasm32_package_allowed(name);
            for h in lib_rs_problems_for_package(&text, display_wasm_shell_gate) {
                fails.push(format!(
                    "{}:{}: {}: {}",
                    rel(root, &lib),
                    h.line,
                    h.token,
                    h.message
                ));
            }
            if display_wasm_shell_gate {
                match fs::read_to_string(m.dir.join("Cargo.toml")) {
                    Ok(manifest) => {
                        fails.extend(display_wasm_shell_manifest_problems(&manifest));
                    }
                    Err(e) => fails.push(format!("{name}: cannot read Cargo.toml: {e}")),
                }
            }
        }
        Err(e) => fails.push(format!("{name}: cannot read src/lib.rs: {e}")),
    }
    if has[0] {
        files.push(src.join("core.rs"));
    }
    if has[1] {
        walk_rs(&src.join("core"), &[], files, fails);
    }
    if has[2] {
        shell_files.push(src.join("shell.rs"));
    }
    if has[3] {
        walk_rs(&src.join("shell"), &[], shell_files, fails);
    }
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
    o.push_str("Platform float-method calls in unsplit crates, count (budget) (B9):\n");
    for (n, (count, budget)) in &rep.float_counts {
        match budget {
            Some(b) => o.push_str(&format!("  {n}: {count} ({b})\n")),
            None => o.push_str(&format!("  {n}: {count} (no budget)\n")),
        }
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

#[cfg(test)]
// The private helper test is synthetic and deliberately reports the token stream on failure.
#[allow(clippy::expect_used, clippy::panic)]
mod width_guard_tests {
    use super::*;

    fn cfg_index(src: &str) -> (Vec<Tok>, usize) {
        let masked = mask(src);
        let toks = tokenize(&masked);
        let index = toks
            .iter()
            .enumerate()
            .find_map(|(index, token)| {
                (token.s == "cfg"
                    && s(&toks, index + 1) == "("
                    && s(&toks, index + 2) == "not"
                    && (s(&toks, index + 4) == "target_pointer_width"
                        || s(&toks, index + 6) == "target_pointer_width"))
                    .then_some(index)
            })
            .unwrap_or_else(|| {
                panic!(
                    "width guard cfg not found in {:?}",
                    toks.iter()
                        .map(|token| token.s.as_str())
                        .collect::<Vec<_>>()
                )
            });
        (toks, index)
    }

    #[test]
    fn exact_native_and_allowlisted_wasm_width_guards_are_recognized() {
        let native = "#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"unsupported\");";
        let (native_toks, native_index) = cfg_index(native);
        assert!(pointer_width_guard(&native_toks, native_index, false));

        let wasm = "#[cfg(not(any(target_pointer_width = \"64\", all(target_arch = \"wasm32\", target_pointer_width = \"32\", target_os = \"unknown\", target_vendor = \"unknown\"))))]\ncompile_error!(\"unsupported\");";
        let (wasm_toks, wasm_index) = cfg_index(wasm);
        assert!(!pointer_width_guard(&wasm_toks, wasm_index, false));
        assert!(pointer_width_guard(&wasm_toks, wasm_index, true));
    }

    #[test]
    fn display_shell_cfg_accepts_rustfmt_wrapping_but_not_changed_targets() {
        const EXPECTED: &str = "#[cfg(not(all(target_arch = \"wasm32\", target_pointer_width = \"32\", target_os = \"unknown\", target_vendor = \"unknown\")))]";
        let wrapped = "#[cfg(not(all(\n    target_arch = \"wasm32\",\n    target_pointer_width = \"32\",\n    target_os = \"unknown\",\n    target_vendor = \"unknown\"\n)))]\nmod shell;";
        assert!(attribute_matches_without_whitespace(
            wrapped, 1, 6, EXPECTED
        ));
        let wrong_target = wrapped.replace("\"32\"", "\"64\"");
        assert!(!attribute_matches_without_whitespace(
            &wrong_target,
            1,
            6,
            EXPECTED
        ));
    }
}
