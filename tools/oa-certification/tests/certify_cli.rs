#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn fresh_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "oa-certification-cli-{}-{nonce}",
        std::process::id()
    ))
}

fn checkpoint_from(output: &str) -> (u64, String) {
    let count = output
        .split_whitespace()
        .find_map(|field| field.strip_prefix("checkpoint_count="))
        .expect("CLI checkpoint count")
        .parse::<u64>()
        .expect("numeric checkpoint count");
    let tail = output
        .split_whitespace()
        .find_map(|field| field.strip_prefix("checkpoint_tail="))
        .expect("CLI checkpoint tail")
        .to_owned();
    (count, tail)
}

struct CliInputs<'a> {
    executable: &'a std::path::Path,
    log: &'a std::path::Path,
    registry: &'a std::path::Path,
    expected: &'a std::path::Path,
    observations: &'a std::path::Path,
}

fn certify(
    inputs: &CliInputs<'_>,
    checkpoint: (u64, String),
    exceptions: &str,
    time_ns: u64,
) -> std::process::Output {
    Command::new(inputs.executable)
        .arg("certify")
        .arg(inputs.log)
        .arg(checkpoint.0.to_string())
        .arg(checkpoint.1)
        .arg(inputs.registry)
        .arg("11")
        .arg(time_ns.to_string())
        .arg("synthetic-service")
        .arg("full")
        .arg(inputs.expected)
        .arg(inputs.observations)
        .arg(exceptions)
        .arg("100000000")
        .output()
        .expect("run certification CLI")
}

// REQ: OA-CERT-001, OA-CERT-002, OA-CERT-003, OA-CERT-004, OA-CERT-005, OA-CERT-006, OA-CERT-007, OA-CERT-008, OA-CERT-009, OA-CERT-010
#[test]
fn certify_cli_commits_all_three_outcomes_from_real_tracker_and_local_digest_inputs() {
    let root = fresh_root();
    fs::create_dir(&root).expect("fixture root");
    let package = root.join("adaptation");
    let snapshot = include_bytes!("../../oa-ca/tests/fixtures/a3-empty-synthetic.oag2");
    let manifest = oa_adapt::manifest::write_new_package(
        &package,
        snapshot,
        "synthetic-1",
        "ZAU",
        "OA-CERT-CLI-TEST",
    )
    .expect("issue and read back synthetic G2 package");
    let artifact = root.join("oa-certification");
    fs::copy(env!("CARGO_BIN_EXE_oa-certification"), &artifact)
        .expect("copy standalone binary for guarded artifact hashing");
    let software_digest = oa_sha256::sha256(&fs::read(&artifact).expect("read synthetic binary"));
    let adaptation_digest = oa_sha256::sha256(snapshot);
    let registry = root.join("principals.tsv");
    fs::write(&registry, b"OACERT-LOCAL-PRINCIPALS-V1\n11\tconfigure\n")
        .expect("configure claim fixture");
    let expected = root.join("expected.tsv");
    let observations = root.join("observations.tsv");
    fs::write(
        &expected,
        format!(
            "OA-CERT-EXPECTED-PROCESSORS-V1\n7\t{}\t{}\n",
            oa_sha256::to_hex(&software_digest),
            oa_sha256::to_hex(&adaptation_digest)
        ),
    )
    .expect("independent roster fixture");
    fs::write(
        &observations,
        format!(
            "OA-CERT-OBSERVATIONS-V1\n7\t{}\t{}\t{}\n",
            artifact.display(),
            package.display(),
            oa_sha256::to_hex(&manifest)
        ),
    )
    .expect("local observation fixture");
    let log = root.join("certification.audit");
    let genesis = (0, "00".repeat(32));
    let inputs = CliInputs {
        executable: &artifact,
        log: &log,
        registry: &registry,
        expected: &expected,
        observations: &observations,
    };

    let first = certify(&inputs, genesis, "", 1_800_000_000_000_000_000);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_output = String::from_utf8_lossy(&first.stdout);
    assert!(first_output.contains("outcome=Certified"), "{first_output}");
    assert!(first_output.contains("audit_durable=true"));
    let checkpoint = checkpoint_from(&first_output);

    let second = certify(
        &inputs,
        checkpoint,
        "synthetic exception",
        1_800_000_000_000_000_001,
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_output = String::from_utf8_lossy(&second.stdout);
    assert!(
        second_output.contains("outcome=CertifiedWithExceptions"),
        "{second_output}"
    );
    let checkpoint = checkpoint_from(&second_output);

    fs::write(
        &expected,
        format!(
            "OA-CERT-EXPECTED-PROCESSORS-V1\n7\t{}\t{}\n",
            oa_sha256::to_hex(&[0xa5; 32]),
            oa_sha256::to_hex(&adaptation_digest)
        ),
    )
    .expect("mismatched expected digest fixture");
    let third = certify(&inputs, checkpoint, "", 1_800_000_000_000_000_002);
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    let third_output = String::from_utf8_lossy(&third.stdout);
    assert!(
        third_output.contains("outcome=NotCertified"),
        "{third_output}"
    );
    let checkpoint = checkpoint_from(&third_output);

    let report = Command::new(&artifact)
        .arg("report")
        .arg(&log)
        .arg(checkpoint.0.to_string())
        .arg(checkpoint.1)
        .output()
        .expect("verify and read committed report");
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let report = String::from_utf8_lossy(&report.stdout);
    assert!(report.contains("outcome=certified"));
    assert!(report.contains("outcome=certified_with_exceptions"));
    assert!(report.contains("outcome=not_certified"));
    assert!(report.contains(&format!(
        "software_observed_sha256={}",
        oa_sha256::to_hex(&software_digest)
    )));
    assert!(report.contains(&format!(
        "adaptation_observed_sha256={}",
        oa_sha256::to_hex(&adaptation_digest)
    )));
    fs::remove_dir_all(root).expect("fixture cleanup");
}
