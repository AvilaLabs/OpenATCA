#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::process::Command;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("oa-trace-reference-{}-{nonce}", std::process::id()))
}

fn verify_reference(binary: &str, root: &Path, reference: &str) -> bool {
    Command::new(binary)
        .args([
            "--root",
            root.to_str().expect("UTF-8 fixture path"),
            "--verify-test",
            reference,
        ])
        .status()
        .expect("run verifier")
        .success()
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
    assert!(verify_reference(
        binary,
        &root,
        "tests/reference.rs::actual_test",
    ));
    for fake in ["comment_fake", "raw_fake", "helper"] {
        let reference = format!("tests/reference.rs::{fake}");
        assert!(!verify_reference(binary, &root, &reference));
    }
    fs::write(
        &source,
        "#[cfg(any())] mod disabled {\n#[test]\nfn disabled_test() {}\n}\n#[cfg(feature = \"never_enabled\")]\nmod unknown {\n#[test]\nfn conditional_test() {}\n}\n#[cfg(test)] mod enabled {\n#[test]\nfn enabled_test() {}\n}\n",
    )
    .expect("write conditional source fixture");
    for reference in [
        "tests/reference.rs::disabled_test",
        "tests/reference.rs::conditional_test",
        "tests/reference.rs::enabled_test",
    ] {
        assert_eq!(
            verify_reference(binary, &root, reference),
            reference.ends_with("enabled_test"),
        );
    }
    let inner_cfg = root.join("tests/crate_gate.rs");
    fs::write(
        &inner_cfg,
        "#![cfg(feature = \"never_enabled\")]\n#[test]\nfn file_gate() {}\n",
    )
    .expect("write crate-gated fixture");
    assert!(!verify_reference(
        binary,
        &root,
        "tests/crate_gate.rs::file_gate",
    ));
    let non_rust = root.join("tests/reference.txt");
    fs::write(&non_rust, "#[test]\nfn text_fake() {}\n").expect("write non-Rust fixture");
    assert!(!verify_reference(
        binary,
        &root,
        "tests/reference.txt::text_fake",
    ));
    let _ = fs::remove_dir_all(root);
}
