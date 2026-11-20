//! Runs the tracker and builds the ADS-B truth from one clip, the way `oa-track`'s `vs_eram`
//! example does: the clip is split into two disjoint halves, set A feeds the tracker, set B alone
//! builds the truth.

use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_bench::input::{load_adsb_with, Opener};
use oa_bench::legacy::NO_ALT;
use oa_sealed::Sealed;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, run_tracker_spilled, score_ranges, scoring_config, split_reports,
    IntervalStats, RowSet, RowSource, SCORE_CHUNK_ROWS, SPLIT_TEXT,
};
use oa_track::DEFAULT_CONFIG;
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

use crate::guard::open_guarded;
use crate::sources::{Row, TrackerSource};

/// The two references of one clip, and the header lines that describe how they were made.
// A7.2: cold struct, size guard waived
pub struct Refs {
    pub tracker: TrackerSource,
    pub truth: BuiltTruth,
    pub header: Vec<String>,
}

/// Runs the tracker over set A of `clip` and keeps its one-second rows of the addresses in
/// `addrs`; builds the truth from set B. `work` is a directory for the tracker's spill files,
/// removed afterwards. The clip is opened through the shared guard `g`; `sealed` gets the rows
/// of the clip dropped in a sealed window as soon as the clip is read.
pub fn build_refs(
    g: &Sealed,
    clip: &Path,
    addrs: &BTreeSet<u32>,
    work: &Path,
    sealed: &mut u64,
) -> Result<Refs, String> {
    let guard = g.clone();
    let opener: Opener = Rc::new(move |p: &Path| {
        open_guarded(&guard, p).map(|f| Box::new(f) as Box<dyn std::io::Read>)
    });
    let loaded = load_adsb_with(clip, &opener).map_err(|e| e.to_string())?;
    let clip_digest = loaded.hash.sha256.clone();
    // The clip's rows left out for their time in a sealed window (OA-SEAL-014), given to the
    // caller before anything that can fail.
    *sealed = loaded.sealed_window_dropped;
    let (set_a, set_b, split) = split_reports(&loaded.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("split refused: {v:?}"))?;
    drop(loaded);
    let (meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    drop(set_a);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let spill = work.join(format!("spill-{}", std::process::id()));
    let run = run_tracker_spilled(&meas, &cfg, &spill).map_err(|e| e.to_string())?;
    let n_meas = meas.len();
    drop(meas);

    let counts = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
    let mut rows: Vec<(u32, Row)> = Vec::new();
    for range in score_ranges(&counts, SCORE_CHUNK_ROWS) {
        let r = run
            .rows_in(RowSet::Ticks, range.0, range.1)
            .map_err(|e| e.0)?;
        for i in 0..r.addr.len() {
            let (Some(&a), Some(&t), Some(&la), Some(&lo), Some(&alt)) = (
                r.addr.get(i),
                r.t_ns.get(i),
                r.lat_e7.get(i),
                r.lon_e7.get(i),
                r.alt_ft.get(i),
            ) else {
                continue;
            };
            if !addrs.contains(&a) {
                continue;
            }
            let vel = match (r.vel_east_mps.get(i), r.vel_north_mps.get(i)) {
                (Some(&e), Some(&n)) if e.is_finite() && n.is_finite() => {
                    Some((f64::from(e), f64::from(n)))
                }
                _ => None,
            };
            rows.push((
                a,
                Row {
                    t_ms: t.div_euclid(1_000_000),
                    lat: f64::from(la) * 1e-7,
                    lon: f64::from(lo) * 1e-7,
                    alt_ft: (alt != NO_ALT).then(|| f64::from(alt)),
                    vel,
                },
            ));
        }
    }
    let run_digest = run.digest.clone();
    drop(run);
    let _ = std::fs::remove_dir_all(&spill);
    let tracker = TrackerSource::from_rows(rows);

    let truth = adsb_truth::build(&set_b, &TRUTH_DEFAULT).map_err(|e| format!("truth: {e}"))?;
    let header = vec![
        format!("ADS-B clip: {} (digest {clip_digest})", clip.display()),
        format!("split: {SPLIT_TEXT}"),
        format!(
            "set A {} reports -> {n_meas} tracker measurements (tracker output digest {run_digest}); set B {} reports -> truth ({} legs, digest {})",
            split.set_a,
            split.set_b,
            truth.legs.len(),
            truth.digest
        ),
        format!(
            "tracker rows kept (one-second output of the addresses in the STDDS files): {}",
            tracker.len()
        ),
    ];
    Ok(Refs {
        tracker,
        truth,
        header,
    })
}
