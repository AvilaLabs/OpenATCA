//! The core/shell class check against fixture workspaces (OA-TRC-200 to OA-TRC-215).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_trace::classcheck::{
    check, check_with, ieee_api_problems, lib_rs_problems, mask, parse_manifest, parse_members,
    render_text, scan_source, Class, Ctx, FloatFacts, UNSPLIT_ALLOWED,
};
use oa_trace::classcheck::{float_method_advice, FLOAT_METHODS};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn has(failures: &[String], needle: &str) -> bool {
    failures.iter().any(|f| f.contains(needle))
}

// REQ: OA-TRC-200
#[test]
fn a_clean_workspace_passes_with_comments_strings_and_test_code_ignored() {
    let rep = check_with(&fixture("good"), &["fx-old"]);
    assert!(rep.ok(), "{:#?}", rep.failures);
    assert_eq!(rep.unsplit, ["fx-old"]);
    assert_eq!(
        rep.classes.get("fx-split").map(String::as_str),
        Some("split")
    );
    assert_eq!(rep.classes.len(), 4);
}

// REQ: OA-TRC-201
#[test]
fn a_missing_or_unknown_class_names_the_crate() {
    let rep = check_with(&fixture("bad"), &[]);
    assert!(
        has(
            &rep.failures,
            "fx-nocls: no [package.metadata.openatca] class"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(&rep.failures, "fx-weird: unknown class \"banana\""),
        "{:#?}",
        rep.failures
    );
}

// REQ: OA-TRC-202
#[test]
fn modules_outside_the_allowlist_are_reported_with_file_line_and_token() {
    let rep = check_with(&fixture("bad"), &[]);
    let want = [
        "crates/fx-badcore/src/lib.rs:2: std::fs",
        "crates/fx-badcore/src/lib.rs:3: std::io",
        "crates/fx-badcore/src/lib.rs:3: std::time",
        "crates/fx-badcore/src/lib.rs:5: HashMap",
        "crates/fx-badcore/src/lib.rs:6: println!",
        "crates/fx-badcore/src/lib.rs:9: std::env",
        "crates/fx-badcore/src/lib.rs:14: thread_local!",
        "crates/fx-badcore/src/lib.rs:15: static mut",
        "crates/fx-badsplit/src/core/mod.rs:2: HashSet",
    ];
    for w in want {
        assert!(has(&rep.failures, w), "missing {w}: {:#?}", rep.failures);
    }
    // the comment and the string on lines 16 and 17 are not hits
    assert!(!has(&rep.failures, "lib.rs:16:") && !has(&rep.failures, "lib.rs:17:"));
    // and the failure names the allowed alternative
    assert!(has(&rep.failures, "core::time::Duration"));
    assert!(has(&rep.failures, "core::fmt"));
}

// REQ: OA-TRC-203
#[test]
fn a_path_into_a_shell_module_is_a_violation() {
    let rep = check_with(&fixture("bad"), &[]);
    assert!(
        has(&rep.failures, "crates/fx-badcore/src/lib.rs:7: shell"),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(&rep.failures, "crates/fx-badsplit/src/core/mod.rs:1: shell"),
        "{:#?}",
        rep.failures
    );
}

// REQ: OA-TRC-204
#[test]
fn comments_strings_and_char_literals_never_match() {
    let src = "// std::fs\n/* HashMap /* nested */ println! */\n\
               const A: &str = \"std::io HashSet\";\n\
               const B: &str = r##\"dbg!(x) \"# static mut\"##;\n\
               const C: char = '\"';\nconst D: char = '\\'';\n\
               fn f<'a>(x: &'a str) -> &'a str { x }\n";
    let masked = mask(src);
    assert_eq!(masked.lines().count(), src.lines().count());
    for bad in ["std", "HashMap", "HashSet", "println", "dbg", "static"] {
        assert!(!masked.contains(bad), "{bad} survived: {masked}");
    }
    // a lifetime is blanked too, so `&'static mut T` is not `static mut`
    assert!(masked.contains("fn f<") && !masked.contains("'a"));
    assert!(tokens("pub fn f(x: &'static mut u8) {}\n").is_empty());
    let hits = scan(src).hits;
    assert!(hits.is_empty(), "{hits:?}");
    // the same names in code are hits, and a path split over lines is still found
    let hits = scan("fn f() {\n let _ = std\n   :: fs::read(\"x\");\n}\n").hits;
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!((hits[0].line, hits[0].token.as_str()), (3, "std::fs"));
}

fn scan(src: &str) -> oa_trace::classcheck::FileScan {
    scan_with(src, false, &FloatFacts::default())
}

/// [`scan`] with the float facts of the other core files (`facts`) and `oa-math`'s exemption.
fn scan_with(src: &str, math: bool, facts: &FloatFacts) -> oa_trace::classcheck::FileScan {
    let classes: BTreeMap<String, Class> = [
        ("fx-core".to_string(), Class::Core),
        ("fx-shell".to_string(), Class::Shell),
        ("fx-split".to_string(), Class::Split),
    ]
    .into_iter()
    .collect();
    let aliases: BTreeMap<String, String> = classes
        .keys()
        .map(|n| (n.replace('-', "_"), n.clone()))
        .collect();
    let cb_types = oa_trace::classcheck::callback_aliases([src]);
    scan_source(
        src,
        &Ctx {
            split: false,
            ieee: false,
            math,
            modules: &BTreeSet::new(),
            floats: facts,
            private_mods: &BTreeSet::new(),
            sealed: &BTreeSet::new(),
            sealing: &BTreeSet::new(),
            cb_types: &cb_types,
            depth: 1,
            classes: &classes,
            aliases: &aliases,
        },
    )
}

fn tokens(src: &str) -> Vec<String> {
    scan(src).hits.into_iter().map(|h| h.token).collect()
}

/// The hit tokens of `src` when other core files declared `others` (struct fields, tuple structs,
/// `-> f64` functions, constants).
fn tokens_with(src: &str, others: &str) -> Vec<String> {
    let mut facts = FloatFacts::default();
    facts.add_source(others);
    scan_with(src, false, &facts)
        .hits
        .into_iter()
        .map(|h| h.token)
        .collect()
}

// REQ: OA-TRC-205
#[test]
fn core_code_may_name_only_core_crates_and_the_core_module_of_split_crates() {
    let rep = check_with(&fixture("bad"), &[]);
    assert!(
        has(
            &rep.failures,
            "fx-badcore: fx-shell: B8: core crate depends on shell crate fx-shell"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(
            &rep.failures,
            "fx-badcore: fx-cleansplit: B8: core crate depends on split crate"
        ),
        "a core crate depends only on core crates: {:#?}",
        rep.failures
    );
    assert!(
        has(
            &rep.failures,
            "crates/fx-badcore/src/lib.rs:10: fx_shell: B8: names a shell or unsplit crate"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(
            &rep.failures,
            "crates/fx-badcore/src/lib.rs:11: fx_cleansplit: B8: names a split crate outside its `core` module"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(
        !has(&rep.failures, "lib.rs:12:"),
        "fx_cleansplit::core is allowed"
    );
    let mut t = tokens("use fx_shell as f; fn g() { f::x() }");
    t.sort();
    assert_eq!(t, ["as", "fx_shell"]);
}

// REQ: OA-TRC-206
#[test]
fn a_split_crate_needs_both_modules_and_a_lib_rs_declaring_exactly_them() {
    let rep = check_with(&fixture("bad"), &[]);
    assert!(
        has(&rep.failures, "fx-badsplit: split crate has no src/shell/"),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(
            &rep.failures,
            "crates/fx-badsplit/src/lib.rs:2: fn: B7: lib.rs of a split crate"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(
        !has(&rep.failures, "crates/fx-cleansplit"),
        "{:#?}",
        rep.failures
    );
    let ok = "#![forbid(unsafe_code)]\n//! doc\npub mod core;\npub(crate) mod shell;\npub use self::core::{a, b};\n#[cfg(test)]\nmod tests { fn t() {} }\n";
    assert!(lib_rs_problems(ok).is_empty());
    for bad in [
        "pub mod core { pub fn f() {} }\nmod shell;",
        "mod core;\nmod shell;\nconst X: u8 = 1;",
        "mod core;\nmod shell;\nmod util;",
        "mod core;",
        "mod core;\nmod shell;\nmod shell;",
        "mod core;\n#[path = \"x.rs\"]\nmod shell;",
    ] {
        assert!(!lib_rs_problems(bad).is_empty(), "{bad}");
    }
}

// REQ: OA-TRC-207
#[test]
fn the_unsplit_list_is_fixed_and_the_allowlist_file_may_only_shrink() {
    let rep = check_with(&fixture("bad"), &[]);
    assert_eq!(rep.unsplit, ["fx-newold"]);
    assert!(
        has(&rep.failures, "fx-newold: B9: new unsplit crate"),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(&rep.failures, "fx-shell: B9: listed but not unsplit"),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(&rep.failures, "fx-ghost: B9: listed but not unsplit"),
        "{:#?}",
        rep.failures
    );
    assert!(
        has(&rep.failures, "fx-ghost: B9: not in the fixed list"),
        "{:#?}",
        rep.failures
    );
    // the clean workspace passes with its crate in the fixed list, fails without it
    assert!(check_with(&fixture("good"), &["fx-old"]).ok());
    assert!(has(
        &check_with(&fixture("good"), &[]).failures,
        "fx-old: B9: new unsplit crate"
    ));
    // the real list: twelve names, sorted, none a duplicate
    assert_eq!(UNSPLIT_ALLOWED.len(), 12);
    let mut sorted = UNSPLIT_ALLOWED;
    sorted.sort();
    assert_eq!(sorted, UNSPLIT_ALLOWED);
}

// REQ: OA-TRC-208
#[test]
fn the_cargo_toml_reader_finds_name_class_dependencies_and_members() {
    let m = parse_manifest(
        "[package]\nname = \"oa-x\" # n\n\n[package.metadata.openatca]\nclass = \"core\"\n\n[dependencies] # c\noa-a = { path = \"../oa-a\" }\n# oa-no = 1\noa-b.workspace = true\nfoo = { package = \"oa-c\", path = \"../oa-c\" }\n\n[target.'cfg(unix)'.dependencies]\noa-t = { path = \"../oa-t\" }\n\n[dev-dependencies.oa-d]\npath = \"../oa-d\"\n",
    );
    assert_eq!(m.name.as_deref(), Some("oa-x"));
    assert_eq!(m.class.as_deref(), Some("core"));
    let keys: Vec<&str> = m.deps.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, ["oa-a", "oa-b", "foo", "oa-t", "oa-d"]);
    assert_eq!(m.deps[2].package.as_deref(), Some("oa-c"));
    assert_eq!(m.deps[3].path.as_deref(), Some("../oa-t"));
    assert_eq!(m.deps[4].path.as_deref(), Some("../oa-d"));
    let t = "[workspace]\nresolver = \"2\"\nexclude = [\"fuzz\"]\nmembers = [\n    \"crates/a\",\n    'tools/b', # c\n]\n[workspace.package]\nmembers = [\"nope\"]\n";
    assert_eq!(
        parse_members(t),
        (vec!["crates/a".into(), "tools/b".into()], vec![])
    );
    let t = "[workspace]\nmembers =\n[\"a\", b]\n";
    let _ = t;
    let (items, bad) = parse_members("[workspace]\nmembers = [\"a\", b, \"c/*\"]\n");
    assert_eq!(
        (items, bad),
        (
            vec!["a".to_string()],
            vec!["b".to_string(), "\"c/*\"".to_string()]
        )
    );
}

// ---------------------------------------------------------------------------------------------
// Negative fixtures: one workspace per bypass the review found, each must fail.

struct Ws {
    root: PathBuf,
}

impl Ws {
    /// A one-crate workspace: `fx-a` (class `class`) with `files`, plus the shell crate `fx-b`.
    fn new(tag: &str, class: &str, files: &[(&str, &str)]) -> Ws {
        Ws::named(tag, "fx-a", class, files)
    }

    /// As [`Ws::new`] with the package name of the crate in `crates/fx-a` set. A core crate's
    /// `src/lib.rs` gets `#![cfg_attr(not(test), no_std)]` on its first line (B10) unless it has one, and the
    /// 64-bit guard (B10) after its last line.
    fn named(tag: &str, pkg: &str, class: &str, files: &[(&str, &str)]) -> Ws {
        // one directory per workspace, so equal tags in parallel tests do not share it
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("oa-trace-class-{}-{n}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ws = Ws { root };
        ws.put(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\"]\n",
        );
        ws.put(ALLOW, "");
        ws.put(CALLBACKS, "");
        ws.put(BUDGET, "");
        ws.put(
            "crates/fx-a/Cargo.toml",
            &format!(
                "[package]\nname = \"{pkg}\"\n\n[package.metadata.openatca]\nclass = \"{class}\"\n\n[lints]\nworkspace = true\n"
            ),
        );
        ws.put(
            "crates/fx-b/Cargo.toml",
            "[package]\nname = \"fx-b\"\n\n[package.metadata.openatca]\nclass = \"shell\"\n",
        );
        ws.put("crates/fx-b/src/lib.rs", "pub fn go() {}\n");
        for (p, c) in files {
            let mut c = (*c).to_string();
            if class == "core" && *p == "src/lib.rs" && !c.contains("compile_error") {
                c = format!("{c}\n#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"64-bit only\");\n");
            }
            if class == "core" && *p == "src/lib.rs" && !c.contains("no_std") {
                c = format!("#![cfg_attr(not(test), no_std)] {c}");
            }
            if matches!(class, "core" | "split")
                && *p == "src/lib.rs"
                && !c.contains("forbid(unsafe_code)")
            {
                c = format!("#![forbid(unsafe_code)] {c}");
            }
            ws.put(&format!("crates/fx-a/{p}"), &c);
        }
        ws
    }

    fn put(&self, rel: &str, content: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn failures(&self) -> Vec<String> {
        check_with(&self.root, &[]).failures
    }
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const ALLOW: &str = "tools/oa-trace/unsplit-allowlist.txt";
const CALLBACKS: &str = "tools/oa-trace/core-callbacks-allowlist.txt";
const BUDGET: &str = "tools/oa-trace/unsplit-float-budget.txt";

fn core_ws(tag: &str, lib: &str) -> Ws {
    Ws::new(tag, "core", &[("src/lib.rs", lib)])
}

fn split_ws(tag: &str, lib: &str, core: &str) -> Ws {
    Ws::new(
        tag,
        "split",
        &[
            ("src/lib.rs", lib),
            ("src/core/mod.rs", core),
            (
                "src/shell/mod.rs",
                "pub fn s() { let _ = std::fs::read(\"x\"); }\n",
            ),
        ],
    )
}

const SPLIT_LIB: &str = "mod core;\nmod shell;\n#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"64-bit only\");\n";

/// The 64-bit guard a core crate's `lib.rs` carries (B10).
const GUARD: &str =
    "#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"64-bit only\");\n";

// REQ: OA-TRC-209
#[test]
fn members_are_parsed_strictly_and_cross_checked_against_the_directories() {
    // a clean one-crate workspace passes, with single quotes and a multi-line list
    let ws = core_ws("m0", "pub fn f() {}\n");
    ws.put(
        "Cargo.toml",
        "[workspace]\nmembers =\n[\n  'crates/fx-a', # a\n  \"crates/fx-b\",\n]\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // zero members
    ws.put("Cargo.toml", "[workspace]\n");
    assert!(has(&ws.failures(), "no workspace members parsed"));
    // an entry that is not a quoted string, and a glob
    ws.put(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/fx-a\", crates/fx-b]\n",
    );
    assert!(
        has(&ws.failures(), "not a plain quoted path"),
        "{:#?}",
        ws.failures()
    );
    ws.put("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    assert!(has(&ws.failures(), "not a plain quoted path"));
    // a crate directory that is neither a member nor excluded; excluding it makes it pass
    ws.put(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\"]\n",
    );
    ws.put("crates/fx-c/Cargo.toml", "[package]\nname = \"fx-c\"\n");
    assert!(has(
        &ws.failures(),
        "crates/fx-c: has a Cargo.toml but is neither"
    ));
    ws.put(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\"]\nexclude = [\"crates/fx-c\"]\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // a member that is not there
    ws.put("Cargo.toml", "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\", \"crates/fx-z\"]\nexclude = [\"crates/fx-c\"]\n");
    assert!(has(&ws.failures(), "crates/fx-z: cannot read Cargo.toml"));
}

// REQ: OA-TRC-210
#[test]
fn unreadable_missing_symlinked_and_redirected_sources_fail() {
    // no src at all: nothing scanned
    let ws = Ws::new("s0", "core", &[]);
    assert!(
        has(&ws.failures(), "fx-a: B8: no core files found to scan"),
        "{:#?}",
        ws.failures()
    );
    // a symlinked source file
    let ws = core_ws("s1", "pub fn f() {}\n");
    ws.put(
        "crates/fx-b/src/io.rs",
        "pub fn r() { let _ = std::fs::read(\"x\"); }\n",
    );
    std::os::unix::fs::symlink(
        ws.root.join("crates/fx-b/src/io.rs"),
        ws.root.join("crates/fx-a/src/io.rs"),
    )
    .unwrap();
    assert!(has(&ws.failures(), "symlink"), "{:#?}", ws.failures());
    // custom [lib] / [[bin]] paths and a build script
    let ws = core_ws("s2", "pub fn f() {}\n");
    let head = "[package]\nname = \"fx-a\"\n\n[package.metadata.openatca]\nclass = \"core\"\n";
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}\n[lib]\npath = \"elsewhere/lib.rs\"\n"),
    );
    assert!(has(
        &ws.failures(),
        "fx-a: B8: no custom [lib] or [[bin]] path"
    ));
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}\n[[bin]]\nname = \"x\"\npath = \"src/cli.rs\"\n"),
    );
    assert!(has(
        &ws.failures(),
        "fx-a: B8: no custom [lib] or [[bin]] path"
    ));
    ws.put("crates/fx-a/Cargo.toml", head);
    ws.put("crates/fx-a/build.rs", "fn main() {}\n");
    assert!(has(&ws.failures(), "fx-a: B8: no build.rs"));
    ws.put(
        "crates/fx-a/Cargo.toml",
        "[package]\nname = \"fx-a\"\nbuild = \"gen.rs\"\n\n[package.metadata.openatca]\nclass = \"core\"\n",
    );
    let _ = fs::remove_file(ws.root.join("crates/fx-a/build.rs"));
    assert!(has(&ws.failures(), "fx-a: B8: no build.rs"));
}

// REQ: OA-TRC-211
#[test]
fn a_test_attribute_strips_items_only() {
    // on a field, a match arm and a parameter nothing is stripped, so the code after is seen
    for (tag, src) in [
        ("t0", "pub struct S { #[cfg(test)] dbg: u8, f: std::fs::File }\n"),
        ("t1", "pub fn f(x: u8) -> u8 { match x { #[cfg(test)] 0 => 1, _ => { let _ = std::fs::read(\"x\"); 2 } } }\n"),
        ("t2", "pub fn f(#[cfg(test)] a: u8, b: u8) { let _ = std::fs::read(\"x\"); }\n"),
        ("t3", "pub enum E { #[cfg(test)] A, B(std::fs::File) }\n"),
        // any(test, ..) is compiled in a product build; not(test) is the product build
        ("t4", "#[cfg(any(test, unix))]\nmod m { use std::fs; }\n"),
        ("t5", "#[cfg(not(test))]\nmod m { use std::fs; }\n"),
        // a statement is not an item
        ("t6", "pub fn f() { #[cfg(test)] let _a = 1; let _ = std::fs::read(\"x\"); }\n"),
    ] {
        let ws = core_ws(tag, src);
        assert!(has(&ws.failures(), "std::fs"), "{tag}: {:#?}", ws.failures());
    }
    // the real spellings are stripped: test, all(test, ..), an inner marker, a test-only file
    let ws = core_ws(
        "t7",
        "#[cfg(test)]\nmod a { use std::fs; }\n#[cfg(all(test, unix))]\nmod b { use std::fs; }\n#[cfg(test)]\nfn c() { std::fs::read(\"x\"); }\n#[cfg(test)]\nuse std::io;\n#[cfg(test)]\nconst X: u8 = { 1 };\n#[cfg(test)]\nmod d;\nmod e;\npub fn ok() {}\n",
    );
    ws.put("crates/fx-a/src/d.rs", "use std::fs;\n");
    ws.put("crates/fx-a/src/e.rs", "#![cfg(test)]\nuse std::fs;\n");
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-212
#[test]
fn aliases_globs_extern_crate_and_includes_are_rejected() {
    let cases: [(&str, &str, &str); 14] = [
        ("a0", "use std as s;\n", "std"),
        ("a1", "use std::fmt as f;\n", "as"),
        ("a2", "use std::*;\n", "*"),
        ("a3", "use std::{self as s};\n", "std::self"),
        ("a4", "extern crate std as s;\n", "extern crate"),
        ("a5", "use fx_b as f;\n", "as"),
        ("a6", "use ::std::fs::read;\n", "std::fs"),
        ("a7", "use fx_b;\n", "fx_b"),
        ("a8", "pub fn f() { let _ = fx_b::go(); }\n", "fx_b"),
        ("a9", "pub fn f() { let _ = ::fx_b::go(); }\n", "fx_b"),
        ("a10", "pub const T: &str = include_str!(\"x.txt\");\n", "include_str!"),
        ("a11", "pub const T: &str = env!(\"HOME\");\n", "env!"),
        ("a12", "#[path = \"../fx-b/src/lib.rs\"]\nmod x;\n", "#[path]"),
        ("a13", "pub fn f() { let _ = std::sync::atomic::AtomicU8::new(0); let _ = core::cell::Cell::new(0); }\n", "std::sync"),
    ];
    for (tag, src, want) in cases {
        let ws = core_ws(tag, src);
        assert!(
            has(&ws.failures(), want),
            "{tag} {src}: {:#?}",
            ws.failures()
        );
    }
    // allowed spellings pass
    let ws = core_ws(
        "a14",
        "use core::fmt::{self, Debug};\nuse alloc::collections::BTreeMap;\nuse core::time::Duration;\npub fn f(_d: Duration, _m: &BTreeMap<u8, u8>) -> usize { core::mem::drop(1u8); 0 }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    let ws = core_ws(
        "a15",
        "use core::fmt::Debug;\nuse alloc::collections::BTreeMap;\nuse core::time::Duration;\npub fn f(_d: Duration, _m: &BTreeMap<u8, u8>) -> usize { core::mem::drop(1u8); 0 }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    for (tag, src) in [
        ("a16", "use std::time::Duration;\n"),
        ("a17", "use std::collections::HashMap;\n"),
        ("a18", "use std::collections::LinkedList;\n"),
        ("a19", "use std::collections;\n"),
        ("a20", "use core::sync::atomic::AtomicU64;\n"),
        ("a21", "use alloc::sync::Arc;\n"),
        ("a22", "use std::os::unix::fs::MetadataExt;\n"),
        ("a23", "use std::path::Path;\n"),
    ] {
        let ws = core_ws(tag, src);
        assert!(!ws.failures().is_empty(), "{tag}: {src}");
    }
}

// REQ: OA-TRC-213
#[test]
fn the_split_layout_cannot_hide_code_from_the_scan() {
    // a third module, not core and not shell
    let ws = split_ws(
        "p0",
        "mod core;\nmod shell;\nmod util;\n",
        "pub fn f() {}\n",
    );
    ws.put(
        "crates/fx-a/src/util.rs",
        "pub fn r() { let _ = std::fs::read(\"x\"); }\n",
    );
    assert!(
        has(&ws.failures(), "B7: lib.rs of a split crate"),
        "{:#?}",
        ws.failures()
    );
    assert!(has(&ws.failures(), "unexpected src/util.rs"));
    // crate:: paths outside crate::core::, super:: out of the module
    let ws = split_ws(
        "p1",
        SPLIT_LIB,
        "pub fn f() { crate::shell_io(); crate::core::g(); }\npub fn g() {}\n",
    );
    assert!(
        has(&ws.failures(), "crate::shell_io"),
        "{:#?}",
        ws.failures()
    );
    assert!(!has(&ws.failures(), "crate::core::g"));
    let ws = split_ws("p2", SPLIT_LIB, "use super::util::r;\n");
    assert!(has(&ws.failures(), "super"), "{:#?}", ws.failures());
    // 2018 layout: src/core.rs is the core root and is scanned
    let ws = Ws::new(
        "p3",
        "split",
        &[
            ("src/lib.rs", SPLIT_LIB),
            ("src/core.rs", "mod sub;\nuse std::fs;\n"),
            ("src/core/sub.rs", "pub fn x() {}\n"),
            ("src/shell.rs", "pub fn s() {}\n"),
        ],
    );
    assert!(
        has(&ws.failures(), "src/core.rs:2: std::fs"),
        "{:#?}",
        ws.failures()
    );
    // a clean 2018-layout split crate passes
    let ws = Ws::new(
        "p4",
        "split",
        &[
            ("src/lib.rs", SPLIT_LIB),
            ("src/core.rs", "mod sub;\npub use self::sub::x;\n"),
            ("src/core/sub.rs", "pub fn x() -> u8 { super::y() }\n"),
            ("src/shell.rs", "pub fn s() {}\n"),
        ],
    );
    assert!(!has(&ws.failures(), "src/core"), "{:#?}", ws.failures());
}

// REQ: OA-TRC-214
#[test]
fn test_file_names_do_not_exempt_and_module_files_resolve_like_rustc() {
    // a normal module named like a test file is scanned
    let ws = Ws::new(
        "r0",
        "core",
        &[
            ("src/lib.rs", "mod parse_tests;\nmod tests;\n"),
            ("src/parse_tests.rs", "use std::fs;\n"),
            ("src/tests.rs", "use std::io;\n"),
        ],
    );
    let f = ws.failures();
    assert!(
        has(&f, "parse_tests.rs:1: std::fs") && has(&f, "tests.rs:1: std::io"),
        "{f:#?}"
    );
    // foo.rs declares a test-only `util`: that is foo/util.rs, not the sibling util.rs
    let ws = Ws::new(
        "r1",
        "core",
        &[
            ("src/lib.rs", "mod foo;\nmod util;\n"),
            ("src/foo.rs", "#[cfg(test)]\nmod util;\n"),
            ("src/foo/util.rs", "use std::fs;\n"),
            ("src/util.rs", "use std::io;\n"),
        ],
    );
    let f = ws.failures();
    assert!(has(&f, "src/util.rs:1: std::io"), "{f:#?}");
    assert!(!has(&f, "foo/util.rs"), "{f:#?}");
    // children of a test-only module are test-only too
    let ws = Ws::new(
        "r2",
        "core",
        &[
            ("src/lib.rs", "#[cfg(test)]\nmod helpers;\n"),
            ("src/helpers/mod.rs", "mod deep;\n"),
            ("src/helpers/deep.rs", "use std::fs;\n"),
        ],
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // only the top-level src/bin and main.rs are skipped
    let ws = Ws::new(
        "r3",
        "core",
        &[
            ("src/lib.rs", "mod sub;\n"),
            ("src/sub/mod.rs", "mod bin;\n"),
            ("src/sub/bin/mod.rs", "use std::fs;\n"),
            ("src/bin/tool.rs", "use std::fs;\n"),
            ("src/main.rs", "use std::fs;\n"),
        ],
    );
    let f = ws.failures();
    assert!(has(&f, "src/sub/bin/mod.rs:1: std::fs"), "{f:#?}");
    assert!(
        !has(&f, "src/bin/tool.rs") && !has(&f, "src/main.rs"),
        "{f:#?}"
    );
}

// REQ: OA-TRC-215
#[test]
fn every_dependency_table_and_rename_is_resolved_by_path() {
    let head = "[package]\nname = \"fx-a\"\n\n[package.metadata.openatca]\nclass = \"core\"\n\n";
    for (tag, deps) in [
        ("d0", "[dependencies]\nfx-b = { path = \"../fx-b\" }\n"),
        (
            "d1",
            "[dependencies]\nfoo = { package = \"fx-b\", path = \"../fx-b\" }\n",
        ),
        (
            "d2",
            "[target.'cfg(unix)'.dependencies]\nfx-b = { path = \"../fx-b\" }\n",
        ),
        ("d3", "[dev-dependencies]\nfx-b = { path = \"../fx-b\" }\n"),
        ("d4", "[dependencies] # why\nbar.path = \"../fx-b\"\n"),
        ("d5", "[dependencies.baz]\npath = \"../fx-b\"\n"),
    ] {
        let ws = core_ws(tag, "pub fn f() {}\n");
        ws.put("crates/fx-a/Cargo.toml", &format!("{head}{deps}"));
        assert!(
            has(&ws.failures(), "core crate depends on shell crate fx-b"),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
    // a path that is not a member
    let ws = core_ws("d6", "pub fn f() {}\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}[dependencies]\nx = {{ path = \"../nowhere\" }}\n"),
    );
    assert!(has(&ws.failures(), "not a workspace member"));
    // a renamed dependency is caught in source too
    let ws = core_ws("d7", "pub fn f() { let _ = foo::go(); }\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}[dependencies]\nfoo = {{ package = \"fx-b\", path = \"../fx-b\" }}\n"),
    );
    assert!(
        has(&ws.failures(), "foo: B8: names a shell or unsplit crate"),
        "{:#?}",
        ws.failures()
    );
}

// REQ: OA-TRC-216
#[test]
fn the_real_workspace_passes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rep = check(&root);
    assert!(rep.ok(), "{:#?}", rep.failures);
    assert_eq!(rep.unsplit, ["oa-replay-diff", "oa-track"]);
}

// REQ: OA-TRC-217
#[test]
fn only_extern_crate_alloc_is_allowed() {
    assert!(core_ws("e0", "extern crate alloc;\npub fn f() {}\n")
        .failures()
        .is_empty());
    for (tag, src) in [
        ("e1", "extern crate std;\n"),
        ("e2", "extern crate alloc as a;\n"),
        ("e3", "extern crate fx_b;\n"),
    ] {
        assert!(
            has(&core_ws(tag, src).failures(), "extern crate"),
            "{tag}: {:#?}",
            core_ws(tag, src).failures()
        );
    }
}

// REQ: OA-TRC-218
#[test]
fn float_methods_core_lacks_are_rejected_in_core_code() {
    for (tag, src, tok) in [
        ("f0", "pub fn f(x: f64) -> f64 { x.sin() }\n", "sin"),
        ("f1", "pub fn f(x: f64) -> f64 { f64::sqrt(x) }\n", "sqrt"),
        (
            "f2",
            "pub fn f(x: f32) -> f32 { f32::hypot(x, x) }\n",
            "hypot",
        ),
        (
            "f3",
            "pub fn f(x: &[f64]) -> Vec<f64> { x.iter().copied().map(f64::ln).collect() }\n",
            "ln",
        ),
        (
            "f4",
            "pub fn f(x: i32) -> i32 { x.rem_euclid(3) }\n",
            "rem_euclid",
        ),
        ("f6", "pub fn f<R: Rt>(x: R) -> R { R::sin(x) }\n", "sin"),
        ("f7", "pub fn f(x: f64) -> f64 { <f64>::sqrt(x) }\n", "sqrt"),
        (
            "f8",
            "pub struct S;\nimpl S { fn g(x: f64) -> f64 { Self::sin(x) } }\n",
            "sin",
        ),
    ] {
        let f = core_ws(tag, src).failures();
        assert!(
            f.iter().any(|m| m.contains(&format!(": {tok}: B8"))
                && (m.contains("oa_ieee::") || m.contains("oa_math::"))
                && m.contains("user-defined")),
            "{tag}: {f:#?}"
        );
    }
    // oa_ieee / oa_math calls and a definition are fine
    let ws = core_ws(
        "f5",
        "pub fn sqrt(x: f64) -> f64 { x }\npub fn f(x: f64) -> f64 { sqrt(x) + oa_ieee::sqrt(x) + oa_math::sin(x) }\n",
    );
    let f = ws.failures();
    assert!(!has(&f, "B8: the standard library"), "{f:#?}");
    // a lowercase segment before `::` is a module or crate path, never a type
    let ws = core_ws(
        "f9",
        "mod dd { pub fn sqrt(x: f64) -> f64 { x } }\nmod exp { pub fn exp(x: f64) -> f64 { x } }\npub use exp::exp;\npub fn f(x: f64) -> f64 { dd::sqrt(x) + self::dd::sqrt(x) + crate::exp::exp(x) }\n",
    );
    let f = ws.failures();
    assert!(!has(&f, "B8: the standard library"), "{f:#?}");
}

// REQ: OA-TRC-219
#[test]
fn a_core_crate_must_be_no_std() {
    let ws = Ws::new("n0", "core", &[("src/lib.rs", "pub fn f() {}\n")]);
    ws.put(
        "crates/fx-a/src/lib.rs",
        &format!("pub fn f() {{}}\n{GUARD}"),
    );
    assert!(has(&ws.failures(), "fx-a: B10"), "{:#?}", ws.failures());
    ws.put(
        "crates/fx-a/src/lib.rs",
        &format!("#![forbid(unsafe_code)]\n#![no_std]\npub fn f() {{}}\n{GUARD}"),
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    ws.put(
        "crates/fx-a/src/lib.rs",
        &format!(
            "#![forbid(unsafe_code)]\n#![cfg_attr( not(test) , no_std )]\npub fn f() {{}}\n{GUARD}"
        ),
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // in a comment, a string, or the wrong cfg it does not count
    for bad in [
        "// #![no_std]\npub fn f() {}\n",
        "#![cfg_attr(unix, no_std)]\npub fn f() {}\n",
        "#![cfg_attr(test, no_std)]\npub fn f() {}\n",
        "pub const S: &str = \"#![no_std]\";\n",
    ] {
        ws.put("crates/fx-a/src/lib.rs", &format!("{bad}{GUARD}"));
        assert!(
            has(
                &ws.failures(),
                "fx-a: B10: src/lib.rs of a core crate must have `#!["
            ),
            "{bad}: {:#?}",
            ws.failures()
        );
    }
    // the bad fixture's core crate has none
    assert!(has(
        &check_with(&fixture("bad"), &[]).failures,
        "fx-badcore: B10"
    ));
}

// REQ: OA-TRC-220
#[test]
fn oa_ieee_exposes_exactly_five_functions_and_may_use_float_methods() {
    let api = "pub fn sqrt(v: f64) -> f64 { v.sqrt() }\npub fn floor(v: f64) -> f64 { v.floor() }\npub fn ceil(v: f64) -> f64 { v.ceil() }\npub fn trunc(v: f64) -> f64 { v.trunc() }\npub fn round(v: f64) -> f64 { v.round() }\nfn private(x: f64) -> f64 { x.sin() * 0.0 }\npub(crate) fn inner() {}\n#[cfg(test)]\nmod tests { pub fn extra() {} }\n";
    let ok = Ws::named("i0", "oa-ieee", "core", &[("src/lib.rs", api)]);
    // std is allowed in oa-ieee: no no_std needed; the float rule is waived (a private `.sin()`
    // is its own business only for the five operations, so it is also waived here)
    assert!(ok.failures().is_empty(), "{:#?}", ok.failures());
    assert!(ieee_api_problems(api).is_empty());
    for (tag, extra) in [
        ("i1", "pub fn sin(x: f64) -> f64 { x }\n"),
        ("i2", "pub mod m {}\n"),
        ("i3", "pub use core::fmt::Debug;\n"),
        ("i4", "#[macro_export]\nmacro_rules! m { () => {}; }\n"),
        ("i5", "pub struct S;\n"),
        ("i6", "mod hidden;\n"),
    ] {
        let ws = Ws::named(
            tag,
            "oa-ieee",
            "core",
            &[("src/lib.rs", &format!("{api}{extra}"))],
        );
        assert!(
            has(&ws.failures(), "B10: the public API of oa-ieee"),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
    // a missing function fails too, and only oa-ieee gets the waiver
    let ws = Ws::named(
        "i7",
        "oa-ieee",
        "core",
        &[("src/lib.rs", "pub fn sqrt(v: f64) -> f64 { v.sqrt() }\n")],
    );
    assert!(has(&ws.failures(), "B10: the public API of oa-ieee"));
    let ws = Ws::named(
        "i8",
        "fx-a",
        "core",
        &[("src/lib.rs", "pub fn sqrt(v: f64) -> f64 { v.sqrt() }\n")],
    );
    assert!(has(&ws.failures(), "B8: the standard library"));
}

// REQ: OA-TRC-221
#[test]
fn the_float_list_is_the_probe_set_and_every_call_form_is_caught() {
    // Derived on 1.89.0 by compiling a throwaway no_std probe that called every f64 (and f32)
    // method of std's docs and keeping those rustc reports as "no method named".
    let probe = [
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
    assert_eq!(oa_trace::classcheck::FLOAT_METHODS, probe);
    for (tag, src, tok) in [
        ("g0", "pub fn f(x: f64) -> f64 { x.exp2() }\n", "exp2"),
        (
            "g1",
            "pub fn f(x: f64) -> (f64, f64) { x.sin_cos() }\n",
            "sin_cos",
        ),
        (
            "g2",
            "pub fn f(x: f64) -> f64 { x.asinh() + x.acosh() + x.atanh() }\n",
            "asinh",
        ),
        (
            "g3",
            "pub fn f(x: f64) -> f64 { x.abs_sub(1.0) }\n",
            "abs_sub",
        ),
        (
            "g4",
            "pub fn f(x: f64) -> f64 { x.round_ties_even() }\n",
            "round_ties_even",
        ),
        ("g5", "pub fn f(x: f64) -> f64 { <f64>::sqrt(x) }\n", "sqrt"),
        (
            "g6",
            "pub trait T { fn f(x: f64) -> f64 { Self::sin(x) } }\n",
            "sin",
        ),
        (
            "g7",
            "pub fn f<R: Copy>(x: f64) -> f64 { R::sin(x) }\n",
            "sin",
        ),
        (
            "g8",
            "pub fn f(x: &[f64]) { let _ = x.iter().copied().map(f64::sin); }\n",
            "sin",
        ),
        ("g9", "pub fn f(x: f32) -> f32 { f32::exp2(x) }\n", "exp2"),
        (
            "g10",
            "pub fn f(x: i64) -> i64 { x.rem_euclid(3) }\n",
            "rem_euclid",
        ),
        (
            "g11",
            "pub fn f(x: f64) -> f64 { f64::powi(x, 2) }\n",
            "powi",
        ),
    ] {
        let f = core_ws(tag, src).failures();
        assert!(
            f.iter()
                .any(|m| m.contains(&format!(": {tok}: B8: the standard library"))),
            "{tag}: {f:#?}"
        );
    }
    // oa_ieee and oa_math paths, the integer path form, and plain names pass
    let ws = core_ws(
        "g12",
        "pub fn f(x: f64, a: i64) -> f64 { let _ = i64::rem_euclid(a, 3); let _ = u8::div_euclid(7, 2); oa_ieee::sqrt(x) + oa_math::pow(x, x) + oa_math::sin(x) }\n",
    );
    assert!(
        !has(&ws.failures(), "B8: the standard library"),
        "{:#?}",
        ws.failures()
    );
    // the alternatives are named
    let f = core_ws(
        "g13",
        "pub fn f(x: f64) -> f64 { x.powf(2.0) + x.sqrt() + x.sin() }\n",
    )
    .failures();
    assert!(
        has(&f, "use oa_math::pow") && has(&f, "use oa_ieee::sqrt") && has(&f, "use oa_math::sin")
    );
}

// REQ: OA-TRC-222
#[test]
fn macros_raw_identifiers_target_code_pointers_and_pointer_formats_are_rejected() {
    for (tag, src, tok) in [
        ("b0", "macro_rules! m { () => {}; }\n", "macro_rules"),
        (
            "b1",
            "pub fn f() { let r#type = 1; let _ = r#type; }\n",
            "r#type",
        ),
        (
            "b2",
            "pub fn f() -> bool { cfg!(target_os = \"linux\") }\n",
            "target_os",
        ),
        (
            "b3",
            "#[cfg(target_pointer_width = \"64\")]\npub fn f() {}\n",
            "target_pointer_width",
        ),
        (
            "b4",
            "#[cfg_attr(target_endian = \"big\", derive(Clone))]\npub struct S;\n",
            "target_endian",
        ),
        ("b5", "pub fn f(_p: *const u8) {}\n", "*const"),
        ("b6", "pub fn f(_p: *mut u8) {}\n", "*mut"),
        (
            "b7",
            "pub fn f(x: &u8) -> core::fmt::Arguments<'_> { format_args!(\"{:p}\", x) }\n",
            "{:p}",
        ),
        (
            "b8",
            "pub fn f() -> &'static str { \"a {x:#18p} b\" }\n",
            "{:p}",
        ),
        ("b9", "#[cfg(unix)]\npub fn f() {}\n", "unix"),
    ] {
        let f = core_ws(tag, src).failures();
        assert!(has(&f, &format!("lib.rs:1: {tok}")), "{tag}: {f:#?}");
    }
    // the `$` of a fragment is rejected on its own
    assert!(tokens("pub fn f() { let _ = $x; }\n").contains(&"$".to_string()));
    // none of these is a hit: references, multiplication, other formats, comments and cfg(test)
    let ws = core_ws(
        "b10",
        "// {:p} *const r#x macro_rules!\npub fn f(a: u8, b: u8) -> u8 { let _s = \"{:?} {:x} {p}\"; let _ = &a as *const u8 as usize; a * b }\n",
    );
    let f = ws.failures();
    assert!(has(&f, "*const"), "{f:#?}");
    assert!(
        !has(&f, "{:p}") && !has(&f, "macro_rules") && !has(&f, "r#x"),
        "{f:#?}"
    );
    let ws = core_ws("b11", "#[cfg(all(test, target_os = \"linux\"))]\nmod t { }\npub fn f(a: u8, b: u8) -> u8 { a * b }\n");
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-223
#[test]
fn a_core_lib_rs_declares_no_main_or_bin_module() {
    for (tag, lib) in [
        ("c0", "mod main;\n"),
        ("c1", "pub mod bin;\n"),
        ("c2", "pub(crate) mod main { pub fn f() {} }\n"),
        ("c3", "mod bin { }\n"),
    ] {
        let ws = core_ws(tag, lib);
        ws.put("crates/fx-a/src/main.rs", "use std::fs;\n");
        ws.put("crates/fx-a/src/bin.rs", "use std::fs;\n");
        let f = ws.failures();
        assert!(has(&f, "lib.rs:1: mod "), "{tag}: {f:#?}");
    }
}

// REQ: OA-TRC-224
#[test]
fn a_test_only_module_file_inside_an_inline_module_is_scanned() {
    let ws = core_ws("i0", "mod outer {\n    #[cfg(test)]\n    mod t;\n}\n");
    ws.put("crates/fx-a/src/outer/t.rs", "use std::fs;\n");
    assert!(
        has(&ws.failures(), "src/outer/t.rs:1: std::fs"),
        "{:#?}",
        ws.failures()
    );
    // at the top level of the file it is still skipped
    let ws = core_ws("i1", "#[cfg(test)]\nmod t;\n");
    ws.put("crates/fx-a/src/t.rs", "use std::fs;\n");
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-225
#[test]
fn dependencies_resolve_through_the_workspace_table_lib_names_and_fail_when_unresolved() {
    let head = "[package]\nname = \"fx-a\"\n\n[package.metadata.openatca]\nclass = \"core\"\n\n";
    let root = "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\"]\n\n[workspace.dependencies]\nfx-b = { path = \"crates/fx-b\" }\n";
    // `workspace = true`, in both spellings, reaches the shell crate through the root table
    for (tag, dep) in [
        ("w0", "[dependencies]\nfx-b.workspace = true\n"),
        ("w1", "[dependencies]\nfx-b = { workspace = true }\n"),
        ("w2", "[dependencies.fx-b]\nworkspace = true\n"),
    ] {
        let ws = core_ws(tag, "pub fn f() {}\n");
        ws.put("Cargo.toml", root);
        ws.put("crates/fx-a/Cargo.toml", &format!("{head}{dep}"));
        assert!(
            has(&ws.failures(), "core crate depends on shell crate fx-b"),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
    // a `workspace = true` entry the root does not have
    let ws = core_ws("w3", "pub fn f() {}\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}[dependencies]\nfx-b.workspace = true\n"),
    );
    assert!(
        has(&ws.failures(), "no [workspace.dependencies] entry"),
        "{:#?}",
        ws.failures()
    );
    // an outside crate, and an outside one in a split crate, cannot be resolved to a workspace crate
    let ws = core_ws("w4", "pub fn f() {}\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}[dependencies]\nserde = \"1\"\n"),
    );
    assert!(
        has(
            &ws.failures(),
            "serde: B8: dependency \"serde\" cannot be resolved"
        ),
        "{:#?}",
        ws.failures()
    );
    let ws = split_ws("w5", SPLIT_LIB, "pub fn f() {}\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        "[package]\nname = \"fx-a\"\n\n[package.metadata.openatca]\nclass = \"split\"\n\n[dependencies]\nserde = \"1\"\nfx-b = { path = \"../fx-b\" }\n",
    );
    let f = ws.failures();
    assert!(has(&f, "serde: B8: dependency"), "{f:#?}");
    assert!(
        !has(&f, "fx-b: B8"),
        "a split crate may depend on a shell crate: {f:#?}"
    );
    // a `[lib] name` is the name source uses
    let ws = core_ws("w6", "pub fn f() { let _ = other_b::go(); }\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        &format!("{head}[dependencies]\nfx-b = {{ path = \"../fx-b\" }}\n"),
    );
    ws.put(
        "crates/fx-b/Cargo.toml",
        "[package]\nname = \"fx-b\"\n\n[lib]\nname = \"other_b\"\n\n[package.metadata.openatca]\nclass = \"shell\"\n",
    );
    assert!(
        has(&ws.failures(), "lib.rs:1: other_b: B8: names a shell"),
        "{:#?}",
        ws.failures()
    );
}

// REQ: OA-TRC-226
#[test]
fn self_super_counts_as_super() {
    for (tag, core) in [
        ("s0", "use self::super::util::r;\n"),
        ("s1", "pub fn f() { self::super::g(); }\n"),
        ("s2", "pub fn f() { self::super::super::g(); }\n"),
    ] {
        let ws = split_ws(tag, SPLIT_LIB, core);
        assert!(
            has(
                &ws.failures(),
                "super: B8: `super::` reaches the crate root"
            ),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
}

// REQ: OA-TRC-227
#[test]
fn no_std_must_be_a_top_of_file_inner_attribute() {
    let ws = Ws::new(
        "n1",
        "core",
        &[("src/lib.rs", "#![no_std]\npub fn f() {}\n")],
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    for bad in [
        "pub fn f() {}\n#![no_std]\n",
        "mod m { #![no_std] }\n",
        "#[cfg(any())]\nmod m { #![no_std] }\npub fn f() {}\n",
        "#[no_std]\npub fn f() {}\n",
        "#![forbid(unsafe_code)]\npub fn f() {}\n#![no_std]\n",
    ] {
        ws.put("crates/fx-a/src/lib.rs", &format!("{bad}{GUARD}"));
        assert!(
            has(
                &ws.failures(),
                "fx-a: B10: src/lib.rs of a core crate must have `#!["
            ),
            "{bad}: {:#?}",
            ws.failures()
        );
    }
    // other inner attributes may come first
    ws.put(
        "crates/fx-a/src/lib.rs",
        &format!(
            "#![forbid(unsafe_code)]\n#![cfg_attr(not(test), no_std)]\npub fn f() {{}}\n{GUARD}"
        ),
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-228
#[test]
fn each_oa_ieee_function_body_is_exactly_the_one_method_call() {
    let f = |body: &str| {
        format!(
            "pub fn sqrt(v: f64) -> f64 {{ {body} }}\npub fn floor(v: f64) -> f64 {{ v.floor() }}\npub fn ceil(v: f64) -> f64 {{ v.ceil() }}\npub fn trunc(v: f64) -> f64 {{ v.trunc() }}\npub fn round(v: f64) -> f64 {{ v.round() }}\n"
        )
    };
    assert!(ieee_api_problems(&f("v.sqrt()")).is_empty());
    for bad in [
        "v.sqrt() + v.sin()",
        "v.floor()",
        "v.sqrt().abs()",
        "{ v.sqrt() }",
        "oa_x::go(v); v.sqrt()",
        "",
    ] {
        let hits = ieee_api_problems(&f(bad));
        assert!(
            hits.iter()
                .any(|h| h.message.contains("exactly `v.<same name>()`")),
            "{bad}: {hits:?}"
        );
    }
}

// REQ: OA-TRC-229
#[test]
fn an_exclude_entry_that_does_not_normalise_fails() {
    let ws = core_ws("x0", "pub fn f() {}\n");
    ws.put(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\"]\nexclude = [\"../outside\"]\n",
    );
    assert!(
        has(
            &ws.failures(),
            "exclude entry \"../outside\" leaves the workspace"
        ),
        "{:#?}",
        ws.failures()
    );
}

/// The public function names of an `oa-math` or `oa-ieee` source: `pub fn name` and the names in
/// `pub use module::{a, b};` or `pub use module::a;` lines of its `lib.rs`, plus `extra` files.
fn public_functions(crate_dir: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let text = fs::read_to_string(root.join(crate_dir).join("src/lib.rs")).unwrap();
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("pub fn ") {
        let name: String = rest[at + 7..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        out.push(name);
        rest = &rest[at + 7..];
    }
    for line in text
        .lines()
        .filter(|l| l.trim_start().starts_with("pub use "))
    {
        let tail = line.rsplit("::").next().unwrap_or("");
        out.extend(
            tail.trim_matches(|c: char| "{}; ".contains(c))
                .split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
        );
    }
    out
}

// REQ: OA-TRC-218
#[test]
fn every_suggested_float_alternative_is_a_real_public_function() {
    let math = public_functions("oa-math");
    let ieee = public_functions("oa-ieee");
    assert!(math.contains(&"pow".to_string()) && math.contains(&"rem_euclid".to_string()));
    assert!(ieee.contains(&"sqrt".to_string()) && ieee.len() == 5);
    for &w in FLOAT_METHODS {
        let advice = float_method_advice(w);
        let mut named = 0;
        for (prefix, api) in [("oa_math::", &math), ("oa_ieee::", &ieee)] {
            for (at, _) in advice.match_indices(prefix) {
                let name: String = advice[at + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                assert!(
                    api.contains(&name),
                    "`{w}`: advice names {prefix}{name}: {advice}"
                );
                named += 1;
            }
        }
        // a method with nothing exact to name must say what to write instead
        assert!(named > 0 || advice.contains("write"), "`{w}`: {advice}");
    }
    assert!(float_method_advice("powf").contains("oa_math::pow"));
    assert!(float_method_advice("div_euclid").contains("`i64::div_euclid(a, b)`"));
    assert!(float_method_advice("rem_euclid").contains("for a float use oa_math::rem_euclid"));
}

// ---------------------------------------------------------------------------------------------
// Round two: bypasses a red-team pass proved on a10ff92, each with its planted violation.

/// A split crate with the given `lib.rs`, core module and shell module.
fn split_with(tag: &str, lib: &str, core: &str, shell: &str) -> Ws {
    Ws::new(
        tag,
        "split",
        &[
            ("src/lib.rs", lib),
            ("src/core/mod.rs", core),
            ("src/shell/mod.rs", shell),
        ],
    )
}

// REQ: OA-TRC-230
#[test]
fn an_attribute_on_a_split_module_or_an_exported_shell_macro_cannot_reach_the_core() {
    let shell_macro = "macro_rules! io { () => { std::fs::read(\"x\") }; }\n";
    // `#[macro_use]` on `mod shell` (or any attribute) makes the macro visible to what follows
    for (tag, lib) in [
        ("h0", "#[macro_use]\nmod shell;\nmod core;\n"),
        ("h1", "#[macro_use]\nmod core;\nmod shell;\n"),
        ("h2", "#[allow(unused)]\nmod shell;\nmod core;\n"),
        (
            "h3",
            "#[cfg_attr(unix, macro_use)]\nmod shell;\nmod core;\n",
        ),
        (
            "h4",
            "#![allow(non_camel_case_types)]\nmod core;\nmod shell;\n",
        ),
        ("h5", "mod core;\n#[doc(hidden)] #[inline]\nmod shell;\n"),
    ] {
        let ws = split_with(tag, lib, "pub fn f() { let _ = io!(); }\n", shell_macro);
        assert!(
            has(&ws.failures(), "B7: lib.rs of a split crate carries only"),
            "{tag}: {:#?}",
            ws.failures()
        );
        assert!(!lib_rs_problems(lib).is_empty(), "{tag}");
    }
    // the attributes a split lib.rs needs are fine
    let ok = "#![forbid(unsafe_code)]\n#![doc = include_str!(\"../README.md\")]\n#[doc(hidden)]\npub mod core;\npub mod shell;\n#[cfg(test)]\n#[macro_use]\nmod tests;\n";
    assert!(lib_rs_problems(ok).is_empty(), "{:?}", lib_rs_problems(ok));
    // `#[macro_export]` in the shell puts the macro at the crate root, where core could name it
    let ws = split_with(
        "h6",
        SPLIT_LIB,
        "pub fn f() {}\n",
        &format!("#[macro_export]\n{shell_macro}"),
    );
    assert!(
        has(&ws.failures(), "B7: a shell macro cannot be exported"),
        "{:#?}",
        ws.failures()
    );
    // a local macro in the shell stays legal
    let ws = split_with("h7", SPLIT_LIB, "pub fn f() {}\n", shell_macro);
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // and core code imports or exports no macro by attribute either
    for (tag, src) in [
        ("h8", "#[macro_use]\nmod m { }\npub fn f() {}\n"),
        ("h9", "#[macro_export]\nfn g() {}\n"),
    ] {
        let ws = split_with(tag, SPLIT_LIB, src, "pub fn s() {}\n");
        assert!(
            has(&ws.failures(), "B8: core code imports no macro")
                || has(&ws.failures(), "B8: core code exports no macro"),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
}

// REQ: OA-TRC-231
#[test]
fn shell_code_may_not_impl_a_core_type_or_a_core_trait() {
    let core = "pub struct Rec { pub x: u8 }\npub trait Tr: sealed::Sealed { fn t(&self) -> u8; }\nmod sealed { pub trait Sealed {} }\n";
    for (tag, shell) in [
        // an inherent impl and a trait impl on a core type, imported
        ("i0", "use crate::core::Rec;\nimpl Rec { pub fn go(&self) -> u8 { 1 } }\n"),
        ("i1", "use crate::core::Rec;\nimpl Default for Rec { fn default() -> Rec { Rec { x: 0 } } }\n"),
        // a core trait implemented for a shell type
        ("i2", "use crate::core::Tr;\npub struct S;\nimpl Tr for S { fn t(&self) -> u8 { 0 } }\n"),
        // spelled by path, through a rename and through a type alias
        ("i3", "impl crate::core::Rec { pub fn go(&self) {} }\n"),
        ("i4", "use crate::core::Rec as R;\nimpl R { pub fn go(&self) {} }\n"),
        ("i5", "type A = crate::core::Rec;\nimpl A { pub fn go(&self) {} }\n"),
        ("i6", "use crate::core::Rec;\ntype A = Rec;\ntype B = A;\nimpl B { pub fn go(&self) {} }\n"),
        // on a reference, generic over a lifetime, with a where clause, inside a function
        ("i7", "use crate::core::Rec;\npub trait Own { fn o(&self); }\nimpl<'a> Own for &'a Rec { fn o(&self) {} }\n"),
        ("i8", "use super::super::core::Tr;\npub struct S;\nimpl Tr for S where S: Sized { fn t(&self) -> u8 { 0 } }\n"),
        ("i9", "use crate::core::Rec;\npub fn f() { impl Rec { fn g(&self) {} } }\n"),
    ] {
        let ws = split_with(tag, SPLIT_LIB, core, shell);
        assert!(
            has(&ws.failures(), "B7: shell code `impl`s the core"),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
    // what a shell may do: its own types, std traits on them, a core type as a trait argument
    let ws = split_with(
        "i10",
        SPLIT_LIB,
        core,
        "use crate::core::Rec;\npub struct Own;\nimpl Own { pub fn new() -> Own { Own } }\nimpl std::fmt::Display for Own { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(\"o\") } }\nimpl From<Rec> for Own { fn from(_: Rec) -> Own { Own } }\npub trait Shelly { fn s(&self) -> impl Iterator<Item = u8>; }\nimpl<T: crate::core::Tr> Shelly for Vec<T> { fn s(&self) -> impl Iterator<Item = u8> { std::iter::empty() } }\n#[cfg(test)]\nmod tests { use crate::core::Rec; impl Rec { fn t(&self) {} } }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // a shell type with the same name as a core type is its own
    let ws = split_with(
        "i11",
        SPLIT_LIB,
        core,
        "pub struct Rec;\nimpl Rec { pub fn go(&self) {} }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-232
#[test]
fn every_core_and_split_crate_takes_the_workspace_lints_and_forbids_unsafe() {
    for class in ["core", "split"] {
        let ws = if class == "core" {
            core_ws("u0", "pub fn f() {}\n")
        } else {
            split_ws("u0", SPLIT_LIB, "pub fn f() {}\n")
        };
        assert!(ws.failures().is_empty(), "{class}: {:#?}", ws.failures());
        // no `[lints] workspace = true`, or lints of its own that could allow `unsafe_code`
        let head = format!(
            "[package]\nname = \"fx-a\"\n\n[package.metadata.openatca]\nclass = \"{class}\"\n"
        );
        for tail in [
            "",
            "\n[lints.rust]\nunsafe_code = \"allow\"\n",
            "\n[lints]\nworkspace = false\n",
        ] {
            ws.put("crates/fx-a/Cargo.toml", &format!("{head}{tail}"));
            assert!(
                has(
                    &ws.failures(),
                    "A0: a core or split crate declares `[lints] workspace = true`"
                ),
                "{class} {tail:?}: {:#?}",
                ws.failures()
            );
        }
    }
    // no `#![forbid(unsafe_code)]` at the top of lib.rs: absent, in a comment, in a string,
    // after an item, in the wrong form
    for (tag, class, lib) in [
        (
            "u1",
            "core",
            "#![cfg_attr(not(test), no_std)]\npub fn f() {}\n",
        ),
        (
            "u2",
            "core",
            "#![cfg_attr(not(test), no_std)]\n// #![forbid(unsafe_code)]\npub fn f() {}\n",
        ),
        (
            "u3",
            "core",
            "#![cfg_attr(not(test), no_std)]\npub fn f() {}\n#![forbid(unsafe_code)]\n",
        ),
        (
            "u4",
            "core",
            "#![cfg_attr(not(test), no_std)]\n#![deny(unsafe_code)]\npub fn f() {}\n",
        ),
        ("u5", "split", "mod core;\nmod shell;\n"),
        (
            "u6",
            "split",
            "// #![forbid(unsafe_code)]\nmod core;\nmod shell;\n",
        ),
        (
            "u7",
            "split",
            "mod core;\nmod shell;\npub const S: &str = \"#![forbid(unsafe_code)]\";\n",
        ),
    ] {
        let files: Vec<(&str, &str)> = if class == "core" {
            vec![("src/lib.rs", "pub fn f() {}\n")]
        } else {
            vec![
                ("src/lib.rs", "pub fn f() {}\n"),
                ("src/core/mod.rs", "pub fn f() {}\n"),
                ("src/shell/mod.rs", "pub fn s() {}\n"),
            ]
        };
        let ws = Ws::new(tag, class, &files);
        ws.put("crates/fx-a/src/lib.rs", lib);
        assert!(
            has(&ws.failures(), "A0: src/lib.rs of a"),
            "{tag}: {:#?}",
            ws.failures()
        );
    }
    // oa-ieee is held to it too
    let api = "pub fn sqrt(v: f64) -> f64 { v.sqrt() }\npub fn floor(v: f64) -> f64 { v.floor() }\npub fn ceil(v: f64) -> f64 { v.ceil() }\npub fn trunc(v: f64) -> f64 { v.trunc() }\npub fn round(v: f64) -> f64 { v.round() }\n";
    let ws = Ws::named("u8", "oa-ieee", "core", &[("src/lib.rs", api)]);
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    ws.put("crates/fx-a/src/lib.rs", api);
    assert!(
        has(&ws.failures(), "A0: src/lib.rs of a core crate"),
        "{:#?}",
        ws.failures()
    );
    // core files may not hold the tokens that open the way to unsafe or foreign code
    for (src, want) in [
        ("pub fn f() { unsafe { } }\n", "unsafe"),
        ("pub unsafe fn f() {}\n", "unsafe"),
        ("extern \"C\" { fn g(); }\n", "extern"),
        ("pub extern \"C\" fn f() {}\n", "extern"),
        ("pub extern fn f() {}\n", "extern"),
        ("#[link(name = \"m\")]\nextern \"C\" {}\n", "#[link"),
        ("#[no_mangle]\npub fn f() {}\n", "no_mangle"),
        ("#[unsafe(no_mangle)]\npub fn f() {}\n", "no_mangle"),
        ("#[export_name = \"x\"]\npub fn f() {}\n", "export_name"),
        (
            "#[link_section = \".x\"]\nstatic X: u8 = 0;\n",
            "link_section",
        ),
    ] {
        assert!(
            tokens(src).iter().any(|t| t == want),
            "{src}: {:?}",
            tokens(src)
        );
    }
    // the lint name and a comment or string are not the keyword
    assert!(tokens(
        "#![forbid(unsafe_code)]\n// unsafe extern \"C\"\npub const S: &str = \"unsafe\";\n"
    )
    .is_empty());
}

// REQ: OA-TRC-233
#[test]
fn a_lowercase_alias_cannot_stand_for_f64_and_an_unknown_qualifier_fails_closed() {
    // the red team's case: the type-name lint silenced, a lowercase alias, a float method
    let src =
        "#[allow(non_camel_case_types)]\ntype fl = f64;\npub fn f(x: f64) -> f64 { fl::sin(x) }\n";
    let t = tokens(src);
    for want in ["non_camel_case_types", "type fl", "sin"] {
        assert!(t.iter().any(|x| x == want), "{want}: {t:?}");
    }
    let ws = core_ws("a0", src);
    assert!(
        has(&ws.failures(), "B8: the standard library's float `sin`"),
        "{:#?}",
        ws.failures()
    );
    for (src, want) in [
        (
            "#![allow(nonstandard_style)]\npub fn f() {}\n",
            "nonstandard_style",
        ),
        ("#![allow(bad_style)]\npub fn f() {}\n", "bad_style"),
        (
            "#[cfg_attr(unix, allow(non_camel_case_types))]\ntype x = u8;\n",
            "non_camel_case_types",
        ),
        ("type fl = f64;\n", "type fl"),
        ("type fl<T> = Vec<T>;\n", "type fl"),
        ("type FL_X = f64;\n", "type FL_X"),
        // a CamelCase alias is allowed, but a float method through it is still a failure
        (
            "type Fl = f64;\npub fn f(x: f64) -> f64 { Fl::sin(x) }\n",
            "sin",
        ),
        // an unknown lowercase qualifier fails closed
        ("pub fn f(x: f64) -> f64 { fl::sin(x) }\n", "sin"),
        ("pub fn f(x: f64) -> f64 { a::b::sqrt(x) }\n", "sqrt"),
    ] {
        assert!(
            tokens(src).iter().any(|x| x == want),
            "{src}: {:?}",
            tokens(src)
        );
    }
    assert!(
        tokens("type Fl = f64;\ntype Items = Vec<u8>;\npub fn f(x: Fl) -> Fl { x }\n").is_empty()
    );
    // a qualifier is known when it is oa_math or oa_ieee, `self`/`super`/`crate`, a module the
    // crate declares, or an integer type for the Euclidean pair
    let ws = core_ws(
        "a1",
        "mod dd { pub fn sqrt(x: f64) -> f64 { x } }\npub fn f(x: f64) -> f64 { dd::sqrt(x) + self::dd::sqrt(x) + crate::dd::sqrt(x) + oa_math::sin(x) + oa_ieee::floor(x) }\npub fn g(a: i64) -> i64 { i64::rem_euclid(a, 3) }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // a split crate's own core modules are known too
    let ws = split_ws(
        "a2",
        SPLIT_LIB,
        "pub mod dd { pub fn sqrt(x: f64) -> f64 { x } }\npub fn f(x: f64) -> f64 { dd::sqrt(x) }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-234
#[test]
fn addresses_platform_sizes_and_built_format_strings_are_banned_in_core() {
    for (src, want) in [
        (
            "pub fn f(x: &[u8]) -> usize { x.as_ptr() as usize }\n",
            "as_ptr",
        ),
        (
            "pub fn f(x: &mut [u8]) { let _ = x.as_mut_ptr(); }\n",
            "as_mut_ptr",
        ),
        ("pub fn f(p: u8) -> usize { p.addr() }\n", ".addr("),
        (
            "pub fn f(p: u8) { let _ = p.expose_provenance(); }\n",
            "expose_provenance",
        ),
        (
            "pub fn f(a: &u8, b: &u8) -> bool { ptr::eq(a, b) }\n",
            "ptr::eq",
        ),
        (
            "pub fn f(a: &u8, b: &u8) -> bool { Rc::ptr_eq(a, b) }\n",
            "ptr_eq",
        ),
        ("pub const M: usize = usize::MAX;\n", "usize::MAX"),
        ("pub const M: isize = isize::MAX;\n", "isize::MAX"),
        ("pub const M: u32 = usize::BITS;\n", "usize::BITS"),
        (
            "pub fn f() -> usize { core::mem::size_of::<u8>() }\n",
            "size_of",
        ),
        (
            "pub const S: usize = core::mem::size_of::<u8>();\n",
            "size_of",
        ),
        (
            "pub fn f() -> usize { core::mem::align_of::<u8>() }\n",
            "align_of",
        ),
        (
            "pub fn f() { assert!(core::mem::size_of::<u8>() == 1); }\n",
            "size_of",
        ),
        (
            "pub const S: &str = concat!(\"{\", \":p\", \"}\");\n",
            "concat!",
        ),
        ("pub const S: &str = stringify!(x);\n", "stringify!"),
        (
            "pub fn f() { let _ = core::fmt::format(format_args!(\"{}\", 1)); }\n",
            "format_args!",
        ),
        ("pub fn f(g: u8) { let _ = g as fn(u8); }\n", "as fn"),
    ] {
        assert!(
            tokens(src).iter().any(|x| x == want),
            "{src}: {:?}",
            tokens(src)
        );
    }
    // a size is allowed in the compile-time size guard, over several lines too
    let guard = "pub struct S { pub a: u32 }\nconst _: () = assert!(core::mem::size_of::<S>() == 4);\nconst _: () = assert!(\n    core::mem::size_of::<S>() == 4 && core::mem::align_of::<S>() == 4,\n    \"S\"\n);\n";
    assert!(tokens(guard).is_empty(), "{:?}", tokens(guard));
    // but not just anything with `const _`
    assert!(!tokens("const _: u8 = core::mem::size_of::<u8>() as u8;\n").is_empty());
    assert!(!tokens("const _: () = { let _ = core::mem::size_of::<u8>(); };\n").is_empty());
    // the real workspace carries neither (the report states its sizes as constants)
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rep = check(&root);
    assert!(
        !has(&rep.failures, "size_of") && !has(&rep.failures, "usize::MAX"),
        "{:#?}",
        rep.failures
    );
}

// REQ: OA-TRC-235
#[test]
fn a_core_pub_fn_with_a_callback_or_a_bound_needs_a_reviewed_allowlist_line() {
    let sigs = [
        ("f", "pub fn f(g: impl Fn(u8) -> u8) -> u8 { g(1) }"),
        ("h", "pub fn h<T: Ord>(v: &mut [T]) { let _ = v; }"),
        (
            "d",
            "pub fn d(x: &dyn core::fmt::Debug) -> usize { let _ = x; 0 }",
        ),
        ("w", "pub fn w<T>(x: T) -> T where T: Clone { x }"),
        (
            "p",
            "type G = Box<dyn FnMut()>;\npub fn p(g: G) { let _ = g; }",
        ),
        ("b", "pub fn b(g: Box<dyn FnOnce() -> u8>) -> u8 { g() }"),
        ("c", "pub const fn c<F: Fn()>(g: F) { let _ = g; }"),
    ];
    for (name, sig) in sigs {
        let ws = core_ws("c0", &format!("{sig}\n"));
        assert!(
            has(
                &ws.failures(),
                &format!("{name}: B5: core code lets the shell hand in behaviour")
            ),
            "{sig}: {:#?}",
            ws.failures()
        );
        // a line in the allowlist with a reason lets that function through
        ws.put(
            CALLBACKS,
            &format!("fx-a lib.rs::{name} reason: the caller passes a pure comparison\n"),
        );
        assert!(ws.failures().is_empty(), "{sig}: {:#?}", ws.failures());
    }
    // not boundaries: a plain generic, a `Sized` bound, an `impl` in return position,
    // a method without a bound in a type's impl, and anything the shell cannot name: a private
    // or `pub(crate)` fn may take closures, `impl Iterator`, `impl Into` and any bound
    let ws = core_ws(
        "c1",
        "pub fn r<T>(x: T) -> T { x }\npub fn s() -> impl Iterator<Item = u8> { core::iter::empty() }\npub struct S;\nimpl S { pub fn m(&self) -> u8 { 0 } }\npub fn h<T: Sized>(x: T) -> T { x }\npub fn z<T: Sized + ?Sized>(x: &T) -> usize { 0 }\npub(crate) fn p(g: impl FnMut()) { let _ = g; }\nfn q(g: impl Fn()) { g() }\nfn i(x: impl Iterator<Item = u8>, y: impl Into<u8>) {}\nfn o<T: Ord>(v: &mut [T]) { let _ = v; }\ntype Cb = Box<dyn Fn()>;\nfn u(f: Cb) { f() }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // a method of an impl block is a signature too
    let ws = core_ws(
        "c2",
        "pub struct S;\nimpl S { pub fn each(&self, mut g: impl FnMut(u8)) { g(0) } }\n",
    );
    assert!(has(&ws.failures(), "each: B5"), "{:#?}", ws.failures());
    // a test-only item is not core
    let ws = core_ws(
        "c3",
        "#[cfg(test)]\nmod t { pub fn f(g: impl Fn()) { g() } }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // the list itself: a stale entry, an entry without a reason and one in the wrong shape fail
    let ws = core_ws("c4", "pub fn r<T>(x: T) -> T { x }\n");
    ws.put(
        CALLBACKS,
        "fx-a lib.rs::gone reason: it used to take a closure\n",
    );
    assert!(
        has(
            &ws.failures(),
            "fx-a lib.rs::gone: B5: listed but no core `pub fn`"
        ),
        "{:#?}",
        ws.failures()
    );
    for bad in [
        "fx-a lib.rs::f\n",
        "fx-a f reason: long enough text\n",
        "fx-a lib.rs::f why\n",
    ] {
        ws.put(CALLBACKS, bad);
        assert!(
            has(&ws.failures(), "is not `crate file.rs::fn reason`"),
            "{bad}: {:#?}",
            ws.failures()
        );
    }
    // the file must exist
    let _ = fs::remove_file(ws.root.join(CALLBACKS));
    assert!(
        has(&ws.failures(), "core-callbacks-allowlist.txt: cannot read"),
        "{:#?}",
        ws.failures()
    );
}

// REQ: OA-TRC-236
#[test]
fn a_floats_bits_in_core_code_pass_through_canon_nan() {
    for src in [
        "pub fn f(x: f64) -> u64 { x.to_bits() }\n",
        "pub fn f() -> u64 { 1.0f64.to_bits() }\n",
        "pub fn f(x: f64, y: f64) -> u64 { (x + y).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { g(x).to_bits() }\n",
        "pub fn f(x: &[f64]) -> u64 { x.iter().map(|v| v.to_bits()).sum() }\n",
        "pub fn f(x: f64) -> u64 { (canon_nan(x) + 1.0).to_bits() }\n",
    ] {
        assert!(
            tokens(src).iter().any(|t| t == "to_bits"),
            "{src}: {:?}",
            tokens(src)
        );
    }
    for src in [
        "use oa_math::canon_nan;\npub fn f(x: f64) -> u64 { canon_nan(x).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { oa_math::canon_nan(x * 2.0).to_bits() }\n",
        "use oa_math::{canon_nan, canon_nan_f32};\npub fn f(x: f64) -> [u8; 8] { canon_nan(x).to_bits().to_le_bytes() }\npub fn g(x: f32) -> u32 { canon_nan_f32(x).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { f64::to_bits(oa_math::canon_nan(x)) }\n",
        "#[cfg(test)]\nmod t { fn f(x: f64) -> u64 { x.to_bits() } }\n",
    ] {
        assert!(tokens(src).is_empty(), "{src}: {:?}", tokens(src));
    }
    // oa-math does bit arithmetic on floats by design
    let src = "pub fn sign(x: f64) -> u64 { x.to_bits() >> 63 }\n";
    let ws = Ws::named("t0", "oa-math", "core", &[("src/lib.rs", src)]);
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    let ws = core_ws("t1", src);
    assert!(
        has(
            &ws.failures(),
            "fx-a/src/lib.rs:1: to_bits: B8: a float's bits"
        ),
        "{:#?}",
        ws.failures()
    );
}

// REQ: OA-TRC-237
#[test]
fn an_unsplit_crates_float_method_count_may_not_rise() {
    let lib = "pub fn f(x: f64) -> f64 { x.sin() + x.sqrt() + f64::exp(x) + oa_math::ln(x) + oa_ieee::floor(x) }\npub fn g(a: i64) -> i64 { i64::rem_euclid(a, 3) }\n#[cfg(test)]\nmod t { fn h(x: f64) -> f64 { x.cos() } }\n#[cfg(test)]\nmod tests;\n";
    let ws = Ws::new(
        "b0",
        "unsplit",
        &[
            ("src/lib.rs", lib),
            ("src/tests.rs", "fn k(x: f64) -> f64 { x.tan() }\n"),
        ],
    );
    let run = |budget: &str| {
        ws.put(BUDGET, budget);
        check_with(&ws.root, &["fx-a"])
    };
    // three calls count: `.sin()`, `.sqrt()`, `f64::exp`; oa_math, oa_ieee, the integer
    // Euclidean remainder and test code do not
    let rep = run("fx-a 3\n");
    assert!(rep.ok(), "{:#?}", rep.failures);
    assert_eq!(rep.float_counts.get("fx-a"), Some(&(3, Some(3))));
    assert!(render_text(&rep).contains("fx-a: 3 (3)"));
    // a count that fell is fine; a count that rose fails
    assert!(run("fx-a 5\n").ok());
    let rep = run("fx-a 2\n");
    assert!(
        has(
            &rep.failures,
            "fx-a: B9: 3 platform float-method calls, the budget is 2"
        ),
        "{:#?}",
        rep.failures
    );
    // no entry, an entry for a crate that is not unsplit, a malformed or doubled entry fail
    assert!(has(&run("").failures, "fx-a: B9: no entry in"));
    assert!(has(
        &run("fx-a 3\nfx-b 0\n").failures,
        "fx-b: B9: listed but not unsplit"
    ));
    assert!(has(&run("fx-a three\n").failures, "is not `crate count`"));
    assert!(has(&run("fx-a 3\nfx-a 3\n").failures, "listed twice"));
    // the file must exist
    let _ = fs::remove_file(ws.root.join(BUDGET));
    assert!(has(
        &check_with(&ws.root, &["fx-a"]).failures,
        "unsplit-float-budget.txt: cannot read"
    ));
    // the real workspace holds its recorded counts
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rep = check(&root);
    for (n, (count, budget)) in &rep.float_counts {
        assert!(
            budget.is_some_and(|b| *count <= b),
            "{n}: {count} {budget:?}"
        );
    }
    assert!(rep.float_counts.contains_key("oa-track"));
}

// REQ: OA-TRC-238
#[test]
fn a_dependency_table_over_several_lines_is_read_as_one_entry() {
    let m = parse_manifest(
        "[package]\nname = \"oa-x\"\n\n[dependencies]\noa-a = {\n    path = \"../oa-a\", # c\n    features = [\"x\"],\n}\noa-b = { path = \"../oa-b\" }\nfoo = { package = \"oa-c\",\n  path = \"../oa-c\" }\n",
    );
    let keys: Vec<&str> = m.deps.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, ["oa-a", "oa-b", "foo"]);
    assert_eq!(m.deps[0].path.as_deref(), Some("../oa-a"));
    assert_eq!(m.deps[2].package.as_deref(), Some("oa-c"));
    assert_eq!(m.deps[2].path.as_deref(), Some("../oa-c"));
    // a split crate may depend on a shell crate through such a table, with no false failure
    let ws = split_ws("l0", SPLIT_LIB, "pub fn f() {}\n");
    ws.put(
        "crates/fx-a/Cargo.toml",
        "[package]\nname = \"fx-a\"\n\n[package.metadata.openatca]\nclass = \"split\"\n\n[lints]\nworkspace = true\n\n[dependencies]\nfx-b = {\n    path = \"../fx-b\",\n}\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-239
#[test]
fn the_pinned_toolchain_is_the_one_ci_installs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tc = fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();
    let lines: Vec<&str> = tc
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .collect();
    assert!(lines.contains(&"channel = \"1.89.0\""), "{tc}");
    assert!(
        lines.contains(&"components = [\"clippy\", \"rustfmt\"]"),
        "{tc}"
    );
    // the workflow installs that channel with those components, and the manifest agrees
    let ci = fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci.contains("rustup toolchain install 1.89.0 --profile minimal --component clippy,rustfmt"),
        "ci.yml does not install the pinned toolchain"
    );
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("rust-version = \"1.89\""));
    // the fuzz workspace sits under the root: it names its own nightly, the one CI installs
    let fuzz = fs::read_to_string(root.join("fuzz/rust-toolchain.toml")).unwrap();
    assert!(fuzz.contains("channel = \"nightly-2026-08-21\""), "{fuzz}");
    assert!(ci.contains("rustup toolchain install nightly-2026-08-21"));
}

// ---------------------------------------------------------------------------------------------
// Round three: what the independent review of round two still got past.

// REQ: OA-TRC-240
#[test]
fn a_turbofish_between_a_method_name_and_its_parentheses_changes_nothing() {
    for (src, want) in [
        ("pub fn f(x: f64) -> f64 { x.sin::<>() }\n", "sin"),
        ("pub fn f(x: f64) -> f64 { x . sin :: < > () }\n", "sin"),
        ("pub fn f(x: f64) -> f64 { x.sqrt::<Vec<u8>>() }\n", "sqrt"),
        (
            "pub fn f(x: f64) -> f64 { x.powi::<Option<fn(u8) -> u8>>(2) }\n",
            "powi",
        ),
        ("pub fn f(p: u8) -> usize { p.addr::<>() }\n", ".addr("),
        ("pub fn f(x: f64) -> u64 { x.to_bits::<>() }\n", "to_bits"),
    ] {
        assert!(
            tokens(src).iter().any(|x| x == want),
            "{src}: {:?}",
            tokens(src)
        );
    }
    // and the unsplit counter counts it
    let lib = "pub fn f(x: f64) -> f64 { x.sin::<>() + x.cos :: <> () }\n";
    let ws = Ws::new("z0", "unsplit", &[("src/lib.rs", lib)]);
    ws.put(BUDGET, "fx-a 2\n");
    let rep = check_with(&ws.root, &["fx-a"]);
    assert_eq!(
        rep.float_counts.get("fx-a"),
        Some(&(2, Some(2))),
        "{:#?}",
        rep.failures
    );
}

// REQ: OA-TRC-241
#[test]
fn callbacks_through_impl_bounds_fn_pointers_and_struct_fields_need_the_allowlist_too() {
    for (name, src) in [
        (
            "call",
            "pub struct W<F>(F);\nimpl<F: Fn()> W<F> { pub fn call(&self) {} }\n",
        ),
        (
            "call",
            "pub struct W<F>(F);\nimpl<F> W<F> where F: Fn() { pub fn call(&self) {} }\n",
        ),
        ("run", "pub fn run(f: fn(u8) -> u8) -> u8 { f(1) }\n"),
        ("run", "pub fn run(f: Option<fn()>) {}\n"),
        ("run", "pub fn run(f: &fn()) {}\n"),
        ("run", "pub fn run() -> fn() { || () }\n"),
        ("S", "pub struct S { pub cb: fn(u8) }\n"),
        ("S", "pub struct S { pub cb: Box<dyn FnMut()> }\n"),
        ("S", "pub struct S(pub Box<dyn Fn()>);\n"),
        ("E", "pub enum E { A(fn()), B }\n"),
        ("T", "pub type T = fn(u8) -> u8;\n"),
    ] {
        let ws = core_ws("z1", src);
        assert!(
            has(
                &ws.failures(),
                &format!("{name}: B5: core code lets the shell hand in behaviour")
            ),
            "{src}: {:#?}",
            ws.failures()
        );
        // every name the check flags is listed (an `impl` block as `impl@Type`, a struct that
        // stores an unbounded parameter by its own name)
        let listed: String = ["W", "impl@W", name]
            .iter()
            .map(|n| {
                format!("fx-a lib.rs::{n} reason: reviewed, the caller passes a pure function\n")
            })
            .collect();
        ws.put(CALLBACKS, &listed);
        let left: Vec<String> = ws
            .failures()
            .into_iter()
            .filter(|f| !f.contains("listed but no core"))
            .collect();
        assert!(left.is_empty(), "{src}: {left:#?}");
    }
    // data-only items and an impl without bounds are not boundaries, nor is a callback in a
    // private field or an unbounded parameter stored in a pub type (core cannot call into it)
    let ws = core_ws(
        "z2",
        "pub struct S { pub a: u8, pub f: f64 }\npub enum E { A(u8) }\npub type T = u32;\npub struct W<F: Sized>(F);\nimpl W<u8> { pub fn get(&self) -> u8 { 0 } }\npub struct P { cb: Box<dyn FnMut()> }\npub struct Q(Box<dyn Fn()>);\npub struct U<T>(T);\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-242
#[test]
fn only_oa_maths_canon_nan_counts_and_every_float_byte_path_goes_through_it() {
    // a local fn, a shadowing binding, another crate's import, a method of that name
    for src in [
        "fn canon_nan(x: f64) -> f64 { x }\npub fn f(x: f64) -> u64 { canon_nan(x).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { let canon_nan = |y: f64| y; canon_nan(x).to_bits() }\n",
        "use crate::util::canon_nan;\npub fn f(x: f64) -> u64 { canon_nan(x).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { canon_nan(x).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { other::canon_nan(x).to_bits() }\n",
        "pub fn f(x: f64) -> u64 { x.canon_nan().to_bits() }\n",
        "pub mod m { pub fn canon_nan_f32(x: f32) -> f32 { x } }\n",
    ] {
        assert!(
            tokens(src).iter().any(|x| x.starts_with("canon_nan")),
            "{src}: {:?}",
            tokens(src)
        );
    }
    // UFCS and the byte forms
    for (src, want) in [
        ("pub fn f(x: f64) -> u64 { f64::to_bits(x) }\n", "to_bits"),
        ("pub fn f(x: f32) -> u32 { f32::to_bits(x) }\n", "to_bits"),
        (
            "pub fn f(x: f64) -> [u8; 8] { f64::to_le_bytes(x) }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(x: f64) -> [u8; 8] { f64::to_be_bytes(x) }\n",
            "to_be_bytes",
        ),
        (
            "pub fn f(x: u32) -> [u8; 8] { (x as f64).to_ne_bytes() }\n",
            "to_ne_bytes",
        ),
        (
            "pub fn f() -> [u8; 8] { 1.5f64.to_le_bytes() }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(x: f64) -> [u8; 8] { x.to_le_bytes() }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(v: &[f64]) -> [u8; 8] { v[0].to_le_bytes() }\n",
            "to_le_bytes",
        ),
        (
            "pub struct S { pub x: f32 }\npub fn f(s: &S) -> [u8; 4] { s.x.to_be_bytes() }\n",
            "to_be_bytes",
        ),
    ] {
        let ws = core_ws("z3", src);
        let want_hit =
            tokens(src).iter().any(|x| x == want) || has(&ws.failures(), &format!("{want}: B8"));
        assert!(want_hit, "{src}: {:#?}", ws.failures());
    }
    // integers are not floats, and the canonical forms pass
    let ws = core_ws(
        "z4",
        "use oa_math::canon_nan;\npub fn f(x: u32, y: f64) -> [u8; 8] { let _ = x.to_le_bytes(); canon_nan(y).to_bits().to_le_bytes() }\npub fn g(v: f64) -> u64 { let n: u32 = 1; let _ = n.to_le_bytes(); 0 }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // a name that is a float in one fn is not a float in another
    let ws = core_ws(
        "z5",
        "pub fn a(v: f64) -> f64 { v }\npub fn b(v: u32) -> [u8; 4] { v.to_le_bytes() }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // oa-math itself may define and use them
    let ws = Ws::named("z6", "oa-math", "core", &[("src/lib.rs", "pub fn canon_nan(x: f64) -> f64 { if x.is_nan() { f64::from_bits(0x7ff8_0000_0000_0000) } else { x } }\n")]);
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-243
#[test]
fn raw_borrows_fmt_pointer_width_aliases_and_debug_assertion_branches_are_banned_in_core() {
    for (src, want) in [
        ("pub fn f(x: &u8) { let _ = &raw const *x; }\n", "&raw"),
        ("pub fn f(x: &mut u8) { let _ = &raw mut *x; }\n", "&raw"),
        ("pub struct S; impl core::fmt::Pointer for S { fn fmt(&self, _f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { Ok(()) } }\n", "Pointer"),
        ("type Wide = usize;\npub const M: usize = Wide::MAX;\n", "type Wide"),
        ("type Narrow = isize;\n", "type Narrow"),
        ("pub const M: usize = <usize>::MAX;\n", "usize::MAX"),
        ("pub fn f() -> u8 { if cfg!(debug_assertions) { 1 } else { 2 } }\n", "debug_assertions"),
        ("#[cfg(debug_assertions)]\npub fn f() {}\n", "debug_assertions"),
        ("#[cfg(not(debug_assertions))]\npub fn f() {}\n", "debug_assertions"),
        ("#[cfg_attr(debug_assertions, inline)]\npub fn f() {}\n", "debug_assertions"),
    ] {
        assert!(tokens(src).iter().any(|x| x == want), "{src}: {:?}", tokens(src));
    }
    // debug_assert! can only panic and stays; a list of usize is not an alias of usize
    assert!(tokens("pub fn f(x: u8) { debug_assert!(x < 9); debug_assert_eq!(x, x); }\ntype Ix = Vec<usize>;\n").is_empty());
    // alloc::format is allowed; hash, cell and rc stay out
    let ws = core_ws("z7", "use alloc::format;\nuse alloc::string::String;\npub fn f(x: u8) -> String { format!(\"{x}\") }\n");
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    for (tag, src) in [
        ("z8", "use core::hash::Hasher;\n"),
        ("z9", "use core::cell::Cell;\n"),
        ("za", "use alloc::rc::Rc;\n"),
    ] {
        assert!(!core_ws(tag, src).failures().is_empty(), "{src}");
    }
}

// ---------------------------------------------------------------------------------------------
// Round four: sealed traits, generic bounds, callbacks anywhere, NaN sign, float bits, address
// names, the 64-bit guard, signed zero.

const SEALED: &str = "mod sealed { pub trait Sealed {} }\n";

// REQ: OA-TRC-244
#[test]
fn a_pub_trait_in_core_is_sealed_or_on_the_reviewed_list() {
    // planted: a `pub trait` the shell can implement
    let ws = core_ws("t0", "pub trait T { fn t(&self) -> u8; }\n");
    assert!(
        has(
            &ws.failures(),
            "T: B5: core code lets the shell hand in behaviour (a `pub trait` that is not sealed"
        ),
        "{:#?}",
        ws.failures()
    );
    // a reviewed allowlist line lets it through, and goes stale when the trait goes
    ws.put(
        CALLBACKS,
        "fx-a lib.rs::T reason: reviewed, every implementor is a core type\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // `pub unsafe trait` is a `pub trait`; `pub(crate)`, private and test-only ones are not
    assert!(has(
        &core_ws("t0b", "pub unsafe trait T {}\n").failures(),
        "T: B5"
    ));
    for (tag, src) in [
        ("t0c", "pub(crate) trait T {}\n"),
        ("t0d", "trait T {}\n"),
        ("t0e", "#[cfg(test)]\nmod t { pub trait T {} }\n"),
    ] {
        assert!(core_ws(tag, src).failures().is_empty(), "{src}");
    }
    // clean: sealed by a supertrait from a private module, however it is named
    for (tag, src) in [
        ("t1", format!("{SEALED}pub trait T: sealed::Sealed {{ fn t(&self) -> u8; }}\nimpl sealed::Sealed for u8 {{}}\n")),
        ("t2", format!("{SEALED}use sealed::Sealed;\npub trait T: Sealed {{}}\n")),
        ("t3", format!("{SEALED}pub trait T: Clone + crate::sealed::Sealed {{}}\n")),
        ("t4", "pub(crate) mod sealed { pub trait Sealed {} }\npub trait T: sealed::Sealed {}\n".to_string()),
        ("t5", format!("{SEALED}pub trait T<X>: sealed::Sealed where X: Sized {{}}\n")),
    ] {
        let ws = core_ws(tag, &src);
        assert!(ws.failures().is_empty(), "{src}: {:#?}", ws.failures());
    }
    // a supertrait from a public module seals nothing, and neither does a plain std one
    for (tag, src) in [
        (
            "t6",
            "pub mod open { pub trait Marker {} }\npub trait T: open::Marker {}\n",
        ),
        ("t7", "pub trait T: Clone {}\n"),
    ] {
        assert!(has(&core_ws(tag, src).failures(), "T: B5"), "{src}");
    }
    // the seal handed out again: a `pub use` of the sealing trait, a blanket impl of it
    let ws = core_ws(
        "t8",
        &format!("{SEALED}pub use sealed::Sealed;\npub trait T: sealed::Sealed {{}}\n"),
    );
    assert!(
        has(
            &ws.failures(),
            "Sealed: B5: core code lets the shell hand in behaviour (`pub use` of a sealing trait"
        ),
        "{:#?}",
        ws.failures()
    );
    let ws = core_ws(
        "t9",
        &format!("{SEALED}pub trait T: sealed::Sealed {{}}\nimpl<X> sealed::Sealed for X {{}}\n"),
    );
    assert!(
        has(
            &ws.failures(),
            "impl@X: B5: core code lets the shell hand in behaviour (a blanket impl"
        ),
        "{:#?}",
        ws.failures()
    );
    // and a private module is not a seal in a crate that re-exports an ordinary item from it
    let ws = core_ws(
        "ta",
        "mod inner { pub struct P { pub x: u8 } }\npub use inner::P;\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-245
#[test]
fn a_reachable_generic_is_bounded_only_by_sized_or_sealed_traits() {
    // planted, each in an item the shell can name (plain `pub`)
    for (name, src) in [
        ("f", "pub fn f<T: Iterator<Item = u8>>(x: T) -> u8 { 0 }\n"),
        (
            "f",
            "pub fn f<T>(x: T) -> u8 where T: Into<u8> { x.into() }\n",
        ),
        ("f", "pub fn f<T: Copy + AsRef<[u8]>>(x: T) -> u8 { 0 }\n"),
        ("f", "pub fn f(x: impl Into<u8>) -> u8 { x.into() }\n"),
        ("f", "pub fn f<T: Ord>(v: &mut [T]) { v.sort(); }\n"),
        // `Copy: Clone`, and `vec![x; n]`, `resize`, `to_vec` and `fill` run a hand-written clone
        ("f", "pub fn f<T: Copy>(x: T) -> T { x }\n"),
        ("f", "pub fn f<T>(x: T) -> T where T: Copy { x }\n"),
        ("f", "pub fn f<T: Eq>(x: T) {}\n"),
        ("f", "pub fn f<T: Clone>(x: T) {}\n"),
        ("f", "pub fn f<T: Default>() -> T { T::default() }\n"),
        ("f", "pub fn f<T: core::fmt::Debug>(x: T) {}\n"),
        ("f", "pub fn f<T: core::hash::Hash>(x: T) {}\n"),
        ("f", "pub fn f<T: PartialOrd>(x: T) {}\n"),
        (
            "f",
            "pub fn f<T>(x: T) -> u8 where for<'a> &'a T: Into<u8> { 0 }\n",
        ),
        ("S", "pub struct S<T: AsRef<u8>>(T);\n"),
        ("E", "pub enum E<T> where T: Iterator { A(T) }\n"),
        (
            "m",
            "pub struct S<T>(T);\nimpl<T: Iterator> S<T> { pub fn m(&self) {} }\n",
        ),
        (
            "m",
            "pub struct S<T>(T);\nimpl<T> S<T> where T: Into<u8> { pub fn m(&self) {} }\n",
        ),
        (
            "m",
            "pub struct S;\nimpl S { pub fn m<X: AsRef<u8>>(&self, x: X) {} }\n",
        ),
        ("T", "pub type T<X: Iterator> = Vec<X>;\n"),
    ] {
        let ws = core_ws("g0", src);
        assert!(
            has(
                &ws.failures(),
                &format!("{name}: B5: core code lets the shell hand in behaviour (")
            ),
            "{src}: {:#?}",
            ws.failures()
        );
        ws.put(
            CALLBACKS,
            &format!("fx-a lib.rs::{name} reason: reviewed, the bound is a pure trait\n"),
        );
        // the allowlist covers the flagged name
        let left = ws.failures();
        assert!(
            left.iter().all(|f| !f.contains(&format!("{name}: B5"))),
            "{src}: {left:#?}"
        );
    }
    // a method of a `pub trait` is reachable
    let ws = core_ws(
        "g0t",
        &format!("{SEALED}pub trait Tr: sealed::Sealed {{ fn m<X: Iterator>(&self, x: X); }}\n"),
    );
    assert!(has(&ws.failures(), "m: B5"), "{:#?}", ws.failures());
    // clean: `Sized`, `?Sized`, lifetimes, const generics, a sealed trait, an unbounded
    // parameter, and any bound at all on an item the shell cannot name
    for (tag, src) in [
        ("g1", "pub fn f<T: Sized + Sized>(x: T) -> T { x }\n".to_string()),
        ("g2", "pub fn f<T: ?Sized>(x: &T) -> usize { 0 }\npub fn g<'a, const N: usize, T: Sized + 'a>(x: &'a [T; N]) -> usize { N }\n".to_string()),
        ("g3", "pub fn f<T>(x: T) -> T where T: Sized, u8: Sized { x }\n".to_string()),
        ("g4", "struct S<T: Ord>(T);\nimpl<T: Ord> S<T> { fn m(&self) {} }\npub(crate) fn p<T: Ord + Clone>(x: T) {}\n".to_string()),
        ("g5", format!("{SEALED}pub trait Sh: sealed::Sealed {{}}\npub fn f<T: Sh>(x: &T) -> usize {{ 0 }}\npub fn g<T>(x: &T) where T: Sh + Sized {{}}\n")),
        ("g6", "pub fn f(x: impl Sized + Sized) -> u8 { 0 }\nfn g(x: impl Clone + Iterator) {}\n".to_string()),
        ("g7", "#[cfg(test)]\nmod t { pub fn f<T: Iterator>(x: T) {} }\n".to_string()),
        ("g8", "pub struct W<T>(T);\npub enum E<T> { A(T), B }\npub struct V<T> { pub x: T }\n".to_string()),
        ("g9", "pub struct W<T: Sized>(T);\n".to_string()),
        ("ga", "impl PartialOrd for X { fn partial_cmp(&self, o: &X) -> Option<core::cmp::Ordering> { None } }\n".to_string()),
    ] {
        let ws = core_ws(tag, &src);
        assert!(ws.failures().is_empty(), "{src}: {:#?}", ws.failures());
    }
}

// REQ: OA-TRC-246
#[test]
fn a_function_pointer_fn_trait_or_dyn_in_a_reachable_item_needs_the_allowlist() {
    for (name, src) in [
        ("q", "pub fn q(g: impl Fn(u8) -> u8) -> u8 { g(1) }\n"),
        ("q", "pub fn q(g: &mut dyn FnMut()) { g() }\n"),
        ("q", "pub fn q(g: Box<dyn FnOnce() -> u8>) -> u8 { g() }\n"),
        ("q", "pub fn q(d: &dyn core::fmt::Debug) {}\n"),
        ("q", "pub fn q(f: fn(u8) -> u8) -> u8 { f(1) }\n"),
        // a private alias that holds one makes every reachable signature that names it fail
        (
            "q",
            "type F = Box<dyn Fn(u8) -> u8>;\npub fn q(f: F) -> u8 { f(1) }\n",
        ),
        (
            "q",
            "type F = fn(u8) -> u8;\npub fn q() -> Option<F> { None }\n",
        ),
        ("S", "type F = fn(u8) -> u8;\npub struct S { pub f: F }\n"),
        (
            "m",
            "pub struct S;\nimpl S { pub fn m(&self, f: &dyn Fn()) { f() } }\n",
        ),
        (
            "x",
            "pub struct S;\nimpl dyn core::fmt::Debug { pub fn x(&self) {} }\n",
        ),
        ("F", "pub type F = fn(u8) -> u8;\n"),
        ("F", "pub type F = Box<dyn Fn(u8) -> u8>;\n"),
        ("S", "pub struct S { pub cb: fn(u8) }\n"),
    ] {
        let ws = core_ws("f0", src);
        assert!(
            has(
                &ws.failures(),
                &format!("{name}: B5: core code lets the shell hand in behaviour")
            ),
            "{src}: {:#?}",
            ws.failures()
        );
    }
    // a method declared in a `pub trait`
    let ws = core_ws(
        "f0t",
        &format!("{SEALED}pub trait Tr: sealed::Sealed {{ fn m(&self, f: fn(u8) -> u8); }}\n"),
    );
    assert!(has(&ws.failures(), "m: B5"), "{:#?}", ws.failures());
    // the allowlist line is by the name of the item that holds it, and covers all of its hits
    let ws = core_ws("f1", "pub fn q(g: &dyn Fn(u8) -> u8) -> u8 { g(1) }\n");
    assert!(!ws.failures().is_empty());
    ws.put(
        CALLBACKS,
        "fx-a lib.rs::q reason: reviewed, called with a pure closure only\n",
    );
    let left = ws.failures();
    assert!(left.is_empty(), "{left:#?}");
    // clean: private items use closures, `dyn`, fn pointers and aliases of them freely; closures
    // in a body, `impl Trait` in return position, a `pub(crate)` fn, test code
    let ws = core_ws(
        "f2",
        "pub fn s(v: &[u8]) -> u8 { v.iter().map(|x| x.wrapping_add(1)).fold(0, |a, b| a ^ b) }\nfn h(x: u8) -> u8 { x }\npub fn r() -> impl Iterator<Item = u8> { core::iter::empty() }\n#[cfg(test)]\nmod t { pub fn q(g: &dyn Fn()) { g() } }\nfn p(g: impl Fn(u8) -> u8) -> u8 { g(1) }\nfn d(g: &mut dyn FnMut()) { g() }\nfn fp(f: fn(u8) -> u8) -> u8 { f(1) }\ntype F = Box<dyn Fn(u8) -> u8>;\nfn a(f: F) -> u8 { f(1) }\nstruct S { cb: fn(u8) }\npub struct P { cb: Box<dyn FnMut()> }\nstatic T: [fn(); 1] = [n];\nfn n() {}\npub(crate) fn pc(g: impl Fn()) { g() }\ntrait Tr { fn m(&self, f: fn(u8) -> u8); }\nimpl dyn core::fmt::Debug { fn x(&self) {} }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-247
#[test]
fn nan_sign_and_total_order_come_from_oa_math_never_from_the_float_methods() {
    for (src, want) in [
        (
            "pub fn f(a: f64, b: f64) -> core::cmp::Ordering { a.total_cmp(&b) }\n",
            "total_cmp",
        ),
        (
            "pub fn f(v: &mut [f64]) { v.sort_by(f64::total_cmp); }\n",
            "total_cmp",
        ),
        (
            "pub fn f(v: &mut [f32]) { v.sort_by(|a, b| a.total_cmp(b)); }\n",
            "total_cmp",
        ),
        (
            "pub fn f(x: f64) -> bool { x.is_sign_negative() }\n",
            "is_sign_negative",
        ),
        (
            "pub fn f(x: f32) -> bool { x.is_sign_positive() }\n",
            "is_sign_positive",
        ),
        (
            "pub fn f(x: f64) -> bool { <f64>::is_sign_negative(x) }\n",
            "is_sign_negative",
        ),
        (
            "pub fn f(a: f64, b: f64) -> f64 { a.copysign(b) }\n",
            "copysign",
        ),
        (
            "pub fn f(a: f64, b: f64) -> f64 { f64::copysign(a, b) }\n",
            "copysign",
        ),
        (
            "pub fn f(a: f64) -> f64 { 1.0_f64.copysign(a) }\n",
            "copysign",
        ),
        ("pub fn f(x: f64) -> f64 { x.signum() }\n", "signum"),
        ("pub fn f(x: f32) -> f32 { f32::signum(x) }\n", "signum"),
        ("pub fn f(x: f64) -> f64 { (x * 2.0).signum() }\n", "signum"),
    ] {
        assert!(
            tokens(src).iter().any(|t| t == want),
            "{src}: {:?}",
            tokens(src)
        );
    }
    // integers have a `signum`; the oa_math functions are the way
    for src in [
        "pub fn f(x: i64) -> i64 { x.signum() }\n",
        "pub fn f(a: u32, b: u32) -> core::cmp::Ordering { a.cmp(&b) }\n",
        "pub fn f(a: f64, b: f64) -> core::cmp::Ordering { oa_math::cmp_canon_f64(a, b) }\n",
        "pub fn f(x: f64) -> bool { oa_math::is_sign_negative_canon_f64(x) }\n",
        "pub fn f(a: f64, b: f64) -> f64 { oa_math::copysign_canon_f64(a, b) }\n",
    ] {
        assert!(tokens(src).is_empty(), "{src}: {:?}", tokens(src));
    }
    // oa-math itself implements them
    let src = "pub fn c(a: f64, b: f64) -> core::cmp::Ordering { a.total_cmp(&b) }\n";
    assert!(scan_with(src, true, &FloatFacts::default()).hits.is_empty());
}

// REQ: OA-TRC-248
#[test]
fn float_bits_are_found_through_paths_values_tuples_untyped_lets_and_fields_of_other_crates() {
    for (src, want) in [
        // a path used as a value, in `UFCS`, with a qualified type
        ("pub fn f() -> u64 { let g = f64::to_bits; g(1.0) }\n", "to_bits"),
        ("pub fn f(v: &[f64]) -> Vec<u64> { v.iter().copied().map(f64::to_bits).collect() }\n", "to_bits"),
        ("pub fn f(x: f64) -> u64 { <f64>::to_bits(x) }\n", "to_bits"),
        ("pub fn f(x: f32) -> u32 { <f32>::to_bits(x) }\n", "to_bits"),
        ("pub fn f() -> [u8; 4] { let g = f32::to_le_bytes; g(1.0) }\n", "to_le_bytes"),
        ("pub fn f(x: f64) -> [u8; 8] { <f64>::to_be_bytes(x) }\n", "to_be_bytes"),
        ("pub fn f(v: &[f64]) { let _: Vec<[u8; 8]> = v.iter().copied().map(f64::to_ne_bytes).collect(); }\n", "to_ne_bytes"),
        // a tuple pattern with float types
        ("pub fn f() -> [u8; 8] { let (a, b): (f64, f64) = (1.0, 2.0); let _ = b; a.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f() -> [u8; 8] { let (a, b): (u8, f64) = (1, 2.0); let _ = a; b.to_be_bytes() }\n", "to_be_bytes"),
        ("pub fn f(p: (u8, f64)) -> [u8; 8] { p.1.to_le_bytes() }\n", "to_le_bytes"),
        // untyped lets: a literal, a float name, a method, a cast, and one let from another
        ("pub fn f() -> [u8; 8] { let x = 1.5; x.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f() -> [u8; 4] { let x = 2f32; x.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f() -> [u8; 8] { let x = 1e3; x.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(a: f64) -> [u8; 8] { let y = a; y.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(a: f64) -> [u8; 8] { let y = a * 2.0; let z = y + 1.0; z.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(n: usize) -> [u8; 8] { let y = n as f64; y.to_be_bytes() }\n", "to_be_bytes"),
        ("pub fn f(a: f64) -> [u8; 8] { let y = oa_math::exp(a); y.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f() -> [u8; 8] { let y = f64::MAX; y.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(v: &[f64]) -> [u8; 8] { let y = v[0]; y.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(v: &[f64]) -> [u8; 8] { let y = v.iter().sum::<f64>(); y.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(x: f64) -> [u8; 8] { let (a, b) = (x, 1u8); let _ = b; a.to_le_bytes() }\n", "to_le_bytes"),
        ("pub fn f(v: &[f64]) -> [u8; 8] { let m = v.iter().fold(0.0, |acc, x| acc + x); m.to_le_bytes() }\n", "to_le_bytes"),
    ] {
        assert!(tokens(src).iter().any(|t| t == want), "{src}: {:?}", tokens(src));
    }
    // a float field, a tuple-struct field and a float function that another core file declares
    for (src, others, want) in [
        (
            "pub fn f(s: &S) -> [u8; 8] { s.w.to_le_bytes() }\n",
            "pub struct S { pub w: f64 }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(s: &S) -> [u8; 8] { let y = s.w; y.to_le_bytes() }\n",
            "pub struct S { pub w: f64 }\n",
            "to_le_bytes",
        ),
        (
            "pub struct P2; impl P { pub fn b(&self) -> [u8; 8] { self.0.to_le_bytes() } }\n",
            "pub struct P(f64);\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(p: &P) -> [u8; 8] { p.1.to_be_bytes() }\n",
            "pub struct P(u8, pub f32);\n",
            "to_be_bytes",
        ),
        (
            "pub fn f() -> [u8; 8] { let y = g(); y.to_le_bytes() }\n",
            "pub fn g() -> f64 { 1.0 }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f() -> [u8; 8] { let y = other::g(); y.to_le_bytes() }\n",
            "pub fn g() -> f64 { 1.0 }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f() -> [u8; 8] { EPS.to_le_bytes() }\n",
            "pub const EPS: f64 = 1e-9;\n",
            "to_le_bytes",
        ),
    ] {
        let t = tokens_with(src, others);
        assert!(t.iter().any(|x| x == want), "{src}: {t:?}");
        // without the declaration in the other file there is nothing to see
        assert!(
            !tokens(src).iter().any(|x| x == want),
            "{src} (no facts): {:?}",
            tokens(src)
        );
    }
    // integers, and ints from a cast, an int tuple field and an int field of the same name
    for (src, others) in [
        ("pub fn f(x: u32) -> [u8; 4] { x.to_le_bytes() }\n", ""),
        ("pub fn f() -> [u8; 4] { let n = 3; n.to_le_bytes() }\n", ""),
        ("pub fn f(a: f64) -> [u8; 4] { let n = (a * 2.0) as u32; n.to_le_bytes() }\n", ""),
        ("pub fn f(a: f64) -> [u8; 8] { let n = a as u64; let n = n + 1; n.to_le_bytes() }\n", ""),
        ("pub fn f(a: f64) -> [u8; 8] { let y = a; let y = y as u64; y.to_le_bytes() }\n", ""),
        ("pub fn f(v: &[f64]) -> [u8; 8] { let n = v.len(); n.to_le_bytes() }\n", ""),
        ("pub struct Id(u32);\nimpl Id { pub fn b(&self) -> [u8; 4] { self.0.to_le_bytes() } }\n", "pub struct Id(u32);\npub struct P(f64);\n"),
        ("pub fn f(p: &Id) -> [u8; 4] { p.0.to_le_bytes() }\n", "pub struct Id(u32);\npub struct P(f64);\n"),
        ("pub fn f(t: (f64, u32)) -> [u8; 4] { t.1.to_le_bytes() }\n", ""),
        ("pub fn f() -> [u8; 4] { let (a, b): (f64, u32) = (1.0, 2); let _ = a; b.to_le_bytes() }\n", ""),
        ("pub fn f(x: f64) -> [u8; 8] { oa_math::canon_nan(x).to_bits().to_le_bytes() }\n", ""),
        ("pub fn f(x: f64) -> u64 { f64::to_bits(oa_math::canon_nan(x)) }\n", ""),
        ("pub fn f(x: f64) -> u64 { <f64>::to_bits(oa_math::canon_nan(x)) }\n", ""),
    ] {
        let t = tokens_with(src, others);
        assert!(
            !t.iter().any(|x| x.contains("bytes") || x == "to_bits"),
            "{src}: {t:?}"
        );
    }
    // a float field of another core crate reaches the whole workspace check
    let ws = core_ws(
        "b0",
        "pub fn f(s: &crate::S) -> [u8; 8] { s.w.to_le_bytes() }\npub struct S { pub n: u8 }\n",
    );
    ws.put(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\", \"crates/fx-c\"]\n",
    );
    for (field, hit) in [("f64", true), ("u64", false)] {
        ws.put(
            "crates/fx-c/Cargo.toml",
            "[package]\nname = \"fx-c\"\n\n[package.metadata.openatca]\nclass = \"core\"\n\n[lints]\nworkspace = true\n",
        );
        ws.put(
            "crates/fx-c/src/lib.rs",
            &format!("#![forbid(unsafe_code)]\n#![cfg_attr(not(test), no_std)]\n{GUARD}pub struct T {{ pub w: {field} }}\n"),
        );
        assert_eq!(
            has(&ws.failures(), "fx-a/src/lib.rs:1: to_le_bytes"),
            hit,
            "{field}: {:#?}",
            ws.failures()
        );
    }
}

// REQ: OA-TRC-249
#[test]
fn every_way_to_leak_or_take_an_address_is_named_and_banned() {
    for (src, want) in [
        ("pub fn f(b: alloc::boxed::Box<u8>) -> &'static mut u8 { alloc::boxed::Box::leak(b) }\n", "leak"),
        ("pub fn f(v: alloc::vec::Vec<u8>) -> &'static mut [u8] { v.leak() }\n", "leak"),
        ("pub fn f(b: alloc::boxed::Box<u8>) { let _ = alloc::boxed::Box::into_raw(b); }\n", "into_raw"),
        ("pub fn f(v: alloc::vec::Vec<u8>) { let _ = v.into_raw_parts(); }\n", "into_raw_parts"),
        ("pub fn f(p: u8) { let _ = alloc::boxed::Box::from_raw(p); }\n", "from_raw"),
        ("pub fn f(s: &[u8]) { let _ = s.as_ptr_range(); }\n", "as_ptr_range"),
        ("pub fn f(s: &mut [u8]) { let _ = s.as_mut_ptr_range(); }\n", "as_mut_ptr_range"),
        ("pub fn f(x: &u8) { let _ = core::ptr::addr_of!(*x); }\n", "addr_of"),
        ("pub fn f(x: &mut u8) { let _ = core::ptr::addr_of_mut!(*x); }\n", "addr_of_mut"),
        // already banned, kept
        ("pub fn f(s: &[u8]) { let _ = s.as_ptr(); }\n", "as_ptr"),
        ("pub fn f(s: &mut [u8]) { let _ = s.as_mut_ptr(); }\n", "as_mut_ptr"),
        ("pub fn f(p: *const u8) -> usize { p.addr() }\n", ".addr("),
        ("pub fn f(p: usize) { let _ = core::ptr::with_exposed_provenance::<u8>(p); }\n", "with_exposed_provenance"),
        ("pub fn f(p: *const u8) -> usize { p.expose_provenance() }\n", "expose_provenance"),
    ] {
        assert!(tokens(src).iter().any(|t| t == want), "{src}: {:?}", tokens(src));
    }
    // other names that merely contain them are fine
    let t = tokens("pub fn leakage(leaked: u8, as_ptr_free: u8) -> u8 { leaked + as_ptr_free }\npub fn into_raw_text() {}\n");
    assert!(t.is_empty(), "{t:?}");
}

// REQ: OA-TRC-250
#[test]
fn a_core_crate_refuses_a_non_64_bit_target_in_its_lib_rs() {
    let planted = |tag: &str, lib: &str| {
        let ws = Ws::new(tag, "core", &[("src/lib.rs", "pub fn f() {}\n")]);
        ws.put(
            "crates/fx-a/src/lib.rs",
            &format!("#![forbid(unsafe_code)]\n#![cfg_attr(not(test), no_std)]\n{lib}"),
        );
        ws.failures()
    };
    const WHY: &str =
        "fx-a: B10: src/lib.rs of a core crate must have `#[cfg(not(target_pointer_width";
    // clean
    assert!(planted("w0", &format!("{GUARD}pub fn f() {{}}\n")).is_empty());
    assert!(planted(
        "w1",
        "pub fn f() {}\n#[cfg( not ( target_pointer_width = \"64\" ) )]\ncompile_error!(\"x\");\n"
    )
    .is_empty());
    // missing, wrong width, wrong cfg, no compile_error, inside a module, test-only, in a comment
    for (tag, lib) in [
        ("w2", "pub fn f() {}\n"),
        ("w3", "#[cfg(not(target_pointer_width = \"32\"))]\ncompile_error!(\"x\");\n"),
        ("w4", "#[cfg(target_pointer_width = \"64\")]\ncompile_error!(\"x\");\n"),
        ("w5", "#[cfg(not(target_pointer_width = \"64\"))]\npub fn f() {}\n"),
        ("w6", "compile_error!(\"x\");\n"),
        ("w7", "mod m {\n#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"x\");\n}\n"),
        ("w8", "#[cfg(test)]\nmod t {\n#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"x\");\n}\n"),
        ("w9", "// #[cfg(not(target_pointer_width = \"64\"))]\n// compile_error!(\"x\");\n"),
        ("wa", "const S: &str = \"#[cfg(not(target_pointer_width = \\\"64\\\"))] compile_error!(x);\";\n"),
    ] {
        let f = planted(tag, lib);
        assert!(has(&f, WHY), "{lib}: {f:#?}");
    }
    // the guard is the one target cfg core code may have: the rest still fail
    for (tag, lib) in [
        ("wb", "#[cfg(target_os = \"linux\")]\npub fn f() {}\n"),
        (
            "wc",
            "#[cfg(not(target_pointer_width = \"64\"))]\npub fn f() {}\n",
        ),
    ] {
        let f = planted(tag, &format!("{GUARD}{lib}"));
        assert!(has(&f, "no target-dependent cfg"), "{lib}: {f:#?}");
    }
    assert!(!has(&planted("wd", GUARD), "no target-dependent cfg"));
    // oa-ieee is a core crate too
    let ws = Ws::named("we", "oa-ieee", "core", &[("src/lib.rs", "pub fn sqrt(v: f64) -> f64 { v.sqrt() }\npub fn floor(v: f64) -> f64 { v.floor() }\npub fn ceil(v: f64) -> f64 { v.ceil() }\npub fn trunc(v: f64) -> f64 { v.trunc() }\npub fn round(v: f64) -> f64 { v.round() }\n")]);
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    let lib = fs::read_to_string(ws.root.join("crates/fx-a/src/lib.rs")).unwrap();
    ws.put(
        "crates/fx-a/src/lib.rs",
        &lib.replace("compile_error", "compile_errors"),
    );
    assert!(
        has(&ws.failures(), &WHY.replace("fx-a", "oa-ieee")),
        "{:#?}",
        ws.failures()
    );
    // the checker reads the real ones
    for c in ["oa-geo", "oa-math", "oa-ieee"] {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../crates/{c}/src/lib.rs"));
        assert!(
            oa_trace::classcheck::has_pointer_width_guard(&fs::read_to_string(p).unwrap()),
            "{c}"
        );
    }
}

// REQ: OA-TRC-251
#[test]
fn float_min_max_clamp_are_oa_maths_so_the_zeros_and_the_nan_are_defined() {
    for (src, want) in [
        ("pub fn f(a: f64, b: f64) -> f64 { a.max(b) }\n", "max"),
        ("pub fn f(a: f64, b: f64) -> f64 { a.min(b) }\n", "min"),
        ("pub fn f(a: f32, b: f32) -> f32 { a.min(b) }\n", "min"),
        ("pub fn f(a: f64) -> f64 { a.clamp(-1.0, 1.0) }\n", "clamp"),
        ("pub fn f(a: f64) -> f64 { a.max(0.0) }\n", "max"),
        (
            "pub fn f(a: u8, b: f64) -> f64 { f64::from(a).min(b) }\n",
            "min",
        ),
        (
            "pub fn f(a: f64, b: f64) -> f64 { (a * 2.0).min(b) }\n",
            "min",
        ),
        (
            "pub fn f(a: f64, b: f64) -> f64 { a.abs().max(b) }\n",
            "max",
        ),
        ("pub fn f(n: usize) -> f64 { (n as f64).max(1.0) }\n", "max"),
        (
            "pub fn f(a: f64, b: f64) -> f64 { f64::max(a, b) }\n",
            "max",
        ),
        (
            "pub fn f(a: f32, b: f32) -> f32 { f32::min(a, b) }\n",
            "min",
        ),
        (
            "pub fn f(a: f64, b: f64) -> f64 { <f64>::clamp(a, b, b) }\n",
            "clamp",
        ),
        (
            "pub fn f(v: &[f64]) -> f64 { v.iter().copied().fold(f64::NEG_INFINITY, f64::max) }\n",
            "max",
        ),
        (
            "pub fn f(v: &[f64]) -> f64 { v.iter().copied().fold(f64::INFINITY, f64::min) }\n",
            "min",
        ),
        ("pub fn f(v: &[f64]) -> f64 { v[0].max(v[1]) }\n", "max"),
        (
            "pub fn f(v: &[f64]) -> f64 { let m = v.iter().fold(0.0, |m, x| m.max(*x)); m }\n",
            "max",
        ),
        ("pub fn f() -> f64 { let m = 1.0; m.min(g()) }\n", "min"),
        ("pub fn f(x: f64) -> f64 { x.sqrt().max(0.0) }\n", "max"),
        (
            "pub fn f(x: f64) -> f64 { oa_math::sin(x).max(0.5) }\n",
            "max",
        ),
        ("pub fn f(a: f64) -> f64 { a.signum() }\n", "signum"),
    ] {
        assert!(
            tokens(src).iter().any(|t| t == want),
            "{src}: {:?}",
            tokens(src)
        );
    }
    // a float field and a float function that another core file declares
    for (src, others, want) in [
        ("pub fn f(s: &S) -> f64 { s.w.min(s.x) }\n", "pub struct S { pub w: f64, pub x: f64 }\n", "min"),
        ("pub fn f(s: &S, k: &T) -> f64 { k.max(s.w) }\n", "pub struct S { pub w: f64 }\n", "max"),
        ("pub fn f(a: &T) -> f64 { a.lat().min(a.lon()) }\n", "pub fn lat() -> f64 { 0.0 }\npub fn lon() -> f64 { 0.0 }\n", "min"),
        ("pub struct P(f64);\nimpl P { pub fn c(&self, lo: &T, hi: &T) -> f64 { self.0.clamp(lo, hi) } }\n", "pub struct P(f64);\n", "clamp"),
    ] {
        let t = tokens_with(src, others);
        assert!(t.iter().any(|x| x == want), "{src}: {t:?}");
        assert!(!tokens(src).iter().any(|x| x == want), "{src} (no facts): {:?}", tokens(src));
    }
    // integers, iterators and the oa_math forms are fine
    for src in [
        "pub fn f(a: usize, b: usize) -> usize { a.max(b) }\n",
        "pub fn f(a: i64) -> i64 { a.clamp(1, 9) }\n",
        "pub fn f(v: &[u8]) -> Option<&u8> { v.iter().max() }\n",
        "pub fn f(v: &[u8]) -> u8 { v.iter().copied().min().unwrap_or(0) }\n",
        "pub fn f(x: f64) -> usize { let n = (x * 2.0) as usize; n.max(1) }\n",
        "pub fn f(x: f64, v: &[f64]) -> usize { let n = v.len(); n.min(x as usize) }\n",
        "pub fn f(x: f64) -> usize { (x as usize).min(3) }\n",
        "pub fn f(x: f64) -> f64 { oa_math::max_f64(x, 0.0) }\n",
        "pub fn f(x: f64) -> f64 { oa_math::clamp_f64(x, -1.0, 1.0) }\n",
        "pub fn f(a: f64) -> usize { let n = if a > 0.5 { 3 } else { 1 }; n.max(2) }\n",
        "pub fn f(a: u8) -> u8 { let a = a.max(1); a.min(9) }\n",
        "pub fn f(a: f64, n: u32) -> u32 { for i in 0..n { let _ = i.max(2); } let _ = a; 0 }\n",
        "pub fn f(a: f64) -> usize { let p = (0usize, a); (p.0 + 1).min(4) }\n",
    ] {
        assert!(tokens(src).is_empty(), "{src}: {:?}", tokens(src));
    }
    // a later `let` of the same name hides an earlier float
    let t = tokens("pub fn f(a: f64) -> u32 { let b = a * 2.0; let b = b as u32; b.max(1) }\n");
    assert!(t.is_empty(), "{t:?}");
    // oa-math itself implements them
    let src = "pub fn m(a: f64, b: f64) -> f64 { if a.max(b) > a { b } else { a } }\n";
    assert!(scan_with(src, true, &FloatFacts::default()).hits.is_empty());
}

// REQ: OA-TRC-252
#[test]
fn the_header_of_every_impl_is_a_boundary_trait_impls_included() {
    for (name, src) in [
        (
            "impl@F",
            "trait Hook {}\nimpl<F: Fn() -> f64> Hook for &F {}\n",
        ),
        (
            "impl@Wrap",
            "pub struct Wrap<T>(T);\ntrait Hook {}\nimpl<T: Into<f64> + Copy> Hook for Wrap<T> {}\n",
        ),
        (
            "impl@Box",
            "trait Hook {}\nimpl Hook for Box<dyn Fn() -> f64> {}\n",
        ),
        (
            "impl@Wrap",
            "pub struct Wrap<T>(T);\nimpl<T: Clone> Default for Wrap<T> { fn default() -> Self { unreachable!() } }\n",
        ),
        (
            "impl@Wrap",
            "pub struct Wrap<T>(T);\ntrait Hook {}\nimpl<T> Hook for Wrap<T> where T: Iterator {}\n",
        ),
        (
            "impl@Wrap",
            "pub struct Wrap<T>(T);\ntrait Hook {}\nimpl<T: Copy> Hook for Wrap<T> {}\n",
        ),
        (
            "impl@Dbg",
            "trait Hook {}\nimpl Hook for &mut dyn Dbg {}\ntrait Dbg {}\n",
        ),
    ] {
        let ws = core_ws("ih0", src);
        assert!(
            has(&ws.failures(), &format!("{name}: B5: core code lets the shell hand in behaviour")),
            "{src}: {:#?}",
            ws.failures()
        );
    }
    // clean: no generics, `Sized`, a sealed trait, an inherent impl without a bound, a test impl
    for (tag, src) in [
        ("ih1", "pub struct CoreType;\ntrait Hook {}\nimpl Hook for CoreType {}\n".to_string()),
        ("ih2", "pub struct Wrap<T>(T);\ntrait Hook {}\nimpl<T: Sized> Hook for Wrap<T> {}\n".to_string()),
        ("ih3", format!("{SEALED}pub trait Sh: sealed::Sealed {{}}\npub struct Wrap<T>(T);\ntrait Hook {{}}\nimpl<T: Sh> Hook for Wrap<T> {{}}\nimpl<T> Hook for &Wrap<T> {{}}\n")),
        ("ih4", "pub struct Wrap<T>(T);\ntrait Hook {}\n#[cfg(test)]\nmod t { impl<T: Clone> Hook for Wrap<T> {} }\n".to_string()),
    ] {
        let ws = core_ws(tag, &src);
        assert!(ws.failures().is_empty(), "{src}: {:#?}", ws.failures());
    }
    // the allowlist names it as `impl@Type`
    let ws = core_ws("ih5", "trait Hook {}\nimpl<F: Fn()> Hook for &F {}\n");
    ws.put(
        CALLBACKS,
        "fx-a lib.rs::impl@F reason: reviewed, a pure closure only\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-253
#[test]
fn a_seal_is_read_from_the_leading_path_and_a_glob_reexport_fails() {
    let seal = "mod sealed { pub trait Sealed {} pub struct Private; }\npub mod open { pub trait Other {} }\n";
    // not seals: a private path inside the generic arguments, a trait of a public module imported
    // next to a sealing one in one grouped `use`
    for (name, src) in [
        ("T", format!("{seal}pub trait T: Iterator<Item = sealed::Private> {{}}\n")),
        (
            "U",
            format!("{seal}use crate::{{sealed::Sealed, open::Other}};\npub trait T: Sealed {{}}\npub trait U: Other {{}}\n"),
        ),
        (
            "U",
            format!("{seal}use crate::open::{{Other}};\nuse crate::sealed::Sealed;\npub trait T: Sealed {{}}\npub trait U: Other {{}}\n"),
        ),
    ] {
        let ws = core_ws("sl0", &src);
        let f = ws.failures();
        assert!(
            has(&f, &format!("{name}: B5: core code lets the shell hand in behaviour (a `pub trait` that is not sealed")),
            "{src}: {f:#?}"
        );
        assert!(!has(&f, "T: B5") || name == "T", "{src}: {f:#?}");
    }
    // a glob re-export, and a grouped `pub use` of a sealing trait
    let ws = core_ws("sl1", &format!("{seal}pub use sealed::*;\n"));
    assert!(
        has(
            &ws.failures(),
            "*: B5: core code lets the shell hand in behaviour (a `pub use` glob"
        ),
        "{:#?}",
        ws.failures()
    );
    let ws = core_ws("sl2", &format!("{seal}pub use open::*;\n"));
    assert!(has(&ws.failures(), "*: B5"), "{:#?}", ws.failures());
    let ws = core_ws(
        "sl3",
        &format!("{seal}pub trait T: sealed::Sealed {{}}\npub use crate::{{sealed::Sealed, open::Other}};\n"),
    );
    let f = ws.failures();
    assert!(
        has(
            &f,
            "Sealed: B5: core code lets the shell hand in behaviour (`pub use` of a sealing trait"
        ),
        "{f:#?}"
    );
    assert!(
        !has(
            &f,
            "Other: B5: core code lets the shell hand in behaviour (`pub use`"
        ),
        "{f:#?}"
    );
    // clean: a grouped import through the private module of a sealing trait
    let ws = core_ws(
        "sl4",
        &format!(
            "{seal}use crate::{{sealed::Sealed, open::Other}};\npub trait T: Sealed + Other {{}}\n"
        ),
    );
    // `Other` is a `pub trait` of a public module: it alone is flagged
    let f = ws.failures();
    assert!(has(&f, "Other: B5"), "{f:#?}");
    assert!(!has(&f, "T: B5") && !has(&f, "Sealed: B5"), "{f:#?}");
}

// REQ: OA-TRC-250
#[test]
fn the_guard_stands_alone_at_the_top_level_and_split_crates_carry_it_too() {
    let inner = "#![forbid(unsafe_code)]\n#![cfg_attr(not(test), no_std)]\n";
    // planted: stacked on another outer attribute, inside a module, for the wrong width
    for (tag, lib) in [
        (
            "gs0",
            format!("{inner}#[cfg(any())]\n{GUARD}pub fn f() {{}}\n"),
        ),
        ("gs1", format!("{inner}#[inline]\n{GUARD}pub fn f() {{}}\n")),
        (
            "gs2",
            format!("{inner}mod m {{ {GUARD} }}\npub fn f() {{}}\n"),
        ),
        (
            "gs3",
            format!("{inner}{}pub fn f() {{}}\n", GUARD.replace("64", "32")),
        ),
    ] {
        let ws = core_ws(tag, &lib);
        assert!(
            has(&ws.failures(), "B10: src/lib.rs of a core crate must have"),
            "{lib}: {:#?}",
            ws.failures()
        );
    }
    // the stacked one is also a target cfg the scan does not excuse
    let ws = core_ws(
        "gs4",
        &format!("{inner}#[cfg(any())]\n{GUARD}pub fn f() {{}}\n"),
    );
    assert!(
        has(&ws.failures(), "target_pointer_width: B8"),
        "{:#?}",
        ws.failures()
    );
    // any other pointer-width cfg in core code fails
    let ws = core_ws("gs5", &format!("{inner}{GUARD}#[cfg(target_pointer_width = \"32\")]\npub fn g() {{}}\npub fn f() {{}}\n"));
    assert!(
        has(&ws.failures(), "target_pointer_width: B8"),
        "{:#?}",
        ws.failures()
    );
    // ok: after the inner attributes, after an item, after a closing brace, with a doc comment
    for (tag, lib) in [
        ("gs6", format!("{inner}{GUARD}pub fn f() {{}}\n")),
        ("gs7", format!("{inner}pub fn f() {{}}\n{GUARD}")),
        ("gs8", format!("{inner}pub struct S {{}}\n{GUARD}")),
        (
            "gs9",
            format!("{inner}/// the guard\n{GUARD}pub fn f() {{}}\n"),
        ),
    ] {
        let ws = core_ws(tag, &lib);
        assert!(ws.failures().is_empty(), "{lib}: {:#?}", ws.failures());
    }
    // a split crate: required in lib.rs, exactly that form, nothing else with it
    let ws = split_ws(
        "gt0",
        "mod core;\nmod shell;\n",
        "pub fn d(x: u8) -> u8 { x }\n",
    );
    assert!(
        has(&ws.failures(), "B10: src/lib.rs of a split crate must have"),
        "{:#?}",
        ws.failures()
    );
    let ws = split_ws("gt1", SPLIT_LIB, "pub fn d(x: u8) -> u8 { x }\n");
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    for (tag, lib) in [
        ("gt2", "mod core;\nmod shell;\n#[cfg(any())]\n#[cfg(not(target_pointer_width = \"64\"))]\ncompile_error!(\"x\");\n".to_string()),
        ("gt3", "mod core;\nmod shell;\n#[cfg(not(target_pointer_width = \"32\"))]\ncompile_error!(\"x\");\n".to_string()),
        ("gt4", "mod core;\nmod shell;\n#[cfg(unix)]\ncompile_error!(\"x\");\n".to_string()),
        ("gt5", "mod core;\nmod shell;\n#[cfg(not(target_pointer_width = \"64\"))]\nconst X: u8 = 1;\n".to_string()),
    ] {
        let ws = split_ws(tag, &lib, "pub fn d(x: u8) -> u8 { x }\n");
        assert!(!ws.failures().is_empty(), "{lib}");
    }
}

// REQ: OA-TRC-254
#[test]
fn type_aliases_resolve_transitively_and_a_use_cannot_rename() {
    for src in [
        "type A = Box<dyn Fn()>;\ntype B = Vec<A>;\npub fn f(x: B) { let _ = x; }\n",
        "type B = Vec<A>;\ntype A = Box<dyn Fn()>;\npub fn f(x: B) { let _ = x; }\n",
        "type A = fn(u8) -> u8;\ntype B = Option<A>;\ntype C = (B, u8);\npub struct S { pub c: C }\n",
        "type A = Box<dyn Fn()>;\ntype B = Vec<A>;\ntype C<T> = (T, B);\npub fn f(x: C<u8>) { let _ = x; }\n",
    ] {
        let ws = core_ws("al0", src);
        assert!(has(&ws.failures(), ": B5: core code lets the shell hand in behaviour (the type alias"), "{src}: {:#?}", ws.failures());
    }
    // private items may use them; an alias with no callback is free
    let ws = core_ws(
        "al1",
        "type A = Box<dyn Fn()>;\ntype B = Vec<A>;\nfn f(x: B) { let _ = x; }\npub type P = Vec<u8>;\npub fn g(x: P) { let _ = x; }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // every `as` in a `use` fails, whatever the path
    for src in [
        "use core::fmt as f;\n",
        "use crate::util::Y as Z;\n",
        "use self::a::b as c;\n",
        "use core::fmt::Debug as _;\n",
        "use a::{b as c, d};\n",
        "pub use sealed::Sealed as Other;\n",
    ] {
        assert!(
            tokens(src).iter().any(|t| t == "as"),
            "{src}: {:?}",
            tokens(src)
        );
    }
    assert!(
        !tokens("use a::{b, c};\npub fn f(x: u8) -> u64 { x as u64 }\n")
            .iter()
            .any(|t| t == "as")
    );
}

// REQ: OA-TRC-255
#[test]
fn float_evidence_follows_same_crate_aliases_and_functions_that_return_float_aggregates() {
    for (src, others, want) in [
        (
            "pub fn f(p: V3) -> f64 { p[0].max(p[1]) }\n",
            "pub type V3 = [f64; 3];\n",
            "max",
        ),
        (
            "pub fn f(p: P) -> [u8; 8] { p.0.to_le_bytes() }\n",
            "type P = (f64, f64);\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(x: Real) -> [u8; 8] { x.to_le_bytes() }\n",
            "type Real = f64;\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(p: W) -> f64 { p[2].clamp(0.0, 1.0) }\n",
            "type V3 = [f64; 3];\ntype W = V3;\n",
            "clamp",
        ),
        (
            "pub fn f(p: W) -> f64 { p[2].clamp(0.0, 1.0) }\n",
            "type W = V3;\ntype V3 = [f64; 3];\n",
            "clamp",
        ),
        (
            "pub fn f() -> [u8; 8] { pair().0.to_le_bytes() }\n",
            "pub fn pair() -> (f64, u8) { (1.0, 2) }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f() -> f64 { v()[0].max(1.0) }\n",
            "type V3 = [f64; 3];\npub fn v() -> V3 { [0.0; 3] }\n",
            "max",
        ),
        (
            "pub fn f() -> f64 { let p = v(); p[1].max(1.0) }\n",
            "type V3 = [f64; 3];\npub fn v() -> V3 { [0.0; 3] }\n",
            "max",
        ),
        (
            "pub fn f() -> [u8; 8] { let p = pair(); p.0.to_le_bytes() }\n",
            "pub fn pair() -> (f64, u8) { (1.0, 2) }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f() -> [u8; 8] { g().to_le_bytes() }\n",
            "type Real = f64;\npub fn g() -> Real { 1.0 }\n",
            "to_le_bytes",
        ),
        (
            "pub fn f(s: &S) -> f64 { s.pos[0].max(1.0) }\n",
            "type V3 = [f64; 3];\npub struct S { pub pos: V3 }\n",
            "max",
        ),
        (
            "pub fn f(p: &[f64; 2]) -> f64 { make()[0].min(p[0]) }\n",
            "pub fn make() -> [f64; 2] { [0.0; 2] }\n",
            "min",
        ),
    ] {
        assert!(
            tokens_with(src, others).iter().any(|t| t == want),
            "{src} with {others}: {:?}",
            tokens_with(src, others)
        );
    }
    // not floats: the integer element of a tuple, an alias of integers, a name that is no alias
    for (src, others) in [
        (
            "pub fn f() -> u8 { pair().1.max(2) }\n",
            "pub fn pair() -> (f64, u8) { (1.0, 2) }\n",
        ),
        (
            "pub fn f(p: Ids) -> u32 { p[0].max(1) }\n",
            "type Ids = [u32; 3];\n",
        ),
        (
            "pub fn f(p: Other) -> u32 { p.0.max(1) }\n",
            "type V = (f64, f64);\n",
        ),
        (
            "pub fn f() -> u32 { ids()[0].max(1) }\n",
            "pub fn ids() -> [u32; 2] { [0; 2] }\n",
        ),
        (
            "pub fn f() -> u8 { let p = pair(); p.1.max(2) }\n",
            "pub fn pair() -> (f64, u8) { (1.0, 2) }\n",
        ),
    ] {
        assert!(
            tokens_with(src, others).is_empty(),
            "{src}: {:?}",
            tokens_with(src, others)
        );
    }
}

// REQ: OA-TRC-256
#[test]
fn an_integer_by_its_outermost_shape_is_not_a_float_whatever_it_mentions() {
    for src in [
        "pub fn f(v: &[f64]) -> usize { let n = v.iter().filter(|x| **x > 0.5).count(); n.max(1) }\n",
        "pub fn f(v: &[f64]) -> usize { let n = v.iter().map(|x| x * 2.0).count(); n.max(1) }\n",
        "pub fn f(v: &[f64]) -> usize { let n = v.iter().map(|x| *x as f64).len(); n.min(1) }\n",
        "pub fn f(v: &[f64]) -> u32 { let k = u32::try_from(v.iter().map(|x| *x as f64).count()).unwrap(); k.max(3) }\n",
        "pub fn f(x: f64) -> usize { let k = usize::try_from(x as u64 * 2).unwrap_or(0); k.max(3) }\n",
        "pub fn f(x: f64) -> usize { let m = (x * 2.0) as usize; m.max(1) }\n",
        "pub fn f(x: f64) -> usize { let m = (x * 2.0) as u8 as usize; m.max(1) }\n",
    ] {
        assert!(tokens(src).is_empty(), "{src}: {:?}", tokens(src));
    }
    // still floats: the same shapes inside another expression, and a plain float expression
    for src in [
        "pub fn f(x: f64) -> f64 { let m = x * 2.0; m.max(1.0) }\n",
        "pub fn f(v: &[f64]) -> f64 { let n = v.len() as f64 * 2.0; n.max(1.0) }\n",
        "pub fn f(v: &[f64]) -> f64 { let n = v.iter().count() as f64; n.max(1.0) }\n",
    ] {
        assert!(
            tokens(src).iter().any(|t| t == "max"),
            "{src}: {:?}",
            tokens(src)
        );
    }
}

// REQ: OA-TRC-257
#[test]
fn fmt_arguments_is_not_taken_by_a_reachable_item() {
    for src in [
        "pub fn f(a: core::fmt::Arguments<'_>) { let _ = a; }\n",
        "use core::fmt;\npub fn f(a: fmt::Arguments<'_>) { let _ = a; }\n",
        "pub struct S;\nimpl S { pub fn m(&self, a: core::fmt::Arguments<'_>) { let _ = a; } }\n",
        "pub struct S { pub a: core::fmt::Arguments<'static> }\n",
    ] {
        let ws = core_ws("fa0", src);
        assert!(
            has(
                &ws.failures(),
                "B5: core code lets the shell hand in behaviour (`fmt::Arguments`"
            ),
            "{src}: {:#?}",
            ws.failures()
        );
    }
    let ws = core_ws(
        "fa1",
        "fn f(a: core::fmt::Arguments<'_>) { let _ = a; }\npub(crate) fn g(a: core::fmt::Arguments<'_>) { let _ = a; }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
}

// REQ: OA-TRC-253
#[test]
fn a_private_module_cannot_be_published_and_a_pub_mod_seals_nothing() {
    let seal = "mod sealed { pub trait Sealed {} }\npub trait T: sealed::Sealed {}\n";
    for src in [
        format!("{seal}pub use self::sealed;\n"),
        format!("{seal}pub use crate::sealed;\n"),
        format!("{seal}pub mod api {{ pub use super::sealed; }}\n"),
    ] {
        let ws = core_ws("pm0", &src);
        assert!(
            has(&ws.failures(), "sealed: B5: core code lets the shell hand in behaviour (a `pub use` of a private module"),
            "{src}: {:#?}",
            ws.failures()
        );
    }
    // a plain import is not a re-export
    let ws = core_ws("pm1", &format!("{seal}use crate::sealed;\n"));
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // a sealing trait in a `pub mod` is no seal, and neither is one when the name is also declared
    // public anywhere in the crate
    for src in [
        "pub mod sealed { pub trait Sealed {} }\npub trait T: sealed::Sealed {}\n",
        "mod sealed { pub trait Sealed {} }\npub mod api { pub mod sealed { pub trait Sealed {} } }\npub trait T: sealed::Sealed {}\n",
    ] {
        let ws = core_ws("pm2", src);
        assert!(has(&ws.failures(), "T: B5: core code lets the shell hand in behaviour (a `pub trait` that is not sealed"), "{src}: {:#?}", ws.failures());
    }
}
