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
    let registry_pin =
        oa_sha256::sha256(&fs::read(inputs.registry).expect("read registry pin input"));
    Command::new(inputs.executable)
        .arg("certify")
        .arg(inputs.log)
        .arg(checkpoint.0.to_string())
        .arg(checkpoint.1.clone())
        .arg(inputs.registry)
        .arg(oa_sha256::to_hex(&registry_pin))
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
    let legacy_snapshot = include_bytes!("../../oa-ca/tests/fixtures/a3-empty-synthetic.oag2");
    let model =
        oa_airspace::snapshot::decode(legacy_snapshot).expect("legacy synthetic snapshot decodes");
    let snapshot =
        oa_airspace::snapshot::encode(&model).expect("current canonical synthetic snapshot");
    let previous = root.join("adaptation-previous");
    let previous_pin = oa_adapt::manifest::write_new_package(
        &previous,
        &snapshot,
        "synthetic-1",
        "ZAU",
        "OA-CERT-CLI-TEST",
    )
    .expect("issue and read back synthetic G2 package");
    let catalog = include_bytes!("../data/synthetic-pver-v1.csv");
    let (_, manifest) = oa_adapt::manifest::issue_new_package_with_pver_catalog(
        &package,
        &snapshot,
        catalog,
        None,
        "synthetic-2",
        "ZAU",
        "OA-CERT-CLI-PVER",
        &previous,
        previous_pin,
        oa_adapt::manifest::ChangeClass::Initial,
        "issuer-a",
        "approver-b",
        "issued synthetic PVER target catalog",
    )
    .expect("issue and read back manifest-bound v4 PVER catalog");
    let artifact = root.join("oa-certification");
    fs::copy(env!("CARGO_BIN_EXE_oa-certification"), &artifact)
        .expect("copy standalone binary for guarded artifact hashing");
    let software_digest = oa_sha256::sha256(&fs::read(&artifact).expect("read synthetic binary"));
    let adaptation_digest = oa_sha256::sha256(&snapshot);
    let pver = Command::new(&artifact)
        .arg("pver")
        .arg("full")
        .arg(&package)
        .arg(oa_sha256::to_hex(&manifest))
        .output()
        .expect("run bound PVER CLI");
    assert!(
        pver.status.success(),
        "{}",
        String::from_utf8_lossy(&pver.stderr)
    );
    let pver_output = String::from_utf8_lossy(&pver.stdout);
    assert!(
        pver_output.contains("catalog_version=synthetic-pver-v1"),
        "{pver_output}"
    );
    assert!(
        pver_output.contains("catalog_adaptation_version=Some(1)"),
        "{pver_output}"
    );
    let abbreviated = Command::new(&artifact)
        .arg("pver")
        .arg("abbreviated")
        .arg(&package)
        .arg(oa_sha256::to_hex(&manifest))
        .output()
        .expect("run abbreviated PVER CLI");
    assert!(
        abbreviated.status.success(),
        "{}",
        String::from_utf8_lossy(&abbreviated.stderr)
    );
    let abbreviated_output = String::from_utf8_lossy(&abbreviated.stdout);
    assert!(
        abbreviated_output.contains("targets=3/3"),
        "{abbreviated_output}"
    );
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
        .arg(checkpoint.1.clone())
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
    assert!(report.contains("catalog_version=synthetic-pver-v1"));
    assert!(report.contains("catalog_adaptation_version=1"));
    assert!(report.contains("catalog_version=synthetic-pver-v1"));
    assert!(report.contains("catalog_adaptation_version=1"));
    assert!(report.contains(&format!(
        "software_observed_sha256={}",
        oa_sha256::to_hex(&software_digest)
    )));
    assert!(report.contains(&format!(
        "adaptation_observed_sha256={}",
        oa_sha256::to_hex(&adaptation_digest)
    )));
    let report_len = fs::metadata(&log).expect("final audit length").len();
    let unknown_version = String::from_utf8(catalog.to_vec())
        .expect("catalog UTF-8")
        .replacen("catalog v1;", "catalog v2;", 1)
        .into_bytes();
    let mut arbitrary_schema_row = catalog.to_vec();
    let insert_at = arbitrary_schema_row
        .windows(1)
        .enumerate()
        .filter_map(|(index, byte)| (byte == b"\n").then_some(index + 1))
        .nth(1)
        .expect("two catalog header lines");
    arbitrary_schema_row.splice(
        insert_at..insert_at,
        b"schema,synthetic-pver-v2\n".iter().copied(),
    );
    let duplicate_abbreviated = String::from_utf8(catalog.to_vec())
        .expect("catalog UTF-8")
        .replace(
            "synthetic-pver-v1,abbreviated,TGT-05,398000000,-748000000,-55.0,20.0,1500,35",
            "synthetic-pver-v1,abbreviated,TGT-01,400000000,-750000000,42.0,12.0,1500,35",
        )
        .into_bytes();
    for (label, bad_catalog) in [
        ("unknown-version", unknown_version),
        ("arbitrary-schema-row", arbitrary_schema_row),
        ("duplicate-abbreviated", duplicate_abbreviated),
    ] {
        let bad_catalog_path = root.join(format!("{label}.csv"));
        fs::write(&bad_catalog_path, &bad_catalog).expect("bad catalog bytes");
        let bad_package = root.join(format!("{label}-package"));
        let (_, bad_pin) = oa_adapt::manifest::issue_new_package_with_pver_catalog(
            &bad_package,
            &snapshot,
            &bad_catalog,
            None,
            "synthetic-invalid",
            "ZAU",
            label,
            &previous,
            previous_pin,
            oa_adapt::manifest::ChangeClass::Initial,
            "issuer-a",
            "approver-b",
            "negative parser fixture",
        )
        .expect("G2 issues exact invalid semantic fixture for loader refusal test");
        let pver = Command::new(&artifact)
            .arg("pver")
            .arg("full")
            .arg(&bad_package)
            .arg(oa_sha256::to_hex(&bad_pin))
            .output()
            .expect("run PVER refusal CLI");
        assert!(!pver.status.success(), "{label} must refuse PVER");
        fs::write(
            &observations,
            format!(
                "OA-CERT-OBSERVATIONS-V1\n7\t{}\t{}\t{}\n",
                artifact.display(),
                bad_package.display(),
                oa_sha256::to_hex(&bad_pin)
            ),
        )
        .expect("bad package observation");
        let refused = certify(&inputs, checkpoint.clone(), "", 1_800_000_000_000_000_010);
        assert!(
            !refused.status.success(),
            "{label} must refuse certification"
        );
        assert_eq!(
            fs::metadata(&log).expect("audit remains readable").len(),
            report_len
        );
    }
    fs::remove_dir_all(root).expect("fixture cleanup");
}

// REQ: OA-AUDIT-002, OA-AUDIT-003, OA-AUDIT-005
#[test]
fn certification_cli_requires_pinned_configure_principal_before_any_append() {
    let root = fresh_root();
    fs::create_dir(&root).expect("fixture root");
    let executable = root.join("oa-certification");
    fs::copy(env!("CARGO_BIN_EXE_oa-certification"), &executable).expect("copy CLI");
    let registry = root.join("principals.tsv");
    let log = root.join("audit.log");
    fs::write(&registry, b"OACERT-LOCAL-PRINCIPALS-V1\n11\tmonitor\n").expect("monitor registry");
    let pin = oa_sha256::sha256(&fs::read(&registry).expect("read synthetic registry"));
    let invoke = |registry_pin: String| {
        Command::new(&executable)
            .arg("certify")
            .arg(&log)
            .arg("0")
            .arg("00".repeat(32))
            .arg(&registry)
            .arg(registry_pin)
            .arg("11")
            .arg("1000")
            .arg("synthetic-service")
            .arg("full")
            .arg("missing-roster")
            .arg("missing-observations")
            .arg("")
            .arg("1000000")
            .output()
            .expect("run CLI")
    };
    let denied_role = invoke(oa_sha256::to_hex(&pin));
    assert!(!denied_role.status.success());
    assert!(
        !log.exists(),
        "denied role must not create or append audit log"
    );

    fs::write(&registry, b"OACERT-LOCAL-PRINCIPALS-V1\n11\tconfigure\n")
        .expect("configure registry");
    let denied_pin = invoke(oa_sha256::to_hex(&pin));
    assert!(
        !denied_pin.status.success(),
        "wrong registry pin must refuse"
    );
    assert!(
        !log.exists(),
        "wrong pin must not create or append audit log"
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}
