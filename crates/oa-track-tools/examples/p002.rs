//! Scoring protocol P-002: OpenATCA, ERAM and a passthrough baseline against one reference.
//!
//! ```text
//! cargo run --release -p oa-track-tools --example p002 -- \
//!     --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> [--label <text>] [--allow-dirty] [--development] \
//!     [--frozen-commit <sha> --candidate-commit <sha>]
//! cargo run --release -p oa-track-tools --example p002 -- --combine <dirA> <dirB> <dirC>
//! cargo run --release -p oa-track-tools --example p002 -- \
//!     --export-eram <file> --sfdps-dir <dir> [--drop-addresses <file>]
//! ```
//!
//! `--export-eram` reads the SFDPS recording of a day and writes the part of it this scorer reads
//! (`oa_bench::eram_export`, OA-REP-001 and following) with a `.sha256` manifest beside it; a
//! development run can then score from that file with `--eram-export <file>` in place of
//! `--sfdps-dir <dir>`, and its two output files are byte for byte those of the run on the
//! recording. `--drop-addresses <file>` (one 24-bit address in hex per line) removes every flight
//! with a listed address or address hint, at export or at scoring; a run that drops any says so in
//! its loader notes.
//!
//! Written from the comparison's scoring protocol P-002 and its amendments P-003 to P-006, fixed by
//! their hashes before any hold-out data was read; the protocol governs where this code and it
//! differ. The split, the
//! tracker run, the reference, the ERAM load, the association and the pairing are those of
//! `vs_eram`, and the error definitions are `oa-bench`'s (cross-track and along-track in the
//! truth frame, the time offset as `metrics_th::fit_offset` defines it, here per aircraft).
//!
//! P-003 adds, for every system alike, the rounding-tolerant along-track error
//! `a_t = sign(a) * max(0, |a| - 0.5 s * v)` of the time-fitted along-track error `a` and the
//! reference ground speed `v` (M2t), and the horizontal error `hypot(cross, a_t)` (M3t); M4 and
//! M7 are also computed on M3t. The headline position metrics are M1 and M3t; M2 and M3 are
//! reported, but no claim of "better than ERAM" may rest on them. The breakdowns by the tracker's
//! coasting, uncertain (`O_UNCERTAIN`) and reacquired (`O_REACQUIRED`) flags report M1, M3t and M5
//! with sample counts, on the same resamples. The scorer records `git rev-parse HEAD` and
//! refuses a tree that `git status --porcelain` lists as modified unless `--allow-dirty` is
//! given, and then stamps every output file as not valid for hold-out scoring. `--development`
//! marks a run on development data the same way (stamped, not a hold-out score); a run with
//! neither flag is a hold-out run and fails if any OpenATCA sample has no tracker row for its
//! flags, or only an ambiguous one (see "The flag join"). A hold-out run also needs
//! `--frozen-commit <40 hex>` and is refused unless the commit the binary was built from (the
//! build-time variables `OA_BUILD_COMMIT` and `OA_BUILD_DIRTY`, see `tools/p002_build.sh`) is that
//! commit, that build's tree was clean, and the working tree at run time is clean and at it.
//!
//! P-004 replaces each system's own flight choice by one rule for all three: an aircraft's scored
//! legs are the truth legs that contain its grid samples (the grid is ERAM's paired samples), and
//! every system is scored on those legs only. The output prints the coverage under both rules. The
//! reacquired group is OpenATCA's rows within 10 s after an update row flagged `O_REACQUIRED`, and
//! every grid sample without a value is counted in one miss class per system. P-006 fixes three
//! primary metrics (M1 median, M5 ground-speed median, M4t at 300 m), the two-way common set
//! (OpenATCA and ERAM both have a value) for M1, M3t and M5 with the three-way set kept for the
//! reference-floor test, the `--candidate-commit` check (`git diff --name-only <candidate>
//! <frozen>` is exactly the sealed list), create-new outputs, and the `--combine` mode that applies
//! the cross-day claim rule to the `CLAIM` lines of three days' `p002.txt`.
//!
//! Writes `p002.txt` (tables M1 to M7 and M2t, M3t, M4t, M7t with bootstrap intervals,
//! breakdowns by phase, altitude and the three flags, exclusions, offsets, digests, commits, the
//! loader's damage counts) and `samples.csv` (one row per grid sample), each only if it does not
//! exist.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{Read as IoRead, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use oa_adsb::{reports_len, NONE_I32, NONE_U16, ZAU_BOX};
use oa_bench::adsb_truth;
use oa_bench::assoc::{associate, astatus, TruthIndex};
use oa_bench::chunked::truth_index;
use oa_bench::eram_export;
use oa_bench::geo::KT_MPS;
use oa_bench::input::{
    file_opener, list_inputs, load_adsb_counted, load_counted, msg_limit, FeedInput,
};
use oa_bench::legacy::{Legacy, NO_ALT};
use oa_bench::pair::{build_pairs_with, PairTable};
use oa_bench::run::RunConfig;
use oa_bench::stats::{band, phase, Exclusions};
use oa_bench::thindex::build_th_index;
use oa_bench::tracker_input::{legacy_from_track_samples_by_leg, TrackSamples};
use oa_bench::truth::{Truth, TruthLeg, TruthSample};
use oa_replay::FeedKind;
use oa_sha256::{sha256, to_hex, Sha256};
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::DEFAULT_CONFIG;
use oa_track_tools::score::{
    run_tracker_spilled, score_ranges, scoring_timing_config, split_reports, RowSet, RowSource,
    ScoringTimingProfile, ScoringTimingReport, ScoringTimingSource, SCORE_CHUNK_ROWS, SPLIT_TEXT,
};
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

/// A sample is matched to a system's row within this, ms (P-002 "Samples").
const TOL_MS: i64 = 500;
/// Samples below this truth ground speed do not enter a time fit, m/s (`OffsetConfig` default).
const FIT_MIN_SPEED_MPS: f64 = 50.0;
// P010 uses the existing P006 pairing direction threshold, not the fit-only cutoff.
const P010_MIN_DIRECTION_SPEED_MPS: f64 = 10.0;
// C7: up to eight legs for each of 100,000 addresses over 1,080 score minutes.
const P010_MAX_ELIGIBILITY_LEG_VISITS: u64 = 864_000_000;
const P010_MAX_TRUTH_LEGS: usize = 800_000;
/// Bootstrap resamples and seed (P-002 "Uncertainty").
const BOOT_N: usize = 1000;
const BOOT_SEED: u64 = 20_261_119;
/// M7 threshold, metres (0.5 NM).
const LARGE_M: f64 = 926.0;
/// The passthrough rule: within this share of OpenATCA's value.
const FLOOR_SHARE: f64 = 0.10;
/// Development-only SFDPS-style quantization applied to OpenATCA tracker rows before pairing.
const OA_PUBLICATION_GRID_PROFILE: &str = "sfdps-v1";
const E7_PER_DEGREE: i128 = 10_000_000;
const ARCSECONDS_PER_DEGREE: i128 = 3600;
const MICRODEGREES_PER_DEGREE: i128 = 1_000_000;
/// The legacy adapter clamps each velocity component to +/-32,000 deciknots in an i16 field.
const MAX_PUBLICATION_COMPONENT_KT: f64 = 3200.0;

/// P-003 rounding tolerance: reference times are whole seconds, so along-track error within this
/// many seconds of travel at the reference ground speed is not charged.
const ROUND_TOL_S: f64 = 0.5;
/// The stamp on every output file of a run on a modified tree (P-003 section 2, item 13).
const DIRTY_STAMP: &str = "DIRTY TREE \u{2014} not valid for hold-out scoring";
/// The time offset of an aircraft with no usable fit: none (scored unfitted, counted).
const UNFITTED_TAU_S: f64 = 0.0;
/// The stamp on every output file of a run marked `--development`.
const DEV_STAMP: &str = "DEVELOPMENT RUN \u{2014} not a hold-out score";
const OA_PUBLICATION_GRID_STAMP: &str = "OA_PUBLICATION_GRID=sfdps-v1; OA tick positions rounded to 1 arcsecond then six decimal degrees and velocity components to 1 kt before pairing";
const P010_WINDOW_PROFILE: &str = "p010-zau-window-v1";
const P010_BOX_PROFILE: &str = "zau-v1";
const P010_MAX_ELIGIBLE_ADDRESS_MINUTES: usize = 10_000_000;
const P010_MAX_ADDRESSES: usize = 100_000;
const P010_MAX_SFDPS_BYTES: u64 = 64_000_000_000;

const SYS: [&str; 3] = ["OpenATCA", "ERAM", "Passthrough"];
const OA: usize = 0;
const ER: usize = 1;
const PT: usize = 2;
const STATS: [&str; 3] = ["median", "p95", "rms"];

// A7.2: cold struct, size guard waived; shell adapter copies one bounded tracker-row range at a time.
#[derive(Default)]
struct PublicationGridCounts {
    invalid_position_rows: u64,
    unavailable_velocity_rows: u64,
    zero_velocity_rows: u64,
    rounded_to_zero_velocity_rows: u64,
}

// A7.2: cold struct, size guard waived; bounded to one SCORE_CHUNK_ROWS tracker slice.
struct PublicationGridColumns {
    lat_e7: Vec<i32>,
    lon_e7: Vec<i32>,
    vel_east_mps: Vec<f32>,
    vel_north_mps: Vec<f32>,
}

/// Round a signed rational to an integer, with exact half ties away from zero.
/// Callers pass only bounded products of validated i32 coordinates/arcseconds and positive fixed denominators.
fn round_ratio_half_away(numerator: i128, denominator: i128) -> i128 {
    let magnitude = if numerator < 0 { -numerator } else { numerator };
    let rounded = (magnitude + denominator / 2) / denominator;
    if numerator < 0 {
        -rounded
    } else {
        rounded
    }
}

/// Convert E7 directly to the nearest whole arcsecond and back to canonical E7, avoiding a
/// floating-point round trip and the incorrect assumption that an arcsecond is 10,000 E7 units.
fn quantize_publication_coordinate(e7: i32, max_abs_e7: i32) -> Option<i32> {
    if e7 == NONE_I32 || i64::from(e7).abs() > i64::from(max_abs_e7) {
        return None;
    }
    let arcseconds = round_ratio_half_away(i128::from(e7) * ARCSECONDS_PER_DEGREE, E7_PER_DEGREE);
    // Development wire census: whole arcseconds are rendered as six decimal degrees before
    // the FIXM E7 parser sees them. Preserve that additional publication rounding too.
    let published_e6 =
        round_ratio_half_away(arcseconds * MICRODEGREES_PER_DEGREE, ARCSECONDS_PER_DEGREE);
    let canonical_e7 = published_e6 * 10;
    i32::try_from(canonical_e7).ok()
}

/// Quantize one east/north component before the legacy adapter converts m/s to deciknots.
fn quantize_publication_velocity_component(mps: f32) -> Option<f32> {
    if !mps.is_finite() {
        return None;
    }
    let knots = (f64::from(mps) / KT_MPS).round();
    if !knots.is_finite() || knots.abs() > MAX_PUBLICATION_COMPONENT_KT {
        return None;
    }
    Some((knots * KT_MPS) as f32)
}

fn publication_grid_columns(
    lat_e7: &[i32],
    lon_e7: &[i32],
    vel_east_mps: &[f32],
    vel_north_mps: &[f32],
    counts: &mut PublicationGridCounts,
) -> Result<PublicationGridColumns, String> {
    let n = lat_e7.len();
    if [lon_e7.len(), vel_east_mps.len(), vel_north_mps.len()]
        .iter()
        .any(|&len| len != n)
    {
        return Err("publication-grid tracker columns have different lengths".into());
    }
    let mut out = PublicationGridColumns {
        lat_e7: Vec::with_capacity(n),
        lon_e7: Vec::with_capacity(n),
        vel_east_mps: Vec::with_capacity(n),
        vel_north_mps: Vec::with_capacity(n),
    };
    for (((&lat, &lon), &east), &north) in lat_e7
        .iter()
        .zip(lon_e7)
        .zip(vel_east_mps)
        .zip(vel_north_mps)
    {
        match (
            quantize_publication_coordinate(lat, 900_000_000),
            quantize_publication_coordinate(lon, 1_800_000_000),
        ) {
            (Some(quantized_lat), Some(quantized_lon)) => {
                out.lat_e7.push(quantized_lat);
                out.lon_e7.push(quantized_lon);
            }
            _ => {
                counts.invalid_position_rows = counts.invalid_position_rows.saturating_add(1);
                out.lat_e7.push(NONE_I32);
                out.lon_e7.push(NONE_I32);
            }
        }
        if east == 0.0 && north == 0.0 {
            counts.zero_velocity_rows = counts.zero_velocity_rows.saturating_add(1);
        }
        if !east.is_finite() || !north.is_finite() {
            counts.unavailable_velocity_rows = counts.unavailable_velocity_rows.saturating_add(1);
            out.vel_east_mps.push(east);
            out.vel_north_mps.push(north);
        } else {
            let quantized_east = quantize_publication_velocity_component(east)
                .ok_or("finite east velocity exceeds the publication-grid representable range")?;
            let quantized_north = quantize_publication_velocity_component(north)
                .ok_or("finite north velocity exceeds the publication-grid representable range")?;
            if (east != 0.0 || north != 0.0) && quantized_east == 0.0 && quantized_north == 0.0 {
                counts.rounded_to_zero_velocity_rows =
                    counts.rounded_to_zero_velocity_rows.saturating_add(1);
            }
            out.vel_east_mps.push(quantized_east);
            out.vel_north_mps.push(quantized_north);
        }
    }
    Ok(out)
}

fn validate_publication_grid_request(
    enabled: bool,
    development: bool,
    frozen: bool,
    candidate: bool,
    export_only: bool,
) -> Result<(), &'static str> {
    if !enabled {
        return Ok(());
    }
    if export_only {
        return Err("--oa-publication-grid cannot be combined with export-only --export-eram mode");
    }
    if !development || frozen || candidate {
        return Err("--oa-publication-grid sfdps-v1 requires --development and cannot be combined with frozen/candidate commits");
    }
    Ok(())
}

fn combine_has_unsupported_profile(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--timing-profile" || arg == "--oa-publication-grid" || arg == "--p010-local-window"
    })
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--combine") {
        if combine_has_unsupported_profile(&argv) {
            eprintln!("p002: refused: scoring profiles are not supported by --combine");
            return ExitCode::from(2);
        }
        return combine_main(argv.get(1..).unwrap_or(&[]));
    }
    let mut args = argv.into_iter();
    let (mut adsb, mut sfdps, mut out, mut label) = (None, None, None, String::new());
    let (mut eram_export, mut export_out, mut drop_path): (
        Option<PathBuf>,
        Option<PathBuf>,
        Option<PathBuf>,
    ) = (None, None, None);
    let (mut allow_dirty, mut development) = (false, false);
    let mut timing_profile: Option<ScoringTimingProfile> = None;
    let mut oa_publication_grid = false;
    let mut p010_local_window = false;
    let (mut warmup_from, mut score_from, mut score_to, mut zau_box_profile): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = (None, None, None, None);
    let mut frozen: Option<String> = None;
    let mut candidate: Option<String> = None;
    let (mut damage_path, mut ledger): (Option<PathBuf>, Option<PathBuf>) = (None, None);
    let mut windows: Vec<Window> = Vec::new();
    while let Some(a) = args.next() {
        if a == "--exclude-window" {
            let (Some(from), Some(to)) = (args.next(), args.next()) else {
                eprintln!("p002: --exclude-window needs <from ISO> <to ISO>");
                return ExitCode::from(2);
            };
            match Window::parse(&from, &to) {
                Ok(w) => windows.push(w),
                Err(e) => {
                    eprintln!("p002: --exclude-window: {e}");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        if a == "--allow-dirty" {
            allow_dirty = true;
            continue;
        }
        if a == "--development" {
            development = true;
            continue;
        }
        if a == "--p010-local-window" {
            if p010_local_window {
                eprintln!("p002: refused: --p010-local-window may be given only once");
                return ExitCode::from(2);
            }
            p010_local_window = true;
            continue;
        }
        if a == "--timing-profile" {
            let Some(value) = args.next() else {
                eprintln!("p002: --timing-profile needs a profile name");
                return ExitCode::from(2);
            };
            if timing_profile.is_some() {
                eprintln!("p002: refused: --timing-profile may be given only once");
                return ExitCode::from(2);
            }
            match value.as_str() {
                "legacy-day-intervals" => {
                    timing_profile = Some(ScoringTimingProfile::LegacyDayIntervals);
                }
                "fixed-10-30-120-v1" => {
                    timing_profile = Some(ScoringTimingProfile::Fixed1030120V1);
                }
                _ => {
                    eprintln!("p002: unknown timing profile {value}");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        if [
            "--warmup-from",
            "--score-from",
            "--score-to",
            "--zau-box-profile",
        ]
        .contains(&a.as_str())
        {
            let Some(value) = args.next() else {
                eprintln!("p002: {a} needs a value");
                return ExitCode::from(2);
            };
            let slot = match a.as_str() {
                "--warmup-from" => &mut warmup_from,
                "--score-from" => &mut score_from,
                "--score-to" => &mut score_to,
                _ => &mut zau_box_profile,
            };
            if slot.replace(value).is_some() {
                eprintln!("p002: refused: {a} may be given only once");
                return ExitCode::from(2);
            }
            continue;
        }
        if a == "--oa-publication-grid" {
            let Some(value) = args.next() else {
                eprintln!("p002: --oa-publication-grid needs a profile name");
                return ExitCode::from(2);
            };
            if oa_publication_grid {
                eprintln!("p002: refused: --oa-publication-grid may be given only once");
                return ExitCode::from(2);
            }
            if value != OA_PUBLICATION_GRID_PROFILE {
                eprintln!("p002: unknown OA publication grid {value}");
                return ExitCode::from(2);
            }
            oa_publication_grid = true;
            continue;
        }
        let v = args.next();
        match a.as_str() {
            "--adsb" => adsb = v.map(PathBuf::from),
            "--sfdps-dir" => sfdps = v.map(PathBuf::from),
            "--eram-export" => eram_export = v.map(PathBuf::from),
            "--export-eram" => export_out = v.map(PathBuf::from),
            "--drop-addresses" => drop_path = v.map(PathBuf::from),
            "--out" => out = v.map(PathBuf::from),
            "--label" => label = v.unwrap_or_default(),
            "--frozen-commit" => frozen = v,
            "--candidate-commit" => candidate = v,
            "--damage-report" => damage_path = v.map(PathBuf::from),
            "--ledger" => ledger = v.map(PathBuf::from),
            _ => {
                eprintln!("p002: unknown option {a}");
                return ExitCode::from(2);
            }
        }
    }
    let p010 = match parse_p010_window(
        warmup_from.as_deref(),
        score_from.as_deref(),
        score_to.as_deref(),
        zau_box_profile.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("p002: refused: {e}");
            return ExitCode::from(2);
        }
    };
    if p010_local_window != p010.is_some() {
        eprintln!("p002: refused: P-010 fields are accepted only with --p010-local-window, which requires all four window/profile fields");
        return ExitCode::from(2);
    }
    if p010_local_window
        && (!development
            || allow_dirty
            || frozen.is_some()
            || candidate.is_some()
            || damage_path.is_some()
            || drop_path.is_some()
            || !label.is_empty()
            || !windows.is_empty()
            || eram_export.is_some()
            || export_out.is_some()
            || timing_profile.is_none())
    {
        eprintln!("p002: refused: P-010 local window requires --development, one explicit --timing-profile, and --sfdps-dir; it rejects dirty override, frozen/candidate commits, damage/drop-addresses, excluded windows, and ERAM export modes");
        return ExitCode::from(2);
    }
    if timing_profile.is_some() && export_out.is_some() {
        eprintln!("p002: refused: --timing-profile cannot be combined with export-only --export-eram mode");
        return ExitCode::from(2);
    }
    if timing_profile.is_some() && (!development || frozen.is_some() || candidate.is_some()) {
        eprintln!("p002: refused: an explicit --timing-profile requires --development and cannot be combined with frozen/candidate commits");
        return ExitCode::from(2);
    }
    if let Err(e) = validate_publication_grid_request(
        oa_publication_grid,
        development,
        frozen.is_some(),
        candidate.is_some(),
        export_out.is_some(),
    ) {
        eprintln!("p002: refused: {e}");
        return ExitCode::from(2);
    }
    if let Some(file) = export_out {
        let stray = adsb.is_some()
            || out.is_some()
            || eram_export.is_some()
            || !label.is_empty()
            || allow_dirty
            || development
            || frozen.is_some()
            || candidate.is_some()
            || damage_path.is_some()
            || ledger.is_some()
            || !windows.is_empty();
        let (Some(dir), false) = (sfdps, stray) else {
            eprintln!(
                "usage: p002 --export-eram <file> --sfdps-dir <dir> [--drop-addresses <file>]"
            );
            return ExitCode::from(2);
        };
        return export_main(&dir, &file, drop_path.as_deref());
    }
    let (Some(adsb), Some(out)) = (adsb, out) else {
        eprintln!("usage: p002 --adsb <clip.oaf> (--sfdps-dir <dir> | --eram-export <file>) --out <dir> [--drop-addresses <file>] [--label <text>] [--allow-dirty] [--development] [--frozen-commit <sha> --candidate-commit <sha>] [--exclude-window <from ISO> <to ISO>]... [--damage-report <oa-feed-gaps report>] [--timing-profile legacy-day-intervals|fixed-10-30-120-v1] [--oa-publication-grid sfdps-v1] --ledger <file>\n       p002 --p010-local-window --adsb <full-day clip.oaf> --sfdps-dir <raw dir> --out <private dir> --development --ledger <fresh empty file> --warmup-from <ISO> --score-from <ISO> --score-to <ISO> --zau-box-profile zau-v1 --timing-profile legacy-day-intervals|fixed-10-30-120-v1 [--oa-publication-grid sfdps-v1]\n       p002 --combine <dirA> <dirB> <dirC>");
        return ExitCode::from(2);
    };
    let source = match (sfdps, eram_export) {
        (Some(d), None) => Source::Recording(d),
        (None, Some(f)) => Source::Export(f),
        _ => {
            eprintln!("p002: exactly one of --sfdps-dir and --eram-export is needed");
            return ExitCode::from(2);
        }
    };
    if p010_local_window && !matches!(&source, Source::Recording(_)) {
        eprintln!("p002: refused: P-010 requires raw --sfdps-dir input");
        return ExitCode::from(2);
    }
    // Every path is checked, as given and resolved, before anything is opened or created.
    for p in [
        Some(adsb.as_path()),
        Some(out.as_path()),
        Some(source.path()),
        drop_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Err(e) = oa_sealed::refuse(p) {
            eprintln!("p002: {e}");
            return ExitCode::from(2);
        }
    }
    let git = git_state();
    if git_is_dirty(&git) && !allow_dirty && !p010_local_window {
        eprintln!(
            "p002: refused: the working tree is not clean ({}); a hold-out score is valid only from a clean checkout at the frozen commit (--allow-dirty runs it, stamped as not valid)",
            git_tree_text(&git)
        );
        return ExitCode::from(2);
    }
    // A hold-out run is one that is neither marked development nor allowed on a dirty tree.
    let mode = Mode {
        holdout: !allow_dirty && !development,
        development,
    };
    if let Err(e) = holdout_binding(
        mode,
        frozen.as_deref(),
        Build {
            commit: option_env!("OA_BUILD_COMMIT"),
            dirty: option_env!("OA_BUILD_DIRTY"),
        },
        &git,
    ) {
        eprintln!("p002: refused: {e}");
        return ExitCode::from(2);
    }
    // An export has no receive times and is of development days: it cannot be a hold-out run, and
    // the damage report's check of the recording's span has nothing to read.
    if matches!(source, Source::Export(_))
        && (!mode.development || mode.holdout || damage_path.is_some())
    {
        eprintln!("p002: refused: --eram-export is for explicit --development runs without --damage-report; a hold-out run reads the recording");
        return ExitCode::from(2);
    }
    let drop_list = match drop_path.as_deref().map(read_drop_list) {
        None => None,
        Some(Ok(l)) => Some(l),
        Some(Err(e)) => {
            eprintln!("p002: {e}");
            return ExitCode::from(2);
        }
    };
    let commits = Commits { frozen, candidate };
    if let Err(e) = candidate_binding(mode, &commits, Path::new(env!("CARGO_MANIFEST_DIR"))) {
        eprintln!("p002: refused: {e}");
        return ExitCode::from(2);
    }
    // P-008 sections 2 and 5: a hold-out run takes its excluded windows only from the damage
    // report of its day, and every run is checked against the ledger of scored days.
    let Some(ledger) = ledger else {
        eprintln!("p002: refused: --ledger <file> is required (the ledger of scored days; a development run may pass a fresh empty file)");
        return ExitCode::from(2);
    };
    if mode.holdout && !windows.is_empty() {
        eprintln!("p002: refused: --exclude-window is for development runs; a hold-out run takes its windows from --damage-report");
        return ExitCode::from(2);
    }
    if mode.holdout && damage_path.is_none() {
        eprintln!(
            "p002: refused: a hold-out run needs --damage-report <oa-feed-gaps report of the day>"
        );
        return ExitCode::from(2);
    }
    let mut damage: Option<DamageReport> = None;
    if let Some(dp) = &damage_path {
        let read = oa_sealed::refuse(dp)
            .and_then(|()| oa_sealed::read(dp).map_err(|e| format!("{}: {e}", dp.display())))
            .and_then(|b| {
                let text = String::from_utf8(b.clone()).map_err(|e| e.to_string())?;
                parse_damage_report(&text, &to_hex(&sha256(&b)))
            });
        match read {
            Ok(d) => damage = Some(d),
            Err(e) => {
                eprintln!("p002: refused: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if let Err(e) = oa_sealed::refuse(&ledger) {
        eprintln!("p002: {e}");
        return ExitCode::from(2);
    }
    if p010_local_window {
        match oa_sealed::read(&ledger) {
            Ok(bytes) if bytes.is_empty() => {}
            Ok(_) => {
                eprintln!("p002: refused: P-010 requires a fresh empty per-arm ledger");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("p002: refused: P-010 ledger could not be read: {e}");
                return ExitCode::from(2);
            }
        }
    }
    // Nothing is written if either output exists (P-006 section 7).
    if let Err(e) = outputs_free(&out) {
        eprintln!("p002: refused: {e}");
        return ExitCode::from(2);
    }
    match run(
        &adsb,
        &source,
        &out,
        &git,
        &Opts {
            label: &label,
            mode,
            commits: &commits,
            windows: &windows,
            damage: damage.as_ref(),
            ledger: &ledger,
            drop_list: drop_list.as_ref(),
            timing_profile: timing_profile.unwrap_or(ScoringTimingProfile::LegacyDayIntervals),
            oa_publication_grid,
            p010,
        },
    ) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("p002: {e}");
            ExitCode::from(1)
        }
    }
}

/// `--combine <dirA> <dirB> <dirC>`: the cross-day claim rule over the `CLAIM` lines of three
/// days' `p002.txt`. Reads through the seal guard; writes nothing.
fn combine_main(dirs: &[String]) -> ExitCode {
    let [a, b, c] = dirs else {
        eprintln!("usage: p002 --combine <dirA> <dirB> <dirC>");
        return ExitCode::from(2);
    };
    let mut texts: Vec<(String, String)> = Vec::new();
    for d in [a, b, c] {
        let dir = Path::new(d);
        if let Err(e) = oa_sealed::refuse(dir) {
            eprintln!("p002: {e}");
            return ExitCode::from(2);
        }
        let mut two = Vec::new();
        for f in ["p002.txt", "samples.csv"] {
            match oa_sealed::read_to_string(dir.join(f)) {
                Ok(t) => two.push(t),
                Err(e) => {
                    eprintln!("p002: {d}/{f}: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        let mut it = two.into_iter();
        let (Some(t), Some(v)) = (it.next(), it.next()) else {
            return ExitCode::from(2);
        };
        texts.push((t, v));
    }
    let [ta, tb, tc] = texts.as_slice() else {
        return ExitCode::from(2);
    };
    fn day(t: &(String, String)) -> DayFiles<'_> {
        DayFiles {
            txt: t.0.as_str(),
            csv: t.1.as_str(),
        }
    }
    match combine([day(ta), day(tb), day(tc)]) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("p002: refused: {e}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Small numerics: PRNG, binomial test, statistics.
// ---------------------------------------------------------------------------------------------

/// splitmix64 (Steele, Lea, Flood): the bootstrap's generator.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (multiply-shift; the bias is below 2^-40 for the sizes used here).
    fn below(&mut self, n: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }
}

/// Two-sided exact binomial (p = 1/2) p-value for `k` of `n`, in f64 with log factorials:
/// twice the smaller tail, capped at 1.
fn sign_test_p(k: u64, n: u64) -> f64 {
    if n == 0 {
        return f64::NAN;
    }
    let mut lf = Vec::with_capacity(n as usize + 1);
    let mut acc = 0.0f64;
    lf.push(0.0);
    for i in 1..=n {
        acc += (i as f64).ln();
        lf.push(acc);
    }
    let at = |i: u64| lf.get(i as usize).copied().unwrap_or(f64::NAN);
    let lo = k.min(n - k.min(n));
    let ln2n = n as f64 * std::f64::consts::LN_2;
    let tail: f64 = (0..=lo)
        .map(|i| (at(n) - at(i) - at(n - i) - ln2n).exp())
        .sum();
    (2.0 * tail).min(1.0)
}

/// Nearest-rank percentile `p` of `buf` (any order, rearranged): rank `ceil(p n / 100)`.
fn rank_of(buf: &mut [f64], p: u64) -> f64 {
    let n = buf.len() as u64;
    if n == 0 {
        return f64::NAN;
    }
    let k = (p * n).div_ceil(100).clamp(1, n) as usize - 1;
    let (_, v, _) = buf.select_nth_unstable_by(k, |a, b| oa_math::cmp_canon_f64(*a, *b));
    *v
}

/// `[median, p95, rms]` of absolute values (the library's definitions: nearest rank, no
/// interpolation, RMS from the sum of squares). NaN for no values.
fn stat3(buf: &mut [f64]) -> [f64; 3] {
    if buf.is_empty() {
        return [f64::NAN; 3];
    }
    let rms = (buf.iter().map(|v| v * v).sum::<f64>() / buf.len() as f64).sqrt();
    let med = rank_of(buf, 50);
    let p95 = rank_of(buf, 95);
    [med, p95, rms]
}

/// Signed nearest-rank percentile of a sorted copy.
fn signed_rank(v: &[f64], p: u64) -> f64 {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    rank_of(&mut s, p)
}

// ---------------------------------------------------------------------------------------------
// Samples per system.
// ---------------------------------------------------------------------------------------------

/// A flag of a tracker row, or no tracker row to read it from (E6: never a default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Tri {
    /// No tracker row matched the sample, or the rows that matched it disagree.
    Unmatched = 0,
    No = 1,
    Yes = 2,
}

/// `0`, `1` or `unmatched`, for the CSV.
fn tri_text(t: Tri) -> String {
    match t {
        Tri::Unmatched => "unmatched".to_string(),
        Tri::No => "0".to_string(),
        Tri::Yes => "1".to_string(),
    }
}

fn tri_of(b: bool) -> Tri {
    if b {
        Tri::Yes
    } else {
        Tri::No
    }
}

/// One system's paired sample at its own row time.
///
/// A7: held per grid sample and system, so the size is asserted. 56 bytes: six 8-byte numbers,
/// five 1-byte fields, padding to 8. A7.1: `Tri` replaces the two `bool` flags of P-003 at the
/// same width, so the flags added no bytes (they sit in what was padding).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Pt {
    t: i64,
    cross: f64,
    along: f64,
    /// Truth ground speed, m/s.
    v: f64,
    /// Ground-speed error, m/s (NaN when absent).
    spd: f64,
    /// Track-angle error, degrees (NaN when absent).
    hdg: f64,
    coast: u8,
    /// The tracker's `O_UNCERTAIN` and `O_REACQUIRED` flags of the row (OpenATCA only).
    uncertain: Tri,
    reacquired: Tri,
    phase: u8,
    band: u8,
}
const _: () = assert!(core::mem::size_of::<Pt>() == 56);

type PerAc = BTreeMap<u32, Vec<Pt>>;

#[derive(Default, Clone, Copy)]
// A7.2: cold struct, size guard waived
struct Dropped {
    /// Finite pairs on a truth leg that holds no grid sample of the aircraft (P-004 section 1).
    off_scored_legs: u64,
    no_cross_along: u64,
    duplicate_time: u64,
}

/// Sample or row times, Unix ms, by address, each list sorted.
type TimesBy = BTreeMap<u32, Vec<i64>>;

/// The truth legs of each address by start time, for the leg a time falls on.
// A7.2: cold struct, size guard waived
struct LegIndex {
    starts: BTreeMap<u32, Vec<i64>>,
}

impl LegIndex {
    fn new(legs: &[TruthLeg]) -> LegIndex {
        let mut starts: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
        for l in legs {
            starts.entry(l.addr).or_default().push(l.start_ms);
        }
        for v in starts.values_mut() {
            v.sort_unstable();
        }
        LegIndex { starts }
    }

    /// The leg of `addr` at `t_ms`: the latest leg that starts at or before it, the first leg if it
    /// precedes them all, leg 0 for an address with none. This is the cut
    /// `legacy_from_track_samples_by_leg` makes (it numbers a tracker row's flight the same way),
    /// and a paired sample always lies within a leg, because the pairing needs a truth value.
    fn leg_of(&self, addr: u32, t_ms: i64) -> u32 {
        self.starts.get(&addr).map_or(0, |v| {
            u32::try_from(v.partition_point(|&x| x <= t_ms).saturating_sub(1)).unwrap_or(u32::MAX)
        })
    }
}

/// The legs each aircraft is scored on (P-004 section 1): those that contain its grid samples.
type ScoredLegs = BTreeMap<u32, BTreeSet<u32>>;

fn scored_legs(grid: &PerAc, legs: &LegIndex) -> ScoredLegs {
    grid.iter()
        .map(|(&a, v)| (a, v.iter().map(|s| legs.leg_of(a, s.t)).collect()))
        .collect()
}

/// Whether `t_ms` of `addr` lies on one of its scored legs.
fn on_scored_leg(scored: &ScoredLegs, legs: &LegIndex, addr: u32, t_ms: i64) -> bool {
    scored
        .get(&addr)
        .is_some_and(|s| s.contains(&legs.leg_of(addr, t_ms)))
}

/// A pair table's finite position pairs per aircraft, sorted by time, one per time, those for
/// which `keep(address, time)` holds. No flight is chosen: a system's samples are kept or left
/// out by the leg rule the caller passes (P-004 section 1).
fn pair_points(
    p: &PairTable,
    flights: Option<&BTreeMap<u32, u32>>,
    keep: &dyn Fn(u32, i64) -> bool,
    per: &mut PerAc,
    dropped: &mut Dropped,
) {
    let n = p.t_ms.len();
    let fin = |i: usize| {
        p.cross_m.get(i).is_some_and(|v| v.is_finite())
            && p.along_m.get(i).is_some_and(|v| v.is_finite())
    };
    let nan = f32::NAN;
    let mut local: PerAc = BTreeMap::new();
    for i in 0..n {
        let (Some(&a), Some(&t)) = (p.addr.get(i), p.t_ms.get(i)) else {
            continue;
        };
        // Only ERAM's grid is limited to a flight (the most-paired one); not counted as dropped.
        if flights
            .is_some_and(|f| f.get(&a).copied() != Some(p.flight.get(i).copied().unwrap_or(0)))
        {
            continue;
        }
        if !fin(i) {
            dropped.no_cross_along += 1;
            continue;
        }
        if !keep(a, t) {
            dropped.off_scored_legs += 1;
            continue;
        }
        let g = |v: &[f32], d: f32| f64::from(v.get(i).copied().unwrap_or(d));
        local.entry(a).or_default().push(Pt {
            t,
            cross: g(&p.cross_m, nan),
            along: g(&p.along_m, nan),
            v: g(&p.truth_speed_mps, nan),
            spd: g(&p.speed_err_kt, nan) * KT_MPS,
            hdg: g(&p.hdg_err_deg, nan),
            coast: p.coast.get(i).copied().unwrap_or(0),
            uncertain: Tri::Unmatched,
            reacquired: Tri::Unmatched,
            phase: p.phase.get(i).copied().unwrap_or(0),
            band: p.band.get(i).copied().unwrap_or(0),
        });
    }
    for (a, mut v) in local {
        v.sort_by_key(|s| s.t);
        let before = v.len();
        v.dedup_by_key(|s| s.t);
        dropped.duplicate_time += (before - v.len()) as u64;
        per.entry(a).or_default().extend(v);
    }
}

/// The times of the finite pairs that P-003 resolution 11 would have kept: per aircraft, the
/// flight with the most finite pairs (the lowest on a tie), one pair per time. Kept only so the
/// output can show the coverage under the replaced rule beside P-004's; no table reads it.
fn old_rule_times(p: &PairTable, out: &mut TimesBy) {
    let n = p.t_ms.len();
    let fin = |i: usize| {
        p.cross_m.get(i).is_some_and(|v| v.is_finite())
            && p.along_m.get(i).is_some_and(|v| v.is_finite())
    };
    let best = most_paired_flights(p);
    let mut local: TimesBy = BTreeMap::new();
    for i in 0..n {
        let (Some(&a), Some(&t)) = (p.addr.get(i), p.t_ms.get(i)) else {
            continue;
        };
        if fin(i) && best.get(&a).copied() == Some(p.flight.get(i).copied().unwrap_or(0)) {
            local.entry(a).or_default().push(t);
        }
    }
    for (a, mut v) in local {
        v.sort_unstable();
        v.dedup();
        out.entry(a).or_default().extend(v);
    }
}

/// Each aircraft's flight with the most finite pairs (the lowest flight on a tie). P-004 keeps
/// this for ERAM only, where it defines the grid ("as before, from ERAM's paired times").
fn most_paired_flights(p: &PairTable) -> BTreeMap<u32, u32> {
    let n = p.t_ms.len();
    let fin = |i: usize| {
        p.cross_m.get(i).is_some_and(|v| v.is_finite())
            && p.along_m.get(i).is_some_and(|v| v.is_finite())
    };
    let mut count: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for i in 0..n {
        if let (Some(&a), true) = (p.addr.get(i), fin(i)) {
            *count
                .entry((a, p.flight.get(i).copied().unwrap_or(0)))
                .or_default() += 1;
        }
    }
    let mut best: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    for (&(a, f), &c) in &count {
        let e = best.entry(a).or_insert((f, c));
        if c > e.1 {
            *e = (f, c);
        }
    }
    best.into_iter().map(|(a, (f, _))| (a, f)).collect()
}

/// The sample times of every aircraft of a per-aircraft table.
fn times_of(per: &PerAc) -> TimesBy {
    per.iter()
        .map(|(&a, v)| (a, v.iter().map(|s| s.t).collect()))
        .collect()
}

/// Whether `v` (sorted) holds a time within [`TOL_MS`] of `t`.
fn near_time(v: &[i64], t: i64) -> bool {
    let i = v.partition_point(|&x| x < t);
    i.checked_sub(1)
        .and_then(|j| v.get(j))
        .is_some_and(|&x| t - x <= TOL_MS)
        || v.get(i).is_some_and(|&x| x - t <= TOL_MS)
}

/// Coverage of a system under the replaced rule: `(grid samples it has a time within 0.5 s of,
/// grid samples)`, the grid being ERAM's under that rule.
fn old_rule_coverage(grid: &TimesBy, sys: &TimesBy) -> (u64, u64) {
    let (mut hit, mut all) = (0u64, 0u64);
    for (a, ts) in grid {
        let own = sys.get(a).map_or(&[][..], Vec::as_slice);
        for &t in ts {
            all += 1;
            hit += u64::from(near_time(own, t));
        }
    }
    (hit, all)
}

// ---------------------------------------------------------------------------------------------
// The flag join.
// ---------------------------------------------------------------------------------------------

/// What the tracker rows at one (address, millisecond) say of the two flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlagEntry {
    One {
        uncertain: bool,
        reacquired: bool,
    },
    /// Two or more rows at the key that disagree: the flags of a sample there are unknown.
    Conflict,
}

/// The tracker's flags by (address, row time in whole milliseconds).
///
/// The join key is the finest time both sources carry: the pair table that gives a sample its
/// time holds whole milliseconds (`t_ns` floored, as `legacy_from_track_samples_by_leg` does), so
/// two tracker rows within one millisecond share a key. Such keys are counted, never overwritten:
/// rows that agree stay one entry, rows that disagree make the key a conflict.
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
struct FlagJoin {
    map: BTreeMap<(u32, i64), FlagEntry>,
    /// Rows whose key already held a row.
    duplicate_rows: u64,
    /// Keys whose rows disagree on a flag.
    conflict_keys: u64,
}

/// Builds the join from a range's tracker rows; the columns must be as long as `addr`.
fn flag_join(
    addr: &[u32],
    t_ns: &[i64],
    uncertain: &[bool],
    reacquired: &[bool],
) -> Result<FlagJoin, String> {
    let n = addr.len();
    if [t_ns.len(), uncertain.len(), reacquired.len()]
        .iter()
        .any(|&l| l != n)
    {
        return Err("tracker row columns have different lengths".to_string());
    }
    let mut j = FlagJoin::default();
    for (((&a, &t), &u), &re) in addr.iter().zip(t_ns).zip(uncertain).zip(reacquired) {
        let new = FlagEntry::One {
            uncertain: u,
            reacquired: re,
        };
        match j.map.entry((a, t.div_euclid(1_000_000))) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(new);
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                j.duplicate_rows = j.duplicate_rows.saturating_add(1);
                if *e.get() != new && *e.get() != FlagEntry::Conflict {
                    j.conflict_keys = j.conflict_keys.saturating_add(1);
                    e.insert(FlagEntry::Conflict);
                }
            }
        }
    }
    Ok(j)
}

/// The `(uncertain, reacquired)` states of a sample: unmatched when no tracker row has its key or
/// the rows there disagree.
fn flag_lookup(j: &FlagJoin, addr: u32, t_ms: i64) -> (Tri, Tri) {
    match j.map.get(&(addr, t_ms)) {
        Some(FlagEntry::One {
            uncertain,
            reacquired,
        }) => (tri_of(*uncertain), tri_of(*reacquired)),
        Some(FlagEntry::Conflict) | None => (Tri::Unmatched, Tri::Unmatched),
    }
}

/// The sample nearest in time to `t` within [`TOL_MS`] (the earlier on a tie).
fn nearest(v: &[Pt], t: i64) -> Option<&Pt> {
    let i = v.partition_point(|s| s.t < t);
    let before = i.checked_sub(1).and_then(|j| v.get(j));
    let pick = match (before, v.get(i)) {
        (Some(b), Some(a)) => {
            if t - b.t <= a.t - t {
                b
            } else {
                a
            }
        }
        (Some(b), None) => b,
        (None, Some(a)) => a,
        (None, None) => return None,
    };
    ((pick.t - t).abs() <= TOL_MS).then_some(pick)
}

/// Association, pairing and exclusions of one legacy table against the reference.
// A7.2: cold struct, size guard waived
struct Paired {
    pairs: PairTable,
    ex: Exclusions,
    assoc_counts: [u64; astatus::COUNT],
}

fn pair_legacy(leg: &Legacy, truth: &Truth<'_>, tix: &TruthIndex, rcfg: &RunConfig) -> Paired {
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let assoc = associate(leg, &ix, truth, tix, &rcfg.assoc);
    let (pairs, ex) = build_pairs_with(leg, &ix, &assoc, truth, &rcfg.pair, &|_, _, _| None);
    Paired {
        pairs,
        ex,
        assoc_counts: assoc.counts,
    }
}

// ---------------------------------------------------------------------------------------------
// The grid.
// ---------------------------------------------------------------------------------------------

/// One system's value at a grid sample, with its fitted-time quantities.
///
/// A7: held per grid sample and system. 96 bytes: a [`Pt`] (56) and five 8-byte quantities.
/// A7.1: P-003 added `along_tol` and `h_tol` (+16 bytes), the rounding-tolerant errors M2t and M3t
/// the metrics read per sample; they are stored, not recomputed, because every table, bootstrap
/// resample and the CSV read them. This is an offline scorer on a few million samples.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Val {
    pt: Pt,
    along_fit: f64,
    h_raw: f64,
    h_fit: f64,
    /// P-003 rounding-tolerant along-track error and horizontal error.
    along_tol: f64,
    h_tol: f64,
}
const _: () = assert!(core::mem::size_of::<Val>() == 96);

/// A grid sample: its three systems' values.
///
/// A7: one per grid sample. 312 bytes: `ac` and `t` (16), `addr` and two 1-byte strata (8 with
/// padding), and three `Option<Val>` of 96 (the `Tri` niche leaves no tag). A7.1: grew with `Val`
/// (+48 bytes for three systems), see there.
#[derive(Clone, Debug)]
#[repr(C)]
struct Row {
    /// Index of the aircraft among the grid aircraft.
    ac: usize,
    t: i64,
    addr: u32,
    /// Phase and altitude band of the ERAM sample (the stratum of every system's value).
    phase: u8,
    band: u8,
    val: [Option<Val>; 3],
}
const _: () = assert!(core::mem::size_of::<Row>() == 312);

impl Row {
    /// The three-way common set: all three systems have a value.
    fn common(&self) -> bool {
        self.val.iter().all(Option::is_some)
    }

    /// The two-way common set (P-006 section 4): OpenATCA and ERAM both have a value.
    fn two_way(&self) -> bool {
        [OA, ER]
            .iter()
            .all(|&s| self.val.get(s).is_some_and(Option::is_some))
    }
}

/// The rows of grid aircraft `ac` (rows are in aircraft order).
fn of_ac(rows: &[Row], ac: usize) -> &[Row] {
    let a = rows.partition_point(|r| r.ac < ac);
    let b = rows.partition_point(|r| r.ac <= ac);
    rows.get(a..b).unwrap_or(&[])
}

/// P-004 section 3: why a system has no value at a grid sample, judged by the system's own rows
/// of the address (OpenATCA's output rows, ERAM's paired samples, the set-A reports).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Miss {
    /// Before the system's first row for the address.
    Before,
    /// After its last row.
    After,
    /// In a gap between rows.
    Gap,
    /// The system has no row for the address at all.
    NeverPresent,
    /// A row lies within 0.5 s, but it is not scored (left out at pairing, off the scored legs,
    /// no finite error).
    Other,
}

/// The order of [`Miss`] in the counts.
const MISS_NAMES: [&str; 5] = [
    "before the system's first row for the address",
    "after its last row",
    "in a gap between rows",
    "address never present",
    "other (a row within 0.5 s that is not scored: left out at pairing, off the scored legs, or no finite error)",
];

impl Miss {
    fn index(self) -> usize {
        match self {
            Miss::Before => 0,
            Miss::After => 1,
            Miss::Gap => 2,
            Miss::NeverPresent => 3,
            Miss::Other => 4,
        }
    }
}

/// The class of a miss at `t` given the system's row times for the address (sorted; `None` or
/// empty when it has no row for the address). Every miss falls in exactly one class.
fn classify_miss(times: Option<&[i64]>, t: i64) -> Miss {
    let Some(v) = times.filter(|v| !v.is_empty()) else {
        return Miss::NeverPresent;
    };
    if near_time(v, t) {
        return Miss::Other;
    }
    let i = v.partition_point(|&x| x < t);
    if i == 0 {
        Miss::Before
    } else if i >= v.len() {
        Miss::After
    } else {
        Miss::Gap
    }
}

/// P-004 section 2: an OpenATCA row lies in the reacquired group when it is within this many ms
/// after an update row of the address flagged `O_REACQUIRED` (the window the display uses).
const REACQ_WINDOW_MS: i64 = 10_000;

/// Whether `t_ms` is within [`REACQ_WINDOW_MS`] after one of `updates` (the times of the
/// address's reacquired update rows, sorted), the update's own time included.
fn reacquired_at(updates: Option<&[i64]>, t_ms: i64) -> bool {
    let Some(v) = updates else {
        return false;
    };
    let i = v.partition_point(|&u| u <= t_ms);
    i.checked_sub(1)
        .and_then(|j| v.get(j))
        .is_some_and(|&u| t_ms - u <= REACQ_WINDOW_MS)
}

/// The grid and what it holds, before any time fit.
// A7.2: cold struct, size guard waived
struct Grid {
    rows: Vec<Row>,
    ac_addr: Vec<u32>,
    /// Misses per system: [aircraft has no paired samples, no paired sample within 0.5 s].
    miss: [[u64; 2]; 3],
    /// Misses per system by [`Miss`] class.
    classes: [[u64; 5]; 3],
}

/// The grid: every sample of `er` (ERAM's paired samples) with each system's nearest sample
/// within 0.5 s of it. `oa` and `pt` hold only samples on the aircraft's scored legs. `row_times`
/// are each system's row times by address, for the miss classes.
fn build_grid(er: &PerAc, oa: &PerAc, pt: &PerAc, row_times: [&TimesBy; 3]) -> Grid {
    let mut g = Grid {
        rows: Vec::new(),
        ac_addr: Vec::new(),
        miss: [[0; 2]; 3],
        classes: [[0; 5]; 3],
    };
    for (&addr, ev) in er {
        let ac = g.ac_addr.len();
        g.ac_addr.push(addr);
        let others = [oa.get(&addr), None, pt.get(&addr)];
        for s in ev {
            let mut val: [Option<Val>; 3] = [None; 3];
            for (k, slot) in val.iter_mut().enumerate() {
                let p = if k == ER {
                    Some(*s)
                } else {
                    match others.get(k).copied().flatten() {
                        None => {
                            if let Some(m) = g.miss.get_mut(k).and_then(|m| m.get_mut(0)) {
                                *m += 1;
                            }
                            None
                        }
                        Some(v) => {
                            let n = nearest(v, s.t).copied();
                            if n.is_none() {
                                if let Some(m) = g.miss.get_mut(k).and_then(|m| m.get_mut(1)) {
                                    *m += 1;
                                }
                            }
                            n
                        }
                    }
                };
                if p.is_none() {
                    let own = row_times
                        .get(k)
                        .and_then(|t| t.get(&addr))
                        .map(Vec::as_slice);
                    let c = classify_miss(own, s.t).index();
                    if let Some(n) = g.classes.get_mut(k).and_then(|c2| c2.get_mut(c)) {
                        *n += 1;
                    }
                }
                *slot = p.map(|pt| Val {
                    pt,
                    along_fit: pt.along,
                    h_raw: pt.cross.hypot(pt.along),
                    h_fit: pt.cross.hypot(pt.along),
                    along_tol: along_tol(pt.along, pt.v),
                    h_tol: pt.cross.hypot(along_tol(pt.along, pt.v)),
                });
            }
            g.rows.push(Row {
                ac,
                addr,
                t: s.t,
                phase: s.phase,
                band: s.band,
                val,
            });
        }
    }
    g
}

/// Whether a grid row is one a system's time offset is fitted on (P-008 section 1): the two-way
/// common set for OpenATCA and ERAM, the three-way set for the passthrough.
fn in_fit_set(r: &Row, system: usize) -> bool {
    if system == PT {
        r.common()
    } else {
        r.two_way()
    }
}

/// The least-squares offset `tau` of `along = tau * v` over `(along, v)` pairs (library fit).
fn fit_tau(it: impl Iterator<Item = (f64, f64)>) -> Option<f64> {
    let (mut sev, mut svv) = (0.0, 0.0);
    for (e, v) in it {
        if e.is_finite() && v.is_finite() && v >= FIT_MIN_SPEED_MPS {
            sev += e * v;
            svv += v * v;
        }
    }
    (svv > 0.0).then(|| sev / svv)
}

/// Per system, the aircraft with a value but no usable time fit, and their samples with a value:
/// `(aircraft, samples)`. Such samples are scored unfitted (P-003 resolution 1).
fn unfitted_counts(rows: &[Row], taus: &[[Option<f64>; 3]]) -> [(u64, u64); 3] {
    let mut acs: [BTreeSet<usize>; 3] = [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
    let mut samples = [0u64; 3];
    for r in rows {
        let tau = taus.get(r.ac).copied().unwrap_or([None; 3]);
        for (((v, t), set), n) in r.val.iter().zip(tau).zip(acs.iter_mut()).zip(&mut samples) {
            if v.is_some() && t.is_none() {
                set.insert(r.ac);
                *n += 1;
            }
        }
    }
    let mut out = [(0u64, 0u64); 3];
    for ((o, set), n) in out.iter_mut().zip(&acs).zip(samples) {
        *o = (set.len() as u64, n);
    }
    out
}

/// M4: per system, the samples within `thr` over ALL grid samples (a miss, and a value that is
/// not a number, are failures), and the number of present values that are not a number.
fn m4_counts(rows: &[Row], n_ac: usize, h: fn(&Val) -> f64, thr: f64) -> ([Cnt; 3], [u64; 3]) {
    let mut c = [Cnt::new(n_ac), Cnt::new(n_ac), Cnt::new(n_ac)];
    let mut nan = [0u64; 3];
    for r in rows {
        for ((cs, n), v) in c.iter_mut().zip(nan.iter_mut()).zip(&r.val) {
            if v.as_ref().is_some_and(|v| h(v).is_nan()) {
                *n += 1;
            }
            cs.add(r.ac, v.as_ref().is_some_and(|v| h(v) <= thr));
        }
    }
    (c, nan)
}

/// M7 and M7t: failures over `LARGE_M` of the horizontal error `h`, a miss and a value that is not
/// a number counting as failures.
// A7.2: cold struct, size guard waived
struct Large {
    cnt: [Cnt; 3],
    /// Present values over the limit or not a number.
    over: [u64; 3],
    /// Present values that are not a number (counted in `over`).
    nan: [u64; 3],
    /// Present raw (unfitted) values over the limit.
    raw_over: [u64; 3],
}

fn large_counts(rows: &[Row], n_ac: usize, h: fn(&Val) -> f64) -> Large {
    let mut l = Large {
        cnt: [Cnt::new(n_ac), Cnt::new(n_ac), Cnt::new(n_ac)],
        over: [0; 3],
        nan: [0; 3],
        raw_over: [0; 3],
    };
    for r in rows {
        for (s, v) in r.val.iter().enumerate() {
            let big = v.as_ref().is_some_and(|v| fails_over(h(v), LARGE_M));
            if let (Some(v), Some(n)) = (v, l.nan.get_mut(s)) {
                *n += u64::from(h(v).is_nan());
            }
            if let (true, Some(n)) = (big, l.over.get_mut(s)) {
                *n += 1;
            }
            if let (true, Some(n)) = (
                v.as_ref().is_some_and(|v| v.h_raw > LARGE_M),
                l.raw_over.get_mut(s),
            ) {
                *n += 1;
            }
            if let Some(c) = l.cnt.get_mut(s) {
                c.add(r.ac, v.is_none() || big);
            }
        }
    }
    l
}

/// M6: per aircraft with common samples, whether OpenATCA's RMS of the fitted horizontal error is
/// lower than ERAM's. An aircraft whose RMS is not finite for either system is a failure for the
/// share, is counted per system in `nan`, and is left out of the win, lose, tie counts and the
/// sign test (a comparison with NaN is not a tie).
// A7.2: cold struct, size guard waived
struct M6 {
    win: u64,
    lose: u64,
    tie: u64,
    /// Aircraft with a non-finite RMS: `[OpenATCA, ERAM]`.
    nan: [u64; 2],
    share: Cnt,
}

fn m6_counts(rows: &[Row], n_ac: usize) -> M6 {
    let mut m = M6 {
        win: 0,
        lose: 0,
        tie: 0,
        nan: [0; 2],
        share: Cnt::new(n_ac),
    };
    for ac in 0..n_ac {
        let mut ss = [(0.0f64, 0u64); 2];
        for r in of_ac(rows, ac).iter().filter(|r| r.common()) {
            for (k, s) in [OA, ER].into_iter().enumerate() {
                if let (Some(v), Some(slot)) =
                    (r.val.get(s).and_then(|v| v.as_ref()), ss.get_mut(k))
                {
                    slot.0 += v.h_fit * v.h_fit;
                    slot.1 += 1;
                }
            }
        }
        let (a, b) = (ss[0], ss[1]);
        if a.1 == 0 {
            continue;
        }
        let (ra, rb) = ((a.0 / a.1 as f64).sqrt(), (b.0 / b.1 as f64).sqrt());
        if !(ra.is_finite() && rb.is_finite()) {
            m.nan[0] += u64::from(!ra.is_finite());
            m.nan[1] += u64::from(!rb.is_finite());
            m.share.add(ac, false);
            continue;
        }
        if ra < rb {
            m.win += 1;
        } else if ra > rb {
            m.lose += 1;
        } else {
            m.tie += 1;
        }
        m.share.add(ac, ra < rb);
    }
    m
}

/// Grid samples whose OpenATCA row has no tracker row for its flags (or only a conflicting one).
/// Only the rows matched to a grid sample count, not every point on a scored leg.
fn unmatched_rows(rows: &[Row]) -> u64 {
    rows.iter()
        .filter_map(|r| r.val.first().and_then(|v| v.as_ref()))
        .filter(|v| v.pt.uncertain == Tri::Unmatched || v.pt.reacquired == Tri::Unmatched)
        .count() as u64
}

/// The hold-out gate: a hold-out run with any unmatched sample is refused.
fn flag_gate(mode: Mode, unmatched: u64, dup_rows: u64, conflicts: u64) -> Result<(), String> {
    if mode.holdout && unmatched > 0 {
        return Err(format!(
            "hold-out run refused: {unmatched} OpenATCA samples have no tracker row (or only conflicting rows) for their flags; {conflicts} keys conflict, {dup_rows} duplicate rows (use --development for a non-hold-out run)"
        ));
    }
    Ok(())
}

/// Whether an error counts as a failure over `limit` in a coverage-penalised metric: over it, or
/// not a number. A value that cannot be computed is never a pass (E6, G6); M4 reads `h <= thr` as
/// the pass and so does this.
fn fails_over(h: f64, limit: f64) -> bool {
    h.is_nan() || h > limit
}

/// P-003 rounding-tolerant along-track error: the fitted error `a` less `0.5 s * v` of travel at
/// the reference ground speed `v`, floored at zero and keeping the sign of `a`. NaN when either
/// is not finite.
fn along_tol(a: f64, v: f64) -> f64 {
    if !(a.is_finite() && v.is_finite()) {
        return f64::NAN;
    }
    let m = (a.abs() - ROUND_TOL_S * v).max(0.0);
    if a < 0.0 {
        -m
    } else {
        m
    }
}

/// What `git` said of the tree the scorer was built from.
// A7.2: cold struct, size guard waived
struct GitState {
    head: String,
    /// `git status --porcelain` output; `None` when git could not say.
    porcelain: Option<String>,
}

/// How a run was started: a hold-out run is neither marked development nor allowed dirty.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
struct Mode {
    holdout: bool,
    development: bool,
}

/// What the binary says it was built from: `OA_BUILD_COMMIT` (a commit) and `OA_BUILD_DIRTY`
/// (`0` for a clean tree), set by whoever built it (`tools/p002_build.sh`); `None` when unset.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
struct Build<'a> {
    commit: Option<&'a str>,
    dirty: Option<&'a str>,
}

/// Whether `s` is a full commit id: 40 lowercase hex digits.
fn is_full_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The hold-out gate on provenance (not on what is computed): a hold-out run needs
/// `--frozen-commit`, a binary built from exactly that commit on a clean tree, and a working tree
/// at run time that is at that commit and clean. Any other mode is not checked here.
fn holdout_binding(
    mode: Mode,
    frozen: Option<&str>,
    build: Build<'_>,
    git: &GitState,
) -> Result<(), String> {
    if !mode.holdout {
        return Ok(());
    }
    let Some(frozen) = frozen else {
        return Err("a hold-out run needs --frozen-commit <sha> (or use --development)".into());
    };
    if !is_full_sha(frozen) {
        return Err(format!(
            "--frozen-commit must be 40 lowercase hex digits, got `{frozen}`"
        ));
    }
    let Some(built) = build.commit else {
        return Err("this binary does not say what it was built from (OA_BUILD_COMMIT unset at build time; build with tools/p002_build.sh)".into());
    };
    if built != frozen {
        return Err(format!(
            "the binary was built from {built}, not the frozen commit {frozen}"
        ));
    }
    if build.dirty != Some("0") {
        return Err("the binary was built from a tree that was not clean".into());
    }
    if git.head != frozen {
        return Err(format!(
            "the working tree is at {}, not the frozen commit {frozen}",
            git.head
        ));
    }
    if git_is_dirty(git) {
        return Err(format!(
            "the working tree is not clean ({})",
            git_tree_text(git)
        ));
    }
    Ok(())
}

fn git_is_dirty(g: &GitState) -> bool {
    tree_is_dirty(g.porcelain.as_deref())
}

fn git_tree_text(g: &GitState) -> String {
    match g.porcelain.as_deref() {
        None => "git status unavailable".to_string(),
        Some(p) => {
            let n = p.lines().filter(|l| !l.trim().is_empty()).count();
            if n == 0 {
                "clean".to_string()
            } else {
                format!("{n} paths modified or untracked")
            }
        }
    }
}

/// Whether a tree is unfit for hold-out scoring: any line of `git status --porcelain`, or no
/// answer from git at all.
fn tree_is_dirty(porcelain: Option<&str>) -> bool {
    porcelain.is_none_or(|p| p.lines().any(|l| !l.trim().is_empty()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cross,
    AlongRaw,
    AlongFit,
    HFit,
    AlongTol,
    HTol,
    Speed,
    Track,
}

impl Kind {
    fn get(self, v: &Val) -> f64 {
        match self {
            Kind::Cross => v.pt.cross.abs(),
            Kind::AlongRaw => v.pt.along.abs(),
            Kind::AlongFit => v.along_fit.abs(),
            Kind::HFit => v.h_fit,
            Kind::AlongTol => v.along_tol.abs(),
            Kind::HTol => v.h_tol,
            Kind::Speed => v.pt.spd.abs(),
            Kind::Track => v.pt.hdg.abs(),
        }
    }
}

/// Which systems a common set needs a value of (P-006 section 4).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    /// OpenATCA and ERAM: the set the claims are made on.
    Two,
    /// OpenATCA, ERAM and passthrough: used for the reference-floor test only.
    Three,
}

impl Set {
    fn has(self, system: usize) -> bool {
        system != PT || self == Set::Three
    }

    /// Whether every system of the set has a value at the row.
    fn present(self, r: &Row) -> bool {
        match self {
            Set::Two => r.two_way(),
            Set::Three => r.common(),
        }
    }
}

/// Whether the row is in the common set `set` for `k`: every system of the set has a finite value.
fn common_for(r: &Row, k: Kind, set: Set) -> bool {
    r.val
        .iter()
        .enumerate()
        .filter(|(s, _)| set.has(*s))
        .all(|(_, v)| v.as_ref().is_some_and(|v| k.get(v).is_finite()))
}

// ---------------------------------------------------------------------------------------------
// Bootstrap over aircraft.
// ---------------------------------------------------------------------------------------------

/// Values per aircraft, concatenated: aircraft `i` owns `data[start[i]..start[i + 1]]`.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct Col {
    start: Vec<usize>,
    data: Vec<f64>,
}

fn build_col(rows: &[Row], n_ac: usize, f: &dyn Fn(&Row) -> Option<f64>) -> Col {
    let mut cnt = vec![0usize; n_ac];
    let mut data = Vec::new();
    for r in rows {
        if let Some(x) = f(r) {
            data.push(x);
            if let Some(c) = cnt.get_mut(r.ac) {
                *c += 1;
            }
        }
    }
    let mut start = Vec::with_capacity(n_ac + 1);
    let mut s = 0;
    start.push(0);
    for c in cnt {
        s += c;
        start.push(s);
    }
    Col { start, data }
}

impl Col {
    fn slice(&self, i: usize) -> &[f64] {
        match (self.start.get(i), self.start.get(i + 1)) {
            (Some(&a), Some(&b)) => self.data.get(a..b).unwrap_or(&[]),
            _ => &[],
        }
    }

    fn point(&self) -> [f64; 3] {
        let mut b = self.data.clone();
        stat3(&mut b)
    }

    fn boot(&self, draws: &[Vec<u32>]) -> Vec<[f64; 3]> {
        let mut buf = Vec::with_capacity(self.data.len() + 16);
        draws
            .iter()
            .map(|d| {
                buf.clear();
                for &i in d {
                    buf.extend_from_slice(self.slice(i as usize));
                }
                stat3(&mut buf)
            })
            .collect()
    }
}

/// Per-aircraft counts of a share: `num` successes of `den` samples.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct Cnt {
    num: Vec<u64>,
    den: Vec<u64>,
}

impl Cnt {
    fn new(n_ac: usize) -> Cnt {
        Cnt {
            num: vec![0; n_ac],
            den: vec![0; n_ac],
        }
    }

    fn add(&mut self, ac: usize, ok: bool) {
        if let (Some(n), Some(d)) = (self.num.get_mut(ac), self.den.get_mut(ac)) {
            *n += u64::from(ok);
            *d += 1;
        }
    }

    fn point(&self) -> (u64, u64) {
        (self.num.iter().sum(), self.den.iter().sum())
    }

    /// The share per resample (NaN for an empty resample).
    fn boot(&self, draws: &[Vec<u32>]) -> Vec<f64> {
        draws
            .iter()
            .map(|d| {
                let (mut n, mut m) = (0u64, 0u64);
                for &i in d {
                    n += self.num.get(i as usize).copied().unwrap_or(0);
                    m += self.den.get(i as usize).copied().unwrap_or(0);
                }
                if m == 0 {
                    f64::NAN
                } else {
                    n as f64 / m as f64
                }
            })
            .collect()
    }
}

/// The 95% interval of resampled values: ranks 25 and 975 of 1,000 sorted values (the 2.5th and
/// 97.5th nearest-rank percentiles); NaN resamples are skipped.
fn interval(v: impl Iterator<Item = f64>) -> (f64, f64) {
    let mut s: Vec<f64> = v.filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let n = s.len() as u64;
    let lo = ((25 * n).div_ceil(1000)).clamp(1, n) as usize - 1;
    let hi = ((975 * n).div_ceil(1000)).clamp(1, n) as usize - 1;
    (
        s.get(lo).copied().unwrap_or(f64::NAN),
        s.get(hi).copied().unwrap_or(f64::NAN),
    )
}

fn draws_of(n_ac: usize) -> Vec<Vec<u32>> {
    let mut rng = SplitMix(BOOT_SEED);
    (0..BOOT_N)
        .map(|_| {
            (0..n_ac)
                .map(|_| rng.below(n_ac as u64) as u32)
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// One metric over the three systems: values, and the resampled statistics.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct Metric {
    n: [usize; 3],
    /// Samples present in every system of the set and kept by the group, left out of the common
    /// set because this system's value is not finite (E6): reported apart from "not common".
    nonfinite: [usize; 3],
    pt: [[f64; 3]; 3],
    boot: [Vec<[f64; 3]>; 3],
}

fn metric(
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
    keep: &dyn Fn(&Row) -> bool,
    set: Set,
) -> Metric {
    let col = |s: usize| {
        build_col(rows, n_ac, &|r: &Row| {
            if set.has(s) && keep(r) && common_for(r, k, set) {
                r.val.get(s).and_then(|v| v.as_ref()).map(|v| k.get(v))
            } else {
                None
            }
        })
    };
    let cols = [col(OA), col(ER), col(PT)];
    let mut nonfinite = [0usize; 3];
    for r in rows
        .iter()
        .filter(|r| keep(r) && set.present(r) && !common_for(r, k, set))
    {
        for (s, (n, v)) in nonfinite.iter_mut().zip(&r.val).enumerate() {
            if set.has(s) && v.as_ref().is_some_and(|v| !k.get(v).is_finite()) {
                *n += 1;
            }
        }
    }
    Metric {
        nonfinite,
        n: [cols[0].data.len(), cols[1].data.len(), cols[2].data.len()],
        pt: [cols[0].point(), cols[1].point(), cols[2].point()],
        boot: [
            cols[0].boot(draws),
            cols[1].boot(draws),
            cols[2].boot(draws),
        ],
    }
}

fn fmt_ci(pt: f64, lo: f64, hi: f64, prec: usize) -> String {
    format!("{pt:.prec$} [{lo:.prec$}, {hi:.prec$}]")
}

/// Point value of statistic `k` of system `s`.
fn pv(m: &Metric, s: usize, k: usize) -> f64 {
    m.pt.get(s)
        .and_then(|a| a.get(k))
        .copied()
        .unwrap_or(f64::NAN)
}

/// Resampled statistics of system `s`.
fn bv(m: &Metric, s: usize) -> &[[f64; 3]] {
    m.boot.get(s).map_or(&[], Vec::as_slice)
}

/// `value [lo, hi]` of statistic `k` of system `s`.
fn cell(m: &Metric, s: usize, k: usize, prec: usize) -> String {
    let (lo, hi) = interval(bv(m, s).iter().filter_map(|b| b.get(k).copied()));
    fmt_ci(pv(m, s, k), lo, hi, prec)
}

/// OpenATCA minus ERAM, statistic `k`: the point difference and the interval of the paired
/// resamples.
fn diff_of(m: &Metric, k: usize) -> (f64, f64, f64) {
    let d = pv(m, OA, k) - pv(m, ER, k);
    let (lo, hi) = interval(
        bv(m, OA)
            .iter()
            .zip(bv(m, ER))
            .filter_map(|(a, b)| Some(a.get(k).copied()? - b.get(k).copied()?)),
    );
    (d, lo, hi)
}

fn diff_cell(m: &Metric, k: usize, prec: usize) -> String {
    let (d, lo, hi) = diff_of(m, k);
    fmt_ci(d, lo, hi, prec)
}

/// The passthrough rule for one value: does the passthrough baseline come within 10 % of
/// OpenATCA's?
fn floor_note(oa: f64, pt: f64) -> String {
    if !(oa.is_finite() && pt.is_finite()) {
        return "undefined (no value)".to_string();
    }
    let rel = if oa != 0.0 {
        (pt - oa).abs() / oa.abs()
    } else if pt == 0.0 {
        0.0
    } else {
        f64::INFINITY
    };
    if rel <= FLOOR_SHARE {
        format!(
            "APPLIES: at the reference floor ({:.1} % apart)",
            rel * 100.0
        )
    } else {
        format!("does not apply ({:.1} % apart)", rel * 100.0)
    }
}

/// One metric: the two-way table `m` (OpenATCA and ERAM, with intervals and the difference) and,
/// for the reference-floor test only, the three-way values `floor`. `primary` is the statistic
/// that is a primary metric of P-006 section 3, when one is.
fn write_metric(
    o: &mut String,
    title: &str,
    unit: &str,
    m: &Metric,
    floor: &Metric,
    primary: Option<usize>,
) {
    let _ = writeln!(
        o,
        "{title} ({unit}; two-way common set, OpenATCA and ERAM both have a value, n={}; absolute values for median and p95)",
        m.n.first().copied().unwrap_or(0)
    );
    for (k, name) in STATS.iter().enumerate() {
        let _ = writeln!(
            o,
            "  {name}{}:",
            if primary == Some(k) {
                " (PRIMARY metric, P-006 section 3)"
            } else {
                ""
            }
        );
        for (s, sys) in SYS.iter().enumerate().filter(|(s, _)| *s != PT) {
            let _ = writeln!(
                o,
                "    {sys:<12} {}   n={}",
                cell(m, s, k, 3),
                m.n.get(s).copied().unwrap_or(0)
            );
        }
        let _ = writeln!(o, "    OpenATCA minus ERAM  {}", diff_cell(m, k, 3));
        let _ = writeln!(
            o,
            "    reference floor, three-way set (n={}): OpenATCA {:.3}, Passthrough {:.3}; passthrough within 10%: {}",
            floor.n.first().copied().unwrap_or(0),
            pv(floor, OA, k),
            pv(floor, PT, k),
            floor_note(pv(floor, OA, k), pv(floor, PT, k))
        );
    }
    let _ = writeln!(o, "  {}", nonfinite_text(m));
    let _ = writeln!(o);
}

/// The samples a metric left out for a value that is not finite, per system, apart from the
/// samples that were simply not in all three systems.
fn nonfinite_text(m: &Metric) -> String {
    format!(
        "left out of the common set for a non-finite value (present in every system of the set): OpenATCA {}, ERAM {}, Passthrough {}",
        m.nonfinite[0], m.nonfinite[1], m.nonfinite[2]
    )
}

/// A breakdown: one block per stratum, medians with intervals and the difference, p95 and RMS as
/// point values.
fn write_strata(
    o: &mut String,
    title: &str,
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
) {
    let _ = writeln!(
        o,
        "{title}: breakdown (two-way common set; median with 95% interval, p95 and rms as points)"
    );
    let mut strata: Vec<(String, Keep)> = Vec::new();
    for (i, name) in phase::NAMES.iter().enumerate() {
        strata.push((
            format!("phase={name}"),
            Box::new(move |r: &Row| usize::from(r.phase) == i),
        ));
    }
    for (i, name) in band::NAMES.iter().enumerate() {
        strata.push((
            format!("altitude={name}"),
            Box::new(move |r: &Row| usize::from(r.band) == i),
        ));
    }
    strata.push((
        "OpenATCA not coasting".to_string(),
        Box::new(|r: &Row| {
            r.val
                .first()
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.pt.coast == 0)
        }),
    ));
    strata.push((
        "OpenATCA coasting".to_string(),
        Box::new(|r: &Row| {
            r.val
                .first()
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.pt.coast != 0)
        }),
    ));
    write_groups(o, rows, n_ac, draws, k, &strata);
}

type Keep = Box<dyn Fn(&Row) -> bool>;

/// The flag groups of P-003: OpenATCA's coasting, uncertain and reacquired rows against the rest,
/// each pair on the same common set and resamples as the other tables.
fn flag_groups() -> Vec<(String, Keep)> {
    let oa = |r: &Row| r.val.first().and_then(|v| v.as_ref()).map(|v| v.pt);
    let mut g: Vec<(String, Keep)> = Vec::new();
    type Flag = fn(&Pt) -> Tri;
    let flags: [(&str, Flag); 3] = [
        ("coasting", |p| tri_of(p.coast != 0)),
        ("uncertain", |p| p.uncertain),
        ("reacquired", |p| p.reacquired),
    ];
    for (name, f) in flags {
        g.push((
            format!("OpenATCA not {name}"),
            Box::new(move |r: &Row| oa(r).is_some_and(|p| f(&p) == Tri::No)),
        ));
        g.push((
            format!("OpenATCA {name}"),
            Box::new(move |r: &Row| oa(r).is_some_and(|p| f(&p) == Tri::Yes)),
        ));
        // A sample with no tracker row to read the flag from is in neither group above.
        g.push((
            format!("OpenATCA {name} UNMATCHED"),
            Box::new(move |r: &Row| oa(r).is_some_and(|p| f(&p) == Tri::Unmatched)),
        ));
    }
    g
}

/// P-003 breakdown by the tracker's flags for one metric.
fn write_flag_groups(
    o: &mut String,
    title: &str,
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
) {
    let _ = writeln!(
        o,
        "{title}: breakdown by OpenATCA row flag (two-way common set; groups by OpenATCA's flag on the grid sample; median with 95% interval, p95 and rms as points)"
    );
    write_groups(o, rows, n_ac, draws, k, &flag_groups());
}

fn write_groups(
    o: &mut String,
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
    strata: &[(String, Keep)],
) {
    for (name, keep) in strata {
        let m = metric(rows, n_ac, draws, k, keep.as_ref(), Set::Two);
        let n = m.n.first().copied().unwrap_or(0);
        if n == 0 {
            let _ = writeln!(o, "  {name:<28} n=0 (no common samples)");
            continue;
        }
        let _ = writeln!(o, "  {name:<28} n={n}");
        if m.nonfinite.iter().any(|&x| x > 0) {
            let _ = writeln!(o, "      {}", nonfinite_text(&m));
        }
        let _ = writeln!(
            o,
            "      median  OpenATCA {}  ERAM {}  OpenATCA minus ERAM {}",
            cell(&m, OA, 0, 2),
            cell(&m, ER, 0, 2),
            diff_cell(&m, 0, 2)
        );
        for k2 in 1..3 {
            let _ = writeln!(
                o,
                "      {:<7} OpenATCA {:.2}  ERAM {:.2}",
                STATS.get(k2).copied().unwrap_or(""),
                pv(&m, OA, k2),
                pv(&m, ER, k2)
            );
        }
    }
    let _ = writeln!(o);
}

// ---------------------------------------------------------------------------------------------
// The run.
// ---------------------------------------------------------------------------------------------

/// A time window excluded from the grid for every system alike (P-007 section 2), in Unix ns.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
struct Window {
    from_ns: i64,
    to_ns: i64,
    /// The times as given.
    from: String,
    to: String,
}

/// P-010's frozen development-only public comparison window.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
struct P010Window {
    warmup_from_ns: i64,
    score_from_ns: i64,
    score_to_ns: i64,
    box_profile: &'static str,
}

fn parse_p010_window(
    warmup_from: Option<&str>,
    score_from: Option<&str>,
    score_to: Option<&str>,
    box_profile: Option<&str>,
) -> Result<Option<P010Window>, String> {
    let supplied = [warmup_from, score_from, score_to, box_profile]
        .iter()
        .filter(|v| v.is_some())
        .count();
    if supplied == 0 {
        return Ok(None);
    }
    if supplied != 4 {
        return Err("P-010 requires --warmup-from, --score-from, --score-to, and --zau-box-profile together".into());
    }
    if warmup_from != Some("2026-11-30T05:00:00Z")
        || score_from != Some("2026-11-30T06:00:00Z")
        || score_to != Some("2026-12-01T00:00:00Z")
    {
        return Err("P-010 timestamp arguments must use the exact canonical UTC strings declared in the protocol".into());
    }
    let (warmup_from_ns, score_from_ns, score_to_ns) = (
        parse_iso_ns(warmup_from.unwrap_or_default())?,
        parse_iso_ns(score_from.unwrap_or_default())?,
        parse_iso_ns(score_to.unwrap_or_default())?,
    );
    let expected = (
        parse_iso_ns("2026-11-30T05:00:00Z")?,
        parse_iso_ns("2026-11-30T06:00:00Z")?,
        parse_iso_ns("2026-12-01T00:00:00Z")?,
    );
    if (warmup_from_ns, score_from_ns, score_to_ns) != expected {
        return Err("P-010 window must be [2026-11-30T05:00:00Z warm-up, 2026-11-30T06:00:00Z score start, 2026-12-01T00:00:00Z score end)".into());
    }
    if box_profile != Some(P010_BOX_PROFILE) {
        return Err(format!(
            "P-010 accepts only --zau-box-profile {P010_BOX_PROFILE}"
        ));
    }
    Ok(Some(P010Window {
        warmup_from_ns,
        score_from_ns,
        score_to_ns,
        box_profile: P010_BOX_PROFILE,
    }))
}

fn p010_clip_time_in_day(time_ns: i64) -> bool {
    let from = days_from_civil(2026, 11, 30) * 86_400 * 1_000_000_000;
    let to = from + 86_400 * 1_000_000_000;
    time_ns >= from && time_ns < to
}

#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
struct P010EligibilityCounts {
    address_minute_opportunities: u64,
    no_active_leg_minutes: u64,
    eligible_address_minutes: u64,
    ambiguous_candidates: u64,
    unusable_truth_minutes: u64,
    no_truth_minutes: u64,
}

type P010EligibleMinutes = BTreeMap<(u32, i64), u32>;

fn p010_truth_eligible(sample: &TruthSample) -> bool {
    let speed = oa_math::hypot(sample.vel_east_mps, sample.vel_north_mps);
    sample.lat.is_finite()
        && sample.lon.is_finite()
        && (f64::from(ZAU_BOX.lat_min_e7) / 10_000_000.0
            ..=f64::from(ZAU_BOX.lat_max_e7) / 10_000_000.0)
            .contains(&sample.lat)
        && (f64::from(ZAU_BOX.lon_min_e7) / 10_000_000.0
            ..=f64::from(ZAU_BOX.lon_max_e7) / 10_000_000.0)
            .contains(&sample.lon)
        && sample.vel_east_mps.is_finite()
        && sample.vel_north_mps.is_finite()
        && speed.is_finite()
        && speed >= P010_MIN_DIRECTION_SPEED_MPS
}

fn p010_public_eligibility(
    truth: &Truth<'_>,
    window: P010Window,
) -> Result<(P010EligibleMinutes, P010EligibilityCounts), String> {
    if truth.legs.len() > P010_MAX_TRUTH_LEGS {
        return Err("P010_REFUSED_TRUTH_LEG_CAP".into());
    }
    let mut leg_visits = 0u64;
    let mut by_addr: BTreeMap<u32, Vec<(usize, i64, i64)>> = BTreeMap::new();
    for (ix, leg) in truth.legs.iter().enumerate() {
        if !by_addr.contains_key(&leg.addr) && by_addr.len() >= P010_MAX_ADDRESSES {
            return Err(format!(
                "P010_REFUSED_ADDRESS_CAP observed_at_least={} cap={P010_MAX_ADDRESSES}",
                P010_MAX_ADDRESSES + 1
            ));
        }
        by_addr
            .entry(leg.addr)
            .or_default()
            .push((ix, leg.start_ms, leg.end_ms));
    }
    let mut eligible = BTreeMap::new();
    let mut counts = P010EligibilityCounts::default();
    let start_ms = window.score_from_ns / 1_000_000;
    let end_ms = window.score_to_ns / 1_000_000;
    let mut minute_ms = start_ms;
    while minute_ms < end_ms {
        for (&addr, legs) in &by_addr {
            // At most 100,000 addresses x 1,080 minutes: this count cannot overflow.
            counts.address_minute_opportunities += 1;
            let mut active_leg = None;
            let mut active_count = 0usize;
            for &(ix, from, to) in legs {
                if leg_visits >= P010_MAX_ELIGIBILITY_LEG_VISITS {
                    return Err("P010_REFUSED_ELIGIBILITY_WORK_CAP".into());
                }
                leg_visits += 1;
                if from <= minute_ms && minute_ms <= to {
                    active_count += 1;
                    active_leg = Some(ix);
                }
            }
            match (active_count, active_leg) {
                (1, Some(leg_ix)) => match (truth.query)(addr, minute_ms) {
                    Ok(sample) if sample.leg as usize == leg_ix && p010_truth_eligible(&sample) => {
                        if eligible.len() >= P010_MAX_ELIGIBLE_ADDRESS_MINUTES {
                            return Err(format!("P010_REFUSED_ELIGIBLE_ADDRESS_MINUTE_CAP observed_at_least={} cap={P010_MAX_ELIGIBLE_ADDRESS_MINUTES}", P010_MAX_ELIGIBLE_ADDRESS_MINUTES + 1));
                        }
                        eligible.insert((addr, minute_ms), sample.leg);
                        counts.eligible_address_minutes += 1;
                    }
                    Ok(_) => counts.unusable_truth_minutes += 1,
                    Err(_) => counts.no_truth_minutes += 1,
                },
                (0, _) => counts.no_active_leg_minutes += 1,
                _ => counts.ambiguous_candidates += 1,
            }
        }
        minute_ms += 60_000;
    }
    // Partition every enumerated address-minute, including those with no active truth leg.
    if counts.address_minute_opportunities
        != counts.no_active_leg_minutes
            + counts.eligible_address_minutes
            + counts.ambiguous_candidates
            + counts.unusable_truth_minutes
            + counts.no_truth_minutes
    {
        return Err("P010_REFUSED_ELIGIBILITY_COUNT_MISMATCH".into());
    }
    Ok((eligible, counts))
}

fn p010_keyset_sha256<I>(domain: &[u8], keys: I) -> Result<String, String>
where
    I: IntoIterator<Item = (u32, i64, Option<u32>)>,
{
    let mut hasher = Sha256::new();
    hasher
        .update(domain)
        .map_err(|e| format!("P-010 cohort hash domain: {e}"))?;
    for (addr, minute_ms, leg_ix) in keys {
        let leg_ix = leg_ix
            .ok_or_else(|| "P-010 available cohort key lacks its eligible truth leg".to_string())?;
        hasher
            .update(&addr.to_be_bytes())
            .map_err(|e| format!("P-010 cohort hash address: {e}"))?;
        hasher
            .update(&minute_ms.to_be_bytes())
            .map_err(|e| format!("P-010 cohort hash minute: {e}"))?;
        hasher
            .update(&leg_ix.to_be_bytes())
            .map_err(|e| format!("P-010 cohort hash truth leg: {e}"))?;
    }
    Ok(to_hex(&hasher.finalize()))
}

fn p010_keeps_time(t_ms: i64, window: P010Window) -> bool {
    t_ms >= window.score_from_ns.div_euclid(1_000_000)
        && t_ms < window.score_to_ns.div_euclid(1_000_000)
}

fn p010_keeps_eligible_time(
    eligible: &BTreeMap<(u32, i64), u32>,
    truth: &Truth<'_>,
    addr: u32,
    t_ms: i64,
    window: P010Window,
) -> bool {
    p010_keeps_time(t_ms, window)
        && eligible
            .get(&(addr, t_ms.div_euclid(60_000) * 60_000))
            .is_some_and(|&leg| (truth.query)(addr, t_ms).is_ok_and(|sample| sample.leg == leg))
}

fn filter_times_by_eligible(
    times: &mut TimesBy,
    eligible: &BTreeMap<(u32, i64), u32>,
    truth: &Truth<'_>,
    window: P010Window,
) {
    times.retain(|&addr, values| {
        values.retain(|&t| p010_keeps_eligible_time(eligible, truth, addr, t, window));
        !values.is_empty()
    });
}

fn p010_preflight_sfdps_bytes(path: &Path) -> Result<u64, String> {
    let (_, files) = list_inputs(path).map_err(|e| format!("P010 SFDPS input listing: {e}"))?;
    let mut total = 0u64;
    for file in files {
        let (_input, bytes) = oa_sealed::open_regular(&file)
            .map_err(|e| format!("P010 SFDPS input {} refused: {e}", file.display()))?;
        total = total
            .checked_add(bytes)
            .ok_or("P010 SFDPS byte count overflow")?;
        if total > P010_MAX_SFDPS_BYTES {
            return Err(format!(
                "P010_REFUSED_SFDPS_BYTE_CAP actual={total} cap={P010_MAX_SFDPS_BYTES}"
            ));
        }
    }
    Ok(total)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDTHH:MM:SS[.n{1,9}]Z` as Unix nanoseconds.
fn parse_iso_ns(t: &str) -> Result<i64, String> {
    let bad = || format!("`{t}` is not YYYY-MM-DDTHH:MM:SS[.nnnnnnnnn]Z");
    let body = t.strip_suffix('Z').ok_or_else(bad)?;
    let (main, frac) = match body.split_once('.') {
        Some((m, f)) => (m, f),
        None => (body, ""),
    };
    if frac.len() > 9 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let (date, time) = main.split_once('T').ok_or_else(bad)?;
    let num = |x: &str, w: usize| -> Result<i64, String> {
        if x.len() != w || !x.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        x.parse::<i64>().map_err(|_| bad())
    };
    let dp: Vec<&str> = date.split('-').collect();
    let tp: Vec<&str> = time.split(':').collect();
    let ([y, mo, d], [h, mi, se]) = (dp.as_slice(), tp.as_slice()) else {
        return Err(bad());
    };
    let (y, mo, d) = (num(y, 4)?, num(mo, 2)?, num(d, 2)?);
    let (h, mi, se) = (num(h, 2)?, num(mi, 2)?, num(se, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 59 {
        return Err(bad());
    }
    let fr = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<9}").parse::<i64>().map_err(|_| bad())?
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se;
    Ok(secs * 1_000_000_000 + fr)
}

impl Window {
    fn parse(from: &str, to: &str) -> Result<Window, String> {
        let (f, t) = (parse_iso_ns(from)?, parse_iso_ns(to)?);
        if t < f {
            return Err(format!("the window {from} to {to} ends before it starts"));
        }
        Ok(Window {
            from_ns: f,
            to_ns: t,
            from: from.to_string(),
            to: to.to_string(),
        })
    }

    /// Whether the sample time `t_ms` is in the window (both ends included).
    fn holds_ms(&self, t_ms: i64) -> bool {
        let t = t_ms.saturating_mul(1_000_000);
        t >= self.from_ns && t <= self.to_ns
    }
}

/// The grid without the samples inside any window, and per window the number of grid samples it
/// holds (a sample in two windows is counted in both, removed once).
fn exclude_windows(grid: &PerAc, windows: &[Window]) -> (PerAc, Vec<u64>, u64) {
    let mut counts = vec![0u64; windows.len()];
    let mut removed = 0u64;
    let mut out: PerAc = BTreeMap::new();
    for (&a, v) in grid {
        let mut keep = Vec::with_capacity(v.len());
        for s in v {
            let mut hit = false;
            for (w, c) in windows.iter().zip(counts.iter_mut()) {
                if w.holds_ms(s.t) {
                    *c += 1;
                    hit = true;
                }
            }
            if hit {
                removed += 1;
            } else {
                keep.push(*s);
            }
        }
        // An aircraft left with no sample is no aircraft of the grid (it takes no bootstrap slot).
        if !keep.is_empty() {
            out.insert(a, keep);
        }
    }
    (out, counts, removed)
}

/// The scored legs and the grid of a run. The legs are those of the grid as it was before any
/// window (P-004 section 1: the legs that contain the aircraft's grid samples), so an excluded
/// window changes which samples are scored and never which legs they lie on. Also the number of
/// samples each window holds, and the number removed.
fn plan_grid(
    er_pts: &PerAc,
    legs: &LegIndex,
    windows: &[Window],
) -> (ScoredLegs, PerAc, Vec<u64>, u64) {
    let scored = scored_legs(er_pts, legs);
    let (grid, counts, removed) = exclude_windows(er_pts, windows);
    (scored, grid, counts, removed)
}

/// `git -C dir args`: its standard output, `None` when git failed or could not run.
fn git_run(dir: &Path, args: &[&str]) -> Option<String> {
    // seal-allow: git describes this source tree for the report; it reads no input data
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

fn git_state() -> GitState {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    GitState {
        head: git_run(dir, &["rev-parse", "HEAD"])
            .map_or("unknown".into(), |s| s.trim().to_string()),
        porcelain: git_run(dir, &["status", "--porcelain"]),
    }
}

/// The one file U may differ from the frozen candidate C in (P-006 section 5).
const SEALED_LIST_PATH: &str = "crates/oa-sealed/sealed_paths.txt";

/// The frozen candidate C and the scored commit U of a run (P-006 section 5).
// A7.2: cold struct, size guard waived
struct Commits {
    /// `--frozen-commit`: the scored commit U.
    frozen: Option<String>,
    /// `--candidate-commit`: the frozen candidate C.
    candidate: Option<String>,
}

/// P-006 section 5: `--candidate-commit` is needed with `--frozen-commit` (so in every hold-out
/// run) and the other way round, both are full commit ids, and `git diff --name-only C U` run in
/// `repo` lists exactly the sealed list and nothing else.
fn candidate_binding(mode: Mode, c: &Commits, repo: &Path) -> Result<(), String> {
    let (frozen, cand) = match (c.frozen.as_deref(), c.candidate.as_deref()) {
        (None, None) if !mode.holdout => return Ok(()),
        (Some(f), Some(c)) => (f, c),
        _ => {
            return Err(
                "--candidate-commit <40 hex> is required together with --frozen-commit (both, in a hold-out run)"
                    .into(),
            )
        }
    };
    for (name, v) in [("--frozen-commit", frozen), ("--candidate-commit", cand)] {
        if !is_full_sha(v) {
            return Err(format!("{name} must be 40 lowercase hex digits, got `{v}`"));
        }
    }
    let Some(names) = git_run(repo, &["diff", "--name-only", cand, frozen]) else {
        return Err(format!(
            "git diff --name-only {cand} {frozen} failed (is either commit missing from this repository?)"
        ));
    };
    let files: Vec<&str> = names.lines().filter(|l| !l.trim().is_empty()).collect();
    if files != [SEALED_LIST_PATH] {
        return Err(format!(
            "the scored commit {frozen} must differ from the candidate {cand} in {SEALED_LIST_PATH} and nothing else; git diff --name-only lists {} file(s): {}",
            files.len(),
            files.join(", ")
        ));
    }
    Ok(())
}

/// The names of the two files a run writes.
const OUTPUT_FILES: [&str; 2] = ["samples.csv", "p002.txt"];

/// The suffix of a file being written; it becomes the final name only when both are complete.
const PARTIAL: &str = ".partial";

/// P-006 section 7: refuses when either output file already exists (nothing is written then).
/// A leftover `.partial` does not refuse; it is returned so the run can report it.
fn outputs_free(out_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut left = Vec::new();
    for f in OUTPUT_FILES {
        let p = out_dir.join(f);
        if p.exists() || p.is_symlink() {
            return Err(format!(
                "{} exists; a run writes its output files only if they do not exist, and writes nothing otherwise",
                p.display()
            ));
        }
        let part = out_dir.join(format!("{f}{PARTIAL}"));
        if part.exists() || part.is_symlink() {
            left.push(part);
        }
    }
    Ok(left)
}

/// Writes `text` to `path` (a `.partial` name: a leftover of an earlier run is replaced), and
/// flushes it to disk.
fn write_partial(path: &Path, text: &str) -> Result<(), String> {
    // seal-allow: the scorer's own output file, write-only, a partial name
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.sync_all().map_err(|e| format!("{}: {e}", path.display()))
}

/// Creates an export temporary only after the seal check; `create_new` refuses stale entries and
/// symlinks atomically. Export temps are never overwritten or followed.
fn write_new_partial(path: &Path, text: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    oa_sealed::refuse_io(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // seal-allow: guarded create-new write of the export temporary; it reads nothing
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let result = f
        .write_all(text.as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()));
    if result.is_err() {
        drop(f);
        // This call created the incomplete file.
        let _ = fs::remove_file(path);
    }
    result
}

/// Publishes an export and its manifest without replacing either destination. The temporary and
/// final files share a directory, so hard-link creation is atomic and fails if a destination exists.
fn publish_export_pair(
    part_a: &Path,
    file: &Path,
    part_b: &Path,
    side: &Path,
) -> Result<(), String> {
    for path in [part_a, part_b] {
        oa_sealed::refuse_io(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // seal-allow: metadata check of a temp file created by write_new_partial; no contents read
        let kind = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !kind.file_type().is_file() {
            return Err(format!(
                "{}: export temp is not a regular file",
                path.display()
            ));
        }
    }
    for path in [file, side] {
        oa_sealed::refuse_io(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let link = |part: &Path, dest: &Path| {
        // seal-allow: publish the guarded temporary file without replacing the guarded destination
        fs::hard_link(part, dest).map_err(|e| format!("{}: {e}", dest.display()))
    };
    link(part_a, file)?;
    if let Err(e) = link(part_b, side) {
        let _ = fs::remove_file(file);
        return Err(e);
    }
    for part in [part_a, part_b] {
        if let Err(e) = fs::remove_file(part) {
            let _ = fs::remove_file(file);
            let _ = fs::remove_file(side);
            return Err(format!("{}: {e}", part.display()));
        }
    }
    Ok(())
}

/// P-008 and P-006 section 7: both files are written to `.partial` names and flushed, then the
/// ledger line (a hold-out run's) is appended and flushed, then both are renamed. A run that
/// stops leaves no result under a final name, and a ledger line is there only when both partials
/// were written. When the ledger cannot be appended to, the partials are deleted and the run
/// fails. Neither final name may exist (checked again after the partials are written).
fn publish_outputs(
    out_dir: &Path,
    csv: &str,
    txt: &str,
    ledger: Option<(&Path, &str)>,
) -> Result<(), String> {
    outputs_free(out_dir)?;
    let part = |f: &str| out_dir.join(format!("{f}{PARTIAL}"));
    let drop_partials = || {
        for f in OUTPUT_FILES {
            let _ = fs::remove_file(part(f));
        }
    };
    let staged = write_partial(&part("samples.csv"), csv)
        .and_then(|()| write_partial(&part("p002.txt"), txt))
        .and_then(|()| outputs_free(out_dir).map(|_| ()))
        .and_then(|()| match ledger {
            Some((l, line)) => ledger_append(l, line),
            None => Ok(()),
        });
    if let Err(e) = staged {
        drop_partials();
        return Err(e);
    }
    for f in OUTPUT_FILES {
        fs::rename(part(f), out_dir.join(f)).map_err(|e| format!("{f}: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The ledger (P-008 section 5).
// ---------------------------------------------------------------------------------------------

/// Whether the ledger already holds `clip_sha` (the first field of a line). A ledger that does
/// not exist yet is empty.
fn ledger_has(ledger: &Path, clip_sha: &str) -> Result<bool, String> {
    match oa_sealed::read_to_string(ledger) {
        Ok(t) => Ok(t
            .lines()
            .any(|l| l.split_whitespace().next() == Some(clip_sha))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("ledger {}: {e}", ledger.display())),
    }
}

/// Appends one line to the ledger (O_APPEND, created if missing) and flushes it.
fn ledger_append(ledger: &Path, line: &str) -> Result<(), String> {
    // seal-allow: the append-only ledger of scored days, write-only
    let mut f = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(ledger)
        .map_err(|e| format!("ledger {}: {e}", ledger.display()))?;
    f.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("ledger {}: {e}", ledger.display()))?;
    f.sync_all()
        .map_err(|e| format!("ledger {}: {e}", ledger.display()))
}

/// The date of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
fn iso_utc(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let r = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        r / 3600,
        r % 3600 / 60,
        r % 60
    )
}

/// The ledger line of a hold-out run: clip SHA-256, scored commit, candidate commit, UTC time.
fn ledger_line(clip_sha: &str, c: &Commits, now_secs: i64) -> String {
    format!(
        "{clip_sha} {} {} {}",
        c.frozen.as_deref().unwrap_or("none"),
        c.candidate.as_deref().unwrap_or("none"),
        iso_utc(now_secs)
    )
}

// ---------------------------------------------------------------------------------------------
// The damage report (P-008 section 2).
// ---------------------------------------------------------------------------------------------

/// No excluded window may be shorter than this (P-007), ns.
const MIN_WINDOW_NS: i64 = 600_000_000_000;
/// The excluded windows of a hold-out day may total at most this (P-007: 60 minutes), ns.
const MAX_WINDOWS_NS: i64 = 3_600_000_000_000;

/// What a run takes from the report of `tools/oa-feed-gaps`.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq, Eq)]
struct DamageReport {
    sha256: String,
    /// The report's window, Unix ns.
    from_ns: i64,
    to_ns: i64,
    windows: Vec<Window>,
    total_ns: i64,
}

/// Reads an `oa-feed-gaps` report. The lines read, and no others, are:
/// `window: <iso> to <iso> (<seconds> s)` (exactly one), `exclude <iso> <iso>` (any number) and
/// the verdict, the single line `CLEAN`; a line starting `DAMAGED:` refuses. Refused: a verdict
/// that is not `CLEAN`, a window under 600 s, windows totalling over 3,600 s, and a window
/// outside the report's. That the report's window is the scored day's is checked against the
/// recordings by [`damage_matches_clip`].
fn parse_damage_report(text: &str, sha256: &str) -> Result<DamageReport, String> {
    let mut window: Option<(i64, i64)> = None;
    let mut windows: Vec<Window> = Vec::new();
    let (mut clean, mut damaged) = (0u32, 0u32);
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("window: ") {
            let mut it = rest.split_whitespace();
            let (Some(f), Some("to"), Some(t)) = (it.next(), it.next(), it.next()) else {
                return Err(format!("damage report: cannot read the window line `{l}`"));
            };
            if window.is_some() {
                return Err("damage report: two window lines".into());
            }
            window = Some((parse_iso_ns(f)?, parse_iso_ns(t)?));
        } else if let Some(rest) = l.strip_prefix("exclude ") {
            let mut it = rest.split_whitespace();
            let (Some(f), Some(t), None) = (it.next(), it.next(), it.next()) else {
                return Err(format!("damage report: cannot read the exclude line `{l}`"));
            };
            windows.push(Window::parse(f, t)?);
        } else if l == "CLEAN" {
            clean += 1;
        } else if l.starts_with("DAMAGED:") {
            damaged += 1;
        }
    }
    if damaged > 0 || clean != 1 {
        return Err(format!(
            "damage report: the verdict is not CLEAN ({clean} CLEAN lines, {damaged} DAMAGED lines); a damaged day is not scored"
        ));
    }
    let Some((from_ns, to_ns)) = window else {
        return Err("damage report: no window line".into());
    };
    let mut total = 0i64;
    for w in &windows {
        let len = w.to_ns - w.from_ns;
        if len < MIN_WINDOW_NS {
            return Err(format!(
                "damage report: the window {} to {} is under 600 s",
                w.from, w.to
            ));
        }
        if w.from_ns < from_ns || w.to_ns > to_ns {
            return Err(format!(
                "damage report: the window {} to {} lies outside the report's window",
                w.from, w.to
            ));
        }
        total += len;
    }
    if total > MAX_WINDOWS_NS {
        return Err(format!(
            "damage report: the windows total {} s, over 3,600 s",
            total / 1_000_000_000
        ));
    }
    Ok(DamageReport {
        sha256: sha256.to_string(),
        from_ns,
        to_ns,
        windows,
        total_ns: total,
    })
}

/// How far each end of the report's window may lie from the scored window S, ns (P-008).
const WINDOW_SLACK_NS: i64 = 60_000_000_000;
/// One UTC day, ns.
const DAY_NS: i64 = 86_400_000_000_000;

/// The scored window S: from the later of the UTC day's start and the first SFDPS message, to the
/// earliest of the day's end, the last SFDPS message and the last clip report. The day is the
/// clip's: both its first and its last report must fall in one UTC day.
fn scored_window(
    clip_first: i64,
    clip_last: i64,
    sfdps_first: i64,
    sfdps_last: i64,
) -> Result<(i64, i64), String> {
    let day = clip_first.div_euclid(DAY_NS) * DAY_NS;
    if clip_last >= day + DAY_NS {
        return Err(format!(
            "the clip's reports run from {} to {}: not one UTC day",
            iso_utc(clip_first.div_euclid(1_000_000_000)),
            iso_utc(clip_last.div_euclid(1_000_000_000))
        ));
    }
    Ok((
        day.max(sfdps_first),
        (day + DAY_NS).min(sfdps_last).min(clip_last),
    ))
}

/// The report's window must lie inside the clip's UTC day and match the scored window S to within
/// 60 s at each end (the day the report checked is the window being scored). Refused, both
/// windows are printed.
fn damage_matches_clip(
    d: &DamageReport,
    clip_first: i64,
    clip_last: i64,
    sfdps_first: i64,
    sfdps_last: i64,
) -> Result<(), String> {
    let (s_from, s_to) = scored_window(clip_first, clip_last, sfdps_first, sfdps_last)?;
    let day = clip_first.div_euclid(DAY_NS) * DAY_NS;
    let show = |a: i64, b: i64| {
        format!(
            "{} to {}",
            iso_utc(a.div_euclid(1_000_000_000)),
            iso_utc(b.div_euclid(1_000_000_000))
        )
    };
    if d.from_ns < day
        || d.to_ns > day + DAY_NS
        || (d.from_ns - s_from).abs() > WINDOW_SLACK_NS
        || (d.to_ns - s_to).abs() > WINDOW_SLACK_NS
    {
        return Err(format!(
            "the damage report's window ({}) is not the scored window ({}): it must lie inside the clip's UTC day and match the scored window to within 60 s at each end",
            show(d.from_ns, d.to_ns),
            show(s_from, s_to)
        ));
    }
    Ok(())
}

/// The first and last receive time of the SFDPS messages in `dir`, Unix ns (each file opened
/// through `open`, which checks the sealed list).
fn sfdps_span(dir: &Path, open: &oa_bench::input::Opener) -> Result<(i64, i64), String> {
    let (_, files) = oa_bench::input::list_inputs(dir).map_err(|e| e.to_string())?;
    let mut it = files.into_iter();
    let open = open.clone();
    let spec = oa_replay::StreamSpec {
        feed: FeedKind::Sfdps,
        open: Box::new(move || {
            let p = it.next()?;
            Some(open(&p))
        }),
    };
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    let (_, end) = oa_replay::merge_counted(vec![spec], 1 << 16, &mut |m| {
        if m.kind == oa_feed::Kind::Message {
            first = first.min(m.recv_ns);
            last = last.max(m.recv_ns);
        }
        Ok(())
    });
    end.map_err(|e| e.to_string())?;
    if first > last {
        return Err(format!("{}: no SFDPS message", dir.display()));
    }
    Ok((first, last))
}

// ---------------------------------------------------------------------------------------------
// The claim rule (P-006 section 3).
// ---------------------------------------------------------------------------------------------

/// What one day's interval of the OpenATCA-minus-ERAM difference says of a primary metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The interval excludes zero in OpenATCA's favour.
    OpenAtca,
    /// It excludes zero in ERAM's favour.
    Eram,
    /// It contains zero (or is not defined).
    Neither,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::OpenAtca => "oa",
            Verdict::Eram => "eram",
            Verdict::Neither => "none",
        }
    }

    fn parse(w: &str) -> Option<Verdict> {
        [Verdict::OpenAtca, Verdict::Eram, Verdict::Neither]
            .into_iter()
            .find(|v| v.word() == w)
    }

    fn text(self) -> &'static str {
        match self {
            Verdict::OpenAtca => "the interval excludes zero in OpenATCA's favour",
            Verdict::Eram => "the interval excludes zero in ERAM's favour",
            Verdict::Neither => "the interval does not exclude zero",
        }
    }
}

/// The primary metrics: id, whether a lower value is better, and its name.
const PRIMARY: [(&str, bool, &str); 3] = [
    ("M1_median", true, "M1 median cross-track error (m)"),
    (
        "M5_speed_median",
        true,
        "M5 ground-speed error median, absolute (m/s)",
    ),
    (
        "M4t_300",
        false,
        "M4t share of ALL grid samples within 300 m (rounding-tolerant, time-fitted)",
    ),
];

/// The verdict of an interval `[lo, hi]` of OpenATCA minus ERAM.
fn verdict_of(lo: f64, hi: f64, lower_is_better: bool) -> Verdict {
    if !(lo.is_finite() && hi.is_finite()) {
        return Verdict::Neither;
    }
    let (oa_better, eram_better) = if lower_is_better {
        (hi < 0.0, lo > 0.0)
    } else {
        (lo > 0.0, hi < 0.0)
    };
    if oa_better {
        Verdict::OpenAtca
    } else if eram_better {
        Verdict::Eram
    } else {
        Verdict::Neither
    }
}

/// One day's piece of the claim for a primary metric.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
struct ClaimPiece {
    id: String,
    diff: f64,
    lo: f64,
    hi: f64,
    verdict: Verdict,
    /// P-008 section 3: the passthrough is within 10 % of OpenATCA on the three-way set (for M4t,
    /// on the share over all grid samples), and the two values it was judged on.
    floor: bool,
    oa3: f64,
    pt3: f64,
}

/// Whether the passthrough is within 10 % of OpenATCA's value (P-002), as [`floor_note`] says.
fn floor_applies(oa: f64, pt: f64) -> bool {
    floor_note(oa, pt).starts_with("APPLIES")
}

/// What a run records of itself in its `CLAIM run` line (P-008 sections 3 to 5).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
struct RunInfo {
    holdout: bool,
    frozen: String,
    candidate: String,
    /// SHA-256 of the ADS-B clip.
    clip_sha: String,
    /// The SFDPS inputs' manifest hash (SHA-256 over the sorted `name:sha256` lines).
    sfdps_sha: String,
    replay_hash: String,
    /// SHA-256 of the `samples.csv` the run wrote.
    samples_sha: String,
    /// SHA-256 of the scorer binary.
    binary_sha: String,
    /// SHA-256 of the damage report, `none` when there was none.
    damage_sha: String,
    /// Total of the excluded windows, seconds.
    window_total_s: String,
}

/// The `CLAIM metric=` line of a piece.
fn claim_metric_line(p: &ClaimPiece) -> String {
    let better = PRIMARY
        .iter()
        .find(|(id, _, _)| *id == p.id)
        .map_or("?", |(_, lower, _)| if *lower { "lower" } else { "higher" });
    format!(
        "CLAIM metric={} better={better} diff={} lo={} hi={} verdict={} floor={} oa3={} pt3={}",
        p.id,
        p.diff,
        p.lo,
        p.hi,
        p.verdict.word(),
        u8::from(p.floor),
        p.oa3,
        p.pt3
    )
}

/// The machine-readable lines of a run, for `--combine`.
fn claim_lines(
    info: &RunInfo,
    pieces: &[ClaimPiece],
    windows: &[Window],
    removed: &[u64],
) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "CLAIM run holdout={} frozen={} candidate={} clip={} sfdps={} replay={} samples={} binary={} damage={} window_total_s={}",
        u8::from(info.holdout),
        info.frozen,
        info.candidate,
        info.clip_sha,
        info.sfdps_sha,
        info.replay_hash,
        info.samples_sha,
        info.binary_sha,
        info.damage_sha,
        info.window_total_s
    );
    for (w, n) in windows.iter().zip(removed) {
        let _ = writeln!(o, "CLAIM window from={} to={} removed={n}", w.from, w.to);
    }
    for p in pieces {
        let _ = writeln!(o, "{}", claim_metric_line(p));
    }
    o
}

/// A day's parsed `CLAIM` lines.
// A7.2: cold struct, size guard waived
struct DayClaim {
    info: RunInfo,
    pieces: Vec<ClaimPiece>,
    /// The excluded windows as `(from, to, grid samples removed)`.
    windows: Vec<(String, String, u64)>,
}

/// Reads the `CLAIM` lines of a `p002.txt`. Every primary metric must appear once, its direction
/// must be the one fixed here, and its verdict must be the one its interval gives.
fn parse_claim_lines(text: &str) -> Result<DayClaim, String> {
    let mut run: Option<RunInfo> = None;
    let mut pieces: Vec<ClaimPiece> = Vec::new();
    let mut windows: Vec<(String, String, u64)> = Vec::new();
    for line in text.lines().filter_map(|l| l.strip_prefix("CLAIM ")) {
        let kv: BTreeMap<&str, &str> = line
            .split_whitespace()
            .filter_map(|w| w.split_once('='))
            .collect();
        let get = |k: &str| {
            kv.get(k)
                .copied()
                .ok_or_else(|| format!("CLAIM line without {k}: {line}"))
        };
        if line.starts_with("run ") {
            if run.is_some() {
                return Err("two CLAIM run lines".into());
            }
            let g = |k: &str| get(k).map(str::to_string);
            run = Some(RunInfo {
                holdout: get("holdout")? == "1",
                frozen: g("frozen")?,
                candidate: g("candidate")?,
                clip_sha: g("clip")?,
                sfdps_sha: g("sfdps")?,
                replay_hash: g("replay")?,
                samples_sha: g("samples")?,
                binary_sha: g("binary")?,
                damage_sha: g("damage")?,
                window_total_s: g("window_total_s")?,
            });
        } else if line.starts_with("window ") {
            let n = get("removed")?
                .parse::<u64>()
                .map_err(|_| format!("CLAIM removed is not a count: {line}"))?;
            windows.push((get("from")?.to_string(), get("to")?.to_string(), n));
        } else {
            let num = |k: &str| -> Result<f64, String> {
                get(k)?
                    .parse::<f64>()
                    .map_err(|_| format!("CLAIM {k} is not a number: {line}"))
            };
            let id = get("metric")?.to_string();
            let Some((_, lower, _)) = PRIMARY.iter().find(|(p, _, _)| *p == id) else {
                return Err(format!("CLAIM names a metric that is not primary: {id}"));
            };
            if get("better")? != if *lower { "lower" } else { "higher" } {
                return Err(format!("CLAIM direction of {id} is not the fixed one"));
            }
            let (diff, lo, hi) = (num("diff")?, num("lo")?, num("hi")?);
            let verdict = Verdict::parse(get("verdict")?)
                .ok_or_else(|| format!("CLAIM verdict unknown: {line}"))?;
            if verdict != verdict_of(lo, hi, *lower) {
                return Err(format!(
                    "CLAIM verdict of {id} is not the one its interval [{lo}, {hi}] gives"
                ));
            }
            let floor = match get("floor")? {
                "1" => true,
                "0" => false,
                f => return Err(format!("CLAIM floor must be 0 or 1, got {f}")),
            };
            if pieces.iter().any(|p| p.id == id) {
                return Err(format!("CLAIM metric {id} appears twice"));
            }
            pieces.push(ClaimPiece {
                id,
                diff,
                lo,
                hi,
                verdict,
                floor,
                oa3: num("oa3")?,
                pt3: num("pt3")?,
            });
        }
    }
    let Some(info) = run else {
        return Err("no CLAIM run line (not a p002.txt of this scorer)".into());
    };
    for (id, _, _) in PRIMARY {
        if !pieces.iter().any(|p| p.id == id) {
            return Err(format!("no CLAIM line for the primary metric {id}"));
        }
    }
    Ok(DayClaim {
        info,
        pieces,
        windows,
    })
}

// ---------------------------------------------------------------------------------------------
// samples.csv: written, and read back to recompute the primary metrics (P-008 section 4).
// ---------------------------------------------------------------------------------------------

/// A number at full precision (it reads back exactly), empty when it is not finite.
fn fmt_full(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        String::new()
    }
}

/// Columns before the per-system ones, and per system.
const CSV_LEAD: usize = 6;
const CSV_PER_SYS: usize = 10;

/// The text of `samples.csv`: one row per grid sample.
fn samples_csv(rows: &[Row], stamps: &[&str]) -> String {
    let mut csv = String::new();
    for st in stamps {
        let _ = writeln!(csv, "# {st}");
    }
    csv.push_str("addr,t_ms,phase,band,common,common_two_way");
    for p in ["oa", "eram", "pt"] {
        let _ = write!(
            csv,
            ",{p}_miss,{p}_cross_m,{p}_along_raw_m,{p}_along_fit_m,{p}_h_fit_m,{p}_along_tol_m,{p}_h_tol_m,{p}_speed_err_mps,{p}_track_err_deg,{p}_row_dt_ms"
        );
    }
    csv.push_str(",oa_coast,oa_uncertain,oa_reacquired\n");
    for r in rows {
        let _ = write!(
            csv,
            "{:06x},{},{},{},{},{}",
            r.addr,
            r.t,
            phase::NAMES
                .get(usize::from(r.phase))
                .copied()
                .unwrap_or(""),
            band::NAMES.get(usize::from(r.band)).copied().unwrap_or(""),
            u8::from(r.common()),
            u8::from(r.two_way())
        );
        for v in &r.val {
            match v {
                None => csv.push_str(",1,,,,,,,,,"),
                Some(v) => {
                    let _ = write!(
                        csv,
                        ",0,{},{},{},{},{},{},{},{},{}",
                        fmt_full(v.pt.cross),
                        fmt_full(v.pt.along),
                        fmt_full(v.along_fit),
                        fmt_full(v.h_fit),
                        fmt_full(v.along_tol),
                        fmt_full(v.h_tol),
                        fmt_full(v.pt.spd),
                        fmt_full(v.pt.hdg),
                        v.pt.t - r.t
                    );
                }
            }
        }
        let oa = r.val.first().and_then(|v| v.as_ref()).map(|v| v.pt);
        let _ = writeln!(
            csv,
            ",{},{},{}",
            oa.map_or(String::new(), |p| p.coast.to_string()),
            oa.map_or(String::new(), |p| tri_text(p.uncertain)),
            oa.map_or(String::new(), |p| tri_text(p.reacquired))
        );
    }
    csv
}

/// The grid rows of a `samples.csv` with the columns the primary metrics read (the cross-track,
/// ground-speed and rounding-tolerant horizontal errors of each system, and which have a value),
/// and the number of aircraft: one per run of rows with one address.
fn rows_from_csv(text: &str) -> Result<(Vec<Row>, usize), String> {
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    let header = lines.next().ok_or("samples.csv is empty")?;
    if !header.starts_with("addr,t_ms,phase,band,common,common_two_way,oa_miss,") {
        return Err("samples.csv: unexpected header".into());
    }
    let num = |f: &str| -> Result<f64, String> {
        if f.is_empty() {
            Ok(f64::NAN)
        } else {
            f.parse::<f64>()
                .map_err(|_| format!("samples.csv: `{f}` is not a number"))
        }
    };
    let mut rows = Vec::new();
    let mut n_ac = 0usize;
    let mut last: Option<&str> = None;
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        if f.len() != CSV_LEAD + 3 * CSV_PER_SYS + 3 {
            return Err(format!("samples.csv: a row has {} fields", f.len()));
        }
        let at = |i: usize| f.get(i).copied().unwrap_or("");
        let addr_text = at(0);
        if last != Some(addr_text) {
            n_ac += 1;
            last = Some(addr_text);
        }
        let mut val: [Option<Val>; 3] = [None; 3];
        for (s, slot) in val.iter_mut().enumerate() {
            let c = CSV_LEAD + s * CSV_PER_SYS;
            if at(c) == "1" {
                continue;
            }
            let pt = Pt {
                t: 0,
                cross: num(at(c + 1))?,
                along: num(at(c + 2))?,
                v: f64::NAN,
                spd: num(at(c + 7))?,
                hdg: num(at(c + 8))?,
                coast: 0,
                uncertain: Tri::Unmatched,
                reacquired: Tri::Unmatched,
                phase: 0,
                band: 0,
            };
            *slot = Some(Val {
                pt,
                along_fit: num(at(c + 3))?,
                h_raw: f64::NAN,
                h_fit: num(at(c + 4))?,
                along_tol: num(at(c + 5))?,
                h_tol: num(at(c + 6))?,
            });
        }
        rows.push(Row {
            ac: n_ac.saturating_sub(1),
            t: at(1)
                .parse::<i64>()
                .map_err(|_| "samples.csv: bad time".to_string())?,
            addr: u32::from_str_radix(addr_text, 16)
                .map_err(|_| "samples.csv: bad address".to_string())?,
            phase: 0,
            band: 0,
            val,
        });
    }
    Ok((rows, n_ac))
}

/// The three primary metrics of a day (P-006 section 3) from its grid rows, with the floor test of
/// P-008 section 3. Used by the run and, on the rows read back from `samples.csv`, by `--combine`.
fn claim_pieces(rows: &[Row], n_ac: usize, draws: &[Vec<u32>]) -> Vec<ClaimPiece> {
    let all = |_: &Row| true;
    let share_point = |c: &Cnt| {
        let (n, d) = c.point();
        if d == 0 {
            f64::NAN
        } else {
            n as f64 / d as f64
        }
    };
    let m1 = metric(rows, n_ac, draws, Kind::Cross, &all, Set::Two);
    let f1 = metric(rows, n_ac, &[], Kind::Cross, &all, Set::Three);
    let m5 = metric(rows, n_ac, draws, Kind::Speed, &all, Set::Two);
    let f5 = metric(rows, n_ac, &[], Kind::Speed, &all, Set::Three);
    let (c4t, _) = m4_counts(rows, n_ac, |v| v.h_tol, 300.0);
    let (oa4, er4) = (&c4t[OA], &c4t[ER]);
    let (ba, bb) = (oa4.boot(draws), er4.boot(draws));
    let (lo4, hi4) = interval(ba.iter().zip(&bb).map(|(x, y)| x - y));
    let d4 = share_point(oa4) - share_point(er4);
    let (o4, p4) = (share_point(oa4), share_point(&c4t[PT]));
    let (d1, lo1, hi1) = diff_of(&m1, 0);
    let (d5, lo5, hi5) = diff_of(&m5, 0);
    let vals = [
        (d1, lo1, hi1, pv(&f1, OA, 0), pv(&f1, PT, 0)),
        (d5, lo5, hi5, pv(&f5, OA, 0), pv(&f5, PT, 0)),
        (d4, lo4, hi4, o4, p4),
    ];
    PRIMARY
        .iter()
        .zip(vals)
        .map(|((id, lower, _), (diff, lo, hi, oa3, pt3))| ClaimPiece {
            id: (*id).to_string(),
            diff,
            lo,
            hi,
            verdict: verdict_of(lo, hi, *lower),
            floor: floor_applies(oa3, pt3),
            oa3,
            pt3,
        })
        .collect()
}

/// One day's files for `--combine`: its `p002.txt` and `samples.csv`, as bytes read.
// A7.2: cold struct, size guard waived
struct DayFiles<'a> {
    txt: &'a str,
    csv: &'a str,
}

/// The cross-day claim rule (P-006 section 3, with P-008 sections 3 and 4): for each primary
/// metric, OpenATCA's favour on day A and on day B, day C not in ERAM's favour, and the reference
/// floor not applying on any day. The three days must be three different clips, each run's
/// `samples.csv` must be the one it recorded, every run a hold-out run at one binary and one pair
/// of full commit ids, and the primary metrics recomputed from the three `samples.csv` files must
/// equal the recorded `CLAIM` lines.
fn combine(days: [DayFiles<'_>; 3]) -> Result<String, String> {
    let mut parsed = Vec::new();
    for (name, d) in ["A", "B", "C"].iter().zip(&days) {
        let c = parse_claim_lines(d.txt).map_err(|e| format!("hold-out {name}: {e}"))?;
        if !c.info.holdout {
            return Err(format!(
                "hold-out {name} is not a hold-out run (development or dirty-tree runs are not scores)"
            ));
        }
        for (what, v) in [("frozen", &c.info.frozen), ("candidate", &c.info.candidate)] {
            if !is_full_sha(v) {
                return Err(format!(
                    "hold-out {name}: the {what} commit is `{v}`, not a full 40-hex commit id"
                ));
            }
        }
        let sha = to_hex(&sha256(d.csv.as_bytes()));
        if sha != c.info.samples_sha {
            return Err(format!(
                "hold-out {name}: samples.csv has SHA-256 {sha}, not the {} its run recorded",
                c.info.samples_sha
            ));
        }
        // The recomputation: same rows, same bootstrap and seed, same code.
        let (rows, n_ac) = rows_from_csv(d.csv).map_err(|e| format!("hold-out {name}: {e}"))?;
        let re = claim_pieces(&rows, n_ac, &draws_of(n_ac));
        for (a, b) in re.iter().zip(&c.pieces) {
            if claim_metric_line(a) != claim_metric_line(b) {
                return Err(format!(
                    "hold-out {name}: the primary metrics recomputed from samples.csv differ from its CLAIM line:\n  recomputed {}\n  recorded   {}",
                    claim_metric_line(a),
                    claim_metric_line(b)
                ));
            }
        }
        parsed.push(c);
    }
    let [a, b, c] = parsed.as_slice() else {
        return Err("three days are needed".into());
    };
    let same =
        |f: &dyn Fn(&RunInfo) -> &String| f(&b.info) == f(&a.info) && f(&c.info) == f(&a.info);
    if !same(&|i| &i.frozen) || !same(&|i| &i.candidate) {
        return Err(format!(
            "the days were not scored at one commit pair: A {}/{}, B {}/{}, C {}/{}",
            a.info.frozen,
            a.info.candidate,
            b.info.frozen,
            b.info.candidate,
            c.info.frozen,
            c.info.candidate
        ));
    }
    if !same(&|i| &i.binary_sha) {
        return Err("the days were not scored by one scorer binary".into());
    }
    let clips = [&a.info.clip_sha, &b.info.clip_sha, &c.info.clip_sha];
    if clips[0] == clips[1] || clips[0] == clips[2] || clips[1] == clips[2] {
        return Err(format!(
            "the three days are not three different clips (clip hashes {}, {}, {})",
            clips[0], clips[1], clips[2]
        ));
    }
    let mut o = String::new();
    let _ = writeln!(
        o,
        "P-006 claim rule over hold-outs A, B, C (scored commit {}, candidate {}; scorer binary {})",
        a.info.frozen, a.info.candidate, a.info.binary_sha
    );
    let _ = writeln!(
        o,
        "rule: the 95% interval of OpenATCA minus ERAM excludes zero in OpenATCA's favour on A and on B, and C's interval does not exclude zero in ERAM's favour; a metric at the reference floor on any day gives no claim (P-008 section 3)"
    );
    for (day, d) in [("A", a), ("B", b), ("C", c)] {
        for (f, t, n) in &d.windows {
            let _ = writeln!(
                o,
                "{day}: excluded window {f} to {t}: {n} grid samples removed"
            );
        }
    }
    for (id, _, name) in PRIMARY {
        let piece = |d: &DayClaim| d.pieces.iter().find(|p| p.id == id).cloned();
        let ps = [piece(a), piece(b), piece(c)];
        let v = |p: &Option<ClaimPiece>| p.as_ref().map_or(Verdict::Neither, |p| p.verdict);
        let (va, vb, vc) = (v(&ps[0]), v(&ps[1]), v(&ps[2]));
        let floor = ps.iter().any(|p| p.as_ref().is_some_and(|p| p.floor));
        let met = va == Verdict::OpenAtca && vb == Verdict::OpenAtca && vc != Verdict::Eram;
        let line = if floor {
            "AT REFERENCE FLOOR: no claim"
        } else if met {
            "RULE MET"
        } else {
            "RULE NOT MET"
        };
        let _ = writeln!(o, "{name}: {line}");
        for (day, p, vd) in [("A", &ps[0], va), ("B", &ps[1], vb), ("C", &ps[2], vc)] {
            let _ = writeln!(
                o,
                "  {day}: difference {} [{}, {}]: {}; reference floor {} (OpenATCA {}, passthrough {} on the three-way set)",
                p.as_ref().map_or(f64::NAN, |p| p.diff),
                p.as_ref().map_or(f64::NAN, |p| p.lo),
                p.as_ref().map_or(f64::NAN, |p| p.hi),
                vd.text(),
                if p.as_ref().is_some_and(|p| p.floor) {
                    "APPLIES"
                } else {
                    "does not apply"
                },
                p.as_ref().map_or(f64::NAN, |p| p.oa3),
                p.as_ref().map_or(f64::NAN, |p| p.pt3)
            );
        }
    }
    let _ = writeln!(
        o,
        "A claim of 'better than ERAM' may be made of a primary metric only if its rule is met, with the inputs, set sizes and exclusions of P-006 section 8 beside it."
    );
    Ok(o)
}

fn peak_rss_mb() -> f64 {
    // seal-allow: /proc, not an input
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

/// What the SFDPS loader and the clip reader reported of damage and drops, for the output
/// (P-006 section 6 needs the numbers on the record).
// A7.2: cold struct, size guard waived
struct Damage {
    clip_bad_frames: u64,
    clip_dropped_records: u64,
    clip_sealed: u64,
    sfdps_sealed: u64,
    replay_hash: String,
    notes: Vec<String>,
    counts: Vec<String>,
}

/// The ingest counts of a loaded legacy table as lines of text.
fn counts_lines(c: &oa_bench::legacy::Counts) -> Vec<String> {
    let mut v = vec![
        format!(
            "documents offered (SFDPS, TBFM, TFMS): {:?}; refused by the parser or decoder: {:?}",
            c.docs, c.doc_errors
        ),
        format!(
            "SFDPS messages rejected: {}; SFDPS field errors in kept rows: {}; rows not decoded (past the table size): {}",
            c.sfdps_rejected, c.sfdps_field_errors, c.rows_not_decoded
        ),
        format!(
            "TBFM flights refused: {}; TFMS messages refused: {}",
            c.tbfm_rejected, c.tfms_rejected
        ),
        format!(
            "rows that did not fit, by table (flights, th, handoffs, routes, tbfm, tfms): {}, {}, {}, {}, {}, {}",
            c.overflow_flights,
            c.overflow_th,
            c.overflow_handoffs,
            c.overflow_routes,
            c.overflow_tbfm,
            c.overflow_tfms
        ),
        format!(
            "TH samples with a coast indicator: {}; without altitude: {}; records left out in a sealed window: {}",
            c.th_coast, c.th_no_alt, c.sealed_window_dropped
        ),
    ];
    for (k, n) in &c.reasons {
        v.push(format!("drop reason: {k}: {n}"));
    }
    v
}

/// The passthrough baseline for the grid `grid` (sample times, Unix ms, by address): for each
/// grid sample the nearest set-A report of that aircraft within 0.5 s, paired with the reference.
/// Returns the paired table and the number of distinct reports chosen.
fn passthrough_pairs(
    set_a: &oa_adsb::Reports,
    by_addr_time: &[(u32, i64, u32)],
    grid: &TimesBy,
    truth: &Truth<'_>,
    tix: &TruthIndex,
    rcfg: &RunConfig,
) -> Result<(Paired, usize), String> {
    let tol_ns = TOL_MS * 1_000_000;
    let mut chosen: BTreeSet<u32> = BTreeSet::new();
    for (&a, v) in grid {
        for &tms in v {
            let t = tms * 1_000_000;
            let i = by_addr_time.partition_point(|x| (x.0, x.1) < (a, t));
            let before = i
                .checked_sub(1)
                .and_then(|j| by_addr_time.get(j))
                .filter(|x| x.0 == a);
            let after = by_addr_time.get(i).filter(|x| x.0 == a);
            let pick = match (before, after) {
                (Some(b), Some(n)) => Some(if t - b.1 <= n.1 - t { b } else { n }),
                (Some(b), None) => Some(b),
                (None, Some(n)) => Some(n),
                (None, None) => None,
            };
            if let Some(p) = pick.filter(|p| (p.1 - t).abs() <= tol_ns) {
                chosen.insert(p.2);
            }
        }
    }
    let mut order: Vec<(u32, i64, u32)> = chosen
        .iter()
        .filter_map(|&i| {
            Some((
                *set_a.icao.get(i as usize)?,
                *set_a.time_ns.get(i as usize)?,
                i,
            ))
        })
        .collect();
    order.sort_unstable();
    let (mut c_addr, mut c_t, mut c_la, mut c_lo, mut c_alt) =
        (vec![], vec![], vec![], vec![], vec![]);
    let (mut c_ve, mut c_vn, mut c_coast) = (vec![], vec![], vec![]);
    for &(a, t, i) in &order {
        let i = i as usize;
        let (Some(&la), Some(&lo)) = (set_a.lat_e7.get(i), set_a.lon_e7.get(i)) else {
            continue;
        };
        c_addr.push(a);
        c_t.push(t);
        c_la.push(la);
        c_lo.push(lo);
        let alt = set_a.baro_alt_ft.get(i).copied().unwrap_or(NONE_I32);
        c_alt.push(if alt == NONE_I32 { NO_ALT } else { alt });
        let (gs, tr) = (
            set_a.gs_dkt.get(i).copied().unwrap_or(NONE_U16),
            set_a.track_ddeg.get(i).copied().unwrap_or(NONE_U16),
        );
        if gs != NONE_U16 && tr != NONE_U16 {
            let sp = f64::from(gs) * 0.1 * KT_MPS;
            let ang = (f64::from(tr) * 0.1).to_radians();
            c_ve.push((sp * ang.sin()) as f32);
            c_vn.push((sp * ang.cos()) as f32);
        } else {
            c_ve.push(f32::NAN);
            c_vn.push(f32::NAN);
        }
        c_coast.push(false);
    }
    let legacy = legacy_from_track_samples_by_leg(
        &TrackSamples {
            addr: &c_addr,
            t_ns: &c_t,
            lat_e7: &c_la,
            lon_e7: &c_lo,
            alt_ft: &c_alt,
            vel_east_mps: &c_ve,
            vel_north_mps: &c_vn,
            coast: &c_coast,
        },
        truth.legs,
    )
    .map_err(|e| format!("passthrough samples: {e:?}"))?;
    Ok((pair_legacy(&legacy, truth, tix, rcfg), c_addr.len()))
}

/// What a run was asked for besides its inputs and output.
#[derive(Clone, Copy)]
// A7.2: cold struct, size guard waived
struct Opts<'a> {
    label: &'a str,
    mode: Mode,
    commits: &'a Commits,
    /// Hand-typed windows (development runs only).
    windows: &'a [Window],
    /// The `oa-feed-gaps` report (its windows are the only ones of a hold-out run).
    damage: Option<&'a DamageReport>,
    ledger: &'a Path,
    /// The addresses whose flights are left out of the ERAM load (`--drop-addresses`).
    drop_list: Option<&'a BTreeSet<u32>>,
    timing_profile: ScoringTimingProfile,
    oa_publication_grid: bool,
    p010: Option<P010Window>,
}

/// Where the ERAM rows come from: the SFDPS recording, or an export of it.
enum Source {
    Recording(PathBuf),
    Export(PathBuf),
}

impl Source {
    fn path(&self) -> &Path {
        match self {
            Source::Recording(p) | Source::Export(p) => p,
        }
    }
}

/// What the ERAM load gives the scorer, from either source. `inputs`, `manifest` and `dir_text` are
/// what the output prints of the recording; an export carries them from the recording it was made
/// from.
// A7.2: cold struct, size guard waived
struct EramLoad {
    legacy: Legacy,
    source_bytes: u64,
    dir_text: String,
    inputs: Vec<String>,
    manifest: String,
    replay_hash: String,
    notes: Vec<String>,
    counts: Vec<String>,
}

/// A drop list read through the guard.
fn read_drop_list(path: &Path) -> Result<BTreeSet<u32>, String> {
    const MAX_DROP_LIST_BYTES: u64 = 1024 * 1024;
    let (mut file, size) =
        oa_sealed::open_regular(path).map_err(|e| format!("drop list {}: {e}", path.display()))?;
    if size > MAX_DROP_LIST_BYTES {
        return Err("drop list exceeds 1 MiB".to_string());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    std::io::Read::take(&mut file, MAX_DROP_LIST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("drop list {}: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|e| format!("drop list {}: {e}", path.display()))?;
    eram_export::parse_drop_list(&text)
}

/// The recording's description as the scorer prints it, from a loaded recording.
fn describe_recording(
    sfdps: &Path,
    loaded: &oa_bench::input::Loaded,
    sealed: u64,
) -> eram_export::Meta {
    eram_export::Meta {
        source_dir: sfdps.display().to_string(),
        source_inputs: loaded
            .inputs
            .iter()
            .map(|h| {
                format!(
                    "{} files={} bytes={} sha256={}",
                    h.label, h.files, h.bytes, h.sha256
                )
            })
            .collect(),
        source_sha256: loaded
            .inputs
            .iter()
            .map(|h| h.sha256.clone())
            .collect::<Vec<_>>()
            .join(","),
        replay_hash: loaded.replay_hash.clone(),
        sealed_dropped: sealed,
        notes: loaded.notes.clone(),
        counts: counts_lines(&loaded.legacy.counts),
    }
}

/// Reads an export and its required manifest through the guard, and verifies both digests/counts.
fn read_export(path: &Path) -> Result<(eram_export::Export, String), String> {
    const MAX_EXPORT_BYTES: u64 = 512 * 1024 * 1024;
    const MAX_SIDECAR_BYTES: u64 = 4096;
    let (mut file, size) =
        oa_sealed::open_regular(path).map_err(|e| format!("export {}: {e}", path.display()))?;
    if size > MAX_EXPORT_BYTES {
        return Err("export exceeds 512 MiB".to_string());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    std::io::Read::take(&mut file, MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("export {}: {e}", path.display()))?;
    let mut side = path.as_os_str().to_os_string();
    side.push(".sha256");
    let side = PathBuf::from(side);
    let (mut side_file, side_size) = oa_sealed::open_regular(&side).map_err(|e| {
        format!(
            "manifest {}: required sidecar could not be read: {e}",
            side.display()
        )
    })?;
    if side_size > MAX_SIDECAR_BYTES {
        return Err("export sidecar exceeds 4 KiB".to_string());
    }
    let mut side_bytes = Vec::with_capacity(side_size as usize);
    std::io::Read::take(&mut side_file, MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut side_bytes)
        .map_err(|e| format!("manifest {}: {e}", side.display()))?;
    let manifest =
        String::from_utf8(side_bytes).map_err(|e| format!("manifest {}: {e}", side.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: not a file name", path.display()))?;
    eram_export::check_manifest(&manifest, name, &bytes)?;
    let sha = eram_export::file_sha256(&bytes);
    let e = eram_export::parse(&bytes).map_err(|e| format!("export {}: {e}", path.display()))?;
    Ok((e, sha))
}

/// `--export-eram <file> --sfdps-dir <dir> [--drop-addresses <file>]`: the export of a recording.
/// Every path goes through the seal guard before anything is opened or created; the recording is
/// read by the loader the scorer uses; the file and its manifest are written as partials and
/// renamed together, only when neither exists.
fn export_main(sfdps: &Path, file: &Path, drop_path: Option<&Path>) -> ExitCode {
    for p in [Some(sfdps), Some(file), drop_path].into_iter().flatten() {
        if let Err(e) = oa_sealed::refuse(p) {
            eprintln!("p002: {e}");
            return ExitCode::from(2);
        }
    }
    let mut drops = oa_sealed::DropReport::new();
    match export_run(sfdps, file, drop_path, &mut drops) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("p002: {e}");
            ExitCode::from(1)
        }
    }
}

fn export_run(
    sfdps: &Path,
    file: &Path,
    drop_path: Option<&Path>,
    drops: &mut oa_sealed::DropReport,
) -> Result<String, String> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: not a file name", file.display()))?
        .to_string();
    let (mut side, mut part_a, mut part_b) = (
        file.as_os_str().to_os_string(),
        file.as_os_str().to_os_string(),
        file.as_os_str().to_os_string(),
    );
    side.push(".sha256");
    part_a.push(PARTIAL);
    part_b.push(".sha256.partial");
    let (side, part_a, part_b) = (
        PathBuf::from(side),
        PathBuf::from(part_a),
        PathBuf::from(part_b),
    );
    for p in [file, side.as_path(), part_a.as_path(), part_b.as_path()] {
        oa_sealed::refuse(p).map_err(|e| format!("refused: {e}"))?;
    }
    for p in [file, side.as_path(), part_a.as_path(), part_b.as_path()] {
        if p.exists() || p.is_symlink() {
            return Err(format!(
                "refused: {} exists; an export is written only when final and temporary paths are absent",
                p.display()
            ));
        }
    }
    let drop_list = drop_path.map(read_drop_list).transpose()?;
    let open: oa_bench::input::Opener = std::rc::Rc::new(|p: &Path| {
        oa_sealed::refuse_io(p)?;
        oa_bench::input::file_opener()(p)
    });
    let mut sealed = 0u64;
    let loaded = load_counted(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: sfdps.to_path_buf(),
        }],
        &open,
        msg_limit(),
        &mut sealed,
    );
    drops.add(sealed);
    let loaded = loaded.map_err(|e| e.to_string())?;
    let meta = describe_recording(sfdps, &loaded, sealed);
    let mut exp = eram_export::from_legacy(&loaded.legacy, meta)?;
    drop(loaded);
    let mut report = String::new();
    if let Some(list) = &drop_list {
        let st = eram_export::drop_addresses(&mut exp, list);
        if st.flights > 0 {
            exp.meta.notes.push(eram_export::drop_note(&st));
        }
        let _ = writeln!(report, "dropped by the address list: {st:?}");
    }
    let text = eram_export::write(&exp)?;
    let manifest = eram_export::manifest_line(&name, text.as_bytes())?;
    write_new_partial(&part_a, &text)?;
    if let Err(e) = write_new_partial(&part_b, &manifest) {
        // A failed write takes care of its own partial.
        let _ = fs::remove_file(&part_a);
        return Err(e);
    }
    if let Err(e) = publish_export_pair(&part_a, file, &part_b, &side) {
        let _ = fs::remove_file(&part_a);
        let _ = fs::remove_file(&part_b);
        return Err(e);
    }
    let s = eram_export::summary(&exp);
    let _ = writeln!(
        report,
        "export {} (format {})",
        file.display(),
        eram_export::FORMAT_VERSION
    );
    let _ = writeln!(
        report,
        "  bytes {}, sha256 {}",
        text.len(),
        eram_export::file_sha256(text.as_bytes())
    );
    let _ = writeln!(report, "  {s:?}");
    Ok(report)
}

fn run(
    clip_path: &Path,
    source: &Source,
    out_dir: &Path,
    git: &GitState,
    opts: &Opts<'_>,
) -> Result<String, String> {
    let Opts {
        label,
        mode,
        commits,
        windows: hand_windows,
        damage,
        ledger,
        drop_list,
        timing_profile,
        oa_publication_grid,
        p010,
    } = *opts;
    let partials_left = outputs_free(out_dir)?;
    let windows_all: Vec<Window> = hand_windows
        .iter()
        .chain(damage.iter().flat_map(|d| d.windows.iter()))
        .cloned()
        .collect();
    let windows = windows_all.as_slice();
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let t_start = Instant::now();

    // Tracker on set A (as vs_eram).
    // The clip's count and the SFDPS loader's, each input once, printed when this function
    // returns, an error included.
    let mut drops = oa_sealed::DropReport::new();
    let mut clip_sealed = 0u64;
    let clip = load_adsb_counted(clip_path, &file_opener(), &mut clip_sealed);
    drops.add(clip_sealed);
    let clip = clip.map_err(|e| e.to_string())?;
    let clip_hash = clip.hash.clone();
    if p010.is_some()
        && (reports_len(&clip.reports) == 0
            || clip
                .reports
                .time_ns
                .iter()
                .any(|&t| !p010_clip_time_in_day(t)))
    {
        return Err("P010_REFUSED_PUBLIC_CLIP_OUTSIDE_FULL_UTC_DAY: expected nonempty 2026-11-30 [00:00, 2026-12-01 00:00) report times".into());
    }
    // P-008 section 5: a clip already scored is refused, whatever the mode.
    if ledger_has(ledger, &clip_hash.sha256)? {
        return Err(format!(
            "the clip {} is already in the ledger {}: one result per day",
            clip_hash.sha256,
            ledger.display()
        ));
    }
    // P-008 section 2: the damage report is that of the window being scored.
    if let Some(d) = damage {
        let t = &clip.reports.time_ns;
        let (first, last) = (
            t.iter().copied().min().unwrap_or(i64::MAX),
            t.iter().copied().max().unwrap_or(i64::MIN),
        );
        let guard: oa_bench::input::Opener = std::rc::Rc::new(|p: &Path| {
            oa_sealed::refuse_io(p)?;
            oa_bench::input::file_opener()(p)
        });
        let Source::Recording(sfdps) = source else {
            return Err("a damage report needs the SFDPS recording, not an export".to_string());
        };
        let (s_first, s_last) = sfdps_span(sfdps, &guard)?;
        damage_matches_clip(d, first, last, s_first, s_last)?;
    }
    let (clip_bad_frames, clip_dropped_records) = (clip.bad_frames, clip.dropped_records);
    let (set_a, set_b, split) = split_reports(&clip.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("split refused: {v:?}"))?;
    drop(clip);
    let (mut meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    let timing_report: ScoringTimingReport =
        scoring_timing_config(&DEFAULT_CONFIG, &meas, timing_profile);
    let cfg = timing_report.tracker;
    let n_meas_before_p010_window = meas.len();
    if let Some(window) = p010 {
        meas.retain(|m| m.toa_ns >= window.warmup_from_ns && m.toa_ns < window.score_to_ns);
    }
    let spill = out_dir.join(format!("spill-{}", std::process::id()));
    let t_track = Instant::now();
    let run = run_tracker_spilled(&meas, &cfg, &spill).map_err(|e| e.to_string())?;
    let track_secs = t_track.elapsed().as_secs_f64();
    let n_meas = meas.len();
    let withheld = run.stats.rows_uncertain;
    drop(meas);
    eprintln!("tracker done in {track_secs:.1} s");

    // Reference from set B only.
    let tcfg = TRUTH_DEFAULT;
    let built = adsb_truth::build(&set_b, &tcfg).map_err(|e| format!("truth: {e}"))?;
    drop(set_b);
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    let (p010_eligible, p010_counts) = match p010 {
        Some(window) => p010_public_eligibility(&truth, window)?,
        None => (BTreeMap::new(), P010EligibilityCounts::default()),
    };
    let p010_truth_cohort_sha256 = if p010.is_some() {
        p010_keyset_sha256(
            b"openatca-p010-public-eligible-address-minute-truth-leg-v1\0",
            p010_eligible
                .iter()
                .map(|(&(addr, minute_ms), &leg_ix)| (addr, minute_ms, Some(leg_ix))),
        )?
    } else {
        "not-applicable".to_string()
    };
    let mut rcfg = RunConfig::default();
    rcfg.pair.max_sigma_m = tcfg.max_sigma95_h_m;
    // frozen p002 protocol: scored without the hint-over-plan rule (OA-BENCH-132 came later)
    rcfg.assoc.hint_over_plan = false;
    let tix = truth_index(&truth);
    let legs = LegIndex::new(truth.legs);
    eprintln!("reference built");

    // ERAM pairs come first: the grid is ERAM's paired samples, and every system is scored on the
    // truth legs that hold them (P-004 section 1).
    // Every file of the directory is checked against the sealed list as it is opened.
    let open: oa_bench::input::Opener = std::rc::Rc::new(|p: &Path| {
        oa_sealed::refuse_io(p)?;
        oa_bench::input::file_opener()(p)
    });
    let mut sfdps_sealed = 0u64;
    let p010_preflight_bytes = if p010.is_some() {
        if let Source::Recording(sfdps) = &source {
            p010_preflight_sfdps_bytes(sfdps)?
        } else {
            0
        }
    } else {
        0
    };
    let mut loaded = match source {
        Source::Recording(sfdps) => {
            let l = load_counted(
                &[FeedInput {
                    feed: FeedKind::Sfdps,
                    dir: sfdps.clone(),
                }],
                &open,
                msg_limit(),
                &mut sfdps_sealed,
            );
            drops.add(sfdps_sealed);
            let l = l.map_err(|e| e.to_string())?;
            if p010.is_some() {
                let actual_bytes = l
                    .inputs
                    .iter()
                    .fold(0u64, |sum, item| sum.saturating_add(item.bytes));
                if actual_bytes > P010_MAX_SFDPS_BYTES {
                    return Err(format!("P010_REFUSED_SFDPS_BYTE_CAP actual={actual_bytes} cap={P010_MAX_SFDPS_BYTES}"));
                }
                if actual_bytes != p010_preflight_bytes {
                    return Err(format!("P010_REFUSED_SFDPS_SOURCE_BYTES_CHANGED preflight={p010_preflight_bytes} loaded={actual_bytes}"));
                }
            }
            let meta = describe_recording(sfdps, &l, sfdps_sealed);
            EramLoad {
                dir_text: meta.source_dir,
                source_bytes: l
                    .inputs
                    .iter()
                    .fold(0u64, |sum, item| sum.saturating_add(item.bytes)),
                inputs: meta.source_inputs,
                manifest: meta.source_sha256,
                replay_hash: meta.replay_hash,
                notes: meta.notes,
                counts: meta.counts,
                legacy: l.legacy,
            }
        }
        Source::Export(file) => {
            let (e, sha) = read_export(file)?;
            eprintln!(
                "ERAM rows from the export {} (sha256 {sha})",
                file.display()
            );
            sfdps_sealed = e.meta.sealed_dropped;
            EramLoad {
                legacy: eram_export::to_legacy(&e)?,
                source_bytes: 0,
                dir_text: e.meta.source_dir,
                inputs: e.meta.source_inputs,
                manifest: e.meta.source_sha256,
                replay_hash: e.meta.replay_hash,
                notes: e.meta.notes,
                counts: e.meta.counts,
            }
        }
    };
    if let Some(list) = drop_list {
        // The flights leave the recording's tables as they leave an export.
        let mut e = eram_export::from_legacy(&loaded.legacy, eram_export::Meta::default())?;
        let st = eram_export::drop_addresses(&mut e, list);
        if st.flights > 0 {
            loaded.legacy = eram_export::to_legacy(&e)?;
            loaded.notes.push(eram_export::drop_note(&st));
        }
    }
    let sfdps_manifest = loaded.manifest.clone();
    let sfdps_source_bytes = loaded.source_bytes;
    let sfdps_inputs: Vec<String> = loaded.inputs.clone();
    let sfdps_dir_text = loaded.dir_text.clone();
    let loader = Damage {
        clip_bad_frames,
        clip_dropped_records,
        clip_sealed,
        sfdps_sealed,
        replay_hash: loaded.replay_hash.clone(),
        notes: loaded.notes.clone(),
        counts: loaded.counts.clone(),
    };
    let ep = pair_legacy(&loaded.legacy, &truth, &tix, &rcfg);
    drop(loaded);
    let mut er_pts: PerAc = BTreeMap::new();
    let mut er_drop = Dropped::default();
    let er_keep = |addr: u32, t_ms: i64| {
        p010.is_none_or(|window| {
            p010_keeps_eligible_time(&p010_eligible, &truth, addr, t_ms, window)
        })
    };
    pair_points(
        &ep.pairs,
        Some(&most_paired_flights(&ep.pairs)),
        &er_keep,
        &mut er_pts,
        &mut er_drop,
    );
    let mut er_old: TimesBy = BTreeMap::new();
    old_rule_times(&ep.pairs, &mut er_old);
    if let Some(window) = p010 {
        filter_times_by_eligible(&mut er_old, &p010_eligible, &truth, window);
    }
    // P-007: the samples inside an excluded window leave the grid for every system alike. The
    // scored legs, the passthrough pairs and the other systems' samples are made from the grid
    // as it was, so no remaining value changes.
    let (scored, er_grid, win_removed, n_win_removed) = plan_grid(&er_pts, &legs, windows);
    let (
        p010_eram_available_address_minutes,
        p010_eram_position_time_rows,
        p010_eram_cohort_sha256,
    ) = if p010.is_some() {
        let mut available = BTreeSet::new();
        let mut rows = 0u64;
        for (&addr, pts) in &er_grid {
            for pt in pts {
                available.insert((addr, pt.t.div_euclid(60_000) * 60_000));
                rows += 1;
            }
        }
        let digest = p010_keyset_sha256(
            b"openatca-p010-eram-available-address-minute-truth-leg-v1\0",
            available.iter().map(|&(addr, minute_ms)| {
                (
                    addr,
                    minute_ms,
                    p010_eligible.get(&(addr, minute_ms)).copied(),
                )
            }),
        )?;
        (available.len() as u64, rows, digest)
    } else {
        (0, 0, "not-applicable".to_string())
    };
    let er_old_win = {
        let t = |m: &TimesBy| -> TimesBy {
            m.iter()
                .map(|(&a, v)| {
                    (
                        a,
                        v.iter()
                            .copied()
                            .filter(|&t| !windows.iter().any(|w| w.holds_ms(t)))
                            .collect(),
                    )
                })
                .collect()
        };
        t(&er_old)
    };
    let on_leg = |a: u32, t: i64| on_scored_leg(&scored, &legs, a, t);
    eprintln!("ERAM paired aircraft: {}", er_pts.len());

    // OpenATCA pairs, one address range at a time (each aircraft is in one range).
    let mut oa_pts: PerAc = BTreeMap::new();
    let mut oa_drop = Dropped::default();
    let mut oa_ex = Exclusions::new();
    let mut oa_assoc = [0u64; astatus::COUNT];
    let mut oa_rows = 0u64;
    let mut oa_old: TimesBy = BTreeMap::new();
    // Every output row time (ms) of the grid aircraft, for the miss classes.
    let mut oa_row_times: TimesBy = BTreeMap::new();
    let mut n_reacq_updates = 0u64;
    let (mut flag_dup_rows, mut flag_conflicts) = (0u64, 0u64);
    let mut publication_grid_counts = PublicationGridCounts::default();
    let counts = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
    for range in score_ranges(&counts, SCORE_CHUNK_ROWS) {
        let r = run
            .rows_in(RowSet::Ticks, range.0, range.1)
            .map_err(|e| e.0)?;
        let legacy = (if oa_publication_grid {
            let quantized = publication_grid_columns(
                &r.lat_e7,
                &r.lon_e7,
                &r.vel_east_mps,
                &r.vel_north_mps,
                &mut publication_grid_counts,
            )?;
            legacy_from_track_samples_by_leg(
                &TrackSamples {
                    addr: &r.addr,
                    t_ns: &r.t_ns,
                    lat_e7: &quantized.lat_e7,
                    lon_e7: &quantized.lon_e7,
                    alt_ft: &r.alt_ft,
                    vel_east_mps: &quantized.vel_east_mps,
                    vel_north_mps: &quantized.vel_north_mps,
                    coast: &r.coast,
                },
                truth.legs,
            )
        } else {
            legacy_from_track_samples_by_leg(
                &TrackSamples {
                    addr: &r.addr,
                    t_ns: &r.t_ns,
                    lat_e7: &r.lat_e7,
                    lon_e7: &r.lon_e7,
                    alt_ft: &r.alt_ft,
                    vel_east_mps: &r.vel_east_mps,
                    vel_north_mps: &r.vel_north_mps,
                    coast: &r.coast,
                },
                truth.legs,
            )
        })
        .map_err(|e| format!("track samples: {e:?}"))?;
        for (&a, &t) in r.addr.iter().zip(&r.t_ns) {
            let t_ms = t.div_euclid(1_000_000);
            if p010.is_none_or(|window| {
                p010_keeps_eligible_time(&p010_eligible, &truth, a, t_ms, window)
            }) {
                oa_rows += 1;
            }
        }
        for (&a, &t) in r.addr.iter().zip(&r.t_ns) {
            let t_ms = t.div_euclid(1_000_000);
            if er_pts.contains_key(&a)
                && p010.is_none_or(|window| {
                    p010_keeps_eligible_time(&p010_eligible, &truth, a, t_ms, window)
                })
            {
                oa_row_times.entry(a).or_default().push(t_ms);
            }
        }
        // The tracker's flags by (address, row time in ms), duplicates counted.
        let flags = flag_join(&r.addr, &r.t_ns, &r.uncertain, &r.reacquired)?;
        drop(r);
        // P-004 section 2: the update rows flagged reacquired (the tick rows never carry it).
        let up = run
            .rows_in(RowSet::Updates, range.0, range.1)
            .map_err(|e| e.0)?;
        let mut reacq: TimesBy = BTreeMap::new();
        for ((&a, &t), &re) in up.addr.iter().zip(&up.t_ns).zip(&up.reacquired) {
            if re {
                reacq.entry(a).or_default().push(t.div_euclid(1_000_000));
                n_reacq_updates += 1;
            }
        }
        for v in reacq.values_mut() {
            v.sort_unstable();
        }
        drop(up);
        flag_dup_rows += flags.duplicate_rows;
        flag_conflicts += flags.conflict_keys;
        let p = pair_legacy(&legacy, &truth, &tix, &rcfg);
        old_rule_times(&p.pairs, &mut oa_old);
        let mut local: PerAc = BTreeMap::new();
        let oa_keep = |a: u32, t: i64| {
            on_leg(a, t)
                && p010.is_none_or(|window| {
                    p010_keeps_eligible_time(&p010_eligible, &truth, a, t, window)
                })
        };
        pair_points(&p.pairs, None, &oa_keep, &mut local, &mut oa_drop);
        for (a, v) in &mut local {
            for pt in v {
                let (u, re) = flag_lookup(&flags, *a, pt.t);
                pt.uncertain = u;
                // A sample with no tracker row stays unmatched; otherwise the group is the window.
                pt.reacquired = if re == Tri::Unmatched {
                    Tri::Unmatched
                } else {
                    tri_of(reacquired_at(reacq.get(a).map(Vec::as_slice), pt.t))
                };
            }
        }
        for (a, v) in local {
            oa_pts.entry(a).or_default().extend(v);
        }
        for (k, v) in p.ex {
            *oa_ex.entry(k).or_default() += v;
        }
        for (a, b) in oa_assoc.iter_mut().zip(p.assoc_counts) {
            *a += b;
        }
    }
    if let Some(window) = p010 {
        filter_times_by_eligible(&mut oa_old, &p010_eligible, &truth, window);
    }
    for v in oa_row_times.values_mut() {
        v.sort_unstable();
    }
    drop(run);
    let _ = fs::remove_dir_all(&spill);
    eprintln!("OpenATCA paired aircraft: {}", oa_pts.len());

    // Passthrough: for each grid sample, the nearest set-A report of that aircraft within 0.5 s.
    let by_addr_time: Vec<(u32, i64, u32)> = {
        let mut v: Vec<(u32, i64, u32)> = set_a
            .icao
            .iter()
            .zip(&set_a.time_ns)
            .enumerate()
            .filter(|&(_, (&a, &t))| {
                let t_ms = t.div_euclid(1_000_000);
                p010.is_none_or(|window| {
                    p010_keeps_eligible_time(&p010_eligible, &truth, a, t_ms, window)
                })
            })
            .map(|(i, (&a, &t))| (a, t, i as u32))
            .collect();
        v.sort_unstable();
        v
    };
    // The set-A report times (ms) of the grid aircraft: the passthrough's rows, for the miss classes.
    let mut pt_row_times: TimesBy = BTreeMap::new();
    for &(a, t, _) in &by_addr_time {
        if er_pts.contains_key(&a) {
            pt_row_times
                .entry(a)
                .or_default()
                .push(t.div_euclid(1_000_000));
        }
    }
    let er_times = times_of(&er_pts);
    let (pp, pt_chosen) = passthrough_pairs(&set_a, &by_addr_time, &er_times, &truth, &tix, &rcfg)?;
    let mut pt_pts: PerAc = BTreeMap::new();
    let mut pt_drop = Dropped::default();
    let pt_keep = |a: u32, t: i64| {
        on_leg(a, t)
            && p010
                .is_none_or(|window| p010_keeps_eligible_time(&p010_eligible, &truth, a, t, window))
    };
    pair_points(&pp.pairs, None, &pt_keep, &mut pt_pts, &mut pt_drop);
    eprintln!("passthrough paired aircraft: {}", pt_pts.len());
    // The replaced rule's passthrough, on the replaced rule's grid (coverage record only).
    let (pp_old, _) = passthrough_pairs(&set_a, &by_addr_time, &er_old, &truth, &tix, &rcfg)?;
    let mut pt_old: TimesBy = BTreeMap::new();
    old_rule_times(&pp_old.pairs, &mut pt_old);
    drop(pp_old);
    drop(by_addr_time);
    let set_a_reports = reports_len(&set_a);
    drop(set_a);
    let old_cov = [
        old_rule_coverage(&er_old_win, &oa_old),
        old_rule_coverage(&er_old_win, &er_old),
        old_rule_coverage(&er_old_win, &pt_old),
    ];
    drop((oa_old, pt_old, er_old));

    // The grid.
    let g = build_grid(
        &er_grid,
        &oa_pts,
        &pt_pts,
        [&oa_row_times, &er_times, &pt_row_times],
    );
    let Grid {
        mut rows,
        ac_addr,
        miss,
        classes,
    } = g;
    let n_ac = ac_addr.len();
    let grid = rows.len();
    let n_common = rows.iter().filter(|r| r.common()).count();
    let n_two = rows.iter().filter(|r| r.two_way()).count();
    // A hold-out score needs every scored sample's flags: an unmatched sample is not scored as if
    // its flags were "no" (E6, G6). Counted over the OpenATCA rows matched to a grid sample.
    let flag_unmatched = unmatched_rows(&rows);
    flag_gate(mode, flag_unmatched, flag_dup_rows, flag_conflicts)?;

    // Per-aircraft, per-system time offsets, applied to all of the aircraft's samples. P-008
    // section 1: OpenATCA's and ERAM's are fitted on the two-way common set (the set the claim is
    // made on); the passthrough's on the three-way set, the only one where it has values.
    let mut taus: Vec<[Option<f64>; 3]> = vec![[None; 3]; n_ac];
    for (ac, tau) in taus.iter_mut().enumerate() {
        for (s, slot) in tau.iter_mut().enumerate() {
            *slot = fit_tau(
                of_ac(&rows, ac)
                    .iter()
                    .filter(|r| in_fit_set(r, s))
                    .filter_map(|r| r.val.get(s).and_then(|v| v.as_ref()))
                    .map(|v| (v.pt.along, v.pt.v)),
            );
        }
    }
    let unfitted = unfitted_counts(&rows, &taus);
    for r in &mut rows {
        let tau = taus.get(r.ac).copied().unwrap_or([None; 3]);
        for (s, v) in r.val.iter_mut().enumerate() {
            if let Some(v) = v {
                // P-003 resolution 1: an aircraft with no usable samples for a system's fit is
                // scored unfitted (offset 0), and counted in `unfitted` and printed below.
                let t = tau.get(s).copied().flatten().unwrap_or(UNFITTED_TAU_S);
                v.along_fit = v.pt.along - t * v.pt.v;
                v.h_fit = v.pt.cross.hypot(v.along_fit);
                v.along_tol = along_tol(v.along_fit, v.pt.v);
                v.h_tol = v.pt.cross.hypot(v.along_tol);
            }
        }
    }
    eprintln!("grid {grid} samples, {n_two} two-way, {n_common} three-way, {n_ac} aircraft");

    // Bootstrap draws and metrics.
    let draws = draws_of(n_ac);
    let all = |_: &Row| true;
    // Each table is on the two-way set; the three-way values, point values only (no resamples),
    // are for the reference-floor test.
    let two = |k: Kind| metric(&rows, n_ac, &draws, k, &all, Set::Two);
    let three = |k: Kind| metric(&rows, n_ac, &[], k, &all, Set::Three);
    let m1 = two(Kind::Cross);
    let m2r = two(Kind::AlongRaw);
    let m2f = two(Kind::AlongFit);
    let m3 = two(Kind::HFit);
    let m2t = two(Kind::AlongTol);
    let m3t = two(Kind::HTol);
    let m5s = two(Kind::Speed);
    let m5t = two(Kind::Track);
    eprintln!("main metrics bootstrapped");

    // samples.csv, and what the run records of itself for `--combine` (P-008 sections 3 to 5).
    let mut stamps: Vec<&str> = Vec::new();
    if git_is_dirty(git) {
        stamps.push(DIRTY_STAMP);
    }
    if mode.development {
        stamps.push(DEV_STAMP);
    }
    let p010_arm_stamp = if oa_publication_grid {
        "P010_ARM=sfdps-v1"
    } else {
        "P010_ARM=raw"
    };
    if p010.is_some() {
        stamps.push("P010_MODE=local-window-v1");
        stamps.push(p010_arm_stamp);
        stamps.push(match timing_report.profile {
            ScoringTimingProfile::Fixed1030120V1 => "P010_TIMING_PROFILE=fixed-10-30-120-v1",
            ScoringTimingProfile::LegacyDayIntervals => "P010_TIMING_PROFILE=legacy-day-intervals",
        });
        stamps.push("P010_WINDOW_PROFILE=p010-zau-window-v1");
        stamps.push("P010_WARMUP_FROM=2026-11-30T05:00:00Z");
        stamps.push("P010_SCORE_FROM=2026-11-30T06:00:00Z");
        stamps.push("P010_SCORE_TO=2026-12-01T00:00:00Z");
        stamps.push("P010_ZAU_BOX_PROFILE=zau-v1");
    }
    if oa_publication_grid {
        stamps.push(OA_PUBLICATION_GRID_STAMP);
    }
    let csv = samples_csv(&rows, &stamps);
    let binary_sha = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|p| {
            // seal-allow: the scorer's own executable, not input data (the guard refuses a hard-linked build output)
            fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
        })
        .map_or_else(|e| format!("unavailable ({e})"), |b| to_hex(&sha256(&b)));
    let window_total_ns: i64 = windows.iter().map(|w| w.to_ns - w.from_ns).sum();
    let info = RunInfo {
        holdout: mode.holdout,
        frozen: commits.frozen.clone().unwrap_or_else(|| "none".into()),
        candidate: commits.candidate.clone().unwrap_or_else(|| "none".into()),
        clip_sha: clip_hash.sha256.clone(),
        sfdps_sha: sfdps_manifest.clone(),
        replay_hash: loader.replay_hash.clone(),
        samples_sha: to_hex(&sha256(csv.as_bytes())),
        binary_sha,
        damage_sha: damage.map_or_else(|| "none".to_string(), |d| d.sha256.clone()),
        window_total_s: format!("{}", window_total_ns / 1_000_000_000),
    };
    let pieces = claim_pieces(&rows, n_ac, &draws);
    let mut o = String::new();
    if git_is_dirty(git) {
        let _ = writeln!(o, "{DIRTY_STAMP}");
    }
    if mode.development {
        let _ = writeln!(o, "{DEV_STAMP}");
    }
    let _ = writeln!(
        o,
        "P-002 scoring with the P-003 to P-006 amendments: OpenATCA, ERAM, passthrough baseline"
    );
    if !label.is_empty() {
        let _ = writeln!(o, "  label: {label}");
    }
    let _ = writeln!(
        o,
        "  git commit: {} (working tree: {})",
        git.head,
        git_tree_text(git)
    );
    if let Some(window) = p010 {
        let avail_pct = if p010_counts.eligible_address_minutes == 0 {
            0.0
        } else {
            p010_eram_available_address_minutes as f64 * 100.0
                / p010_counts.eligible_address_minutes as f64
        };
        let timing_name = match timing_report.profile {
            ScoringTimingProfile::Fixed1030120V1 => "fixed-10-30-120-v1",
            ScoringTimingProfile::LegacyDayIntervals => "legacy-day-intervals",
        };
        let arm = if oa_publication_grid {
            "sfdps-v1"
        } else {
            "raw"
        };
        let _ = writeln!(o, "P010_MODE=local-window-v1");
        let _ = writeln!(o, "P010_STATUS=measurement-complete");
        let _ = writeln!(
            o,
            "P010_QUALIFICATION=not-assessed; frozen-coverage-and-input-health-policy-required"
        );
        let _ = writeln!(o, "P010_WINDOW_PROFILE={P010_WINDOW_PROFILE}");
        let _ = writeln!(o, "P010_ARM={arm}");
        let _ = writeln!(o, "P010_TIMING_PROFILE={timing_name}");
        let timing_inputs = if timing_report.profile == ScoringTimingProfile::Fixed1030120V1 {
            "versioned-constants-no-cadence-statistics"
        } else {
            "full-set-A-before-warmup-filter-noncausal-replication-arm"
        };
        let _ = writeln!(o, "P010_TIMING_CALIBRATION_INPUTS={timing_inputs}");
        let _ = writeln!(o, "P010_FULL_CLIP_SPLIT=once-before-set-A-filter");
        let _ = writeln!(o, "P010_COHORT_IDENTITY_FORMAT=domain-separated-sorted-addr-u32be-minute-i64be-truth-leg-u32be-v1");
        let _ = writeln!(o, "P010_CLIP_SHA256={}", clip_hash.sha256);
        let _ = writeln!(o, "P010_SFDPS_AGGREGATE_SHA256={sfdps_manifest}");
        let _ = writeln!(o, "P010_SFDPS_INPUT_BYTES={sfdps_source_bytes}");
        let _ = writeln!(o, "P010_SFDPS_PREFLIGHT_INPUT_BYTES={p010_preflight_bytes}");
        let _ = writeln!(o, "P010_WARMUP_FROM=2026-11-30T05:00:00Z");
        let _ = writeln!(o, "P010_SCORE_FROM=2026-11-30T06:00:00Z");
        let _ = writeln!(o, "P010_SCORE_TO=2026-12-01T00:00:00Z");
        let _ = writeln!(o, "P010_WARMUP_FROM_NS={}", window.warmup_from_ns);
        let _ = writeln!(o, "P010_SCORE_FROM_NS={}", window.score_from_ns);
        let _ = writeln!(o, "P010_SCORE_TO_NS={}", window.score_to_ns);
        let _ = writeln!(o, "P010_SCORE_INTERVAL=half-open");
        let _ = writeln!(o, "P010_ELIGIBILITY_MINUTE_STEP_MS=60000");
        let _ = writeln!(o, "P010_ELIGIBILITY_MINUTE_ANCHOR=score-from");
        let _ = writeln!(o, "P010_ZAU_BOX_PROFILE={}", window.box_profile);
        let _ = writeln!(o, "P010_ZAU_BOX_LAT_MIN_E7={}", ZAU_BOX.lat_min_e7);
        let _ = writeln!(o, "P010_ZAU_BOX_LAT_MAX_E7={}", ZAU_BOX.lat_max_e7);
        let _ = writeln!(o, "P010_ZAU_BOX_LON_MIN_E7={}", ZAU_BOX.lon_min_e7);
        let _ = writeln!(o, "P010_ZAU_BOX_LON_MAX_E7={}", ZAU_BOX.lon_max_e7);
        let _ = writeln!(o, "P010_PUBLIC_ELIGIBILITY=one-unambiguous-set-B-truth-leg-per-address-at-whole-UTC-minute; valid-finite-position-and-velocity; speed-at-least-{P010_MIN_DIRECTION_SPEED_MPS}-mps; actual-time-truth-leg-must-match-eligible-minute-leg; fixed-ZAU-box");
        let _ = writeln!(
            o,
            "P010_ADDRESS_MINUTE_OPPORTUNITIES={}",
            p010_counts.address_minute_opportunities
        );
        let _ = writeln!(
            o,
            "P010_NO_ACTIVE_LEG_ADDRESS_MINUTES={}",
            p010_counts.no_active_leg_minutes
        );
        let _ = writeln!(
            o,
            "P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES={}",
            p010_counts.eligible_address_minutes
        );
        let _ = writeln!(
            o,
            "P010_PUBLIC_ELIGIBLE_COHORT_SHA256={p010_truth_cohort_sha256}"
        );
        let _ = writeln!(
            o,
            "P010_AMBIGUOUS_ADDRESS_MINUTE_CANDIDATES={}",
            p010_counts.ambiguous_candidates
        );
        let _ = writeln!(
            o,
            "P010_UNUSABLE_TRUTH_ADDRESS_MINUTES={}",
            p010_counts.unusable_truth_minutes
        );
        let _ = writeln!(
            o,
            "P010_NO_TRUTH_ADDRESS_MINUTES={}",
            p010_counts.no_truth_minutes
        );
        let _ = writeln!(
            o,
            "P010_ERAM_FINITE_PAIRED_POSITION_TIME_ROWS={p010_eram_position_time_rows}"
        );
        let _ = writeln!(
            o,
            "P010_ERAM_AVAILABLE_ADDRESS_MINUTES={p010_eram_available_address_minutes}"
        );
        let _ = writeln!(
            o,
            "P010_ERAM_AVAILABLE_COHORT_SHA256={p010_eram_cohort_sha256}"
        );
        let _ = writeln!(o, "P010_ERAM_AVAILABILITY_SCOPE=post-P006-pairing-and-flight-selection; not-all-local-SFDPS-rows");
        let _ = writeln!(o, "P010_ADSB_BAD_FRAMES={clip_bad_frames}");
        let _ = writeln!(o, "P010_ADSB_DROPPED_RECORDS={clip_dropped_records}");
        let _ = writeln!(o, "P010_ADSB_SEALED_DROPS={clip_sealed}");
        let _ = writeln!(o, "P010_SFDPS_SEALED_DROPS={sfdps_sealed}");
        let _ = writeln!(
            o,
            "P010_ERAM_AVAILABILITY_DENOMINATOR={}",
            p010_counts.eligible_address_minutes
        );
        let _ = writeln!(o, "P010_ERAM_AVAILABILITY_PERCENT={avail_pct:.6}");
        let _ = writeln!(
            o,
            "P010_TRACKER_MEASUREMENTS_BEFORE_WINDOW_FILTER={n_meas_before_p010_window}"
        );
        let _ = writeln!(o, "P010_TRACKER_MEASUREMENTS_AFTER_WARMUP_FILTER={n_meas}");
        let _ = writeln!(o, "P010_CAP_MAX_ADDRESSES={P010_MAX_ADDRESSES}");
        let _ = writeln!(
            o,
            "P010_CAP_MAX_ELIGIBLE_ADDRESS_MINUTES={P010_MAX_ELIGIBLE_ADDRESS_MINUTES}"
        );
        let _ = writeln!(o, "P010_CAP_MAX_SFDPS_INPUT_BYTES={P010_MAX_SFDPS_BYTES}");
        let _ = writeln!(o, "P010_CAP_STATUS=within-limits");
        let _ = writeln!(o, "P010_INTERPRETATION=local-P010-comparison-only; not-P006-holdout; no-causal-or-safety-claim");
        let _ = writeln!(
            o,
            "P010_PRIVATE_OUTPUTS=p002.txt,samples.csv; do-not-publish-samples.csv"
        );
    }
    let _ = writeln!(
        o,
        "  scored commit U (--frozen-commit): {}; frozen candidate C (--candidate-commit): {}; U differs from C in {SEALED_LIST_PATH} only (checked with git diff --name-only C U, when both are given)",
        commits.frozen.as_deref().unwrap_or("none given"),
        commits.candidate.as_deref().unwrap_or("none given")
    );
    let _ = writeln!(
        o,
        "  binary built from: {} (build tree clean: {})",
        option_env!("OA_BUILD_COMMIT").unwrap_or("unknown"),
        option_env!("OA_BUILD_DIRTY").map_or("unknown", |d| if d == "0" { "yes" } else { "no" })
    );
    let _ = writeln!(
        o,
        "  scorer binary sha256: {}; damage report sha256: {} (excluded windows total {} s); ledger: {}",
        info.binary_sha,
        info.damage_sha,
        info.window_total_s,
        ledger.display()
    );
    for p in &partials_left {
        eprintln!(
            "p002: a leftover {} from an earlier run was replaced",
            p.display()
        );
        let _ = writeln!(
            o,
            "  a leftover {} from an earlier run was found and replaced",
            p.display()
        );
    }
    let _ = writeln!(
        o,
        "  P-003: M2t = sign(a) * max(0, |a| - {ROUND_TOL_S} s * v) of the time-fitted along-track error a and the reference ground speed v; M3t = hypot(cross, M2t); all three systems alike; headline position metrics are M1 and M3t (no 'better than ERAM' claim may rest on M2 or M3); M4 and M7 are also given on M3t"
    );
    let _ = writeln!(
        o,
        "  ADS-B clip: {} sha256={} (files={}, bytes={})",
        clip_path.display(),
        clip_hash.sha256,
        clip_hash.files,
        clip_hash.bytes
    );
    let _ = writeln!(o, "  SFDPS dir: {sfdps_dir_text}");
    for l in &sfdps_inputs {
        let _ = writeln!(o, "    {l}");
    }
    let _ = writeln!(o, "  split: {SPLIT_TEXT}");
    let _ = writeln!(
        o,
        "  set A {} reports ({} after counting), set B {} reports -> reference ({} legs); tracker measurements {n_meas}",
        split.set_a, set_a_reports, split.set_b, built.legs.len()
    );
    if timing_report.profile == ScoringTimingProfile::Fixed1030120V1 {
        let _ = writeln!(o, "SCORING_TIMING_PROFILE=fixed-10-30-120-v1");
        let _ = writeln!(
            o,
            "SCORING_TIMING_SOURCE=versioned-constants; no measurement-cadence statistics computed"
        );
        let _ = writeln!(
            o,
            "SCORING_EXPECTED_INTERVAL_NS={}",
            timing_report.expected_interval_ns
        );
        let _ = writeln!(
            o,
            "SCORING_INIT_INTERVAL_NS={}",
            timing_report.init_interval_ns
        );
        let _ = writeln!(o, "SCORING_DROP_AFTER_NS={}", timing_report.drop_after_ns);
        let _ = writeln!(
            o,
            "SCORING_OUTPUT_CAPACITY={}",
            timing_report.tracker.out_capacity
        );
        let _ = writeln!(o, "SCORING_INTERVAL_STATS=not-computed");
        let _ = writeln!(o, "SCORING_TIMING_INPUTS=ADS-B set-A measurements only as tracker inputs; no set-B truth or ERAM/SFDPS cadence used for parameter selection");
    }
    if p010.is_some() && timing_report.profile == ScoringTimingProfile::LegacyDayIntervals {
        let source = match timing_report.source {
            ScoringTimingSource::MeasuredIntervals => "measured-intervals",
            ScoringTimingSource::LegacyOneSecondFallback => "legacy-one-second-fallback",
            ScoringTimingSource::FixedProfileConstants => "versioned-constants",
        };
        let _ = writeln!(o, "SCORING_TIMING_PROFILE=legacy-day-intervals");
        let _ = writeln!(o, "SCORING_TIMING_SOURCE={source}");
        let _ = writeln!(
            o,
            "SCORING_TIMING_INPUTS=full-ADS-B-set-A-before-warmup-filter"
        );
        let _ = writeln!(
            o,
            "SCORING_EXPECTED_INTERVAL_NS={}",
            timing_report.expected_interval_ns
        );
        let _ = writeln!(
            o,
            "SCORING_INIT_INTERVAL_NS={}",
            timing_report.init_interval_ns
        );
        let _ = writeln!(o, "SCORING_DROP_AFTER_NS={}", timing_report.drop_after_ns);
        let _ = writeln!(
            o,
            "SCORING_OUTPUT_CAPACITY={}",
            timing_report.tracker.out_capacity
        );
        if let Some(intervals) = timing_report.intervals {
            let _ = writeln!(o, "SCORING_INTERVAL_N={}", intervals.n);
            let _ = writeln!(o, "SCORING_INTERVAL_P50_NS={}", intervals.p50_ns);
            let _ = writeln!(o, "SCORING_INTERVAL_P90_NS={}", intervals.p90_ns);
            let _ = writeln!(o, "SCORING_INTERVAL_P99_NS={}", intervals.p99_ns);
        } else {
            let _ = writeln!(o, "SCORING_INTERVAL_STATS=empty-one-second-fallback");
        }
    }
    if oa_publication_grid {
        let _ = writeln!(o, "OA_PUBLICATION_GRID=sfdps-v1");
        let _ = writeln!(o, "OA_PUBLICATION_GRID_MODE=explicit development-only OpenATCA tick quantization before legacy adaptation, pairing, and scoring; tracker core and replay unchanged");
        let _ = writeln!(o, "OA_PUBLICATION_GRID_POSITION=nearest whole arcsecond, then six-decimal-degree publication rendering; exact signed integer rational conversions; ties half away from zero; canonical E7 output");
        let _ = writeln!(o, "OA_PUBLICATION_GRID_VELOCITY=east and north components independently rounded from raw tracker f32 m/s to nearest 1 kt before legacy 0.1-kt conversion; nonfinite pairs retain input components and are counted; finite components beyond {} kt refuse the run; M5 remains vector magnitude, not scalar ground speed", MAX_PUBLICATION_COMPONENT_KT);
        let _ = writeln!(
            o,
            "OA_PUBLICATION_GRID_INVALID_POSITION_ROWS={}",
            publication_grid_counts.invalid_position_rows
        );
        let _ = writeln!(
            o,
            "OA_PUBLICATION_GRID_UNAVAILABLE_VELOCITY_ROWS={}",
            publication_grid_counts.unavailable_velocity_rows
        );
        let _ = writeln!(
            o,
            "OA_PUBLICATION_GRID_ZERO_VELOCITY_ROWS={}",
            publication_grid_counts.zero_velocity_rows
        );
        let _ = writeln!(
            o,
            "OA_PUBLICATION_GRID_ROUNDED_TO_ZERO_VELOCITY_ROWS={}",
            publication_grid_counts.rounded_to_zero_velocity_rows
        );
        let _ = writeln!(o, "OA_PUBLICATION_GRID_ZERO_COUNTER_SEMANTICS=zero_velocity_rows counts exact input (0,0); rounded_to_zero_velocity_rows counts nonzero input vectors quantized to (0,0)");
    }
    let _ = writeln!(
        o,
        "  bootstrap: {BOOT_N} resamples of whole aircraft, splitmix64 seed {BOOT_SEED}, 95% interval = ranks 25 and 975 of the sorted resampled values; every table uses the same resamples"
    );
    let _ = writeln!(
        o,
        "  statistics: median and p95 are nearest-rank over absolute values, rms over all; cross-track, along-track, horizontal as oa-bench defines them (truth frame); ground-speed error in m/s (system minus truth)"
    );
    let _ = writeln!(
        o,
        "  P-004: an aircraft's scored legs are the truth legs that contain its grid samples; every system, the passthrough included, is scored on those legs and no others"
    );
    for (w, n) in windows.iter().zip(&win_removed) {
        let _ = writeln!(
            o,
            "  excluded window (P-007) {} to {}: {n} grid samples removed for all three systems",
            w.from, w.to
        );
    }
    let _ = writeln!(
        o,
        "  excluded windows: {}; grid samples removed in all: {n_win_removed}",
        windows.len()
    );
    let _ = writeln!(
        o,
        "  grid: {grid} samples on {n_ac} aircraft (ERAM's paired samples on each aircraft's most-paired ERAM flight, as before, finite cross/along)"
    );
    let _ = writeln!(
        o,
        "  set sizes: grid {grid}; two-way common set (OpenATCA and ERAM both have a value; M1, M3t, M5 and their breakdowns, and the claim): {n_two}; three-way common set (passthrough too; the reference-floor test only): {n_common}; M4, M4t, M7, M7t are on all {grid} grid samples"
    );
    let _ = writeln!(
        o,
        "  inputs: OpenATCA used ADS-B only (set A); ERAM fuses radar and ADS-B; the reference comes from the same ADS-B feed (set B); passthrough is set A as reported"
    );
    let _ = writeln!(o);

    // Coverage and exclusions.
    let _ = writeln!(o, "COVERAGE AND EXCLUSIONS (per system and reason)");
    let _ = writeln!(
        o,
        "  OpenATCA output rows scored: {oa_rows}; rows flagged uncertain by the tracker (95% radius over 926 m), all scored: {withheld}"
    );
    let _ = writeln!(
        o,
        "  passthrough rows paired (one per distinct nearest set-A report of the grid): {pt_chosen}"
    );
    let _ = writeln!(
        o,
        "  OpenATCA update rows flagged reacquired (they define the reacquired group, within {} s after each): {n_reacq_updates}",
        REACQ_WINDOW_MS / 1000
    );
    // P-004 section 1 and P-006 section 2: the coverage under both rules, on the record.
    let _ = writeln!(
        o,
        "  COVERAGE UNDER THE TWO SCORED-LEG RULES (grid samples with a value within 0.5 s)"
    );
    let _ = writeln!(
        o,
        "    P-003 rule 11, replaced by P-004 (each system on its own most-sampled flight; grid = ERAM's samples on its own most-sampled flight, {} samples):",
        old_cov[ER].1
    );
    for (sys, (hit, all)) in SYS.iter().zip(old_cov) {
        let _ = writeln!(
            o,
            "      {sys}: {hit} of {all} ({:.2} %), misses {}",
            100.0 * hit as f64 / all.max(1) as f64,
            all - hit
        );
    }
    let _ = writeln!(
        o,
        "    P-004 (every system on the truth legs that contain the grid samples; used for every table; grid {grid} samples):"
    );
    for (s, sys) in SYS.iter().enumerate() {
        let valued = rows
            .iter()
            .filter(|r| r.val.get(s).is_some_and(Option::is_some))
            .count();
        let _ = writeln!(
            o,
            "      {sys}: {valued} of {grid} ({:.2} %), misses {}",
            100.0 * valued as f64 / grid.max(1) as f64,
            grid - valued
        );
    }
    let _ = writeln!(
        o,
        "  MISS CLASSES (P-004 section 3; every grid sample without a value is in exactly one class; classes are judged by the system's own rows of the address: OpenATCA's output rows, ERAM's paired samples, the set-A reports)"
    );
    for (s, sys) in SYS.iter().enumerate() {
        let c = classes.get(s).copied().unwrap_or([0; 5]);
        let valued = rows
            .iter()
            .filter(|r| r.val.get(s).is_some_and(Option::is_some))
            .count();
        let _ = writeln!(
            o,
            "    {sys}: {} misses: {}",
            c.iter().sum::<u64>(),
            MISS_NAMES
                .iter()
                .zip(c)
                .map(|(n, k)| format!("{n}: {k}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
        debug_assert_eq!(c.iter().sum::<u64>() as usize, grid - valued);
    }
    for (s, sys) in SYS.iter().enumerate() {
        let valued = rows
            .iter()
            .filter(|r| r.val.get(s).is_some_and(Option::is_some))
            .count();
        let _ = writeln!(
            o,
            "  {sys}: grid samples with a value {valued} of {grid}; misses {}: [{}] aircraft with no paired samples at all, [{}] no paired sample within 0.5 s of the grid time",
            grid - valued,
            miss.get(s).map_or(0, |m| m[0]),
            miss.get(s).map_or(0, |m| m[1]),
        );
    }
    for (sys, (n_ac_unfit, n_samples)) in SYS.iter().zip(unfitted) {
        let _ = writeln!(
            o,
            "  {sys}: aircraft with no usable time fit (no common sample at {FIT_MIN_SPEED_MPS} m/s or more): {n_ac_unfit}, with {n_samples} samples, scored UNFITTED (offset 0 s) in M2, M3, M2t, M3t, M4, M6 and M7 per P-003 resolution 1"
        );
    }
    let _ = writeln!(
        o,
        "  OpenATCA grid samples that are coasting rows (scored, broken out): {}",
        rows.iter()
            .filter(|r| r
                .val
                .first()
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.pt.coast != 0))
            .count()
    );
    let sets: [(&str, &Paired, &Dropped); 3] = [
        (
            "OpenATCA",
            &Paired {
                pairs: PairTable::default(),
                ex: oa_ex,
                assoc_counts: oa_assoc,
            },
            &oa_drop,
        ),
        ("ERAM", &ep, &er_drop),
        ("Passthrough", &pp, &pt_drop),
    ];
    for (name, p, d) in sets {
        let _ = writeln!(o, "  {name} flights by association status (the 'no truth' statuses are 'no ADS-B reference for the aircraft'; the implausible statuses are 'association flagged'):");
        for (n, c) in astatus::NAMES.iter().zip(p.assoc_counts) {
            if c > 0 {
                let _ = writeln!(o, "      {n}: {c} flights");
            }
        }
        let _ = writeln!(
            o,
            "  {name} pairing exclusions (samples, by scope and reason):"
        );
        for ((scope, why), c) in &p.ex {
            let _ = writeln!(o, "      {scope}: {why}: {c}");
        }
        let _ = writeln!(
            o,
            "  {name} paired samples left out of the grid: {} on a leg with no grid sample of the aircraft, {} without finite cross/along (truth nearly stationary), {} at a time already used",
            d.off_scored_legs, d.no_cross_along, d.duplicate_time
        );
    }
    let _ = writeln!(o);

    // Damage visibility (P-006 section 6): what the readers counted, as printed.
    let _ = writeln!(
        o,
        "INPUT DAMAGE AND INGEST DROPS (as the readers counted them)"
    );
    let _ = writeln!(
        o,
        "  ADS-B clip: frames that did not decode {}; records dropped from decoded frames {}; rows left out in a sealed window {}",
        loader.clip_bad_frames, loader.clip_dropped_records, loader.clip_sealed
    );
    let _ = writeln!(
        o,
        "  SFDPS: replay state hash {}; frames and records left out in a sealed window {}",
        loader.replay_hash, loader.sfdps_sealed
    );
    for n in &loader.notes {
        let _ = writeln!(o, "  SFDPS loader note: {n}");
    }
    for c in &loader.counts {
        let _ = writeln!(o, "  SFDPS ingest: {c}");
    }
    let _ = writeln!(o);

    // Offsets.
    let _ = writeln!(
        o,
        "TIME OFFSETS (per aircraft least squares of along = tau * v on the aircraft's common samples with truth speed >= {FIT_MIN_SPEED_MPS} m/s, applied to every sample of the aircraft; tau > 0: the system is ahead of the reference)"
    );
    for (s, sys) in SYS.iter().enumerate() {
        let t: Vec<f64> = taus
            .iter()
            .filter_map(|t| t.get(s).copied().flatten())
            .collect();
        let nofit = n_ac - t.len();
        let mean = t.iter().sum::<f64>() / t.len().max(1) as f64;
        let _ = writeln!(
            o,
            "  {sys}: aircraft fitted {} ({} left unfitted, raw used); tau s: median {:.3}, p5 {:.3}, p25 {:.3}, p75 {:.3}, p95 {:.3}, min {:.3}, max {:.3}, mean {:.3}; spread (IQR) {:.3}",
            t.len(),
            nofit,
            signed_rank(&t, 50),
            signed_rank(&t, 5),
            signed_rank(&t, 25),
            signed_rank(&t, 75),
            signed_rank(&t, 95),
            t.iter().copied().fold(f64::INFINITY, f64::min),
            t.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            mean,
            signed_rank(&t, 75) - signed_rank(&t, 25)
        );
    }
    let _ = writeln!(o);

    // Signed raw errors, a sanity view (the bench reports ERAM's along-track mean near +100 m).
    let _ = writeln!(
        o,
        "SIGNED RAW ERRORS (m; signed mean and median of the unfitted errors; along > 0: ahead of the reference)"
    );
    for (s, sys) in SYS.iter().enumerate() {
        for (set, only_common) in [
            ("all valued grid samples", false),
            ("three-way common set", true),
        ] {
            let sel = |f: &dyn Fn(&Val) -> f64| -> Vec<f64> {
                rows.iter()
                    .filter(|r| !only_common || r.common())
                    .filter_map(|r| r.val.get(s).and_then(|v| v.as_ref()))
                    .map(f)
                    .collect()
            };
            let (c, a) = (sel(&|v| v.pt.cross), sel(&|v| v.pt.along));
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
            let _ = writeln!(
                o,
                "  {sys}, {set} (n={}): cross mean {:.2} median {:.2}; along mean {:.2} median {:.2}",
                a.len(),
                mean(&c),
                signed_rank(&c, 50),
                mean(&a),
                signed_rank(&a, 50)
            );
        }
    }
    let _ = writeln!(o);

    // The primary metrics and the claim rule (P-006 section 3).
    let _ = writeln!(
        o,
        "PRIMARY METRICS AND THE CLAIM RULE (P-006 section 3): three metrics are primary: M1 median, M5 ground-speed error median (absolute), M4t at 300 m. A claim of 'better than ERAM' may rest on a primary metric only; every other metric is secondary."
    );
    let _ = writeln!(
        o,
        "  rule: the 95% interval of OpenATCA minus ERAM excludes zero in OpenATCA's favour on hold-out A and on hold-out B, and hold-out C's interval does not exclude zero in ERAM's favour. One run scores one day, so each day's verdict is given here and the rule across days is applied by `p002 --combine <dirA> <dirB> <dirC>`."
    );
    for ((_, _, name), p) in PRIMARY.iter().zip(&pieces) {
        let _ = writeln!(
            o,
            "  PRIMARY {name}: OpenATCA minus ERAM {} [{}, {}]: this day: {}; the day's part of the rule (favours OpenATCA, needed on A and B): {}; (not in ERAM's favour, needed on C): {}",
            p.diff,
            p.lo,
            p.hi,
            p.verdict.text(),
            if p.verdict == Verdict::OpenAtca { "met" } else { "not met" },
            if p.verdict != Verdict::Eram { "met" } else { "not met" },
        );
    }
    let _ = writeln!(
        o,
        "  the rule across the three days is met for a metric only if the A and B parts and the C part are all met: not decidable from one day"
    );
    let _ = writeln!(o, "  machine-readable (read by --combine):");
    o.push_str(&claim_lines(&info, &pieces, windows, &win_removed));
    let _ = writeln!(o);

    // M1 to M3, M5.
    write_metric(
        &mut o,
        "M1 cross-track error",
        "m",
        &m1,
        &three(Kind::Cross),
        Some(0),
    );
    write_metric(
        &mut o,
        "M2 along-track error, raw",
        "m",
        &m2r,
        &three(Kind::AlongRaw),
        None,
    );
    write_metric(
        &mut o,
        "M2 along-track error, time-fitted",
        "m",
        &m2f,
        &three(Kind::AlongFit),
        None,
    );
    write_metric(
        &mut o,
        "M3 horizontal error after time fit",
        "m",
        &m3,
        &three(Kind::HFit),
        None,
    );
    write_metric(
        &mut o,
        "M2t along-track error, rounding-tolerant (P-003)",
        "m",
        &m2t,
        &three(Kind::AlongTol),
        None,
    );
    write_metric(
        &mut o,
        "M3t horizontal error, rounding-tolerant (P-003)",
        "m",
        &m3t,
        &three(Kind::HTol),
        None,
    );
    write_metric(
        &mut o,
        "M5 ground-speed error",
        "m/s",
        &m5s,
        &three(Kind::Speed),
        Some(0),
    );
    write_metric(
        &mut o,
        "M5 track-angle error",
        "deg",
        &m5t,
        &three(Kind::Track),
        None,
    );

    // M4.
    for (what, h) in [
        (
            "time-fitted horizontal error (M3)",
            (|v: &Val| v.h_fit) as fn(&Val) -> f64,
        ),
        (
            "rounding-tolerant horizontal error (M3t, P-003)",
            |v: &Val| v.h_tol,
        ),
    ] {
        let _ = writeln!(
            o,
            "M4{} coverage-penalised share of ALL {grid} grid samples with {what} within threshold (a miss is a failure){}",
            if what.contains("M3t") { "t" } else { "" },
            if what.contains("M3t") {
                "; PRIMARY at 300 m (P-006 section 3)"
            } else {
                ""
            }
        );
        for (name, thr) in [("within 100 m", 100.0), ("within 300 m", 300.0)] {
            let (c, nan) = m4_counts(&rows, n_ac, h, thr);
            let _ = writeln!(
                o,
                "  {name} (present values not a number, counted as failures: OpenATCA {}, ERAM {}, Passthrough {}):",
                nan[0], nan[1], nan[2]
            );
            write_share(&mut o, &c, &draws, 1.0);
        }
        let _ = writeln!(o);
    }

    // M6.
    let m6r = m6_counts(&rows, n_ac);
    let (win, lose, tie) = (m6r.win, m6r.lose, m6r.tie);
    let _ = writeln!(
        o,
        "M6 per-aircraft: share of aircraft (with common samples) where OpenATCA's M3 RMS is lower than ERAM's"
    );
    let _ = writeln!(
        o,
        "  OpenATCA lower {win}, ERAM lower {lose}, tied {tie}; aircraft compared {}; aircraft with an RMS that is not a number (a failure for the share, left out of the counts and the sign test): OpenATCA {}, ERAM {}",
        win + lose + tie,
        m6r.nan[0],
        m6r.nan[1]
    );
    write_share(&mut o, &[m6r.share], &draws, 1.0);
    let _ = writeln!(
        o,
        "  sign test (exact binomial, p = 1/2, ties dropped, two-sided): p = {:.3e} on {win} of {}",
        sign_test_p(win, win + lose),
        win + lose
    );
    let _ = writeln!(
        o,
        "  passthrough within 10%: not defined for M6 (it compares OpenATCA with ERAM)"
    );
    let _ = writeln!(o);

    // M7.
    let _ = writeln!(
        o,
        "M7 large errors: samples over {LARGE_M} m (time-fitted horizontal) on ALL {grid} grid samples; a miss is counted as a failure"
    );
    let l7 = large_counts(&rows, n_ac, |v| v.h_fit);
    for (s, sys) in SYS.iter().enumerate() {
        let (f, _) = l7.cnt.get(s).map_or((0, 0), Cnt::point);
        let misses = grid
            - rows
                .iter()
                .filter(|r| r.val.get(s).is_some_and(Option::is_some))
                .count();
        let _ = writeln!(
            o,
            "  {sys}: failures {f} = {} over {LARGE_M} m or not a number (of which {} not a number) + {misses} misses (raw, unfitted over {LARGE_M} m: {})",
            l7.over.get(s).copied().unwrap_or(0),
            l7.nan.get(s).copied().unwrap_or(0),
            l7.raw_over.get(s).copied().unwrap_or(0)
        );
    }
    write_share(&mut o, &l7.cnt, &draws, 10_000.0);
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "M7t large errors on the rounding-tolerant horizontal error (M3t, P-003): samples over {LARGE_M} m on ALL {grid} grid samples; a miss is counted as a failure"
    );
    let l7t = large_counts(&rows, n_ac, |v| v.h_tol);
    for (s, sys) in SYS.iter().enumerate() {
        let (f, _) = l7t.cnt.get(s).map_or((0, 0), Cnt::point);
        let _ = writeln!(
            o,
            "  {sys}: failures {f} (over {LARGE_M} m, or not a number: {}, or missed)",
            l7t.nan.get(s).copied().unwrap_or(0)
        );
    }
    write_share(&mut o, &l7t.cnt, &draws, 10_000.0);
    let _ = writeln!(o);

    // Breakdowns.
    write_strata(
        &mut o,
        "M1 cross-track error (m)",
        &rows,
        n_ac,
        &draws,
        Kind::Cross,
    );
    write_strata(
        &mut o,
        "M3 horizontal error after time fit (m)",
        &rows,
        n_ac,
        &draws,
        Kind::HFit,
    );
    write_strata(
        &mut o,
        "M5 ground-speed error (m/s)",
        &rows,
        n_ac,
        &draws,
        Kind::Speed,
    );
    write_strata(
        &mut o,
        "M5 track-angle error (deg)",
        &rows,
        n_ac,
        &draws,
        Kind::Track,
    );

    // P-003 flag groups (P-002 item 9: the low-confidence breakdown is the uncertain group).
    write_flag_groups(
        &mut o,
        "M1 cross-track error (m)",
        &rows,
        n_ac,
        &draws,
        Kind::Cross,
    );
    write_flag_groups(
        &mut o,
        "M3t rounding-tolerant horizontal error (m)",
        &rows,
        n_ac,
        &draws,
        Kind::HTol,
    );
    write_flag_groups(
        &mut o,
        "M5 ground-speed error (m/s)",
        &rows,
        n_ac,
        &draws,
        Kind::Speed,
    );
    write_flag_groups(
        &mut o,
        "M5 track-angle error (deg)",
        &rows,
        n_ac,
        &draws,
        Kind::Track,
    );

    // The run's own timings and memory differ on every run, so they are not in the result file
    // (OA-REP-005: two runs on the same inputs write the same bytes).
    let secs = t_start.elapsed().as_secs_f64();
    eprintln!(
        "RUN: tracker {track_secs:.1} s; total {secs:.1} s; peak resident memory {:.0} MB",
        peak_rss_mb()
    );
    let _ = writeln!(
        o,
        "NOTE: OpenATCA's uncertain flag is joined to the paired samples by address and row time in whole milliseconds (the finest time the pair table carries; tracker times are floored to the millisecond); paired OpenATCA samples with no tracker row to read the flags from (or only rows that disagree), shown as the UNMATCHED groups and as `unmatched` in samples.csv: {flag_unmatched}; tracker rows sharing a key with an earlier row: {flag_dup_rows}; keys whose rows disagree: {flag_conflicts}; the reacquired group is not read from the tick rows (they never carry the flag) but from the update rows, P-004 section 2; run is a {} run",
        if mode.holdout { "hold-out" } else { "non-hold-out" }
    );

    // P-008 section 5: a hold-out run's ledger line goes in after both partial files are written
    // and before either is renamed; a clip already in the ledger was refused above.
    let line = mode.holdout.then(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        ledger_line(&clip_hash.sha256, commits, now)
    });
    // Create-new via `.partial` names, flushed, then renamed (P-006 section 7, P-008).
    publish_outputs(out_dir, &csv, &o, line.as_deref().map(|l| (ledger, l)))?;
    Ok(o)
}

/// Shares (scaled by `scale`) of the counts of OpenATCA, ERAM, passthrough with intervals, the
/// OpenATCA-minus-ERAM difference, and the passthrough rule. A single count prints as OpenATCA's.
fn write_share(o: &mut String, c: &[Cnt], draws: &[Vec<u32>], scale: f64) {
    let boots: Vec<Vec<f64>> = c.iter().map(|x| x.boot(draws)).collect();
    let pts: Vec<f64> = c
        .iter()
        .map(|x| {
            let (n, d) = x.point();
            if d == 0 {
                f64::NAN
            } else {
                n as f64 / d as f64 * scale
            }
        })
        .collect();
    let prec = if scale > 1.0 { 1 } else { 4 };
    for (s, (p, b)) in pts.iter().zip(&boots).enumerate() {
        let (lo, hi) = interval(b.iter().map(|v| v * scale));
        let name = if c.len() == 1 {
            "OpenATCA"
        } else {
            SYS.get(s).copied().unwrap_or("")
        };
        let (n, d) = c.get(s).map_or((0, 0), Cnt::point);
        let _ = writeln!(
            o,
            "    {name:<12} {} ({n} of {d})",
            fmt_ci(*p, lo, hi, prec)
        );
    }
    if let (Some(a), Some(b), Some(ba), Some(bb)) =
        (pts.first(), pts.get(1), boots.first(), boots.get(1))
    {
        if c.len() > 1 {
            let (lo, hi) = interval(ba.iter().zip(bb).map(|(x, y)| (x - y) * scale));
            let _ = writeln!(
                o,
                "    OpenATCA minus ERAM  {}",
                fmt_ci(a - b, lo, hi, prec)
            );
        }
    }
    if let (Some(a), Some(p), true) = (pts.first(), pts.get(PT), c.len() > 2) {
        let _ = writeln!(o, "    passthrough within 10%: {}", floor_note(*a, *p));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oa_bench::tracker_input::legacy_from_track_samples;

    fn p010_test_window() -> P010Window {
        parse_p010_window(
            Some("2026-11-30T05:00:00Z"),
            Some("2026-11-30T06:00:00Z"),
            Some("2026-12-01T00:00:00Z"),
            Some("zau-v1"),
        )
        .unwrap()
        .unwrap()
    }

    // REQ: OA-TRK-410
    #[test]
    fn p010_requires_exact_typed_window_and_explicit_box_profile() {
        assert!(parse_p010_window(None, None, None, None).unwrap().is_none());
        assert!(parse_p010_window(
            Some("2026-11-30T05:00:00.000Z"),
            Some("2026-11-30T06:00:00Z"),
            Some("2026-12-01T00:00:00Z"),
            Some("zau-v1"),
        )
        .is_err());
        assert!(parse_p010_window(
            Some("2026-11-30T05:00:00Z"),
            Some("2026-11-30T06:00:00Z"),
            Some("2026-12-01T00:00:00Z"),
            Some("other"),
        )
        .is_err());
        let day_start = days_from_civil(2026, 11, 30) * 86_400 * 1_000_000_000;
        assert!(!p010_clip_time_in_day(day_start - 1));
        assert!(p010_clip_time_in_day(day_start));
        assert!(!p010_clip_time_in_day(day_start + 86_400 * 1_000_000_000));
    }

    // REQ: OA-TRK-411
    #[test]
    fn p010_eligibility_requires_a_single_usable_truth_leg_inside_the_box() {
        let window = p010_test_window();
        let minute = window.score_from_ns / 1_000_000;
        let leg = TruthLeg {
            addr: 0x123456,
            start_ms: minute,
            end_ms: minute,
            callsign: None,
            start: oa_bench::truth::LegStart::Unknown,
        };
        let query = |_: u32, _: i64| {
            Ok(TruthSample {
                lat: 41.0,
                lon: -87.0,
                baro_alt_ft: Some(20_000),
                vel_east_mps: 0.0,
                vel_north_mps: 60.0,
                vert_rate_fpm: None,
                sigma95_h_m: 20.0,
                leg: 0,
            })
        };
        let truth = Truth {
            query: &query,
            legs: std::slice::from_ref(&leg),
        };
        let (eligible, counts) = p010_public_eligibility(&truth, window).unwrap();
        assert_eq!(eligible, BTreeMap::from([((leg.addr, minute), 0)]));
        assert_eq!(counts.eligible_address_minutes, 1);
        assert_eq!(counts.address_minute_opportunities, 1080);
        assert_eq!(counts.no_active_leg_minutes, 1079);
        assert!(p010_keeps_eligible_time(
            &eligible,
            &truth,
            leg.addr,
            minute + 30_000,
            window
        ));
        assert!(!p010_keeps_eligible_time(
            &eligible,
            &truth,
            leg.addr,
            minute - 1,
            window
        ));
        assert!(!p010_keeps_eligible_time(
            &eligible,
            &truth,
            leg.addr,
            window.score_to_ns / 1_000_000,
            window,
        ));
        let ambiguous = [
            leg,
            TruthLeg {
                start_ms: minute,
                ..leg
            },
        ];
        let truth = Truth {
            query: &query,
            legs: &ambiguous,
        };
        let (eligible, counts) = p010_public_eligibility(&truth, window).unwrap();
        assert!(eligible.is_empty());
        assert_eq!(counts.ambiguous_candidates, 1);
    }

    // REQ: OA-TRK-411
    #[test]
    fn p010_cohort_digest_is_order_independent_and_commits_membership() {
        let domain = b"openatca-p010-public-eligible-address-minute-truth-leg-v1\0";
        let forward_rows = BTreeMap::from([
            ((0x123456, 1_796_018_400), 3),
            ((0xabcdef, 1_796_018_460), 4),
        ]);
        let reverse_rows = BTreeMap::from([
            ((0xabcdef, 1_796_018_460), 4),
            ((0x123456, 1_796_018_400), 3),
        ]);
        let changed_rows = BTreeMap::from([
            ((0x123456, 1_796_018_400), 3),
            ((0xabcdef, 1_796_018_520), 4),
        ]);
        let forward = p010_keyset_sha256(
            domain,
            forward_rows.iter().map(|(&(a, t), &leg)| (a, t, Some(leg))),
        )
        .unwrap();
        let reverse = p010_keyset_sha256(
            domain,
            reverse_rows.iter().map(|(&(a, t), &leg)| (a, t, Some(leg))),
        )
        .unwrap();
        let changed = p010_keyset_sha256(
            domain,
            changed_rows.iter().map(|(&(a, t), &leg)| (a, t, Some(leg))),
        )
        .unwrap();
        let other_domain = p010_keyset_sha256(
            b"openatca-p010-eram-available-address-minute-truth-leg-v1\0",
            forward_rows.iter().map(|(&(a, t), &leg)| (a, t, Some(leg))),
        )
        .unwrap();
        let changed_leg_rows = BTreeMap::from([
            ((0x123456, 1_796_018_400), 9),
            ((0xabcdef, 1_796_018_460), 4),
        ]);
        let changed_leg = p010_keyset_sha256(
            domain,
            changed_leg_rows
                .iter()
                .map(|(&(a, t), &leg)| (a, t, Some(leg))),
        )
        .unwrap();
        assert_eq!(forward, reverse);
        assert_ne!(forward, changed);
        assert_ne!(forward, other_domain);
        assert_ne!(forward, changed_leg);
    }

    // REQ: OA-TRK-411
    #[test]
    fn p010_minute_membership_keeps_the_exact_truth_leg_and_pairing_speed_threshold() {
        let window = p010_test_window();
        let minute = window.score_from_ns / 1_000_000;
        let sample = |leg, speed| TruthSample {
            lat: 41.0,
            lon: -87.0,
            baro_alt_ft: Some(20_000),
            vel_east_mps: 0.0,
            vel_north_mps: speed,
            vert_rate_fpm: None,
            sigma95_h_m: 20.0,
            leg,
        };
        assert_eq!(
            P010_MIN_DIRECTION_SPEED_MPS,
            oa_bench::pair::PairConfig::default().min_speed_mps
        );
        assert!(!p010_truth_eligible(&sample(0, 9.99)));
        assert!(p010_truth_eligible(&sample(0, 10.0)));
        assert!(p010_truth_eligible(&sample(0, 20.0)));
        assert!(!p010_truth_eligible(&sample(0, f64::NAN)));
        let query = |_: u32, t: i64| Ok(sample(if t < minute + 30_000 { 7 } else { 8 }, 20.0));
        let truth = Truth {
            query: &query,
            legs: &[],
        };
        let eligible = BTreeMap::from([((0x123456, minute), 7)]);
        assert!(p010_keeps_eligible_time(
            &eligible,
            &truth,
            0x123456,
            minute + 29_999,
            window
        ));
        assert!(!p010_keeps_eligible_time(
            &eligible,
            &truth,
            0x123456,
            minute + 30_000,
            window
        ));
        assert!(!p010_keeps_eligible_time(
            &eligible, &truth, 0x654321, minute, window
        ));
    }

    // REQ: OA-TRK-411, OA-TRK-412
    #[test]
    fn p010_rejects_the_address_cap_before_minute_work_and_counts_unusable_truth() {
        let window = p010_test_window();
        let legs: Vec<TruthLeg> = (0..=P010_MAX_ADDRESSES)
            .map(|addr| TruthLeg {
                addr: addr as u32,
                start_ms: 0,
                end_ms: 0,
                callsign: None,
                start: oa_bench::truth::LegStart::Unknown,
            })
            .collect();
        let query = |_: u32, _: i64| Err(oa_bench::truth::NoTruth::NoNearbyReport);
        let truth = Truth {
            query: &query,
            legs: &legs,
        };
        assert!(p010_public_eligibility(&truth, window)
            .unwrap_err()
            .contains("P010_REFUSED_ADDRESS_CAP"));

        let minute = window.score_from_ns / 1_000_000;
        let leg = TruthLeg {
            addr: 9,
            start_ms: minute,
            end_ms: minute,
            callsign: None,
            start: oa_bench::truth::LegStart::Unknown,
        };
        let query = |_: u32, _: i64| {
            Ok(TruthSample {
                lat: 41.0,
                lon: -87.0,
                baro_alt_ft: Some(20_000),
                vel_east_mps: 0.0,
                vel_north_mps: 0.0,
                vert_rate_fpm: None,
                sigma95_h_m: 20.0,
                leg: 0,
            })
        };
        let truth = Truth {
            query: &query,
            legs: std::slice::from_ref(&leg),
        };
        let (eligible, counts) = p010_public_eligibility(&truth, window).unwrap();
        assert!(eligible.is_empty());
        assert_eq!(counts.unusable_truth_minutes, 1);
    }

    // REQ: OA-TRK-407
    #[test]
    fn publication_grid_coordinates_use_signed_arcsecond_rounding_and_canonical_e7() {
        assert_eq!(
            quantize_publication_coordinate(12_500, 900_000_000),
            Some(13_890)
        );
        assert_eq!(
            quantize_publication_coordinate(-12_500, 900_000_000),
            Some(-13_890)
        );
        assert_eq!(
            quantize_publication_coordinate(2778, 900_000_000),
            Some(2780)
        );
        assert_eq!(
            quantize_publication_coordinate(2780, 900_000_000),
            Some(2780)
        );
        assert_eq!(
            quantize_publication_coordinate(-2778, 900_000_000),
            Some(-2780)
        );
        assert_eq!(
            quantize_publication_coordinate(900_000_000, 900_000_000),
            Some(900_000_000)
        );
        assert_eq!(
            quantize_publication_coordinate(-900_000_000, 900_000_000),
            Some(-900_000_000)
        );
        assert_eq!(
            quantize_publication_coordinate(1_800_000_000, 1_800_000_000),
            Some(1_800_000_000)
        );
        assert_eq!(
            quantize_publication_coordinate(-1_800_000_000, 1_800_000_000),
            Some(-1_800_000_000)
        );
        assert_eq!(quantize_publication_coordinate(NONE_I32, 900_000_000), None);
        assert_eq!(
            quantize_publication_coordinate(900_000_001, 900_000_000),
            None
        );
        assert_eq!(
            quantize_publication_coordinate(1_800_000_001, 1_800_000_000),
            None
        );
    }

    // REQ: OA-TRK-407
    #[test]
    fn publication_grid_rounds_velocity_components_before_deci_knot_conversion() {
        let half = (0.5 * KT_MPS) as f32;
        let below = f32::from_bits(half.to_bits() - 1);
        let above = f32::from_bits(half.to_bits() + 1);
        assert_eq!(quantize_publication_velocity_component(below), Some(0.0));
        assert_eq!(
            quantize_publication_velocity_component(above),
            Some(KT_MPS as f32)
        );
        assert_eq!(quantize_publication_velocity_component(-below), Some(0.0));
        assert_eq!(
            quantize_publication_velocity_component(-above),
            Some((-KT_MPS) as f32)
        );
        assert_eq!(quantize_publication_velocity_component(0.0), Some(0.0));
        assert_eq!(quantize_publication_velocity_component(f32::NAN), None);
        assert_eq!(quantize_publication_velocity_component(f32::INFINITY), None);
        assert_eq!(
            quantize_publication_velocity_component((3300.0 * KT_MPS) as f32),
            None
        );
    }

    // REQ: OA-TRK-407
    #[test]
    fn publication_grid_quantizes_non_axis_components_before_adapter_deci_knots() {
        let raw_east = [(1.49 * KT_MPS) as f32];
        let raw_north = [(2.51 * KT_MPS) as f32];
        let mut counts = PublicationGridCounts::default();
        let q = publication_grid_columns(
            &[410_000_000],
            &[-880_000_000],
            &raw_east,
            &raw_north,
            &mut counts,
        )
        .unwrap();
        let addr = [1];
        let time = [1_000_000_000];
        let alt = [30_000];
        let coast = [false];
        let legacy = legacy_from_track_samples(&TrackSamples {
            addr: &addr,
            t_ns: &time,
            lat_e7: &q.lat_e7,
            lon_e7: &q.lon_e7,
            alt_ft: &alt,
            vel_east_mps: &q.vel_east_mps,
            vel_north_mps: &q.vel_north_mps,
            coast: &coast,
        })
        .unwrap();
        assert_eq!(legacy.th.vx_dkt, [10]);
        assert_eq!(legacy.th.vy_dkt, [30]);
        assert_eq!(
            (f64::from(raw_east[0]).hypot(f64::from(raw_north[0])) / KT_MPS).round(),
            3.0
        );
    }

    // REQ: OA-TRK-407
    #[test]
    fn publication_grid_counts_refused_rows_and_zero_vectors() {
        let mut counts = PublicationGridCounts::default();
        let columns = publication_grid_columns(
            &[410_000_000, NONE_I32, 410_000_000, 410_000_000],
            &[-880_000_000, -880_000_000, -880_000_000, -880_000_000],
            &[0.0, 20.0, f32::NAN, 0.2],
            &[0.0, 0.0, 30.0, 0.2],
            &mut counts,
        )
        .unwrap();
        assert_eq!(columns.lat_e7.len(), 4);
        assert_eq!(columns.lat_e7[1], NONE_I32);
        assert!(columns.vel_east_mps[2].is_nan());
        assert_eq!(columns.vel_north_mps[2], 30.0);
        assert_eq!(counts.invalid_position_rows, 1);
        assert_eq!(counts.unavailable_velocity_rows, 1);
        assert_eq!(counts.zero_velocity_rows, 1);
        assert_eq!(counts.rounded_to_zero_velocity_rows, 1);
        assert!(publication_grid_columns(
            &[410_000_000],
            &[-880_000_000],
            &[(3300.0 * KT_MPS) as f32],
            &[(2.0 * KT_MPS) as f32],
            &mut counts,
        )
        .is_err());
        assert!(publication_grid_columns(&[1], &[], &[1.0], &[1.0], &mut counts).is_err());
    }

    // REQ: OA-TRK-407
    #[test]
    fn publication_grid_refuses_strict_and_non_scoring_modes() {
        assert!(validate_publication_grid_request(true, false, false, false, false).is_err());
        assert!(validate_publication_grid_request(true, true, true, false, false).is_err());
        assert!(validate_publication_grid_request(true, true, false, true, false).is_err());
        assert!(validate_publication_grid_request(true, true, false, false, true).is_err());
        assert!(validate_publication_grid_request(false, false, false, false, false).is_ok());
        assert!(combine_has_unsupported_profile(&[
            "--combine".into(),
            "day-a".into(),
            "--oa-publication-grid".into(),
            "sfdps-v1".into(),
        ]));
        assert!(!combine_has_unsupported_profile(&[
            "--combine".into(),
            "day-a".into(),
        ]));
    }

    // REQ: OA-TRK-407
    #[test]
    fn publication_grid_stamp_is_opt_in_and_default_csv_header_stays_unchanged() {
        let legacy = samples_csv(&[], &[]);
        assert!(legacy.starts_with("addr,t_ms,phase,band,common,common_two_way,oa_miss,"));
        assert!(!legacy.contains("OA_PUBLICATION_GRID"));
        let exploratory = samples_csv(&[], &[OA_PUBLICATION_GRID_STAMP]);
        assert!(exploratory.starts_with("# OA_PUBLICATION_GRID=sfdps-v1;"));
    }

    #[test]
    fn splitmix_known_values() {
        let mut r = SplitMix(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn below_stays_in_range_and_is_deterministic() {
        let (mut a, mut b) = (SplitMix(BOOT_SEED), SplitMix(BOOT_SEED));
        for _ in 0..1000 {
            let x = a.below(7);
            assert!(x < 7);
            assert_eq!(x, b.below(7));
        }
    }

    #[test]
    fn sign_test_matches_exact_values() {
        assert!((sign_test_p(0, 10) - 2.0 / 1024.0).abs() < 1e-12);
        assert!((sign_test_p(5, 10) - 1.0).abs() < 1e-12);
        // 9 of 10: 2 * (1 + 10) / 1024.
        assert!((sign_test_p(9, 10) - 22.0 / 1024.0).abs() < 1e-12);
        assert!(sign_test_p(0, 0).is_nan());
    }

    #[test]
    fn stat3_uses_nearest_rank_and_rms() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = stat3(&mut v);
        assert_eq!(s[0], 50.0);
        assert_eq!(s[1], 95.0);
        assert!((s[2] - (338_350.0f64 / 100.0).sqrt()).abs() < 1e-9);
        assert!(stat3(&mut []).iter().all(|x| x.is_nan()));
    }

    #[test]
    fn along_tolerance_below_at_and_above_half_second_of_travel() {
        // v = 200 m/s: the tolerance is 100 m.
        assert_eq!(along_tol(60.0, 200.0), 0.0);
        assert_eq!(along_tol(100.0, 200.0), 0.0);
        assert_eq!(along_tol(130.0, 200.0), 30.0);
        assert_eq!(along_tol(0.0, 200.0), 0.0);
        // Zero speed charges nothing.
        assert_eq!(along_tol(7.5, 0.0), 7.5);
    }

    #[test]
    fn along_tolerance_keeps_the_sign() {
        assert_eq!(along_tol(-130.0, 200.0), -30.0);
        assert_eq!(along_tol(-100.0, 200.0), 0.0);
        assert_eq!(along_tol(-60.0, 200.0), 0.0);
        assert!(along_tol(f64::NAN, 200.0).is_nan());
        assert!(along_tol(10.0, f64::NAN).is_nan());
    }

    #[test]
    fn dirty_tree_is_refused_unless_porcelain_is_empty() {
        assert!(!tree_is_dirty(Some("")));
        assert!(!tree_is_dirty(Some("\n")));
        assert!(tree_is_dirty(Some(
            " M crates/oa-track-tools/src/score.rs\n"
        )));
        assert!(tree_is_dirty(Some("?? new.rs\n")));
        // No answer from git is not a clean tree.
        assert!(tree_is_dirty(None));
    }

    #[test]
    fn tau_recovers_a_constant_offset() {
        let pts: Vec<(f64, f64)> = [60.0, 120.0, 200.0].iter().map(|&v| (0.5 * v, v)).collect();
        assert!((fit_tau(pts.iter().copied()).unwrap() - 0.5).abs() < 1e-12);
        // Slow samples do not enter the fit.
        assert!(fit_tau([(10.0, 10.0)].into_iter()).is_none());
    }

    #[test]
    fn interval_picks_ranks_25_and_975() {
        let (lo, hi) = interval((1..=1000).map(f64::from));
        assert_eq!((lo, hi), (25.0, 975.0));
    }

    #[test]
    fn bootstrap_of_one_aircraft_is_its_own_statistic() {
        let col = Col {
            start: vec![0, 3],
            data: vec![1.0, 2.0, 3.0],
        };
        let draws = draws_of(1);
        assert_eq!(draws.len(), BOOT_N);
        let b = col.boot(&draws);
        assert!(b.iter().all(|x| *x == col.point()));
        assert_eq!(draws_of(5), draws_of(5));
    }

    #[test]
    fn nearest_respects_the_tolerance() {
        let p = |t| Pt {
            t,
            cross: 0.0,
            along: 0.0,
            v: 0.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: Tri::No,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        };
        let v = vec![p(1000), p(2000)];
        assert_eq!(nearest(&v, 1400).unwrap().t, 1000);
        assert_eq!(nearest(&v, 1500).unwrap().t, 1000);
        assert!(nearest(&v, 1501).is_some());
        assert!(nearest(&v, 3000).is_none());
    }

    // REQ: OA-TRK-373
    #[test]
    fn a_duplicate_flag_key_is_counted_and_never_overwritten() {
        let ns = |ms: i64| ms * 1_000_000;
        // Two rows in one millisecond that agree, then two that disagree, then a unique key.
        let j = flag_join(
            &[1, 1, 2, 2, 3],
            &[ns(5), ns(5) + 400_000, ns(7), ns(7) + 1, ns(9)],
            &[true, true, false, true, false],
            &[false, false, false, false, true],
        )
        .unwrap();
        assert_eq!((j.duplicate_rows, j.conflict_keys), (2, 1));
        assert_eq!(flag_lookup(&j, 1, 5), (Tri::Yes, Tri::No));
        // Disagreeing rows: neither wins; the sample is unmatched.
        assert_eq!(flag_lookup(&j, 2, 7), (Tri::Unmatched, Tri::Unmatched));
        assert_eq!(flag_lookup(&j, 3, 9), (Tri::No, Tri::Yes));
        // A key with no tracker row is unmatched, not "no".
        assert_eq!(flag_lookup(&j, 3, 10), (Tri::Unmatched, Tri::Unmatched));
        assert_eq!(flag_lookup(&j, 4, 9), (Tri::Unmatched, Tri::Unmatched));
        // A third row after a conflict does not count the key twice.
        let j = flag_join(
            &[1, 1, 1],
            &[ns(1), ns(1), ns(1)],
            &[true, false, true],
            &[false; 3],
        )
        .unwrap();
        assert_eq!((j.duplicate_rows, j.conflict_keys), (2, 1));
        // Columns of different lengths are refused.
        assert!(flag_join(&[1], &[1, 2], &[true], &[true]).is_err());
    }

    // REQ: OA-TRK-373
    #[test]
    fn an_unmatched_sample_is_in_its_own_group_and_not_in_no() {
        let pt = |u| Pt {
            t: 0,
            cross: 0.0,
            along: 0.0,
            v: 0.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: u,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        };
        let val = |u| Val {
            pt: pt(u),
            along_fit: 0.0,
            h_raw: 0.0,
            h_fit: 0.0,
            along_tol: 0.0,
            h_tol: 0.0,
        };
        let row = |u| Row {
            ac: 0,
            t: 0,
            addr: 1,
            phase: 0,
            band: 0,
            val: [Some(val(u)), None, None],
        };
        let groups = flag_groups();
        let find = |name: &str| groups.iter().find(|g| g.0 == name).unwrap();
        for (u, no, yes, un) in [
            (Tri::No, true, false, false),
            (Tri::Yes, false, true, false),
            (Tri::Unmatched, false, false, true),
        ] {
            let r = row(u);
            assert_eq!((find("OpenATCA not uncertain").1)(&r), no);
            assert_eq!((find("OpenATCA uncertain").1)(&r), yes);
            assert_eq!((find("OpenATCA uncertain UNMATCHED").1)(&r), un);
        }
    }

    // REQ: OA-TRK-374
    #[test]
    fn a_not_a_number_error_is_a_failure_in_the_large_error_metrics() {
        assert!(fails_over(f64::NAN, LARGE_M));
        assert!(fails_over(LARGE_M + 1.0, LARGE_M));
        assert!(fails_over(f64::INFINITY, LARGE_M));
        assert!(!fails_over(LARGE_M, LARGE_M));
        assert!(!fails_over(0.0, LARGE_M));
        // The same sample is a failure of the M4 test as well: `h <= thr` is false for NaN.
        assert!(f64::NAN.partial_cmp(&100.0).is_none());
        // A sample whose truth speed is not known has a not-a-number M3t and fails M7t.
        let h_tol = 5.0f64.hypot(along_tol(3.0, f64::NAN));
        assert!(h_tol.is_nan() && fails_over(h_tol, LARGE_M));
    }

    // REQ: OA-TRK-374
    #[test]
    fn aircraft_with_no_time_fit_are_counted_and_their_samples_are_named() {
        let pt = Pt {
            t: 0,
            cross: 1.0,
            along: 1.0,
            v: 100.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: Tri::No,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        };
        let val = Val {
            pt,
            along_fit: 1.0,
            h_raw: 1.0,
            h_fit: 1.0,
            along_tol: 1.0,
            h_tol: 1.0,
        };
        let row = |ac, vals: [Option<Val>; 3]| Row {
            ac,
            t: 0,
            addr: ac as u32,
            phase: 0,
            band: 0,
            val: vals,
        };
        let rows = vec![
            row(0, [Some(val), Some(val), None]),
            row(0, [Some(val), Some(val), None]),
            row(1, [Some(val), None, None]),
        ];
        // Aircraft 0 is fitted for OpenATCA only; aircraft 1 has no fit at all.
        let taus = vec![[Some(0.1), None, None], [None, None, None]];
        let u = unfitted_counts(&rows, &taus);
        assert_eq!(u[0], (1, 1)); // OpenATCA: aircraft 1, one sample
        assert_eq!(u[1], (1, 2)); // ERAM: aircraft 0, two samples
        assert_eq!(u[2], (0, 0)); // passthrough has no values: nothing to count
    }

    fn fx_pt() -> Pt {
        Pt {
            t: 0,
            cross: 1.0,
            along: 1.0,
            v: 100.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: Tri::No,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        }
    }

    fn fx_val(h: f64) -> Val {
        Val {
            pt: fx_pt(),
            along_fit: 0.0,
            h_raw: 1.0,
            h_fit: h,
            along_tol: 0.0,
            h_tol: h,
        }
    }

    fn fx_row(ac: usize, h: [Option<f64>; 3]) -> Row {
        Row {
            ac,
            t: 0,
            addr: ac as u32,
            phase: 0,
            band: 0,
            val: h.map(|x| x.map(fx_val)),
        }
    }

    // REQ: OA-TRK-374
    #[test]
    fn the_m4_and_m7_loops_count_a_nan_as_a_failure_and_print_the_count() {
        let rows = vec![
            fx_row(0, [Some(50.0), Some(f64::NAN), Some(500.0)]),
            fx_row(0, [Some(f64::NAN), Some(50.0), None]),
            fx_row(1, [Some(2000.0), Some(50.0), Some(50.0)]),
        ];
        let (c, nan) = m4_counts(&rows, 2, |v| v.h_fit, 100.0);
        assert_eq!(nan, [1, 1, 0]);
        // Within 100 m: OpenATCA 1 of 3 (50; NaN and 2000 fail), ERAM 2 of 3, passthrough 1 of 3.
        assert_eq!(c[0].point(), (1, 3));
        assert_eq!(c[1].point(), (2, 3));
        assert_eq!(c[2].point(), (1, 3));
        let l = large_counts(&rows, 2, |v| v.h_tol);
        assert_eq!(l.nan, [1, 1, 0]);
        assert_eq!(l.over, [2, 1, 0]);
        // Failures: over-or-NaN plus the miss of passthrough in row 2.
        assert_eq!(l.cnt[0].point(), (2, 3));
        assert_eq!(l.cnt[1].point(), (1, 3));
        assert_eq!(l.cnt[2].point(), (1, 3));
    }

    // REQ: OA-TRK-374
    #[test]
    fn m6_does_not_turn_a_nan_into_a_tie() {
        let rows = vec![
            fx_row(0, [Some(10.0), Some(20.0), Some(1.0)]),
            fx_row(1, [Some(f64::NAN), Some(20.0), Some(1.0)]),
            fx_row(2, [Some(30.0), Some(20.0), Some(1.0)]),
            fx_row(3, [Some(5.0), Some(f64::NAN), Some(1.0)]),
        ];
        let m = m6_counts(&rows, 4);
        assert_eq!((m.win, m.lose, m.tie), (1, 1, 0));
        assert_eq!(m.nan, [1, 1]);
        // The two NaN aircraft are failures of the share, in a denominator of four.
        assert_eq!(m.share.point(), (1, 4));
    }

    // REQ: OA-TRK-374
    #[test]
    fn a_non_finite_sample_is_counted_apart_from_not_common() {
        let mut rows = vec![
            fx_row(0, [Some(10.0), Some(20.0), Some(30.0)]),
            fx_row(0, [Some(f64::NAN), Some(20.0), Some(30.0)]),
            fx_row(0, [None, Some(20.0), Some(30.0)]),
        ];
        // Cross-track is finite everywhere; use the fitted horizontal error as the kind.
        let m = metric(&rows, 1, &draws_of(1), Kind::HFit, &|_| true, Set::Three);
        assert_eq!(m.n, [1, 1, 1]);
        assert_eq!(m.nonfinite, [1, 0, 0]);
        assert!(nonfinite_text(&m).contains("OpenATCA 1, ERAM 0"));
        rows.clear();
    }

    // REQ: OA-TRK-377
    #[test]
    fn a_hold_out_run_needs_the_frozen_commit_a_clean_build_and_a_clean_tree_at_it() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let other = "fedcba9876543210fedcba9876543210fedcba98";
        let git = |head: &str, dirty: bool| GitState {
            head: head.into(),
            porcelain: Some(if dirty {
                " M x\n".into()
            } else {
                String::new()
            }),
        };
        let b = |c: Option<&'static str>, d: Option<&'static str>| Build {
            commit: c,
            dirty: d,
        };
        let hold = Mode {
            holdout: true,
            development: false,
        };
        let dev = Mode {
            holdout: false,
            development: true,
        };
        let good = b(Some(sha), Some("0"));
        assert!(holdout_binding(hold, Some(sha), good, &git(sha, false)).is_ok());
        // No --frozen-commit, a short or upper-case one.
        assert!(holdout_binding(hold, None, good, &git(sha, false)).is_err());
        assert!(holdout_binding(hold, Some("0123456"), good, &git(sha, false)).is_err());
        // Embedded sha differs from the argument.
        let e = holdout_binding(hold, Some(other), good, &git(other, false)).unwrap_err();
        assert!(e.contains("built from"), "{e}");
        // Dirty build, unknown build.
        let e = holdout_binding(hold, Some(sha), b(Some(sha), Some("1")), &git(sha, false))
            .unwrap_err();
        assert!(e.contains("not clean"), "{e}");
        assert!(holdout_binding(hold, Some(sha), b(Some(sha), None), &git(sha, false)).is_err());
        assert!(holdout_binding(hold, Some(sha), b(None, None), &git(sha, false)).is_err());
        // Run-time tree at another commit, or dirty.
        assert!(holdout_binding(hold, Some(sha), good, &git(other, false)).is_err());
        assert!(holdout_binding(hold, Some(sha), good, &git(sha, true)).is_err());
        // Development mode is unchanged: no argument, no build information needed.
        assert!(holdout_binding(dev, None, b(None, None), &git(other, true)).is_ok());
    }

    #[test]
    fn share_counts_resample_whole_aircraft() {
        let mut c = Cnt::new(2);
        c.add(0, true);
        c.add(0, true);
        c.add(1, false);
        assert_eq!(c.point(), (2, 3));
        let b = c.boot(&[vec![0, 0], vec![1, 1], vec![0, 1]]);
        assert_eq!(b[0], 1.0);
        assert_eq!(b[1], 0.0);
        assert!((b[2] - 2.0 / 3.0).abs() < 1e-12);
    }

    // ----- P-004 section 1: the scored legs -----

    fn leg(addr: u32, start_ms: i64, end_ms: i64) -> TruthLeg {
        TruthLeg {
            addr,
            start_ms,
            end_ms,
            callsign: None,
            start: oa_bench::truth::LegStart::default(),
        }
    }

    fn pt_at(t: i64) -> Pt {
        Pt { t, ..fx_pt() }
    }

    /// A pair table of `(address, flight, time)` samples with finite errors.
    fn table(samples: &[(u32, u32, i64)]) -> PairTable {
        let mut p = PairTable::default();
        for &(a, f, t) in samples {
            p.flight.push(f);
            p.addr.push(a);
            p.t_ms.push(t);
            p.cross_m.push(1.0);
            p.along_m.push(1.0);
            p.speed_err_kt.push(0.0);
            p.hdg_err_deg.push(0.0);
            p.truth_speed_mps.push(100.0);
            p.coast.push(0);
            p.phase.push(0);
            p.band.push(0);
        }
        p
    }

    // REQ: OA-TRK-389
    #[test]
    fn a_grid_on_leg_one_is_scored_on_leg_one_though_openatca_has_more_samples_on_leg_two() {
        let legs = LegIndex::new(&[leg(1, 0, 100_000), leg(1, 1_000_000, 1_100_000)]);
        assert_eq!(legs.leg_of(1, 50_000), 0);
        assert_eq!(legs.leg_of(1, 1_050_000), 1);
        let mut er: PerAc = BTreeMap::new();
        er.insert(1, vec![pt_at(10_000), pt_at(20_000), pt_at(30_000)]);
        let scored = scored_legs(&er, &legs);
        assert_eq!(
            scored.get(&1).unwrap().iter().copied().collect::<Vec<_>>(),
            [0]
        );
        // OpenATCA: 3 samples on leg 1 (flight 0), 10 on leg 2 (flight 1).
        let mut samples: Vec<(u32, u32, i64)> = [10_000, 20_000, 30_000]
            .iter()
            .map(|&t| (1, 0, t))
            .collect();
        samples.extend((0..10).map(|i| (1, 1, 1_000_000 + i * 1000)));
        let p = table(&samples);
        let mut oa: PerAc = BTreeMap::new();
        let mut dropped = Dropped::default();
        pair_points(
            &p,
            None,
            &|a, t| on_scored_leg(&scored, &legs, a, t),
            &mut oa,
            &mut dropped,
        );
        assert_eq!(oa.get(&1).unwrap().len(), 3);
        assert_eq!(dropped.off_scored_legs, 10);
        // Every grid sample has an OpenATCA value.
        let times = times_of(&oa);
        let empty = TimesBy::new();
        let g = build_grid(&er, &oa, &BTreeMap::new(), [&times, &times_of(&er), &empty]);
        assert_eq!(g.rows.len(), 3);
        assert!(g.rows.iter().all(|r| r.val[OA].is_some()));
        // The replaced rule took the flight with the most samples, which is on leg 2: no coverage.
        let mut old = TimesBy::new();
        old_rule_times(&p, &mut old);
        assert_eq!(old.get(&1).unwrap().len(), 10);
        assert_eq!(old_rule_coverage(&times_of(&er), &old), (0, 3));
        // An address with no scored leg keeps nothing.
        let other = table(&[(2, 0, 10_000)]);
        let mut none: PerAc = BTreeMap::new();
        let mut d2 = Dropped::default();
        pair_points(
            &other,
            None,
            &|a, t| on_scored_leg(&scored, &legs, a, t),
            &mut none,
            &mut d2,
        );
        assert!(none.is_empty());
        assert_eq!(d2.off_scored_legs, 1);
    }

    // ----- P-004 section 2: the reacquired window -----

    // REQ: OA-TRK-390
    #[test]
    fn a_row_is_reacquired_within_ten_seconds_after_a_flagged_update_row() {
        let u = [1_000_i64, 50_000];
        let at = |t| reacquired_at(Some(&u), t);
        assert!(at(1_000));
        assert!(at(11_000));
        assert!(!at(11_001));
        // Before the update row it is not.
        assert!(!at(999));
        assert!(at(50_000));
        assert!(at(60_000));
        assert!(!at(60_001));
        assert!(!reacquired_at(None, 5_000));
        assert!(!reacquired_at(Some(&[]), 5_000));
    }

    // ----- P-004 section 3: the miss classes -----

    // REQ: OA-TRK-391
    #[test]
    fn each_miss_has_one_class() {
        let rows = [10_000_i64, 20_000, 30_000];
        let c = |t| classify_miss(Some(&rows), t);
        assert_eq!(c(5_000), Miss::Before);
        assert_eq!(c(40_000), Miss::After);
        assert_eq!(c(15_000), Miss::Gap);
        // A row within 0.5 s that was not scored.
        assert_eq!(c(10_400), Miss::Other);
        assert_eq!(c(20_000), Miss::Other);
        assert_eq!(classify_miss(None, 5_000), Miss::NeverPresent);
        assert_eq!(classify_miss(Some(&[]), 5_000), Miss::NeverPresent);
        let idx: Vec<usize> = [
            Miss::Before,
            Miss::After,
            Miss::Gap,
            Miss::NeverPresent,
            Miss::Other,
        ]
        .iter()
        .map(|m| m.index())
        .collect();
        assert_eq!(idx, [0, 1, 2, 3, 4]);
    }

    // REQ: OA-TRK-391
    #[test]
    fn the_grid_counts_every_miss_in_exactly_one_class_per_system() {
        let mut er: PerAc = BTreeMap::new();
        er.insert(1, vec![pt_at(0), pt_at(1_000), pt_at(100_000)]);
        let oa: PerAc = BTreeMap::new();
        let er_times = times_of(&er);
        // OpenATCA's rows: one at 1.0 s (not scored). Passthrough has none for the address.
        let oa_rows: TimesBy = [(1, vec![1_000_i64])].into_iter().collect();
        let none = TimesBy::new();
        let g = build_grid(&er, &oa, &BTreeMap::new(), [&oa_rows, &er_times, &none]);
        assert_eq!(g.rows.len(), 3);
        // t = 0: before the first row (1 s away); t = 1 s: a row is there but it is not scored;
        // t = 100 s: after the last row.
        assert_eq!(g.classes[OA], [1, 1, 0, 0, 1]);
        assert_eq!(g.classes[ER], [0; 5]);
        assert_eq!(g.classes[PT], [0, 0, 0, 3, 0]);
        // Every miss is in a class: OpenATCA and passthrough have no value anywhere.
        assert_eq!(g.classes[OA].iter().sum::<u64>(), 3);
        assert_eq!(g.classes[PT].iter().sum::<u64>(), 3);
        // A gap between two rows.
        let rows2: TimesBy = [(1, vec![0_i64, 50_000, 100_000])].into_iter().collect();
        let mut er2: PerAc = BTreeMap::new();
        er2.insert(1, vec![pt_at(25_000)]);
        let g2 = build_grid(
            &er2,
            &oa,
            &BTreeMap::new(),
            [&rows2, &times_of(&er2), &none],
        );
        assert_eq!(g2.classes[OA], [0, 0, 1, 0, 0]);
    }

    // ----- P-006 section 4: the two-way set -----

    // REQ: OA-TRK-392
    #[test]
    fn the_two_way_set_needs_openatca_and_eram_only_and_the_three_way_set_needs_passthrough_too() {
        let rows = vec![
            fx_row(0, [Some(1.0), Some(2.0), None]),
            fx_row(0, [Some(1.0), Some(2.0), Some(3.0)]),
            fx_row(0, [Some(1.0), None, Some(3.0)]),
            fx_row(1, [Some(f64::NAN), Some(2.0), Some(3.0)]),
        ];
        assert_eq!(rows.iter().filter(|r| r.two_way()).count(), 3);
        assert_eq!(rows.iter().filter(|r| r.common()).count(), 2);
        let draws = draws_of(2);
        let two = metric(&rows, 2, &draws, Kind::HFit, &|_| true, Set::Two);
        let three = metric(&rows, 2, &draws, Kind::HFit, &|_| true, Set::Three);
        // The NaN sample is in no common set; passthrough has no column in the two-way set.
        assert_eq!(two.n, [2, 2, 0]);
        assert_eq!(three.n, [1, 1, 1]);
        assert_eq!(two.nonfinite, [1, 0, 0]);
        assert!(pv(&two, PT, 0).is_nan());
        assert_eq!(pv(&two, OA, 0), 1.0);
        assert_eq!(pv(&three, PT, 0), 3.0);
    }

    // ----- P-006 section 3: the claim rule -----

    // REQ: OA-TRK-393
    #[test]
    fn a_verdict_excludes_zero_in_the_direction_the_metric_prefers() {
        // Lower is better: OpenATCA minus ERAM below zero favours OpenATCA.
        assert_eq!(verdict_of(-3.0, -1.0, true), Verdict::OpenAtca);
        assert_eq!(verdict_of(1.0, 3.0, true), Verdict::Eram);
        assert_eq!(verdict_of(-1.0, 1.0, true), Verdict::Neither);
        assert_eq!(verdict_of(0.0, 1.0, true), Verdict::Neither);
        // Higher is better: the other way round.
        assert_eq!(verdict_of(1.0, 3.0, false), Verdict::OpenAtca);
        assert_eq!(verdict_of(-3.0, -1.0, false), Verdict::Eram);
        assert_eq!(verdict_of(f64::NAN, 1.0, false), Verdict::Neither);
    }

    // ----- P-006 section 5: the candidate commit -----

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("p002-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn commit_file(dir: &Path, files: &[(&str, &str)], msg: &str) -> String {
        for (f, text) in files {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        git_ok(dir, &["add", "-A"]);
        git_ok(dir, &["commit", "-q", "-m", msg]);
        git_ok(dir, &["rev-parse", "HEAD"])
    }

    // REQ: OA-TRK-394
    #[test]
    fn the_scored_commit_may_differ_from_the_candidate_in_the_sealed_list_only() {
        let d = scratch("cand");
        git_ok(&d, &["init", "-q"]);
        let c = commit_file(
            &d,
            &[(SEALED_LIST_PATH, "one\n"), ("crates/x/lib.rs", "a\n")],
            "candidate",
        );
        let u = commit_file(&d, &[(SEALED_LIST_PATH, "two\n")], "sealed list");
        let v = commit_file(
            &d,
            &[(SEALED_LIST_PATH, "three\n"), ("crates/x/lib.rs", "b\n")],
            "list and code",
        );
        let w = commit_file(&d, &[("crates/x/lib.rs", "c\n")], "code only");
        let hold = Mode {
            holdout: true,
            development: false,
        };
        let dev = Mode {
            holdout: false,
            development: true,
        };
        let pair = |f: &str, c: &str| Commits {
            frozen: Some(f.into()),
            candidate: Some(c.into()),
        };
        assert_eq!(candidate_binding(hold, &pair(&u, &c), &d), Ok(()));
        // Another file changed too, only another file changed, nothing changed.
        let e = candidate_binding(hold, &pair(&v, &c), &d).unwrap_err();
        assert!(
            e.contains("crates/x/lib.rs") && e.contains(SEALED_LIST_PATH),
            "{e}"
        );
        assert!(candidate_binding(hold, &pair(&w, &v), &d).is_err());
        assert!(candidate_binding(hold, &pair(&c, &c), &d).is_err());
        // Not a commit of this repository, not a full id.
        assert!(candidate_binding(hold, &pair(&u, &"1".repeat(40)), &d).is_err());
        assert!(candidate_binding(hold, &pair(&u, "abc"), &d).is_err());
        // Needed together: a hold-out run needs it, so does a frozen commit without one.
        let only_frozen = Commits {
            frozen: Some(u.clone()),
            candidate: None,
        };
        let only_cand = Commits {
            frozen: None,
            candidate: Some(c.clone()),
        };
        let neither = Commits {
            frozen: None,
            candidate: None,
        };
        assert!(candidate_binding(hold, &only_frozen, &d).is_err());
        assert!(candidate_binding(dev, &only_frozen, &d).is_err());
        assert!(candidate_binding(dev, &only_cand, &d).is_err());
        assert!(candidate_binding(hold, &neither, &d).is_err());
        assert_eq!(candidate_binding(dev, &neither, &d), Ok(()));
        // A development run that gives both is checked too.
        assert!(candidate_binding(dev, &pair(&v, &c), &d).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    // ----- P-006 section 7: create-new outputs -----

    // REQ: OA-TRK-395
    #[test]
    fn the_ingest_drop_counts_are_listed_with_their_reasons() {
        let mut c = oa_bench::legacy::Counts::default();
        c.sfdps_rejected = 7;
        c.note("a reason".to_string(), 3);
        let text = counts_lines(&c).join("\n");
        assert!(text.contains("SFDPS messages rejected: 7"), "{text}");
        assert!(text.contains("drop reason: a reason: 3"), "{text}");
    }

    // REQ: OA-TRK-396
    #[test]
    fn iso_times_parse_to_nanoseconds() {
        assert_eq!(parse_iso_ns("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(
            parse_iso_ns("1970-01-02T00:00:01.5Z"),
            Ok(86_401_500_000_000)
        );
        assert_eq!(
            parse_iso_ns("2026-11-19T23:24:42.123456789Z"),
            Ok((20_776 * 86_400 + 23 * 3600 + 24 * 60 + 42) * 1_000_000_000 + 123_456_789)
        );
        assert_eq!(
            parse_iso_ns("2000-03-01T00:00:00Z"),
            Ok(951_868_800_000_000_000)
        );
        for bad in [
            "2026-11-19 23:24:42Z",
            "2026-11-19T23:24:42",
            "2026-13-19T00:00:00Z",
            "2026-11-19T00:00:00.1234567890Z",
        ] {
            assert!(parse_iso_ns(bad).is_err(), "{bad}");
        }
        assert!(Window::parse("1970-01-01T00:00:10Z", "1970-01-01T00:00:05Z").is_err());
    }

    // REQ: OA-TRK-396
    #[test]
    fn a_window_removes_the_same_samples_from_every_system_and_changes_no_other_value() {
        let mut er: PerAc = BTreeMap::new();
        er.insert(1, (0..10).map(|i| pt_at(i * 1000)).collect());
        er.insert(2, (0..10).map(|i| pt_at(i * 1000)).collect());
        // OpenATCA and passthrough miss every third sample.
        let thin = |k: i64| -> PerAc {
            (1..=2)
                .map(|a| {
                    (
                        a,
                        (0..10)
                            .filter(|i| i % k != 0)
                            .map(|i| Pt {
                                cross: i as f64,
                                ..pt_at(i * 1000)
                            })
                            .collect(),
                    )
                })
                .collect()
        };
        let (oa, pt) = (thin(3), thin(2));
        let none = TimesBy::new();
        let times = times_of(&er);
        let rt = [&times, &times, &times];
        let full = build_grid(&er, &oa, &pt, rt);
        let w = Window::parse("1970-01-01T00:00:03Z", "1970-01-01T00:00:05.000000000Z").unwrap();
        let (cut, counts, removed) = exclude_windows(&er, &[w.clone()]);
        // Samples at 3, 4 and 5 s of both aircraft.
        assert_eq!((counts, removed), (vec![6], 6));
        let g = build_grid(&cut, &oa, &pt, rt);
        assert_eq!(g.rows.len(), full.rows.len() - 6);
        let kept: Vec<&Row> = full.rows.iter().filter(|r| !w.holds_ms(r.t)).collect();
        assert_eq!(kept.len(), g.rows.len());
        for (a, b) in kept.iter().zip(&g.rows) {
            assert_eq!((a.addr, a.t), (b.addr, b.t));
            for s in 0..3 {
                assert_eq!(
                    a.val[s].map(|v| v.pt.cross.to_bits()),
                    b.val[s].map(|v| v.pt.cross.to_bits())
                );
            }
        }
        // No removed time is left for any system.
        assert!(g.rows.iter().all(|r| !w.holds_ms(r.t)));
        let _ = none;
        // The window is recorded in the machine-readable lines and read back.
        let rows = synth_rows(true, false);
        let (txt, _) = make_day(&rows, "clip", &"a".repeat(40), true, &[w], &[6]);
        let d = parse_claim_lines(&txt).unwrap();
        assert_eq!(
            d.windows,
            vec![(
                "1970-01-01T00:00:03Z".to_string(),
                "1970-01-01T00:00:05.000000000Z".to_string(),
                6
            )]
        );
    }

    // REQ: OA-TRK-396
    #[test]
    fn a_window_changes_the_samples_scored_and_never_the_scored_legs() {
        let legs = LegIndex::new(&[leg(1, 0, 50_000), leg(1, 100_000, 150_000)]);
        let mut er: PerAc = BTreeMap::new();
        er.insert(1, vec![pt_at(10_000), pt_at(20_000), pt_at(110_000)]);
        er.insert(2, vec![pt_at(30_000)]);
        // The window holds every sample of leg 0 of aircraft 1, and all of aircraft 2.
        let w = Window::parse("1970-01-01T00:00:05Z", "1970-01-01T00:00:35Z").unwrap();
        let (scored, grid, counts, removed) = plan_grid(&er, &legs, &[w]);
        assert_eq!((counts, removed), (vec![3], 3));
        // The legs are those of the grid before the window: leg 0 stays scored.
        assert_eq!(scored, scored_legs(&er, &legs));
        assert!(scored.get(&1).unwrap().contains(&0));
        assert!(scored.get(&2).is_some());
        // Computed from the grid after the window they would differ.
        assert_ne!(scored, scored_legs(&grid, &legs));
        // The grid has lost the emptied aircraft.
        assert_eq!(grid.keys().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(grid.get(&1).unwrap().len(), 1);
    }

    // REQ: OA-TRK-389
    #[test]
    fn only_eram_most_paired_flight_forms_the_grid() {
        // Address 1: flight 0 has 2 pairs, flight 1 has 4, flight 2 has 4 (lower flight wins a tie).
        let mut samples: Vec<(u32, u32, i64)> = vec![(1, 0, 0), (1, 0, 1000)];
        samples.extend((0..4).map(|i| (1, 1, 100_000 + i * 1000)));
        samples.extend((0..4).map(|i| (1, 2, 200_000 + i * 1000)));
        samples.push((2, 5, 0));
        let p = table(&samples);
        let best = most_paired_flights(&p);
        assert_eq!(best.get(&1), Some(&1));
        assert_eq!(best.get(&2), Some(&5));
        let mut er: PerAc = BTreeMap::new();
        let mut d = Dropped::default();
        pair_points(&p, Some(&best), &|_, _| true, &mut er, &mut d);
        let ts: Vec<i64> = er.get(&1).unwrap().iter().map(|s| s.t).collect();
        assert_eq!(ts, [100_000, 101_000, 102_000, 103_000]);
        assert_eq!(er.get(&2).unwrap().len(), 1);
        // The scored legs are the legs of those samples only.
        let legs = LegIndex::new(&[
            leg(1, 0, 50_000),
            leg(1, 100_000, 150_000),
            leg(1, 200_000, 250_000),
        ]);
        let sc = scored_legs(&er, &legs);
        assert_eq!(sc.get(&1).unwrap().iter().copied().collect::<Vec<_>>(), [1]);
    }

    // ----- P-008 -----

    fn syn_val(cross: f64, spd: f64, h: f64) -> Val {
        Val {
            pt: Pt {
                cross,
                spd,
                ..fx_pt()
            },
            along_fit: 0.0,
            h_raw: 1.0,
            h_fit: h,
            along_tol: 0.0,
            h_tol: h,
        }
    }

    /// 12 aircraft of 20 grid samples. OpenATCA beats ERAM when `oa_better`, the two are equal
    /// when not and `eram_better` is false; the passthrough has every second sample and equals
    /// OpenATCA when `pt_equal`, else is far better.
    fn synth_rows_with(oa_better: bool, eram_better: bool, pt_equal: bool) -> Vec<Row> {
        let mut rows = Vec::new();
        for ac in 0..12usize {
            for i in 0..20usize {
                let base = 1.0 + ((i + ac) % 4) as f64 * 0.3;
                let slow = 8.0 + ((i * 3 + ac) % 5) as f64 * 0.5;
                let (oa_c, er_c) = if oa_better {
                    (base, slow)
                } else if eram_better {
                    (slow, base)
                } else {
                    (base, base)
                };
                let spd = |c: f64| c * 0.1;
                let pt = (i % 2 == 0).then(|| {
                    let c = if pt_equal { oa_c } else { oa_c * 0.2 };
                    syn_val(c, spd(c), c * 40.0)
                });
                rows.push(Row {
                    ac,
                    t: i as i64 * 1000,
                    addr: ac as u32 + 1,
                    phase: 0,
                    band: 0,
                    val: [
                        Some(syn_val(oa_c, spd(oa_c), oa_c * 40.0)),
                        Some(syn_val(er_c, spd(er_c), er_c * 40.0)),
                        pt,
                    ],
                });
            }
        }
        rows
    }

    fn synth_rows(oa_better: bool, pt_equal: bool) -> Vec<Row> {
        synth_rows_with(oa_better, false, pt_equal)
    }

    /// One day's `p002.txt` and `samples.csv` as the run writes them.
    fn make_day(
        rows: &[Row],
        clip: &str,
        frozen: &str,
        holdout: bool,
        windows: &[Window],
        removed: &[u64],
    ) -> (String, String) {
        let csv = samples_csv(rows, &[]);
        let n_ac = rows.iter().map(|r| r.ac).max().map_or(0, |m| m + 1);
        let pieces = claim_pieces(rows, n_ac, &draws_of(n_ac));
        let info = RunInfo {
            holdout,
            frozen: frozen.into(),
            candidate: "c".repeat(40),
            clip_sha: clip.into(),
            sfdps_sha: "s".into(),
            replay_hash: "r".into(),
            samples_sha: to_hex(&sha256(csv.as_bytes())),
            binary_sha: "b".into(),
            damage_sha: "d".into(),
            window_total_s: "0".into(),
        };
        (
            format!(
                "other text\n{}",
                claim_lines(&info, &pieces, windows, removed)
            ),
            csv,
        )
    }

    fn files<'a>(d: &'a [(String, String); 3]) -> [DayFiles<'a>; 3] {
        [
            DayFiles {
                txt: &d[0].0,
                csv: &d[0].1,
            },
            DayFiles {
                txt: &d[1].0,
                csv: &d[1].1,
            },
            DayFiles {
                txt: &d[2].0,
                csv: &d[2].1,
            },
        ]
    }

    const U: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    // REQ: OA-TRK-393
    #[test]
    fn the_cross_day_rule_is_met_only_on_a_and_b_in_openatcas_favour_and_c_not_against() {
        let win = synth_rows(true, false);
        let flat = synth_rows_with(false, false, false);
        let lose = synth_rows_with(false, true, false);
        let day = |r: &[Row], clip: &str| make_day(r, clip, U, true, &[], &[]);
        let d = [day(&win, "c1"), day(&win, "c2"), day(&flat, "c3")];
        let r = combine(files(&d)).unwrap();
        assert_eq!(
            to_hex(&sha256(r.as_bytes())),
            "cd54b9880a95154f58dc5316b377eb06c69659af398b38bda23ef54572afc288"
        );
        assert_eq!(r.matches("RULE MET").count(), 3, "{r}");
        // C against OpenATCA, or B silent, breaks it.
        let d = [day(&win, "c1"), day(&win, "c2"), day(&lose, "c3")];
        assert_eq!(
            combine(files(&d)).unwrap().matches("RULE NOT MET").count(),
            3
        );
        let d = [day(&win, "c1"), day(&flat, "c2"), day(&win, "c3")];
        assert_eq!(
            combine(files(&d)).unwrap().matches("RULE NOT MET").count(),
            3
        );
    }

    // REQ: OA-TRK-393
    #[test]
    fn combine_refuses_one_day_three_times_and_runs_that_are_not_holdout_or_not_one_commit_pair() {
        let win = synth_rows(true, false);
        let day = |clip: &str, f: &str, h: bool| make_day(&win, clip, f, h, &[], &[]);
        // The same clip three times.
        let d = [day("c1", U, true), day("c1", U, true), day("c1", U, true)];
        let e = combine(files(&d)).unwrap_err();
        assert!(e.contains("three different clips"), "{e}");
        // A development run.
        let d = [day("c1", U, true), day("c2", U, true), day("c3", U, false)];
        assert!(combine(files(&d))
            .unwrap_err()
            .contains("not a hold-out run"));
        // frozen=none, a short commit, another commit.
        let d = [
            day("c1", U, true),
            day("c2", U, true),
            day("c3", "none", true),
        ];
        assert!(combine(files(&d)).unwrap_err().contains("40-hex"));
        let d = [
            day("c1", U, true),
            day("c2", U, true),
            day("c3", "abc", true),
        ];
        assert!(combine(files(&d)).is_err());
        let other = "b".repeat(40);
        let d = [
            day("c1", U, true),
            day("c2", U, true),
            day("c3", &other, true),
        ];
        assert!(combine(files(&d)).unwrap_err().contains("one commit pair"));
        // No CLAIM lines.
        let mut d = [day("c1", U, true), day("c2", U, true), day("c3", U, true)];
        d[2].0 = "nothing".into();
        assert!(combine(files(&d)).is_err());
    }

    // REQ: OA-TRK-393
    #[test]
    fn combine_recomputes_and_refuses_an_edited_interval_or_an_edited_samples_file() {
        let win = synth_rows(true, false);
        let day = |clip: &str| make_day(&win, clip, U, true, &[], &[]);
        let good = [day("c1"), day("c2"), day("c3")];
        assert!(combine(files(&good)).is_ok());
        // An edited difference or interval in a CLAIM line.
        let mut d = [day("c1"), day("c2"), day("c3")];
        let line = d[1]
            .0
            .lines()
            .find(|l| l.contains("metric=M1_median"))
            .unwrap()
            .to_string();
        let lo = line
            .split_whitespace()
            .find(|w| w.starts_with("lo="))
            .unwrap()
            .to_string();
        d[1].0 = d[1].0.replace(&lo, "lo=-99");
        let e = combine(files(&d)).unwrap_err();
        assert!(e.contains("recomputed"), "{e}");
        // An edited samples.csv no longer has its recorded hash.
        let mut d = [day("c1"), day("c2"), day("c3")];
        d[0].1 = d[0].1.replacen(",1,", ",2,", 1).replacen("0,", "1,", 0);
        d[0].1.push('\n');
        let e = combine(files(&d)).unwrap_err();
        assert!(e.contains("samples.csv has SHA-256"), "{e}");
        // A samples.csv edited AND rehashed in the CLAIM line still fails the recomputation.
        let mut d = [day("c1"), day("c2"), day("c3")];
        let mut rows = win.clone();
        for r in &mut rows {
            if let Some(v) = r.val[OA].as_mut() {
                v.pt.cross += 50.0;
            }
        }
        let forged_csv = samples_csv(&rows, &[]);
        let old_sha = to_hex(&sha256(d[2].1.as_bytes()));
        d[2].0 = d[2]
            .0
            .replace(&old_sha, &to_hex(&sha256(forged_csv.as_bytes())));
        d[2].1 = forged_csv;
        assert!(combine(files(&d)).unwrap_err().contains("recomputed"));
    }

    // REQ: OA-TRK-397
    #[test]
    fn a_metric_at_the_reference_floor_on_any_day_gives_no_claim_and_never_rule_met() {
        let win = synth_rows(true, false);
        // The passthrough equals OpenATCA: at the floor on M1 and M5.
        let floor_rows = synth_rows(true, true);
        let day = |r: &[Row], clip: &str| make_day(r, clip, U, true, &[], &[]);
        let d = [
            day(&floor_rows, "c1"),
            day(&floor_rows, "c2"),
            day(&floor_rows, "c3"),
        ];
        let r = combine(files(&d)).unwrap();
        assert!(
            r.contains("M1 median cross-track error (m): AT REFERENCE FLOOR: no claim"),
            "{r}"
        );
        assert!(r.contains(
            "M5 ground-speed error median, absolute (m/s): AT REFERENCE FLOOR: no claim"
        ));
        // M4t is judged on the share, where the passthrough is far below: not at the floor.
        assert!(!r.contains("M4t share of ALL grid samples within 300 m (rounding-tolerant, time-fitted): AT REFERENCE"), "{r}");
        // On one day only, the metric still gets no claim.
        let d = [day(&win, "c1"), day(&win, "c2"), day(&floor_rows, "c3")];
        let r = combine(files(&d)).unwrap();
        assert_eq!(r.matches("AT REFERENCE FLOOR: no claim").count(), 2, "{r}");
        assert!(r.contains(
            "M4t share of ALL grid samples within 300 m (rounding-tolerant, time-fitted): RULE MET"
        ));
        // Without any floor, the rule is met and the flag is 0 in the CLAIM lines.
        let d = [day(&win, "c1"), day(&win, "c2"), day(&win, "c3")];
        assert!(!combine(files(&d))
            .unwrap()
            .contains("AT REFERENCE FLOOR: no claim"));
        assert!(d[0].0.contains("floor=0"));
        // Floor test on the share.
        assert!(floor_applies(0.95, 0.99));
        assert!(!floor_applies(0.997, 0.19));
    }

    // REQ: OA-TRK-398
    #[test]
    fn an_oa_feed_gaps_report_gives_the_windows_of_a_clean_day_and_is_refused_otherwise() {
        let head = "oa-feed-gaps report (P-006 section 6)\nwindow: 2026-11-19T00:00:00.000000000Z to 2026-11-19T23:59:59.000000000Z (86399.000000000 s)\nlimit: 600.000000000 s\n";
        let ex = |a: &str, b: &str| {
            format!("exclude 2026-11-19T{a}.000000000Z 2026-11-19T{b}.000000000Z\n")
        };
        let ok = format!(
            "{head}{}{}CLEAN\n",
            ex("10:00:00", "10:15:00"),
            ex("12:00:00", "12:10:00")
        );
        let d = parse_damage_report(&ok, "abc").unwrap();
        assert_eq!(
            (d.windows.len(), d.total_ns, d.sha256.as_str()),
            (2, 1_500_000_000_000, "abc")
        );
        assert!(parse_damage_report(&format!("{head}CLEAN\n"), "x")
            .unwrap()
            .windows
            .is_empty());
        // Not CLEAN, no verdict, two verdicts.
        assert!(parse_damage_report(&format!("{head}DAMAGED: an interval\n"), "x").is_err());
        assert!(parse_damage_report(head, "x").is_err());
        assert!(parse_damage_report(&format!("{head}CLEAN\nCLEAN\n"), "x").is_err());
        // A window under 600 s, windows over 3,600 s in all, outside the report's window.
        let e = parse_damage_report(&format!("{head}{}CLEAN\n", ex("10:00:00", "10:09:59")), "x")
            .unwrap_err();
        assert!(e.contains("under 600 s"), "{e}");
        let many: String = (0..5)
            .map(|i| ex(&format!("0{}:00:00", i + 1), &format!("0{}:15:00", i + 1)))
            .collect();
        let e = parse_damage_report(&format!("{head}{many}CLEAN\n"), "x").unwrap_err();
        assert!(e.contains("over 3,600 s"), "{e}");
        let outside = "exclude 2026-11-20T00:00:00.000000000Z 2026-11-20T00:20:00.000000000Z\n";
        assert!(parse_damage_report(&format!("{head}{outside}CLEAN\n"), "x").is_err());
    }

    /// Unix ns of `hh:mm:ss` on 2026-11-15, the development day.
    fn at(hms: &str) -> i64 {
        parse_iso_ns(&format!("2026-11-15T{hms}Z")).unwrap()
    }

    // REQ: OA-TRK-398
    #[test]
    fn the_report_window_must_match_the_scored_window_not_a_whole_day() {
        let report = |from: &str, to: &str| DamageReport {
            sha256: "x".into(),
            from_ns: at(from),
            to_ns: if to == "24:00:00" {
                at("00:00:00") + DAY_NS
            } else {
                at(to)
            },
            windows: vec![],
            total_ns: 0,
        };
        // The clip runs the whole day; the SFDPS run starts at 00:17:04Z and ends at 23:59:40Z.
        let (cf, cl) = (at("00:00:03"), at("23:59:50"));
        let (sf, sl) = (at("00:17:04"), at("23:59:40"));
        assert_eq!(
            scored_window(cf, cl, sf, sl),
            Ok((at("00:17:04"), at("23:59:40")))
        );
        // The day's start and end bound S when the recordings run past them.
        assert_eq!(
            scored_window(
                at("01:00:00"),
                cl,
                at("00:00:00") - 5,
                at("00:00:00") + DAY_NS + 5
            ),
            Ok((at("00:00:00"), cl))
        );
        // 00:17:00Z to 23:59:59Z, on an SFDPS run starting 00:17:04Z: accepted.
        let ok = report("00:17:00", "23:59:59");
        assert_eq!(damage_matches_clip(&ok, cf, cl, sf, sl), Ok(()));
        // A report starting at 02:00Z, two hours late: refused, both windows printed.
        let e = damage_matches_clip(&report("02:00:00", "23:59:59"), cf, cl, sf, sl).unwrap_err();
        assert!(
            e.contains("2026-11-15T02:00:00Z to 2026-11-15T23:59:59Z")
                && e.contains("2026-11-15T00:17:04Z to 2026-11-15T23:59:40Z"),
            "{e}"
        );
        // A report window ending exactly at the day's end is inside the day; one reaching into
        // the next day is refused, even when the recordings run on past midnight.
        assert_eq!(
            damage_matches_clip(&report("00:17:00", "24:00:00"), cf, cl, sf, sl),
            Ok(())
        );
        let mut next = report("00:17:00", "23:59:59");
        next.to_ns = at("00:00:00") + DAY_NS + 30_000_000_000;
        let e = damage_matches_clip(&next, cf, cl, sf, at("00:00:00") + DAY_NS + 20_000_000_000)
            .unwrap_err();
        assert!(e.contains("2026-11-16T00:00:30Z"), "{e}");
        // 61 s off at either end is refused; 60 s is not.
        let mut r = report("00:17:00", "23:59:59");
        r.from_ns = sf - 61_000_000_000;
        assert!(damage_matches_clip(&r, cf, cl, sf, sl).is_err());
        r.from_ns = sf - 60_000_000_000;
        assert_eq!(damage_matches_clip(&r, cf, cl, sf, sl), Ok(()));
        r.to_ns = sl - 61_000_000_000;
        assert!(damage_matches_clip(&r, cf, cl, sf, sl).is_err());
        // A clip that is not one UTC day has no scored window.
        assert!(scored_window(cf, at("00:00:00") + DAY_NS, sf, sl).is_err());
    }

    // REQ: OA-TRK-398
    #[test]
    fn the_reports_the_tool_prints_are_read_exactly() {
        // Printed by oa-feed-gaps (commit eee7e93) on small synthetic recordings: an SFDPS file
        // from 00:17:04Z and an ADS-B clip, a message every 300 s with a 1,000 s hole, window
        // 00:17:00Z to 23:59:59Z. In the second the SFDPS recording stops at 11:57:04Z.
        let clean = include_str!("fixtures/oa-feed-gaps-clean.txt");
        let damaged = include_str!("fixtures/oa-feed-gaps-damaged.txt");
        let d = parse_damage_report(clean, "sha").unwrap();
        assert_eq!(
            (d.from_ns, d.to_ns),
            (at("00:17:00"), at("23:59:59")),
            "window: line"
        );
        assert_eq!(d.windows.len(), 1, "exclude line");
        assert_eq!(
            (d.windows[0].from_ns, d.windows[0].to_ns),
            (at("00:47:04"), at("01:12:04"))
        );
        assert_eq!(d.total_ns, 1_500_000_000_000);
        // Its one verdict is the last line, CLEAN.
        assert!(clean.ends_with("\nCLEAN\n") && !clean.contains("DAMAGED:"));
        // The damaged report has two exclude lines and a DAMAGED: verdict, and is refused.
        assert_eq!(damaged.matches("\nexclude ").count(), 2);
        assert!(damaged.lines().last().unwrap().starts_with("DAMAGED: "));
        let e = parse_damage_report(damaged, "sha").unwrap_err();
        assert!(e.contains("not CLEAN") && e.contains("1 DAMAGED"), "{e}");
        // Only the lines read count: the same report with the DAMAGED line cut off has no verdict.
        let cut: String = damaged
            .lines()
            .filter(|l| !l.starts_with("DAMAGED:"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(parse_damage_report(&cut, "sha").is_err());
        // The window of the tool's report is the scored one of a run that began at 00:17:04Z.
        let (cf, cl) = (at("00:17:04"), at("23:59:50"));
        assert_eq!(
            damage_matches_clip(&d, cf, cl, at("00:17:04"), at("23:59:40")),
            Ok(())
        );
    }

    // REQ: OA-TRK-399
    #[test]
    fn the_ledger_refuses_a_clip_it_holds_and_is_only_ever_appended_to() {
        let dir = scratch("ledger");
        let l = dir.join("ledger.txt");
        assert_eq!(ledger_has(&l, "clip1"), Ok(false));
        let c = Commits {
            frozen: Some(U.into()),
            candidate: Some("c".repeat(40)),
        };
        assert_eq!(
            ledger_line("clip1", &c, 1_795_130_682),
            format!("clip1 {U} {} 2026-11-19T23:24:42Z", "c".repeat(40))
        );
        ledger_append(&l, &ledger_line("clip1", &c, 0)).unwrap();
        ledger_append(&l, &ledger_line("clip2", &c, 0)).unwrap();
        let text = fs::read_to_string(&l).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(ledger_has(&l, "clip1"), Ok(true));
        assert_eq!(ledger_has(&l, "clip2"), Ok(true));
        assert_eq!(ledger_has(&l, "clip"), Ok(false));
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        let _ = fs::remove_dir_all(&dir);
    }

    // REQ: OA-TRK-399
    #[test]
    fn the_ledger_line_is_there_only_when_both_partials_were_written() {
        let line = Some("clip1 f c 2026-11-19T00:00:00Z");
        // Both written: the line is in, both files are renamed.
        let d = scratch("order-ok");
        let l = d.join("ledger.txt");
        publish_outputs(&d, "csv", "txt", line.map(|x| (l.as_path(), x))).unwrap();
        assert_eq!(fs::read_to_string(&l).unwrap().lines().count(), 1);
        assert!(d.join("samples.csv").exists() && d.join("p002.txt").exists());
        let _ = fs::remove_dir_all(&d);
        // The second partial cannot be written (its name is a directory): no ledger line, the
        // first partial is removed, nothing is under a final name.
        let d = scratch("order-partial");
        let l = d.join("ledger.txt");
        fs::create_dir(d.join("p002.txt.partial")).unwrap();
        assert!(publish_outputs(&d, "csv", "txt", line.map(|x| (l.as_path(), x))).is_err());
        assert!(!l.exists() && !d.join("samples.csv.partial").exists());
        assert!(!d.join("samples.csv").exists() && !d.join("p002.txt").exists());
        let _ = fs::remove_dir_all(&d);
        // The ledger cannot be appended to (its directory is not there): the partials are
        // deleted, the run fails, nothing is under a final name.
        let d = scratch("order-ledger");
        let l = d.join("no-such-dir").join("ledger.txt");
        let e = publish_outputs(&d, "csv", "txt", line.map(|x| (l.as_path(), x))).unwrap_err();
        assert!(e.contains("ledger"), "{e}");
        for f in OUTPUT_FILES {
            assert!(!d.join(f).exists() && !d.join(format!("{f}{PARTIAL}")).exists());
        }
        // Without a ledger line (not a hold-out run) the files are still published.
        publish_outputs(&d, "csv", "txt", None).unwrap();
        assert!(d.join("p002.txt").exists());
        let _ = fs::remove_dir_all(&d);
    }

    // REQ: OA-TRK-395
    #[test]
    fn outputs_are_written_to_partial_names_then_renamed_and_never_over_a_result() {
        let d = scratch("out");
        assert_eq!(outputs_free(&d), Ok(vec![]));
        // A leftover partial does not block a run, and is reported.
        fs::write(d.join("p002.txt.partial"), "old").unwrap();
        assert_eq!(outputs_free(&d).unwrap(), vec![d.join("p002.txt.partial")]);
        publish_outputs(&d, "csv", "txt", None).unwrap();
        assert_eq!(fs::read_to_string(d.join("p002.txt")).unwrap(), "txt");
        assert_eq!(fs::read_to_string(d.join("samples.csv")).unwrap(), "csv");
        assert!(!d.join("p002.txt.partial").exists() && !d.join("samples.csv.partial").exists());
        // A result under a final name refuses the run and nothing is written.
        let e = publish_outputs(&d, "new", "new", None).unwrap_err();
        assert!(e.contains("exists"), "{e}");
        assert_eq!(fs::read_to_string(d.join("samples.csv")).unwrap(), "csv");
        assert!(!d.join("samples.csv.partial").exists());
        // Either final file alone refuses; so does a dangling symlink.
        for f in OUTPUT_FILES {
            fs::remove_file(d.join(f)).unwrap();
        }
        fs::write(d.join("samples.csv"), "kept").unwrap();
        assert!(outputs_free(&d).unwrap_err().contains("samples.csv"));
        fs::remove_file(d.join("samples.csv")).unwrap();
        std::os::unix::fs::symlink(d.join("nowhere"), d.join("p002.txt")).unwrap();
        assert!(outputs_free(&d).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    // REQ: OA-TRK-400
    #[test]
    fn the_time_fit_of_openatca_and_eram_uses_the_two_way_set_and_the_passthroughs_the_three_way_set(
    ) {
        let both = fx_row(0, [Some(1.0), Some(1.0), None]);
        let all3 = fx_row(0, [Some(1.0), Some(1.0), Some(1.0)]);
        let pt_only = fx_row(0, [Some(1.0), None, Some(1.0)]);
        for (r, oa, er, pt) in [
            (&both, true, true, false),
            (&all3, true, true, true),
            (&pt_only, false, false, false),
        ] {
            assert_eq!(in_fit_set(r, OA), oa);
            assert_eq!(in_fit_set(r, ER), er);
            assert_eq!(in_fit_set(r, PT), pt);
        }
    }

    // REQ: OA-TRK-373
    #[test]
    fn the_hold_out_gate_counts_only_openatca_rows_matched_to_a_grid_sample() {
        let mut matched = syn_val(1.0, 0.0, 1.0);
        matched.pt.uncertain = Tri::No;
        matched.pt.reacquired = Tri::No;
        let mut bad = matched;
        bad.pt.uncertain = Tri::Unmatched;
        let row = |v: Option<Val>| Row {
            ac: 0,
            t: 0,
            addr: 1,
            phase: 0,
            band: 0,
            val: [v, None, None],
        };
        // An unmatched OpenATCA point that no grid sample uses is not in the rows at all.
        assert_eq!(unmatched_rows(&[row(Some(matched)), row(None)]), 0);
        assert_eq!(unmatched_rows(&[row(Some(matched)), row(Some(bad))]), 1);
        let hold = Mode {
            holdout: true,
            development: false,
        };
        let dev = Mode {
            holdout: false,
            development: true,
        };
        assert!(flag_gate(hold, 0, 3, 1).is_ok());
        let e = flag_gate(hold, 1, 3, 1).unwrap_err();
        assert!(e.contains("hold-out run refused") && e.contains("1 OpenATCA samples"));
        assert!(flag_gate(dev, 5, 0, 0).is_ok());
    }

    #[test]
    fn export_requires_sidecar_and_checks_its_digest() {
        let dir =
            std::env::temp_dir().join(format!("p002-export-integrity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let path = dir.join("eram.txt");
        let export = eram_export::Export {
            meta: eram_export::Meta {
                source_dir: "synthetic".into(),
                source_sha256: "0".repeat(64),
                replay_hash: "1".repeat(64),
                ..eram_export::Meta::default()
            },
            flights: Vec::new(),
        };
        let bytes = eram_export::write(&export).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(read_export(&path).unwrap_err().contains("required sidecar"));

        let side = dir.join("eram.txt.sha256");
        fs::write(
            &side,
            eram_export::manifest_line("eram.txt", bytes.as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(read_export(&path).is_ok());
        fs::write(&side, format!("{}  eram.txt\n", "0".repeat(64))).unwrap();
        assert!(read_export(&path).unwrap_err().contains("does not match"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn partial_writer_refuses_symlinks_and_sealed_targets() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("p002-partial-guard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("sentinel");
        fs::write(&target, "keep").unwrap();
        let ordinary_link = dir.join("ordinary.partial");
        symlink(&target, &ordinary_link).unwrap();
        assert!(write_new_partial(&ordinary_link, "changed").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");

        let sealed_target = dir.join("scds/rec-20261119/day2/sealed.txt");
        fs::create_dir_all(sealed_target.parent().unwrap()).unwrap();
        fs::write(&sealed_target, "sealed fixture").unwrap();
        let sealed_link = dir.join("sealed.partial");
        symlink(&sealed_target, &sealed_link).unwrap();
        let err = write_new_partial(&sealed_link, "changed").unwrap_err();
        assert!(err.contains("OA-SEAL-001"), "{err}");
        assert_eq!(
            fs::read_to_string(&sealed_target).unwrap(),
            "sealed fixture"
        );

        let export = dir.join("export.txt");
        let partial = dir.join("export.txt.partial");
        symlink(&target, &partial).unwrap();
        let err = export_run(
            Path::new("/nonexistent-synthetic-sfdps"),
            &export,
            None,
            &mut oa_sealed::DropReport::new(),
        )
        .unwrap_err();
        assert!(
            err.contains("exists") || err.contains("OA-SEAL-001"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_pair_publication_never_replaces_and_rolls_back_a_half_pair() {
        let dir = std::env::temp_dir().join(format!("p002-export-publish-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let part_a = dir.join("eram.partial");
        let part_b = dir.join("eram.sha256.partial");
        let file = dir.join("eram.txt");
        let side = dir.join("eram.txt.sha256");
        fs::write(&part_a, "export").unwrap();
        fs::write(&part_b, "manifest").unwrap();
        fs::write(&side, "preexisting").unwrap();
        assert!(publish_export_pair(&part_a, &file, &part_b, &side).is_err());
        assert!(
            !file.exists(),
            "first half is rolled back if second destination exists"
        );
        assert_eq!(fs::read_to_string(&side).unwrap(), "preexisting");
        fs::remove_file(&side).unwrap();
        publish_export_pair(&part_a, &file, &part_b, &side).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "export");
        assert_eq!(fs::read_to_string(&side).unwrap(), "manifest");
        assert!(!part_a.exists() && !part_b.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
