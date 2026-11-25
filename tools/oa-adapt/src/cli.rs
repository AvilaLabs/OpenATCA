//! The `oa-adapt` command line. **Shell** (B1, B7): argument parsing, file reads and writes, output.
//! All decisions are made by the core modules it calls.
//!
//! ```text
//! oa-adapt build --nasr <zip> --cifp <zip> --artcc ZAU [--buffer-nm 50]
//!                [--expect-sha256 <file>=<hex>]... --out <snapshot>
//! oa-adapt diff <a> <b>
//! oa-adapt package --snapshot <snapshot> --out-root <new-directory> --release <id>
//!                   --facility <ARTCC> --change-ref <approved-record>
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
    let (file, declared_len) = oa_sealed::open_regular(path)
        .map_err(|e| failed(&format!("open {}", path.display()), e))?;
    let max = u64::try_from(MAX_SNAPSHOT_BYTES)
        .map_err(|_| CliError::Failed("snapshot byte limit width".into()))?;
    if declared_len > max {
        return Err(CliError::Failed(format!(
            "{} is larger than the {MAX_SNAPSHOT_BYTES}-byte snapshot limit",
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
    if buf.len() > MAX_SNAPSHOT_BYTES {
        return Err(CliError::Failed(format!(
            "{} is larger than the {MAX_SNAPSHOT_BYTES}-byte snapshot limit",
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

/// Dispatches on the first argument; `args` excludes the program name.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "build" => run_build(rest, out),
        Some((cmd, rest)) if cmd == "diff" => run_diff(rest, out),
        Some((cmd, rest)) if cmd == "package" => run_package(rest, out),
        Some((cmd, rest)) if cmd == "route" => crate::route_cli::run_route(rest, out),
        Some((cmd, _)) if cmd == "--help" || cmd == "-h" => {
            let _ = writeln!(out, "{USAGE}");
            Ok(())
        }
        _ => Err(CliError::Usage(
            "expected a command: build, diff, package, or route".into(),
        )),
    }
}
