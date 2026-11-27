#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_audit_core::{genesis, Checkpoint};
use oa_audit_log::shell::verify;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fresh_root() -> PathBuf {
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "oa-audit-action-cli-{}-{sequence}",
        std::process::id()
    ))
}

fn append(
    log: &Path,
    registry: &Path,
    pin: &str,
    principal: &str,
    action: &str,
    cp: Checkpoint,
) -> std::process::Output {
    let count = cp.count.to_string();
    let tail = oa_sha256::to_hex(&cp.tail);
    Command::new(env!("CARGO_BIN_EXE_oa-audit-action"))
        .args([
            "--log",
            log.to_str().unwrap(),
            "--count",
            &count,
            "--tail",
            &tail,
            "--registry",
            registry.to_str().unwrap(),
            "--registry-sha256",
            pin,
            "--principal",
            principal,
            "--action",
            action,
            "--target",
            "OA-VULN-1",
            "--details",
            "E6-INTENT-V1|test|revision=1|register_sha256=synthetic",
        ])
        .output()
        .unwrap()
}

// REQ: OA-AUDIT-002
#[test]
fn action_cli_derives_roles_and_appends_before_register_mutation_boundary() {
    let root = fresh_root();
    fs::create_dir(&root).unwrap();
    let log = root.join("actions.audit");
    let registry = root.join("principals.tsv");
    let bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n1\tmonitor\n2\toperate\n3\tconfigure\n";
    fs::write(&registry, bytes).unwrap();
    let pin = oa_sha256::to_hex(&oa_sha256::sha256(bytes));
    let refused = append(
        &log,
        &registry,
        &pin,
        "1",
        "vulnerability-intake",
        genesis(),
    );
    assert!(!refused.status.success());
    assert!(!log.exists());

    let accepted = append(
        &log,
        &registry,
        &pin,
        "2",
        "vulnerability-intake",
        genesis(),
    );
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let next_tail = String::from_utf8_lossy(&accepted.stdout)
        .split('\t')
        .nth(2)
        .unwrap()
        .to_owned();
    assert_eq!(next_tail.len(), 64);
    let mut tail = [0; 32];
    for (index, slot) in tail.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&next_tail[index * 2..index * 2 + 2], 16).unwrap();
    }
    assert_eq!(
        verify(&log, Checkpoint { count: 1, tail }).unwrap().count,
        1
    );

    let denied = append(
        &log,
        &registry,
        &pin,
        "2",
        "vulnerability-disposition",
        Checkpoint { count: 1, tail },
    );
    assert!(!denied.status.success());
    assert_eq!(
        verify(&log, Checkpoint { count: 1, tail }).unwrap().count,
        1
    );
    fs::remove_dir_all(root).unwrap();
}
