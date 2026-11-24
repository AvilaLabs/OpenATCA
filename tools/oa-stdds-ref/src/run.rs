//! Runs the tracker and builds the ADS-B truth from one clip, the way `oa-track`'s `vs_eram`
//! example does: the clip is split into two disjoint halves, set A feeds the tracker, set B alone
//! builds the truth.

use std::collections::BTreeSet;
use std::io::{self, Read};
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

/// ADS-B clip source cap. Combined with the 384 MiB STDDS source cap, total source input is at
/// most 512 MiB for one comparator invocation.
pub const MAX_ADSB_CLIP_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum decoded ADS-B rows retained by the comparison.
pub const MAX_ADSB_ROWS: usize = 100_000;

struct LimitedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(
                    "ADS-B clip grew beyond its byte limit while reading",
                )),
            };
        }
        let cap = usize::try_from(self.remaining.min(out.len() as u64))
            .map_err(|_| io::Error::other("ADS-B read size exceeds this platform"))?;
        let n = self.inner.read(out.get_mut(..cap).ok_or_else(|| {
            io::Error::other("ADS-B input reader buffer is shorter than its declared limit")
        })?)?;
        self.remaining = self.remaining.saturating_sub(n as u64);
        Ok(n)
    }
}

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
        let f = open_guarded(&guard, p)?;
        let size = f.metadata()?.len();
        if size > MAX_ADSB_CLIP_BYTES {
            return Err(io::Error::other(format!(
                "{} exceeds the {}-byte ADS-B clip limit",
                p.display(),
                MAX_ADSB_CLIP_BYTES
            )));
        }
        Ok(Box::new(LimitedReader {
            inner: f,
            remaining: MAX_ADSB_CLIP_BYTES,
        }) as Box<dyn std::io::Read>)
    });
    let loaded = load_adsb_with(clip, &opener).map_err(|e| e.to_string())?;
    if oa_adsb::reports_len(&loaded.reports) > MAX_ADSB_ROWS {
        return Err(format!(
            "ADS-B clip exceeds the {MAX_ADSB_ROWS}-row retained-data limit"
        ));
    }
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
    let settings = format!("tracker={cfg:?}; truth={TRUTH_DEFAULT:?}; interval-stats={iv:?}");
    let settings_hash = oa_sha256::to_hex(&oa_sha256::sha256(settings.as_bytes()));
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
        format!("effective tracker/truth settings: {settings}"),
        format!("effective settings SHA-256: {settings_hash}"),
    ];
    Ok(Refs {
        tracker,
        truth,
        header,
    })
}
