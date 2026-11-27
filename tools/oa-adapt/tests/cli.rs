//! The `oa-adapt` command line, end to end over the fixture archives.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_adapt::cli::*;
use oa_airspace::load::parse_expectation;
use oa_airspace::snapshot::decode;
use oa_sha256::{sha256, to_hex};
use std::path::{Path, PathBuf};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

/// A scratch directory unique to this test process and name, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("oa-airspace-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_archives(s: &Scratch) -> (PathBuf, PathBuf, Vec<u8>, Vec<u8>) {
    let nasr = nasr_zip(&nasr_files());
    let cifp = cifp_zip(&cifp_file());
    let (np, cp) = (s.path(NASR_NAME), s.path(CIFP_NAME));
    std::fs::write(&np, &nasr).expect("write nasr");
    std::fs::write(&cp, &cifp).expect("write cifp");
    (np, cp, nasr, cifp)
}

fn build_args(np: &Path, cp: &Path, out: &Path, extra: &[String]) -> Vec<String> {
    let mut a = args(&[
        "build",
        "--nasr",
        &np.to_string_lossy(),
        "--cifp",
        &cp.to_string_lossy(),
        "--artcc",
        "ZAU",
        "--out",
        &out.to_string_lossy(),
    ]);
    a.extend_from_slice(extra);
    a
}

// REQ: OA-GEN-001
#[test]
fn command_binary_forbids_unsafe_code() {
    let main = include_str!("../src/main.rs");
    assert!(main.starts_with("#![forbid(unsafe_code)]"));
}

// REQ: OA-CERT-001
#[test]
fn issue_cli_binds_catalog_then_runway_aids_and_reads_v5_back() {
    let scratch = Scratch::new("issue-pver-v4");
    let legacy_snapshot = include_bytes!("../../oa-ca/tests/fixtures/a3-empty-synthetic.oag2");
    let model = decode(legacy_snapshot).expect("legacy v3 synthetic snapshot");
    // Package issuance requires the current canonical encoding, while this fixture
    // intentionally starts from the legacy snapshot used by older callers.
    let snapshot = oa_airspace::snapshot::encode(&model).expect("current canonical snapshot");
    let snapshot_path = scratch.path("snapshot.oag2");
    std::fs::write(&snapshot_path, &snapshot).expect("synthetic snapshot");
    let previous_root = scratch.path("previous");
    let previous_pin = oa_adapt::manifest::write_new_package(
        &previous_root,
        &snapshot,
        "before",
        "ZAU",
        "initial",
    )
    .expect("previous package");
    let catalog_path = scratch.path("pver.csv");
    let catalog = include_bytes!("../../oa-certification/data/synthetic-pver-v1.csv");
    std::fs::write(&catalog_path, catalog).expect("synthetic catalog");
    let registry_path = scratch.path("principals.tsv");
    let registry = b"OACERT-LOCAL-PRINCIPALS-V1\n11\tconfigure\n";
    std::fs::write(&registry_path, registry).expect("synthetic principal registry");
    let audit = scratch.path("adaptation.audit");
    let output = scratch.path("issued");
    let issue_args = args(&[
        "issue",
        "--snapshot",
        snapshot_path.to_str().unwrap(),
        "--out-root",
        output.to_str().unwrap(),
        "--previous-package-root",
        previous_root.to_str().unwrap(),
        "--previous-manifest-sha256",
        &to_hex(&previous_pin),
        "--release",
        "after",
        "--facility",
        "ZAU",
        "--change-ref",
        "PVER-ISSUE-1",
        "--change-class",
        "initial",
        "--issuer",
        "issuer-a",
        "--approver",
        "approver-b",
        "--reason",
        "issue synthetic PVER catalog",
        "--audit-registry",
        registry_path.to_str().unwrap(),
        "--audit-registry-sha256",
        &to_hex(&sha256(registry)),
        "--audit-log",
        audit.to_str().unwrap(),
        "--audit-count",
        "0",
        "--audit-tail",
        &"00".repeat(32),
        "--audit-principal",
        "11",
        "--audit-role",
        "configure",
        "--audit-time-ns",
        "1800000000000000000",
        "--pver-catalog",
        catalog_path.to_str().unwrap(),
    ]);
    let mut stdout = Vec::new();
    run(&issue_args, &mut stdout).expect("CLI issues v4 package and appends audit event");
    let text = String::from_utf8(stdout).expect("CLI output is UTF-8");
    let pin = text
        .lines()
        .find_map(|line| line.strip_prefix("manifest_sha256\t"))
        .and_then(|text| oa_adapt::manifest::parse_pin(text).ok())
        .expect("issued manifest pin");
    let verified = oa_adapt::manifest::verify_current(&output, pin).expect("v4 package verifies");
    assert_eq!(verified.identity().adaptation_version, Some(1));
    assert_eq!(verified.pver_catalog_bytes(), Some(catalog.as_slice()));
    assert_eq!(
        verified.identity().pver_catalog_sha256,
        Some(sha256(catalog))
    );

    let runway_profile = {
        let mut bytes =
            vec![0u8; oa_runway::adaptation::HEADER_BYTES + oa_runway::adaptation::PAIR_BYTES];
        bytes[..8].copy_from_slice(oa_runway::adaptation::MAGIC);
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        let row = &mut bytes[10..];
        row[..4].copy_from_slice(b"KORD");
        row[4..8].copy_from_slice(&1u32.to_le_bytes());
        row[8..11].copy_from_slice(b"27L");
        row[11..13].copy_from_slice(&150u16.to_le_bytes());
        row[13..17].copy_from_slice(&2u32.to_le_bytes());
        row[17..20].copy_from_slice(b"27R");
        row[20..22].copy_from_slice(&150u16.to_le_bytes());
        row[22..24].copy_from_slice(&2_000u16.to_le_bytes());
        row[24..26].copy_from_slice(&3_100u16.to_le_bytes());
        row[26..30].copy_from_slice(&10_000u32.to_le_bytes());
        row[30..34].copy_from_slice(&1_200u32.to_le_bytes());
        row[34..38].copy_from_slice(&10_000u32.to_le_bytes());
        row[38..42].copy_from_slice(&50_000u32.to_le_bytes());
        row[42..44].copy_from_slice(&500u16.to_le_bytes());
        bytes
    };
    let runway_profile_path = scratch.path("runway-aids.oarw");
    std::fs::write(&runway_profile_path, &runway_profile).expect("synthetic runway profile");
    let runway_output = scratch.path("issued-runway-aids");
    let next_count = text
        .lines()
        .find_map(|line| line.strip_prefix("audit_count\t"))
        .expect("first issue audit count");
    let next_tail = text
        .lines()
        .find_map(|line| line.strip_prefix("audit_tail\t"))
        .expect("first issue audit tail");
    let pin_hex = to_hex(&pin);
    let mut v5_args = Vec::new();
    for pair in issue_args.chunks_exact(2) {
        let [flag, value] = pair else { continue };
        if flag == "--pver-catalog" {
            continue;
        }
        let replacement = match flag.as_str() {
            "--out-root" => runway_output.to_str().unwrap(),
            "--previous-package-root" => output.to_str().unwrap(),
            "--previous-manifest-sha256" => &pin_hex,
            "--change-ref" => "RUNWAY-AID-ISSUE-2",
            "--change-class" => "routine",
            "--reason" => "issue synthetic runway-aid companion",
            "--audit-count" => next_count,
            "--audit-tail" => next_tail,
            "--audit-time-ns" => "1800000000000000001",
            _ => value,
        };
        v5_args.push(flag.clone());
        v5_args.push(replacement.to_owned());
    }
    v5_args.extend(args(&[
        "--runway-aids",
        runway_profile_path.to_str().unwrap(),
    ]));
    let mut v5_stdout = Vec::new();
    run(&v5_args, &mut v5_stdout).expect("CLI issues v5 runway-aid package");
    let v5_text = String::from_utf8(v5_stdout).expect("v5 output is UTF-8");
    let v5_pin = v5_text
        .lines()
        .find_map(|line| line.strip_prefix("manifest_sha256\t"))
        .and_then(|value| oa_adapt::manifest::parse_pin(value).ok())
        .expect("v5 manifest pin");
    let v5 =
        oa_adapt::manifest::verify_current(&runway_output, v5_pin).expect("v5 package verifies");
    assert_eq!(v5.identity().adaptation_version, Some(2));
    assert_eq!(v5.pver_catalog_bytes(), Some(catalog.as_slice()));
    assert_eq!(v5.runway_aids_bytes(), Some(runway_profile.as_slice()));
}

// REQ: OA-ASP-001
#[test]
fn build_arguments_parse_with_the_default_buffer_of_50_nm() {
    let a = parse_build_args(&args(&[
        "--nasr", "n.zip", "--cifp", "c.zip", "--artcc", "ZAU", "--out", "o.snap",
    ]))
    .expect("parses");
    assert_eq!(a.buffer_m, 92_600);
    assert_eq!(a.artcc, "ZAU");
    assert!(a.expect.is_empty());
    let hex = "00".repeat(32);
    let b = parse_build_args(&args(&[
        "--out",
        "o",
        "--artcc",
        "ZMP",
        "--buffer-nm",
        "10",
        "--cifp",
        "c",
        "--nasr",
        "n",
        "--expect-sha256",
        &format!("n={hex}"),
        "--expect-sha256",
        &format!("c={hex}"),
    ]))
    .expect("parses");
    assert_eq!(b.buffer_m, 18_520);
    assert_eq!(b.expect.len(), 2, "--expect-sha256 repeats");
    assert_eq!(parse_buffer_nm("0").expect("zero"), 0);
    assert_eq!(parse_buffer_nm("0.5").expect("half"), 926);
    for bad in ["-1", "nan", "inf", "abc", "", "1e9", "99999"] {
        assert!(parse_buffer_nm(bad).is_err(), "{bad:?}");
    }
    for bad in [
        args(&["--nasr", "n", "--cifp", "c", "--artcc", "ZAU"]),
        args(&["--nasr", "n", "--cifp", "c", "--out", "o"]),
        args(&["--cifp", "c", "--artcc", "ZAU", "--out", "o"]),
        args(&["--nasr", "n", "--artcc", "ZAU", "--out", "o"]),
        args(&["--nasr"]),
        args(&[
            "--nasr", "n", "--cifp", "c", "--artcc", "ZAU", "--out", "o", "--bogus", "x",
        ]),
        args(&[
            "--nasr",
            "n",
            "--cifp",
            "c",
            "--artcc",
            "ZAU",
            "--out",
            "o",
            "--expect-sha256",
            "bad",
        ]),
    ] {
        assert!(
            matches!(parse_build_args(&bad), Err(CliError::Usage(_))),
            "{bad:?}"
        );
    }
    assert_eq!(
        report_path(&PathBuf::from("a/b.snap")),
        PathBuf::from("a/b.snap.report.txt")
    );
}

// REQ: OA-ASP-006, OA-CORE-006
#[test]
fn build_writes_the_snapshot_and_report_and_a_rebuild_is_byte_identical() {
    let s = Scratch::new("build");
    let (np, cp, nasr, cifp) = write_archives(&s);
    let out1 = s.path("one.snap");
    let out2 = s.path("two.snap");
    let expect = vec![
        "--expect-sha256".to_string(),
        format!("{NASR_NAME}={}", to_hex(&sha256(&nasr))),
        "--expect-sha256".to_string(),
        format!("{CIFP_NAME}={}", to_hex(&sha256(&cifp))),
    ];
    let mut stdout = Vec::new();
    run(&build_args(&np, &cp, &out1, &expect), &mut stdout).expect("build");
    run(&build_args(&np, &cp, &out2, &expect), &mut Vec::new()).expect("rebuild");
    let (b1, b2) = (
        std::fs::read(&out1).expect("snapshot"),
        std::fs::read(&out2).expect("snapshot"),
    );
    assert_eq!(b1, b2, "identical inputs give a byte-identical snapshot");
    let r1 = std::fs::read_to_string(report_path(&out1)).expect("report");
    // Reports differ only in nothing: no path or time enters them.
    let r2 = std::fs::read_to_string(report_path(&out2)).expect("report");
    assert_eq!(r1, r2);
    // The summary and the report name the snapshot's SHA-256, which is the SHA-256 of the file.
    let hash = to_hex(&sha256(&b1));
    assert!(String::from_utf8_lossy(&stdout).contains(&hash));
    assert!(r1.contains(&hash));
    assert!(r1.contains("matches the --expect-sha256 value given"));
    // The file reads back.
    let m = decode(&b1).expect("decode");
    assert_eq!(m.meta.artcc, "ZAU");
    assert_eq!(m.meta.buffer_m, 92_600);
}

// REQ: OA-ASP-001
#[test]
fn a_wrong_expected_hash_refuses_the_build_and_writes_nothing() {
    let s = Scratch::new("refuse");
    let (np, cp, nasr, _) = write_archives(&s);
    let out = s.path("x.snap");
    let mut wrong = sha256(&nasr);
    wrong[31] ^= 0x10;
    let extra = args(&[
        "--expect-sha256",
        &format!("{NASR_NAME}={}", to_hex(&wrong)),
    ]);
    let err = run(&build_args(&np, &cp, &out, &extra), &mut Vec::new()).expect_err("mismatch");
    let msg = err.to_string();
    assert!(
        msg.contains("sha256") || msg.contains("SHA-256") || msg.contains("hash"),
        "{msg}"
    );
    assert!(msg.contains(NASR_NAME), "{msg}");
    assert!(!out.exists() && !report_path(&out).exists());
    // Missing input file, unknown ARTCC, unknown command: errors, not panics.
    let mut a = build_args(&np, &cp, &out, &[]);
    a[2] = s.path("missing.zip").to_string_lossy().into_owned();
    assert!(run(&a, &mut Vec::new()).is_err());
    let mut a = build_args(&np, &cp, &out, &[]);
    a[6] = "ZZZ".to_string();
    assert!(run(&a, &mut Vec::new()).is_err());
    assert!(matches!(
        run(&args(&["frobnicate"]), &mut Vec::new()),
        Err(CliError::Usage(_))
    ));
    assert!(matches!(run(&[], &mut Vec::new()), Err(CliError::Usage(_))));
    assert!(!out.exists());
}

// REQ: OA-ASP-007
#[test]
fn diff_command_lists_changes_between_two_snapshots() {
    let s = Scratch::new("diff");
    let (np, cp, _, _) = write_archives(&s);
    let (a, b) = (s.path("a.snap"), s.path("b.snap"));
    run(&build_args(&np, &cp, &a, &[]), &mut Vec::new()).expect("build a");
    run(
        &build_args(&np, &cp, &b, &args(&["--buffer-nm", "0"])),
        &mut Vec::new(),
    )
    .expect("build b");
    let mut out = Vec::new();
    run(
        &args(&["diff", &a.to_string_lossy(), &b.to_string_lossy()]),
        &mut out,
    )
    .expect("diff");
    let text = String::from_utf8(out).expect("utf-8");
    assert!(text.contains("header buffer (m): 92600 -> 0"), "{text}");
    assert!(
        text.contains("- point "),
        "the smaller buffer removes points"
    );
    let mut same = Vec::new();
    run(
        &args(&["diff", &a.to_string_lossy(), &a.to_string_lossy()]),
        &mut same,
    )
    .expect("diff");
    let same = String::from_utf8(same).expect("utf-8");
    assert!(
        same.contains("headers identical") && same.contains("entries: 0"),
        "{same}"
    );
    // Wrong argument count, a missing file and a corrupt file are errors.
    assert!(run(&args(&["diff", "only-one"]), &mut Vec::new()).is_err());
    assert!(run(&args(&["diff", "a", "b", "c"]), &mut Vec::new()).is_err());
    assert!(run(
        &args(&[
            "diff",
            &a.to_string_lossy(),
            &s.path("nope").to_string_lossy()
        ]),
        &mut Vec::new()
    )
    .is_err());
    let bad = s.path("bad.snap");
    let mut bytes = std::fs::read(&a).expect("read");
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&bad, bytes).expect("write");
    assert!(run(
        &args(&["diff", &a.to_string_lossy(), &bad.to_string_lossy()]),
        &mut Vec::new()
    )
    .is_err());
}

// REQ: OA-ASP-013
#[test]
fn arguments_are_strict_about_hex_duplicates_empty_values_and_unknown_flags() {
    let hex = "ab".repeat(32);
    // `u8::from_str_radix` accepts a leading plus sign; the hash parser must not.
    let plus = format!("A.zip=+f{}", &hex[2..]);
    assert_eq!(plus.len() - "A.zip=".len(), 64);
    assert!(parse_expectation(&plus).is_err());
    assert!(parse_expectation(&format!("A.zip={}\u{e9}", &hex[..62])).is_err());

    let base = ["--nasr", "n", "--cifp", "c", "--artcc", "ZAU", "--out", "o"];
    let with = |extra: &[&str]| {
        let mut v: Vec<&str> = base.to_vec();
        v.extend_from_slice(extra);
        args(&v)
    };
    let h0 = "00".repeat(32);
    let h1 = "11".repeat(32);
    for (what, bad) in [
        ("duplicate --nasr", with(&["--nasr", "n2"])),
        ("duplicate --out", with(&["--out", "o2"])),
        (
            "duplicate --buffer-nm",
            with(&["--buffer-nm", "1", "--buffer-nm", "2"]),
        ),
        ("duplicate --artcc", with(&["--artcc", "ZMP"])),
        (
            "one file expected twice",
            with(&[
                "--expect-sha256",
                &format!("n={h0}"),
                "--expect-sha256",
                &format!("n={h1}"),
            ]),
        ),
        (
            "empty value",
            args(&["--nasr", "", "--cifp", "c", "--artcc", "ZAU", "--out", "o"]),
        ),
        (
            "a flag where a value belongs",
            args(&["--nasr", "--cifp", "c", "--artcc", "ZAU", "--out", "o"]),
        ),
        (
            "bad artcc",
            args(&["--nasr", "n", "--cifp", "c", "--artcc", "Z A", "--out", "o"]),
        ),
        (
            "long artcc",
            args(&[
                "--nasr",
                "n",
                "--cifp",
                "c",
                "--artcc",
                "ABCDEFGHI",
                "--out",
                "o",
            ]),
        ),
        ("bare word", with(&["stray"])),
        ("malformed hash", with(&["--expect-sha256", &plus])),
    ] {
        assert!(
            matches!(parse_build_args(&bad), Err(CliError::Usage(_))),
            "{what}: {bad:?}"
        );
    }
    // The same flags without the fault parse.
    assert!(parse_build_args(&with(&["--expect-sha256", &format!("n={h0}")])).is_ok());
    // `diff` takes exactly two paths and no flags.
    let mut sink = Vec::new();
    for bad in [
        args(&["a"]),
        args(&["a", "b", "c"]),
        args(&["a", "--bogus"]),
        args(&["a", ""]),
    ] {
        assert!(
            matches!(
                run(&[vec!["diff".to_string()], bad.clone()].concat(), &mut sink),
                Err(CliError::Usage(_))
            ),
            "{bad:?}"
        );
    }
}
