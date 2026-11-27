#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_audit_core::{details, genesis, target, Action, Checkpoint, Event, Role};
use oa_audit_log::shell::append;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fresh_root() -> PathBuf {
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "oa-audit-verify-cli-{}-{sequence}",
        std::process::id()
    ))
}

fn event() -> Event {
    let (target, target_len) = target(b"audit-test").unwrap();
    let (details, details_len) = details(b"synthetic").unwrap();
    Event {
        principal: 3,
        role: Role::Configure,
        action: Action::Certification,
        target,
        target_len,
        time_ns: 5,
        details,
        details_len,
    }
}

fn verify(
    log: &Path,
    registry: &Path,
    pin: &str,
    principal: &str,
    checkpoint: Checkpoint,
) -> std::process::Output {
    let count = checkpoint.count.to_string();
    let tail = oa_sha256::to_hex(&checkpoint.tail);
    Command::new(env!("CARGO_BIN_EXE_oa-audit-verify"))
        .args([
            "--log",
            log.to_str().unwrap(),
            "--count",
            count.as_str(),
            "--tail",
            tail.as_str(),
            "--registry",
            registry.to_str().unwrap(),
            "--registry-sha256",
            pin,
            "--principal",
            principal,
        ])
        .output()
        .expect("run audit verifier")
}

// REQ: OA-AUDIT-007, OA-AUDIT-008
#[test]
fn verifier_requires_a_pinned_registry_principal_and_derives_configured_role() {
    let root = fresh_root();
    fs::create_dir(&root).unwrap();
    let log = root.join("actions.audit");
    let registry = root.join("principals.tsv");
    let registry_bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n1\tmonitor\n2\toperate\n3\tconfigure\n";
    fs::write(&registry, registry_bytes).unwrap();
    let pin = oa_sha256::to_hex(&oa_sha256::sha256(registry_bytes));
    let checkpoint = append(&log, genesis(), event()).unwrap();

    for (principal, expected_role) in [("1", "Monitor"), ("2", "Operate"), ("3", "Configure")] {
        let result = verify(&log, &registry, &pin, principal, checkpoint);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout)
            .contains(&format!("registry_role={expected_role}")));
    }

    let unknown = verify(&log, &registry, &pin, "4", checkpoint);
    assert!(!unknown.status.success());
    let wrong_pin = verify(&log, &registry, &"00".repeat(32), "1", checkpoint);
    assert!(!wrong_pin.status.success());
    fs::remove_dir_all(root).unwrap();
}
