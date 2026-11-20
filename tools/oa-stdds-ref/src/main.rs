//! `oa-stdds-ref --adsb <clip.oaf> --out <dir> <stdds.oaf[.gz]>...`
//!
//! Creates `stdds_ref.txt` (printed too) and `stdds_ref.csv` in `--out`; an existing output is refused (exit 1), never overwritten. Paths that name a sealed
//! recording folder or ADS-B day, as given or through links, exit 2 before anything is opened; the
//! refusal is made by the shared guard `oa-sealed` and names the path. The text printed holds counts
//! and aggregate statistics only, never a track number, callsign, address or position.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oa_sealed::Sealed;
use oa_stdds_ref::engine::analyse;
use oa_stdds_ref::guard::{create_output, is_refusal, refuse_sealed};
use oa_stdds_ref::load::load_file;
use oa_stdds_ref::report::build_report;
use oa_stdds_ref::run::build_refs;
use oa_stdds_ref::sources::TruthSource;

const TXT: &str = "stdds_ref.txt";
const CSV: &str = "stdds_ref.csv";
const USAGE: &str = "usage: oa-stdds-ref --adsb <clip.oaf> --out <dir> <stdds.oaf[.gz]>...";

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
    let g = Sealed::production();
    let outputs = [out.join(TXT), out.join(CSV)];
    for p in [&adsb, &out].into_iter().chain(&outputs).chain(&files) {
        if let Err(e) = refuse_sealed(g, p) {
            eprintln!("oa-stdds-ref: refused: {e}");
            return ExitCode::from(2);
        }
    }
    match run(g, &adsb, &out, &files) {
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

fn run(g: &Sealed, adsb: &Path, out: &Path, files: &[PathBuf]) -> Result<String, String> {
    // An existing output is refused before any work is done (and again, race-free, at the create).
    for name in [TXT, CSV] {
        let p = out.join(name);
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
    let mut sealed = 0u64;
    for (i, f) in files.iter().enumerate() {
        let ix = u32::try_from(i).map_err(|_| "too many input files".to_string())?;
        let info = load_file(g, f, ix, &mut recs).map_err(|e| e.to_string())?;
        sealed += info.sealed_window_dropped;
        header.push(format!(
            "STDDS file {ix}: {} (digest {}); message frames {}, other frames {}, damaged frames {}, messages not TAIS or refused {}, TAIS records {}, gzip damaged {}, gzip truncated {}",
            info.path,
            info.sha256,
            info.frames_message,
            info.frames_other,
            info.frames_damaged,
            info.messages_not_tais,
            info.records,
            info.gzip_damaged,
            info.gzip_truncated
        ));
    }
    let addrs: BTreeSet<u32> = recs.iter().filter_map(|r| r.addr).collect();
    let refs = build_refs(g, adsb, &addrs, out)?;
    // After the last read: the recordings' frames and records and the clip's rows, each once.
    eprintln!(
        "{}",
        oa_sealed::drop_line(sealed + refs.sealed_window_dropped)
    );
    header.extend(refs.header.iter().cloned());
    let truth = TruthSource {
        truth: &refs.truth.truth,
    };
    let analysis = analyse(&recs, &refs.tracker, &truth);
    let report = build_report(&header, &analysis);
    for (name, text) in [(TXT, &report.summary), (CSV, &report.csv)] {
        let p = out.join(name);
        let mut f = create_output(g, &p).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(report.summary)
}
