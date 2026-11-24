//! `oa-stdds-ref --adsb <clip.oaf> --out <dir> <stdds.oaf[.gz]>...`
//!
//! Creates `stdds_ref.txt` (printed too), `stdds_ref.csv`, `stdds_ref.json`, and a final
//! `stdds_ref.complete` marker in `--out`; existing output names are refused, never overwritten.
//! Paths that name a sealed recording folder or ADS-B day, as given or through links, exit 2 before anything is opened; the
//! refusal is made by the shared guard `oa-sealed` and names the path. The text printed holds counts
//! and aggregate statistics only, never a track number, callsign, address or position.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oa_sealed::Sealed;
use oa_stdds_ref::engine::{analyse, estimate_analysis_work};
use oa_stdds_ref::guard::{is_refusal, open_guarded, refuse_sealed};
use oa_stdds_ref::load::{load_file_counted_with_budgets, MAX_STDDS_FILES};
use oa_stdds_ref::publish::publish_report;
use oa_stdds_ref::report::{build_report, check_report_bound, MAX_REPORT_BYTES};
use oa_stdds_ref::run::build_refs;
use oa_stdds_ref::sources::TruthSource;

const TXT: &str = "stdds_ref.txt";
const CSV: &str = "stdds_ref.csv";
const JSON: &str = "stdds_ref.json";
const COMPLETE: &str = "stdds_ref.complete";
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SOURCE_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SOURCE_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;
// Each segment contributes min(segment rows, 128) samples, each checked at all 301 offsets.
const MAX_ANALYSIS_WORK: usize = 80_000_000;
const SOURCE_FILES: [&str; 14] = [
    "src/lib.rs",
    "src/main.rs",
    "src/engine.rs",
    "src/guard.rs",
    "src/load.rs",
    "src/model.rs",
    "src/publish.rs",
    "src/report.rs",
    "src/run.rs",
    "src/sources.rs",
    "Cargo.toml",
    "../../Cargo.toml",
    "../../Cargo.lock",
    "../../rust-toolchain.toml",
];
const USAGE: &str = "usage: oa-stdds-ref --adsb <clip.oaf> --out <dir> <stdds.oaf[.gz]>...";

fn hash_guarded(g: &Sealed, path: &Path, byte_limit: u64) -> Result<(String, u64), String> {
    let mut file = open_guarded(g, path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size > byte_limit {
        return Err(format!(
            "{} exceeds the {byte_limit}-byte provenance read limit",
            path.display()
        ));
    }
    let mut hasher = oa_sha256::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut read = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        read = read
            .checked_add(n as u64)
            .ok_or_else(|| "provenance byte count overflow".to_string())?;
        if read > byte_limit {
            return Err(format!(
                "{} grew beyond the {byte_limit}-byte provenance read limit",
                path.display()
            ));
        }
        hasher
            .update(buf.get(..n).ok_or("source read length exceeds buffer")?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if read != size {
        return Err(format!(
            "{} changed length while its provenance was hashed",
            path.display()
        ));
    }
    Ok((oa_sha256::to_hex(&hasher.finalize()), read))
}

fn selected_source_fingerprint(g: &Sealed) -> Result<(String, String), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hasher = oa_sha256::Sha256::new();
    let mut total = 0u64;
    for relative in SOURCE_FILES {
        let path = root.join(relative);
        let (digest, size) = hash_guarded(g, &path, MAX_SOURCE_FILE_BYTES)?;
        total = total
            .checked_add(size)
            .ok_or_else(|| "source fingerprint size overflow".to_string())?;
        if total > MAX_SOURCE_BUNDLE_BYTES {
            return Err(format!("selected source/config files exceed the {MAX_SOURCE_BUNDLE_BYTES}-byte fingerprint limit"));
        }
        hasher
            .update(relative.as_bytes())
            .map_err(|e| e.to_string())?;
        hasher.update(&[0]).map_err(|e| e.to_string())?;
        hasher
            .update(digest.as_bytes())
            .map_err(|e| e.to_string())?;
        hasher.update(&[0]).map_err(|e| e.to_string())?;
    }
    let selected = format!("{} files: {}", SOURCE_FILES.len(), SOURCE_FILES.join(","));
    Ok((oa_sha256::to_hex(&hasher.finalize()), selected))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut out) = (None, None);
    let mut files: Vec<PathBuf> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--adsb" => adsb = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            s if s.starts_with("--") => {
                eprintln!("oa-stdds-ref: unknown option {s}");
                return ExitCode::from(2);
            }
            _ => files.push(PathBuf::from(a)),
        }
    }
    let (Some(adsb), Some(out)) = (adsb, out) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if files.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    if files.len() > MAX_STDDS_FILES {
        eprintln!("oa-stdds-ref: {USAGE}; at most {MAX_STDDS_FILES} STDDS files are admitted");
        return ExitCode::from(2);
    }
    let g = Sealed::production();
    let outputs = [
        out.join(TXT),
        out.join(CSV),
        out.join(JSON),
        out.join(COMPLETE),
    ];
    for p in [&adsb, &out].into_iter().chain(&outputs).chain(&files) {
        if let Err(e) = refuse_sealed(g, p) {
            eprintln!("oa-stdds-ref: refused: {e}");
            return ExitCode::from(2);
        }
    }
    // Printed when `main` returns: on every exit after this point, an error included.
    let mut drops = oa_sealed::DropReport::new();
    match run(g, &adsb, &out, &files, &mut drops) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-stdds-ref: {e}");
            // A refusal that came at the open (a link swapped after the check) is the same exit.
            ExitCode::from(if is_refusal(&e) { 2 } else { 1 })
        }
    }
}

fn run(
    g: &Sealed,
    adsb: &Path,
    out: &Path,
    files: &[PathBuf],
    drops: &mut oa_sealed::DropReport,
) -> Result<String, String> {
    // An existing output is refused before any work is done (and again, race-free, at the create).
    for name in [TXT, CSV, JSON, COMPLETE] {
        let p = out.join(name);
        // The guard first: an output name inside sealed data is refused before it is probed.
        g.check(&p).map_err(|e| e.to_string())?;
        // seal-allow: the guard checked this output name on the line above; the probe reads no file
        if std::fs::symlink_metadata(&p).is_ok() {
            return Err(format!(
                "{}: output file exists (never overwritten)",
                p.display()
            ));
        }
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut recs = Vec::new();
    let mut header = Vec::new();
    let (mut stdds_source_bytes, mut stdds_decoded_bytes) = (0u64, 0u64);
    for (i, f) in files.iter().enumerate() {
        let ix = u32::try_from(i).map_err(|_| "too many input files".to_string())?;
        let mut file_sealed = 0u64;
        let loaded = load_file_counted_with_budgets(
            g,
            f,
            ix,
            &mut recs,
            &mut file_sealed,
            stdds_source_bytes,
            stdds_decoded_bytes,
        );
        drops.add(file_sealed);
        let info = loaded.map_err(|e| e.to_string())?;
        stdds_source_bytes = stdds_source_bytes
            .checked_add(info.source_bytes)
            .ok_or_else(|| "STDDS total source byte count overflow".to_string())?;
        stdds_decoded_bytes = stdds_decoded_bytes
            .checked_add(info.decoded_bytes)
            .ok_or_else(|| "STDDS total decoded byte count overflow".to_string())?;
        header.push(format!(
            "STDDS file {ix}: {} (digest {}); source bytes {}, decoded bytes {}, message frames {}, other frames {}, damaged frames {}, messages not TAIS or refused {}, TAIS records {}, gzip damaged {}, gzip truncated {}",
            info.path,
            info.sha256,
            info.source_bytes,
            info.decoded_bytes,
            info.frames_message,
            info.frames_other,
            info.frames_damaged,
            info.messages_not_tais,
            info.records,
            info.gzip_damaged,
            info.gzip_truncated
        ));
    }
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot identify running binary: {e}"))?;
    let (exe_hash, _) = hash_guarded(g, &exe, MAX_BINARY_BYTES).map_err(|e| {
        format!("cannot hash running binary; install or copy it as a single-link file: {e}")
    })?;
    header.push(format!("binary SHA-256: {exe_hash}"));
    let (source_hash, source_selection) = selected_source_fingerprint(g)?;
    header.push(format!(
        "oa-stdds-ref selected source/config fingerprint SHA-256: {source_hash}"
    ));
    header.push(format!("source fingerprint scope (partial, not dependency closure): {source_selection}, rust-toolchain.toml, source files and workspace manifests listed; toolchain version is separately pinned by rust-toolchain.toml"));
    header.push(format!(
        "research envelope: at most {MAX_STDDS_FILES} STDDS files; 128 MiB source and decoded per file; 384 MiB total STDDS source; 512 MiB total STDDS decoded; 128 MiB ADS-B clip; 100000 STDDS and ADS-B rows; 256 MiB report; {MAX_ANALYSIS_WORK} analysis work units"
    ));
    header.push(format!(
        "settings: tracker=oa-track::DEFAULT_CONFIG, truth=oa-truth::DEFAULT_CONFIG, reuse-gap-ms={}, fit-window-ms=+/-15000, fit-step-ms=100, fit-min-samples=8",
        oa_stdds_ref::engine::TRACK_REUSE_GAP_MS
    ));
    let analysis_work = estimate_analysis_work(&recs)?;
    if analysis_work > MAX_ANALYSIS_WORK {
        return Err(format!("analysis work estimate {analysis_work} exceeds the {MAX_ANALYSIS_WORK}-unit research limit"));
    }
    let provenance_bytes = header
        .iter()
        .try_fold(0usize, |sum, line| {
            sum.checked_add(line.len()).and_then(|n| n.checked_add(1))
        })
        .ok_or_else(|| "provenance report bound overflow".to_string())?;
    check_report_bound(recs.len(), provenance_bytes)?;
    let addrs: BTreeSet<u32> = recs
        .iter()
        .filter_map(|r| r.addr.filter(|a| *a != 0))
        .collect();
    // The clip's rows are added as soon as the clip is read, before anything that can fail.
    let mut clip_sealed = 0u64;
    let refs = build_refs(g, adsb, &addrs, out, &mut clip_sealed);
    drops.add(clip_sealed);
    let refs = refs?;
    header.extend(refs.header.iter().cloned());
    let truth = TruthSource {
        truth: &refs.truth.truth,
    };
    let analysis = analyse(&recs, &refs.tracker, &truth);
    let report = build_report(&header, &analysis);
    let report_bytes = report
        .summary
        .len()
        .checked_add(report.csv.len())
        .and_then(|n| n.checked_add(report.json.len()))
        .and_then(|n| n.checked_add(report.completion_marker.len()))
        .ok_or_else(|| "report size overflow".to_string())?;
    if report_bytes > MAX_REPORT_BYTES {
        return Err(format!(
            "report output {report_bytes} bytes exceeds {MAX_REPORT_BYTES}"
        ));
    }
    publish_report(g, out, &report)?;
    Ok(report.summary)
}
