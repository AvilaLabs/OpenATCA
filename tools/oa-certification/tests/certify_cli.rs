#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn fresh_root() -> PathBuf {
    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);
    let parent = std::env::temp_dir();
    for _ in 0..128 {
        let fixture_id = NEXT_FIXTURE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("fixture identifier exhausted");
        let root = parent.join(format!(
            "oa-certification-cli-{}-{fixture_id}",
            std::process::id()
        ));
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create fixture root: {error}"),
        }
    }
    panic!("could not allocate a unique certification CLI fixture root after 128 attempts");
}

#[test]
fn concurrent_fixture_roots_are_unique_and_exclusive() {
    let workers = (0..32)
        .map(|_| std::thread::spawn(fresh_root))
        .collect::<Vec<_>>();
    let roots = workers
        .into_iter()
        .map(|worker| worker.join().expect("fixture allocation worker"))
        .collect::<Vec<_>>();
    let unique = roots.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), roots.len());
    for (index, root) in roots.iter().enumerate() {
        assert!(root.is_dir(), "fixture root was not created exclusively");
        fs::write(root.join("marker"), index.to_string()).expect("write fixture marker");
    }
    for root in roots {
        fs::remove_dir_all(root).expect("remove concurrent fixture root");
    }
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
    certify_mode(inputs, checkpoint, exceptions, time_ns, "full")
}

fn certify_mode(
    inputs: &CliInputs<'_>,
    checkpoint: (u64, String),
    exceptions: &str,
    time_ns: u64,
    mode: &str,
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
        .arg(mode)
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
    assert!(
        pver_output.contains("registration_qc=target_lat_e7:"),
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
    assert!(
        first_output.contains("techops_certification_view=committed outcome=Certified mode=Full"),
        "{first_output}"
    );
    assert!(first_output.contains("processors_expected=1 observed=1 missing=0"));
    assert!(first_output.contains("techops_target_map="));
    assert!(first_output.contains("techops_processor_evidence="));
    assert!(first_output.contains(&format!(
        "catalog_sha256={}",
        oa_sha256::to_hex(&oa_sha256::sha256(catalog))
    )));
    assert!(first_output.contains(&format!(
        "expected_inventory_sha256={}",
        oa_sha256::to_hex(&oa_sha256::sha256(
            &fs::read(&expected).expect("expected roster bytes")
        ))
    )));
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
    assert!(second_output.contains(
        "techops_certification_view=committed outcome=CertifiedWithExceptions mode=Full"
    ));
    assert!(second_output.contains("exceptions=synthetic exception"));
    let checkpoint = checkpoint_from(&second_output);

    let abbreviated = certify_mode(
        &inputs,
        checkpoint,
        "",
        1_800_000_000_000_000_002,
        "abbreviated",
    );
    assert!(
        abbreviated.status.success(),
        "{}",
        String::from_utf8_lossy(&abbreviated.stderr)
    );
    let abbreviated_output = String::from_utf8_lossy(&abbreviated.stdout);
    assert!(abbreviated_output.contains("outcome=Certified"));
    assert!(abbreviated_output
        .contains("techops_certification_view=committed outcome=Certified mode=Abbreviated"));
    assert!(abbreviated_output.contains("targets=3/3"));
    assert_eq!(abbreviated_output.matches("techops_target_map=").count(), 3);
    let checkpoint = checkpoint_from(&abbreviated_output);

    fs::write(
        &expected,
        format!(
            "OA-CERT-EXPECTED-PROCESSORS-V1\n7\t{}\t{}\n8\t{}\t{}\n",
            oa_sha256::to_hex(&[0xa5; 32]),
            oa_sha256::to_hex(&adaptation_digest),
            oa_sha256::to_hex(&[0x55; 32]),
            oa_sha256::to_hex(&adaptation_digest),
        ),
    )
    .expect("mismatched expected digest fixture");
    let third = certify(&inputs, checkpoint, "", 1_800_000_000_000_000_003);
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
    assert!(third_output
        .contains("techops_certification_view=committed outcome=NotCertified mode=Full"));
    assert!(third_output.contains("processors_expected=2 observed=1 missing=1"));
    assert!(third_output.contains("processor_id: 8"));
    assert!(third_output.contains(
        "observed_software: CertificationDigestEvidence { state: Missing, sha256: None }"
    ));
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
    assert!(report.contains("registration_qc=target_lat_e7="));
    assert!(report.contains("observed_lat_e7="));
    assert!(report.contains("east_velocity_residual_um_s="));
    assert!(report.contains("runtime_loaded_adaptation=unavailable"));
    assert!(report.contains("adaptation_observed_source=verified_local_g2_snapshot"));
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

// REQ: OA-AUDIT-012
#[test]
fn techops_status_cli_reports_verified_empty_log_and_unverified_bad_config() {
    let root = fresh_root();
    // Execute Cargo's already-built test binary directly. This test exercises the command's
    // verification output, not executable-copy semantics; copying immediately before exec
    // needlessly depends on the runner filesystem's copy/execute behavior (notably ETXTBSY).
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_oa-certification"));
    let log = root.join("audit.log");
    let config = root.join("checkpoint.conf");
    fs::write(&log, b"").expect("empty audit log");
    let checkpoint = oa_audit_core::genesis();
    fs::write(
        &config,
        format!(
            "OA-AUDIT-WATCH-V1\ngeneration=1\ncount=0\ntail={}\nrefresh_ms=0\n",
            oa_sha256::to_hex(&checkpoint.tail)
        ),
    )
    .expect("exact checkpoint config");
    let invoke = || {
        Command::new(&executable)
            .arg("techops-status")
            .arg(&log)
            .arg(&config)
            .arg("1")
            .arg("64")
            .output()
            .expect("run status CLI")
    };

    let verified = invoke();
    assert!(verified.status.success(), "{:?}", verified.stderr);
    let output = String::from_utf8(verified.stdout).expect("UTF-8 status output");
    assert!(output.contains("verified_exact_checkpoint"));
    assert!(output.contains("verified_count=0"));
    assert!(output.contains("active_alarms=0"));
    assert!(!output.contains(&oa_sha256::to_hex(&checkpoint.tail)));

    fs::write(&config, b"not a checkpoint").expect("replace config with malformed bytes");
    let unavailable = invoke();
    assert!(unavailable.status.success(), "{:?}", unavailable.stderr);
    let output = String::from_utf8(unavailable.stdout).expect("UTF-8 unavailable status");
    assert!(output.contains("verification_unavailable"));
    assert!(output.contains("verified_count=-"));

    fs::remove_dir_all(root).expect("fixture cleanup");
}
