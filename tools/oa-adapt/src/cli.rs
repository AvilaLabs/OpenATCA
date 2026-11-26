//! The `oa-adapt` command line. **Shell** (B1, B7): argument parsing, file reads and writes, output.
//! All decisions are made by the core modules it calls.
//!
//! ```text
//! oa-adapt build --nasr <zip> --cifp <zip> --artcc ZAU [--buffer-nm 50]
//!                [--expect-sha256 <file>=<hex>]... --out <snapshot>
//! oa-adapt diff <a> <b>
//! oa-adapt package --snapshot <snapshot> --out-root <new-directory> --release <id>
//!                   --facility <ARTCC> --change-ref <approved-record>
//! oa-adapt issue --snapshot <snapshot> --out-root <new-directory> --previous-package-root <dir>
//!                 --previous-manifest-sha256 <hex> --release <id> --facility <ARTCC>
//!                 --change-ref <record> --change-class initial|routine|field-change
//!                 --issuer <principal-id> --approver <principal-id> --reason <text>
//! oa-adapt fleet-check --issued-package-root <dir> --expected-manifest-sha256 <hex>
//!                       --expected <processor-list.tsv> --reports-dir <dir>
//!                       --previous-epoch <number> --epoch <number> --clock-domain-id <number>
//!                       --query-time-ns <number> --freshness-limit-ns <number>
//! ```
//!
//! `build` writes `<snapshot>` and `<snapshot>.report.txt`. `diff` prints to standard output.

use oa_airspace::build::{build_model, BuildParams, MAX_ARTCC_LEN, MAX_BUFFER_M, NM_M};
use oa_airspace::diff::{diff_models, render_diff};
use oa_airspace::load::{parse_expectation, Expectation, LoadError};
use oa_airspace::report::{render_report, ReportInput};
use oa_airspace::snapshot::{decode, encode, MAX_SNAPSHOT_BYTES};
use oa_sha256::{sha256, to_hex};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Reads the two archives through the private path guard before passing exact bytes to the
/// reusable parser. The parser itself remains path-policy neutral.
pub fn load_files(
    nasr: &Path,
    cifp: &Path,
    expect: &[Expectation],
) -> Result<oa_airspace::sources::Sources, LoadError> {
    let name = |path: &Path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let nasr_name = name(nasr);
    let cifp_name = name(cifp);
    let nasr_bytes = oa_sealed::open(nasr)
        .map_err(oa_zip::ZipError::Io)
        .and_then(oa_zip::read_archive)
        .map_err(|source| LoadError::Zip {
            archive: nasr_name.clone(),
            source,
        })?;
    let cifp_bytes = oa_sealed::open(cifp)
        .map_err(oa_zip::ZipError::Io)
        .and_then(oa_zip::read_archive)
        .map_err(|source| LoadError::Zip {
            archive: cifp_name.clone(),
            source,
        })?;
    oa_airspace::load::load_bytes(&nasr_name, &nasr_bytes, &cifp_name, &cifp_bytes, expect)
}

/// Default selection buffer, nautical miles.
pub const DEFAULT_BUFFER_NM: f64 = 50.0;
const MAX_PACKAGE_ARGUMENT_BYTES: usize = 4096;

/// Why a command failed; `Display` is the message printed to the user.
#[derive(Debug)]
pub enum CliError {
    /// The arguments do not match the usage.
    Usage(String),
    /// A step failed; the text names the step and the cause.
    Failed(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}\n\n{USAGE}"),
            CliError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

/// The usage text.
pub const USAGE: &str = "usage:
  oa-adapt build --nasr <zip> --cifp <zip> --artcc <ID> [--buffer-nm <NM>]
                 [--expect-sha256 <file>=<hex>]... [--allow-cycle-mismatch] --out <snapshot>
  oa-adapt diff <snapshot-a> <snapshot-b>
  oa-adapt package --snapshot <snapshot> --out-root <new-directory> --release <id>
                   --facility <ARTCC> --change-ref <approved-record>
  oa-adapt issue --snapshot <snapshot> --out-root <new-directory> --previous-package-root <dir>
                 --previous-manifest-sha256 <hex> --release <id> --facility <ARTCC>
                 --change-ref <record> --change-class initial|routine|field-change
                 --issuer <principal-id> --approver <principal-id> --reason <text>
  oa-adapt fleet-check --issued-package-root <dir> --expected-manifest-sha256 <hex>
                       --expected <processor-list.tsv> --reports-dir <dir>
                       --previous-epoch <number> --epoch <number> --clock-domain-id <number>
                       --query-time-ns <number> --freshness-limit-ns <number>
  oa-adapt route --snapshot <snapshot> --steps <identity-qualified instructions>
  oa-adapt route --package-root <directory> --expected-manifest-sha256 <hex> --steps <instructions>";

fn failed(step: &str, e: impl fmt::Display) -> CliError {
    CliError::Failed(format!("{step}: {e}"))
}

/// Parsed `build` arguments.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct BuildArgs {
    pub nasr: PathBuf,
    pub cifp: PathBuf,
    pub artcc: String,
    pub buffer_m: u32,
    pub expect: Vec<Expectation>,
    pub out: PathBuf,
    /// `--allow-cycle-mismatch`.
    pub allow_cycle_mismatch: bool,
}

/// A `--buffer-nm` value to whole metres; rejects non-finite, negative and over-limit values.
pub fn parse_buffer_nm(text: &str) -> Result<u32, CliError> {
    let nm: f64 = text
        .parse()
        .map_err(|_| CliError::Usage(format!("--buffer-nm {text:?} is not a number")))?;
    let m = nm * NM_M;
    if !m.is_finite() || m < 0.0 || m > f64::from(MAX_BUFFER_M) {
        return Err(CliError::Usage(format!(
            "--buffer-nm {text} is outside 0 to {} NM",
            f64::from(MAX_BUFFER_M) / NM_M
        )));
    }
    // In range and finite, so the rounded value fits.
    Ok(m.round() as u32)
}

fn need<T>(v: Option<T>, flag: &str) -> Result<T, CliError> {
    v.ok_or_else(|| CliError::Usage(format!("missing {flag}")))
}

/// Stores a flag's value; a flag given twice is an error, not "the last one wins".
fn set_once<T>(slot: &mut Option<T>, flag: &str, v: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::Usage(format!("{flag} given more than once")));
    }
    *slot = Some(v);
    Ok(())
}

/// An ARTCC identifier: 1 to [`MAX_ARTCC_LEN`] ASCII letters or digits.
pub fn parse_artcc(text: &str) -> Result<String, CliError> {
    let ok = !text.is_empty()
        && text.len() <= MAX_ARTCC_LEN
        && text.bytes().all(|b| b.is_ascii_alphanumeric());
    if !ok {
        return Err(CliError::Usage(format!(
            "--artcc {text:?} is not 1 to {MAX_ARTCC_LEN} letters or digits"
        )));
    }
    Ok(text.to_string())
}

/// Parses the arguments after `build`.
pub fn parse_build_args(args: &[String]) -> Result<BuildArgs, CliError> {
    let mut nasr = None;
    let mut cifp = None;
    let mut artcc = None;
    let mut buffer_m = None;
    let mut expect = Vec::new();
    let mut out = None;
    let mut allow_cycle_mismatch = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        // A value is the next argument; a missing, empty or flag-looking one is an error (a
        // forgotten value would otherwise swallow the next flag).
        let mut value = |what: &str| match it.next() {
            None => Err(CliError::Usage(format!("{what} needs a value"))),
            Some(v) if v.is_empty() || v.starts_with("--") => Err(CliError::Usage(format!(
                "{what} needs a value, found {v:?}"
            ))),
            Some(v) => Ok(v.clone()),
        };
        match flag.as_str() {
            "--nasr" => set_once(&mut nasr, "--nasr", PathBuf::from(value("--nasr")?))?,
            "--cifp" => set_once(&mut cifp, "--cifp", PathBuf::from(value("--cifp")?))?,
            "--artcc" => set_once(&mut artcc, "--artcc", parse_artcc(&value("--artcc")?)?)?,
            "--buffer-nm" => set_once(
                &mut buffer_m,
                "--buffer-nm",
                parse_buffer_nm(&value("--buffer-nm")?)?,
            )?,
            "--expect-sha256" => {
                let v = value("--expect-sha256")?;
                let e = parse_expectation(&v).map_err(|e| CliError::Usage(e.to_string()))?;
                if expect.iter().any(|x: &Expectation| x.file == e.file) {
                    return Err(CliError::Usage(format!(
                        "--expect-sha256 given twice for {:?}",
                        e.file
                    )));
                }
                expect.push(e);
            }
            "--allow-cycle-mismatch" => allow_cycle_mismatch = true,
            "--out" => set_once(&mut out, "--out", PathBuf::from(value("--out")?))?,
            other => return Err(CliError::Usage(format!("unknown argument {other:?}"))),
        }
    }
    Ok(BuildArgs {
        nasr: need(nasr, "--nasr")?,
        cifp: need(cifp, "--cifp")?,
        artcc: need(artcc, "--artcc")?,
        buffer_m: buffer_m.unwrap_or((DEFAULT_BUFFER_NM * NM_M).round() as u32),
        expect,
        out: need(out, "--out")?,
        allow_cycle_mismatch,
    })
}

/// `<snapshot>.report.txt`.
pub fn report_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_owned();
    s.push(".report.txt");
    PathBuf::from(s)
}

fn read_snapshot(path: &Path) -> Result<Vec<u8>, CliError> {
    read_bounded_file(path, MAX_SNAPSHOT_BYTES, "snapshot")
}

fn read_bounded_file(path: &Path, limit: usize, kind: &str) -> Result<Vec<u8>, CliError> {
    let (file, declared_len) = oa_sealed::open_regular(path)
        .map_err(|e| failed(&format!("open {}", path.display()), e))?;
    let max =
        u64::try_from(limit).map_err(|_| CliError::Failed(format!("{kind} byte limit width")))?;
    if declared_len > max {
        return Err(CliError::Failed(format!(
            "{} is larger than the {limit}-byte {kind} limit",
            path.display()
        )));
    }
    let capacity = usize::try_from(declared_len)
        .map_err(|_| CliError::Failed("snapshot byte limit width".into()))?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(capacity)
        .map_err(|_| CliError::Failed("snapshot allocation could not be reserved".into()))?;
    // `open_regular` refuses FIFOs and other non-files before this bounded read.
    file.take(
        max.checked_add(1)
            .ok_or_else(|| CliError::Failed("snapshot byte limit overflow".into()))?,
    )
    .read_to_end(&mut buf)
    .map_err(|e| failed(&format!("read {}", path.display()), e))?;
    if buf.len() > limit {
        return Err(CliError::Failed(format!(
            "{} is larger than the {limit}-byte {kind} limit",
            path.display()
        )));
    }
    if u64::try_from(buf.len()).ok() != Some(declared_len) {
        return Err(CliError::Failed(format!(
            "{} changed length while being read",
            path.display()
        )));
    }
    Ok(buf)
}

/// `build`: load, build, write the snapshot and its report; the summary goes to `out`.
pub fn run_build(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    let a = parse_build_args(args)?;
    oa_sealed::refuse(&a.out).map_err(|error| failed("snapshot output", error))?;
    oa_sealed::refuse(&report_path(&a.out)).map_err(|error| failed("report output", error))?;
    let src = load_files(&a.nasr, &a.cifp, &a.expect).map_err(|e| failed("load", e))?;
    let params = BuildParams {
        artcc: a.artcc.clone(),
        buffer_m: a.buffer_m,
        allow_cycle_mismatch: a.allow_cycle_mismatch,
    };
    let built = build_model(&src, &params).map_err(|e| failed("build", e))?;
    let bytes = encode(&built.model).map_err(|e| failed("snapshot encode", e))?;
    let digest = sha256(&bytes);
    let report = render_report(&ReportInput {
        model: &built.model,
        info: &built.info,
        nasr: &src.nasr_archive,
        cifp: &src.cifp_archive,
        diag: &src.diag,
        snapshot_sha256: digest,
        snapshot_bytes: bytes.len() as u64,
    });
    std::fs::write(&a.out, &bytes).map_err(|e| failed(&format!("write {}", a.out.display()), e))?;
    let rp = report_path(&a.out);
    std::fs::write(&rp, report.as_bytes())
        .map_err(|e| failed(&format!("write {}", rp.display()), e))?;
    let _ = writeln!(out, "snapshot {} ({} bytes)", a.out.display(), bytes.len());
    let _ = writeln!(out, "sha256   {}", to_hex(&digest));
    let _ = writeln!(out, "report   {}", rp.display());
    Ok(())
}

/// `diff`: compare two snapshots and print the change list to `out`.
pub fn run_diff(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    if let Some(flag) = args.iter().find(|a| a.starts_with("--")) {
        return Err(CliError::Usage(format!("unknown argument {flag:?}")));
    }
    let [a, b] = args else {
        return Err(CliError::Usage("diff takes two snapshot paths".into()));
    };
    if a.is_empty() || b.is_empty() {
        return Err(CliError::Usage(
            "diff needs two snapshot paths, not empty ones".into(),
        ));
    }
    let load = |p: &String| -> Result<_, CliError> {
        let bytes = read_snapshot(Path::new(p))?;
        decode(&bytes).map_err(|e| failed(&format!("read {p}"), e))
    };
    let (ma, mb) = (load(a)?, load(b)?);
    let d = diff_models(&ma, &mb).map_err(|e| failed("diff", e))?;
    let text = render_diff(&d, a, b);
    out.write_all(text.as_bytes())
        .map_err(|e| failed("write output", e))?;
    Ok(())
}

/// `package`: wrap one validated canonical snapshot in a content-addressed adaptation package.
pub fn run_package(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    if args.len() != 10 {
        return Err(CliError::Usage("package needs exactly --snapshot FILE --out-root DIR --release ID --facility ARTCC --change-ref REF".into()));
    }
    if args
        .iter()
        .any(|argument| argument.len() > MAX_PACKAGE_ARGUMENT_BYTES)
    {
        return Err(CliError::Usage(
            "package argument exceeds byte limit".into(),
        ));
    }
    let mut snapshot = None;
    let mut root = None;
    let mut release = None;
    let mut facility = None;
    let mut change_ref = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| CliError::Usage(format!("missing value for {flag}")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::Usage(format!("{flag} needs a value")));
        }
        let slot = match flag.as_str() {
            "--snapshot" => &mut snapshot,
            "--out-root" => &mut root,
            "--release" => &mut release,
            "--facility" => &mut facility,
            "--change-ref" => &mut change_ref,
            _ => {
                return Err(CliError::Usage(format!(
                    "unknown package argument {flag:?}"
                )))
            }
        };
        if slot.replace(value.clone()).is_some() {
            return Err(CliError::Usage(format!("{flag} given more than once")));
        }
    }
    let snapshot =
        PathBuf::from(snapshot.ok_or_else(|| CliError::Usage("package needs --snapshot".into()))?);
    let root =
        PathBuf::from(root.ok_or_else(|| CliError::Usage("package needs --out-root".into()))?);
    let release = release.ok_or_else(|| CliError::Usage("package needs --release".into()))?;
    let facility = facility.ok_or_else(|| CliError::Usage("package needs --facility".into()))?;
    let change_ref =
        change_ref.ok_or_else(|| CliError::Usage("package needs --change-ref".into()))?;
    let bytes = read_snapshot(&snapshot)?;
    let digest =
        crate::manifest::write_new_package(&root, &bytes, &release, &facility, &change_ref)
            .map_err(|e| failed("adaptation package", e))?;
    writeln!(
        out,
        "adaptation_package_status\tcreated_unverified_for_activation"
    )
    .map_err(|e| failed("write output", e))?;
    writeln!(out, "manifest_sha256\t{}", to_hex(&digest)).map_err(|e| failed("write output", e))?;
    writeln!(
        out,
        "integrity_scope\tsha256 consistency only; issuer is not authenticated"
    )
    .map_err(|e| failed("write output", e))?;
    Ok(())
}

/// `issue`: create a bounded v2 or v3 manifest at the next version after verifying the prior pin.
pub fn run_issue(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    const FIELDS: [&str; 11] = [
        "--snapshot",
        "--out-root",
        "--previous-package-root",
        "--previous-manifest-sha256",
        "--release",
        "--facility",
        "--change-ref",
        "--change-class",
        "--issuer",
        "--approver",
        "--reason",
    ];
    if args.len() < FIELDS.len() * 2 || args.len() > (FIELDS.len() + 1) * 2 || args.len() % 2 != 0 {
        return Err(CliError::Usage(
            "issue requires exactly twelve flag/value pairs".into(),
        ));
    }
    if args
        .iter()
        .any(|arg| arg.len() > MAX_PACKAGE_ARGUMENT_BYTES)
    {
        return Err(CliError::Usage("issue argument exceeds byte limit".into()));
    }
    let mut values = std::collections::BTreeMap::<&str, &str>::new();
    for pair in args.chunks_exact(2) {
        let flag = pair
            .first()
            .ok_or_else(|| CliError::Usage("missing issue flag".into()))?;
        let value = pair
            .get(1)
            .ok_or_else(|| CliError::Usage("missing issue value".into()))?;
        if !FIELDS.contains(&flag.as_str()) && flag != "--ca-preset" {
            return Err(CliError::Usage(format!("unknown issue argument {flag:?}")));
        }
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::Usage(format!("{flag} needs a value")));
        }
        if values.insert(flag, value).is_some() {
            return Err(CliError::Usage(format!("{flag} given more than once")));
        }
    }
    let get = |name: &'static str| {
        values
            .get(name)
            .copied()
            .ok_or_else(|| CliError::Usage(format!("issue needs {name}")))
    };
    let change_class = match get("--change-class")? {
        "initial" => crate::manifest::ChangeClass::Initial,
        "routine" => crate::manifest::ChangeClass::Routine,
        "field-change" => crate::manifest::ChangeClass::FieldChange,
        other => {
            return Err(CliError::Usage(format!(
                "unsupported --change-class {other:?}"
            )))
        }
    };
    let snapshot_path = PathBuf::from(get("--snapshot")?);
    let output_root = PathBuf::from(get("--out-root")?);
    let previous_root = PathBuf::from(get("--previous-package-root")?);
    let previous_pin = crate::manifest::parse_pin(get("--previous-manifest-sha256")?)
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let snapshot_bytes = read_snapshot(&snapshot_path)?;
    let issued = if let Some(path) = values.get("--ca-preset") {
        let preset = read_bounded_file(Path::new(path), oa_ca::CA_PRESET_MAX_BYTES, "CA preset")?;
        crate::manifest::issue_new_package_with_ca_preset(
            &output_root,
            &snapshot_bytes,
            &preset,
            get("--release")?,
            get("--facility")?,
            get("--change-ref")?,
            &previous_root,
            previous_pin,
            change_class,
            get("--issuer")?,
            get("--approver")?,
            get("--reason")?,
        )
    } else {
        crate::manifest::issue_new_package(
            &output_root,
            &snapshot_bytes,
            get("--release")?,
            get("--facility")?,
            get("--change-ref")?,
            &previous_root,
            previous_pin,
            change_class,
            get("--issuer")?,
            get("--approver")?,
            get("--reason")?,
        )
    };
    let (version, digest) = issued.map_err(|e| failed("adaptation issue", e))?;
    writeln!(
        out,
        "adaptation_issue_status\tissued_unverified_for_activation"
    )
    .map_err(|e| failed("write output", e))?;
    writeln!(out, "adaptation_version\t{version}").map_err(|e| failed("write output", e))?;
    writeln!(out, "manifest_sha256\t{}", to_hex(&digest)).map_err(|e| failed("write output", e))?;
    writeln!(
        out,
        "integrity_scope\tsha256 consistency only; principal IDs are not authenticated"
    )
    .map_err(|e| failed("write output", e))?;
    Ok(())
}

/// Dispatches on the first argument; `args` excludes the program name.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "build" => run_build(rest, out),
        Some((cmd, rest)) if cmd == "diff" => run_diff(rest, out),
        Some((cmd, rest)) if cmd == "package" => run_package(rest, out),
        Some((cmd, rest)) if cmd == "issue" => run_issue(rest, out),
        Some((cmd, rest)) if cmd == "route" => crate::route_cli::run_route(rest, out),
        Some((cmd, _)) if cmd == "--help" || cmd == "-h" => {
            let _ = writeln!(out, "{USAGE}");
            Ok(())
        }
        Some((cmd, rest)) if cmd == "fleet-check" => {
            crate::fleet_cli::run(rest, out).map_err(|error| CliError::Failed(error.to_string()))
        }
        _ => Err(CliError::Usage(
            "expected a command: build, diff, package, issue, fleet-check, or route".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::manifest::{
        issue_new_package, parse_pin, verify_current, write_new_package, ChangeClass,
    };
    use oa_airspace::model::{empty_model, Meta};
    use oa_airspace::snapshot::encode;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn issue_command_computes_next_version_and_records_field_change() {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "oa-adapt-cli-issue-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create synthetic test directory");
        let snapshot = encode(&empty_model(Meta {
            nasr_effective: 20_261_029,
            cifp_cycle: 2611,
            artcc: "ZAU".to_owned(),
            buffer_m: 92_600,
            nasr_sha256: [0x11; 32],
            cifp_sha256: [0x22; 32],
        }))
        .expect("synthetic snapshot encodes");
        let snapshot_path = root.join("candidate.snapshot");
        std::fs::write(&snapshot_path, &snapshot).expect("write synthetic snapshot");

        let prior_root = root.join("prior");
        let prior_pin = write_new_package(&prior_root, &snapshot, "legacy", "ZAU", "BASE-1")
            .expect("legacy package writes");
        let first_root = root.join("first");
        let (_, first_pin) = issue_new_package(
            &first_root,
            &snapshot,
            "adapt-1",
            "ZAU",
            "CHG-1",
            &prior_root,
            prior_pin,
            ChangeClass::Initial,
            "issuer-1",
            "approver-2",
            "initial issue",
        )
        .expect("first issue establishes version one");

        let candidate_root = root.join("candidate");
        let first_pin_hex = oa_sha256::to_hex(&first_pin);
        let args = [
            "issue",
            "--snapshot",
            snapshot_path.to_str().expect("snapshot path is utf8"),
            "--out-root",
            candidate_root.to_str().expect("output path is utf8"),
            "--previous-package-root",
            first_root.to_str().expect("prior path is utf8"),
            "--previous-manifest-sha256",
            first_pin_hex.as_str(),
            "--release",
            "adapt-2",
            "--facility",
            "ZAU",
            "--change-ref",
            "FIELD-1",
            "--change-class",
            "field-change",
            "--issuer",
            "issuer-3",
            "--approver",
            "approver-4",
            "--reason",
            "sector field correction",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let mut output = Vec::new();
        run(&args, &mut output).expect("issue CLI command succeeds without a version argument");
        let text = String::from_utf8(output).expect("CLI output is UTF-8");
        assert!(text.lines().any(|line| line == "adaptation_version\t2"));
        let manifest_hash = text
            .lines()
            .find_map(|line| line.strip_prefix("manifest_sha256\t"))
            .expect("command prints issued manifest digest");
        let pin = parse_pin(manifest_hash).expect("output pin is canonical");
        let verified = verify_current(&candidate_root, pin).expect("issued output verifies");
        let issue = verified
            .identity()
            .issue
            .as_ref()
            .expect("issued metadata exists");
        assert_eq!(issue.adaptation_version, 2);
        assert_eq!(issue.change_class, ChangeClass::FieldChange);
        assert_eq!(issue.previous_manifest_sha256, first_pin);
        assert_eq!(issue.issuer_principal_id, "issuer-3");
        assert_eq!(issue.approver_principal_id, "approver-4");
        assert_eq!(issue.reason, "sector field correction");
        let _ = std::fs::remove_dir_all(root);
    }
}
