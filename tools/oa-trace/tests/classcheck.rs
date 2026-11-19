//! The core/shell class check against fixture workspaces (OA-TRC-200 to OA-TRC-208).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_trace::classcheck::{
    check, lib_rs_problem, mask, parse_manifest, parse_members, scan_source,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    let rep = check(&fixture("good"));
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
    let rep = check(&fixture("bad"));
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
fn forbidden_names_in_core_code_are_reported_with_file_line_and_token() {
    let rep = check(&fixture("bad"));
    let want = [
        "crates/fx-badcore/src/lib.rs:2: std::fs",
        "crates/fx-badcore/src/lib.rs:3: std::io",
        "crates/fx-badcore/src/lib.rs:3: std::time",
        "crates/fx-badcore/src/lib.rs:5: HashMap",
        "crates/fx-badcore/src/lib.rs:6: println!",
        "crates/fx-badcore/src/lib.rs:8: std::env",
        "crates/fx-badcore/src/lib.rs:14: thread_local!",
        "crates/fx-badcore/src/lib.rs:15: static mut",
        "crates/fx-badsplit/src/core/mod.rs:2: HashSet",
    ];
    for w in want {
        assert!(has(&rep.failures, w), "missing {w}: {:#?}", rep.failures);
    }
    // the comment and the string on lines 16 and 17 are not hits
    assert!(!has(&rep.failures, "lib.rs:16:") && !has(&rep.failures, "lib.rs:17:"));
}

// REQ: OA-TRC-203
#[test]
fn a_path_into_a_shell_module_is_a_violation() {
    let rep = check(&fixture("bad"));
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
    assert!(masked.contains("fn f<'a>(x: &'a str)"));
    let (hits, _) = scan_source(src, "fx-x", &BTreeMap::new());
    assert!(hits.is_empty(), "{hits:?}");
    // the same names in code are hits, and a path split over lines is still found
    let (hits, _) = scan_source(
        "fn f() {\n let _ = std\n   :: fs::read(\"x\");\n}\n",
        "fx-x",
        &BTreeMap::new(),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!((hits[0].line, hits[0].token.as_str()), (2, "std::fs"));
}

// REQ: OA-TRC-205
#[test]
fn core_code_may_name_only_core_crates_and_the_core_module_of_split_crates() {
    let rep = check(&fixture("bad"));
    assert!(
        has(
            &rep.failures,
            "fx-badcore: fx-shell: B8: core crate depends on a shell crate"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(
        !has(&rep.failures, "fx-badcore: fx-cleansplit"),
        "a split crate is allowed as a dependency"
    );
    assert!(
        has(
            &rep.failures,
            "crates/fx-badcore/src/lib.rs:10: fx_shell: B8: names a shell or unsplit crate"
        ),
        "{:#?}",
        rep.failures
    );
    assert!(has(&rep.failures, "crates/fx-badcore/src/lib.rs:11: fx_cleansplit: B8: names a split crate outside its `core` module"), "{:#?}", rep.failures);
    assert!(
        !has(&rep.failures, "lib.rs:12:"),
        "fx_cleansplit::core is allowed"
    );
    // and the good split crate's core names fx_core freely
    assert!(check(&fixture("good")).ok());
}

// REQ: OA-TRC-206
#[test]
fn a_split_crate_needs_both_directories_and_a_declarations_only_lib_rs() {
    let rep = check(&fixture("bad"));
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
    assert!(!has(&rep.failures, "fx-cleansplit"), "{:#?}", rep.failures);
    assert!(lib_rs_problem("#![forbid(unsafe_code)]\n//! doc\npub mod core;\npub(crate) mod shell;\npub use core::{a, b};\n#[cfg(test)]\nmod tests { fn t() {} }\n").is_none());
    for bad in [
        "pub mod core { pub fn f() {} }",
        "const X: u8 = 1;",
        "use a::b;\nstruct S;",
        "pub mod core;\nimpl X {}",
    ] {
        assert!(lib_rs_problem(bad).is_some(), "{bad}");
    }
}

// REQ: OA-TRC-207
#[test]
fn a_new_unsplit_crate_and_a_stale_allowlist_entry_both_fail() {
    let rep = check(&fixture("bad"));
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
    // the clean workspace lists exactly its one unsplit crate
    assert!(check(&fixture("good")).ok());
}

// REQ: OA-TRC-208
#[test]
fn the_cargo_toml_reader_finds_name_class_dependencies_and_members() {
    let m = parse_manifest(
        "[package]\nname = \"oa-x\" # n\n\n[package.metadata.openatca]\nclass = \"core\"\n\n[dependencies]\noa-a = { path = \"../oa-a\" }\n# oa-no = 1\noa-b.workspace = true\n\n[dev-dependencies]\noa-c = { path = \"../oa-c\" }\n",
    );
    assert_eq!(m.name.as_deref(), Some("oa-x"));
    assert_eq!(m.class.as_deref(), Some("core"));
    assert_eq!(m.deps, ["oa-a", "oa-b"]);
    let t = "[workspace]\nresolver = \"2\"\nexclude = [\"fuzz\"]\nmembers = [\n    \"crates/a\",\n    \"tools/b\", # c\n]\n[workspace.package]\nmembers = [\"nope\"]\n";
    assert_eq!(parse_members(t), ["crates/a", "tools/b"]);
}
