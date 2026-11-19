//! The core/shell class check against fixture workspaces (OA-TRC-200 to OA-TRC-215).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_trace::classcheck::{
    check, check_with, ieee_api_problems, lib_rs_problems, mask, parse_manifest, parse_members,
    scan_source, Class, Ctx, UNSPLIT_ALLOWED,
};
use oa_trace::classcheck::{float_method_advice, FLOAT_METHODS};
use std::collections::BTreeMap;
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
    scan_source(
        src,
        &Ctx {
            split: false,
            ieee: false,
            depth: 1,
            classes: &classes,
            aliases: &aliases,
        },
    )
}

fn tokens(src: &str) -> Vec<String> {
    scan(src).hits.into_iter().map(|h| h.token).collect()
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
    /// `src/lib.rs` gets `#![cfg_attr(not(test), no_std)]` on its first line (B10) unless it has one.
    fn named(tag: &str, pkg: &str, class: &str, files: &[(&str, &str)]) -> Ws {
        let root =
            std::env::temp_dir().join(format!("oa-trace-class-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ws = Ws { root };
        ws.put(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/fx-a\", \"crates/fx-b\"]\n",
        );
        ws.put(ALLOW, "");
        ws.put(
            "crates/fx-a/Cargo.toml",
            &format!(
                "[package]\nname = \"{pkg}\"\n\n[package.metadata.openatca]\nclass = \"{class}\"\n"
            ),
        );
        ws.put(
            "crates/fx-b/Cargo.toml",
            "[package]\nname = \"fx-b\"\n\n[package.metadata.openatca]\nclass = \"shell\"\n",
        );
        ws.put("crates/fx-b/src/lib.rs", "pub fn go() {}\n");
        for (p, c) in files {
            let c = if class == "core" && *p == "src/lib.rs" && !c.contains("no_std") {
                format!("#![cfg_attr(not(test), no_std)] {c}")
            } else {
                (*c).to_string()
            };
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

const SPLIT_LIB: &str = "mod core;\nmod shell;\n";

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
        "use core::fmt::{self, Debug};\nuse alloc::collections::BTreeMap;\nuse core::time::Duration;\npub fn f(_d: Duration, _m: &BTreeMap<u8, u8>) -> usize { core::mem::size_of::<u8>() }\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    let ws = core_ws(
        "a15",
        "use core::fmt::Debug;\nuse alloc::collections::BTreeMap;\nuse core::time::Duration;\npub fn f(_d: Duration, _m: &BTreeMap<u8, u8>) -> usize { core::mem::size_of::<u8>() }\n",
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
    assert_eq!(
        rep.unsplit,
        ["oa-display", "oa-replay-diff", "oa-scope-replay", "oa-track"]
    );
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
        (
            "f6",
            "pub fn f<R: Rt>(x: R) -> R { R::sin(x) }\n",
            "sin",
        ),
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
    ws.put("crates/fx-a/src/lib.rs", "pub fn f() {}\n");
    assert!(has(&ws.failures(), "fx-a: B10"), "{:#?}", ws.failures());
    ws.put("crates/fx-a/src/lib.rs", "#![no_std]\npub fn f() {}\n");
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    ws.put(
        "crates/fx-a/src/lib.rs",
        "#![cfg_attr( not(test) , no_std )]\npub fn f() {}\n",
    );
    assert!(ws.failures().is_empty(), "{:#?}", ws.failures());
    // in a comment, a string, or the wrong cfg it does not count
    for bad in [
        "// #![no_std]\npub fn f() {}\n",
        "#![cfg_attr(unix, no_std)]\npub fn f() {}\n",
        "#![cfg_attr(test, no_std)]\npub fn f() {}\n",
        "pub const S: &str = \"#![no_std]\";\n",
    ] {
        ws.put("crates/fx-a/src/lib.rs", bad);
        assert!(
            has(&ws.failures(), "fx-a: B10"),
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
        ws.put("crates/fx-a/src/lib.rs", bad);
        assert!(
            has(
                &ws.failures(),
                "fx-a: B10: src/lib.rs of a core crate must have"
            ),
            "{bad}: {:#?}",
            ws.failures()
        );
    }
    // other inner attributes may come first
    ws.put(
        "crates/fx-a/src/lib.rs",
        "#![forbid(unsafe_code)]\n#![cfg_attr(not(test), no_std)]\npub fn f() {}\n",
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
