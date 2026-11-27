#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("oa-trace-reference-{}-{nonce}", std::process::id()))
}

// REQ: OA-TRC-001
#[test]
fn cli_validator_accepts_a_real_test_and_rejects_comment_and_raw_string_fakes() {
    let root = scratch();
    let source = root.join("tests/reference.rs");
    fs::create_dir_all(source.parent().expect("parent path")).expect("create fixture directory");
    fs::write(
        &source,
        "// #[test]\n// fn comment_fake() {}\nconst TEXT: &str = r###\"#[test]\\nfn raw_fake() {}\"###;\nfn helper() {}\n#[test]\nfn actual_test() {}\n",
    )
    .expect("write source fixture");
    let binary = env!("CARGO_BIN_EXE_oa-trace");
    let valid = Command::new(binary)
        .args([
            "--root",
            root.to_str().expect("UTF-8 fixture path"),
            "--verify-test",
            "tests/reference.rs::actual_test",
        ])
        .status()
        .expect("run verifier");
    assert!(valid.success());
    for fake in ["comment_fake", "raw_fake", "helper"] {
        let reference = format!("tests/reference.rs::{fake}");
        let invalid = Command::new(binary)
            .args([
                "--root",
                root.to_str().expect("UTF-8 fixture path"),
                "--verify-test",
                &reference,
            ])
            .status()
            .expect("run verifier");
        assert!(!invalid.success());
    }
    fs::write(
        &source,
        "#[cfg(any())]\nmod disabled {\n#[test]\nfn disabled_test() {}\n}\n#[test]\nfn enabled_test() {}\n",
    )
    .expect("write conditional source fixture");
    for reference in [
        "tests/reference.rs::disabled_test",
        "tests/reference.rs::enabled_test",
    ] {
        let result = Command::new(binary)
            .args([
                "--root",
                root.to_str().expect("UTF-8 fixture path"),
                "--verify-test",
                reference,
            ])
            .status()
            .expect("run conditional verifier");
        assert_eq!(result.success(), reference.ends_with("enabled_test"));
    }
    let non_rust = root.join("tests/reference.txt");
    fs::write(&non_rust, "#[test]\nfn text_fake() {}\n").expect("write non-Rust fixture");
    let invalid = Command::new(binary)
        .args([
            "--root",
            root.to_str().expect("UTF-8 fixture path"),
            "--verify-test",
            "tests/reference.txt::text_fake",
        ])
        .status()
        .expect("run non-Rust verifier");
    assert!(!invalid.success());
    let _ = fs::remove_dir_all(root);
}
