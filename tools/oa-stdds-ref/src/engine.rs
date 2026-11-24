//! Pairing, the time-offset fit and the three error sets.
//!
//! A STDDS record is paired with the tracker and the ADS-B truth by Mode S address and time.
//! Each paired sample carries three error sets on the same sample: tracker minus STDDS, ADS-B
//! truth minus STDDS, tracker minus ADS-B truth. Along and cross track are taken against the
//! direction of the ADS-B truth velocity in all three, so the sets are comparable; cross is
//! positive to the right of the direction of travel.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Class, Fix, Rec, Reference, Unpaired, MIN_SPEED_MPS};

/// Furthest the fitted stamp offset may be from zero, ms.
pub const OFFSET_MAX_MS: i64 = 15_000;
/// Step of the offset search, ms.
pub const OFFSET_STEP_MS: i64 = 100;
/// Most samples of one track used in its offset fit (an even stride over the track).
pub const FIT_MAX_SAMPLES: usize = 128;
/// Fewest samples a track needs to be fitted.
pub const FIT_MIN_SAMPLES: usize = 8;
/// A changed TAIS key after a silence this long starts a new calibration segment. TAIS carries
/// no generation identifier, so this is a reported segmentation policy, not proof of flight identity.
pub const TRACK_REUSE_GAP_MS: i64 = 60_000;

/// Names of the three error sets, in the order of [`Sample::errs`].
pub const SET_NAMES: [&str; 3] = [
    "tracker - stdds",
    "ADS-B truth - stdds",
    "tracker - ADS-B truth",
];
/// Short column names of the three sets.
pub const SET_CODES: [&str; 3] = ["trk_std", "tru_std", "trk_tru"];

/// The differences between two fixes, first minus second.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairErr {
    /// Horizontal distance, metres.
    pub h_m: f64,
    /// Along the truth direction of travel, metres; `None` when the truth is too slow.
    pub along_m: Option<f64>,
    /// Across it, positive to the right, metres.
    pub cross_m: Option<f64>,
    /// Altitude difference, feet; `None` unless both fixes have an altitude.
    pub dalt_ft: Option<f64>,
    /// Length of the velocity difference, m/s; `None` unless both have a velocity.
    pub dvel_mps: Option<f64>,
}

/// One paired sample: the three error sets and whether the tracker fix was extrapolated.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub errs: [PairErr; 3],
    pub tracker_extrapolated: bool,
    /// STDDS altitude as published (assumed feet) and the truth's barometric altitude, feet.
    pub std_alt_ft: Option<f64>,
    pub tru_alt_ft: Option<f64>,
    /// STDDS speed (assumed m/s) and the truth's speed, m/s; `None` without a velocity.
    pub std_speed_mps: Option<f64>,
    pub tru_speed_mps: Option<f64>,
}

fn speed(f: &Fix) -> Option<f64> {
    let (e, n) = f.vel?;
    let s = e.hypot(n);
    s.is_finite().then_some(s)
}

/// The unit direction of a fix's velocity, `None` below [`MIN_SPEED_MPS`].
fn direction(f: &Fix) -> Option<(f64, f64)> {
    let (e, n) = f.vel?;
    let s = e.hypot(n);
    (s.is_finite() && s >= MIN_SPEED_MPS).then(|| (e / s, n / s))
}

/// `a` minus `b`, in the frame of `b` (distances are exact at `b`), split along `dir`.
pub fn pair_err(a: &Fix, b: &Fix, dir: Option<(f64, f64)>) -> Option<PairErr> {
    let frame = oa_geo::aeqd::frame_new(b.lat, b.lon);
    let (e, n) = oa_geo::aeqd::project(&frame, a.lat, a.lon);
    let h_m = e.hypot(n);
    if !h_m.is_finite() {
        return None;
    }
    let (along_m, cross_m) = match dir {
        Some((ux, uy)) => (Some(e * ux + n * uy), Some(e * uy - n * ux)),
        None => (None, None),
    };
    let dalt_ft = match (a.alt_ft, b.alt_ft) {
        (Some(x), Some(y)) => Some(x - y),
        _ => None,
    };
    let dvel_mps = match (a.vel, b.vel) {
        (Some(x), Some(y)) => Some((x.0 - y.0).hypot(x.1 - y.1)),
        _ => None,
    };
    let finite = |v: Option<f64>| v.is_none_or(f64::is_finite);
    (finite(along_m) && finite(cross_m) && finite(dalt_ft) && finite(dvel_mps)).then_some(PairErr {
        h_m,
        along_m,
        cross_m,
        dalt_ft,
        dvel_mps,
    })
}

fn stdds_fix(r: &Rec) -> Option<Fix> {
    let (lat, lon) = r.pos?;
    Some(Fix {
        lat,
        lon,
        alt_ft: r.alt_ft,
        vel: r.vel,
        extrapolated: false,
    })
}

/// The sample of `rec` with the tracker and the truth read at `t_ms`.
pub fn evaluate(
    rec: &Rec,
    t_ms: i64,
    tracker: &dyn Reference,
    truth: &dyn Reference,
) -> Result<Sample, Unpaired> {
    let addr = rec.addr.ok_or(Unpaired::NoAddress)?;
    if addr == 0 {
        return Err(Unpaired::ZeroAddress);
    }
    let std = stdds_fix(rec).ok_or(Unpaired::NoTimeOrPosition)?;
    let trk = tracker.at(addr, t_ms).map_err(Unpaired::NoTracker)?;
    let tru = truth.at(addr, t_ms).map_err(Unpaired::NoTruth)?;
    let dir = direction(&tru);
    match (
        pair_err(&trk, &std, dir),
        pair_err(&tru, &std, dir),
        pair_err(&trk, &tru, dir),
    ) {
        (Some(a), Some(b), Some(c)) => Ok(Sample {
            errs: [a, b, c],
            tracker_extrapolated: trk.extrapolated,
            std_alt_ft: std.alt_ft,
            tru_alt_ft: tru.alt_ft,
            std_speed_mps: speed(&std),
            tru_speed_mps: speed(&tru),
        }),
        _ => Err(Unpaired::NotFinite),
    }
}

/// An observed TAIS key: positive Mode S address under one producing facility and track number.
/// This is not a guaranteed flight generation; [`TRACK_REUSE_GAP_MS`] segments long silences.
pub type TrackKey = (u32, [u8; 8], Option<u32>);

fn timed_fit_key(rec: &Rec) -> Option<(TrackKey, i64)> {
    if !rec.src_present || rec.pos.is_none() {
        return None;
    }
    Some((
        (rec.addr.filter(|a| *a != 0)?, rec.src, Some(rec.track_num?)),
        rec.t_ms?,
    ))
}

/// Why a track has no fitted offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitMiss {
    /// The track has enough records, but fewer than [`FIT_MIN_SAMPLES`] of the samples taken have
    /// an ADS-B truth fix at every offset of the search window (the truth does not cover
    /// the +-15 s around them).
    NoTruthCoverage,
    /// Fewer than [`FIT_MIN_SAMPLES`] usable samples for another reason: the track has too few
    /// records, or the truth is covered but too slow to give a direction.
    TooFewSamples,
}

/// The fitted offset of one track.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fitted {
    /// How late the stamps are: the STDDS position stamped `s` was where the aircraft was at
    /// `s - offset_ms`.
    pub offset_ms: i64,
    /// The offset is at either end of the search range: the true one may lie beyond.
    pub at_edge: bool,
    /// Samples the fit used.
    pub samples: usize,
}

/// One track's fit, with what identifies the track.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackFit {
    pub class: Class,
    pub key: TrackKey,
    /// Zero-based fit segment after reuse gaps and unambiguous class changes.
    pub segment: u32,
    pub result: Result<Fitted, FitMiss>,
}

/// Conservative fit-sample and report-scan estimate, evaluated before reference construction.
/// Duplicate key/timestamp rows are excluded just as in [`analyse`]; each unambiguous class/reuse
/// segment contributes at most `min(segment rows, FIT_MAX_SAMPLES)` fit samples.
pub fn estimate_analysis_work(recs: &[Rec]) -> Result<usize, String> {
    let mut histories: BTreeMap<(u32, [u8; 8], u32), Vec<&Rec>> = BTreeMap::new();
    for rec in recs {
        if rec.pos.is_none() {
            continue;
        }
        if let (Some(addr), Some(track_num), Some(_)) = (
            rec.addr.filter(|addr| *addr != 0),
            rec.track_num.filter(|_| rec.src_present),
            rec.t_ms,
        ) {
            histories
                .entry((addr, rec.src, track_num))
                .or_default()
                .push(rec);
        }
    }
    let mut fit_samples = 0usize;
    for history in histories.values_mut() {
        history.sort_by_key(|rec| (rec.t_ms, rec.file, rec.index));
        let mut segment_rows: BTreeMap<(u32, Class), usize> = BTreeMap::new();
        let mut previous_time = None;
        let mut previous_class = None;
        let mut segment = 0u32;
        let mut begin = 0usize;
        while begin < history.len() {
            let Some(time) = history.get(begin).and_then(|rec| rec.t_ms) else {
                begin += 1;
                continue;
            };
            let mut end = begin + 1;
            while history.get(end).is_some_and(|rec| rec.t_ms == Some(time)) {
                end += 1;
            }
            // Same-key duplicate timestamps are ambiguous; don't let file order choose churn.
            if end - begin == 1 {
                let Some(rec) = history.get(begin).copied() else {
                    break;
                };
                if previous_time
                    .is_some_and(|prev: i64| time.saturating_sub(prev) > TRACK_REUSE_GAP_MS)
                {
                    segment = segment.saturating_add(1);
                }
                if previous_class.is_some_and(|class| class != rec.class) {
                    segment = segment.saturating_add(1);
                }
                previous_time = Some(time);
                previous_class = Some(rec.class);
                if rec.pos.is_some() {
                    *segment_rows.entry((segment, rec.class)).or_default() += 1;
                }
            }
            begin = end;
        }
        for rows in segment_rows.values() {
            fit_samples = fit_samples
                .checked_add((*rows).min(FIT_MAX_SAMPLES))
                .ok_or_else(|| "fit work estimate overflow".to_string())?;
        }
    }
    let offset_width = usize::try_from(2 * (OFFSET_MAX_MS / OFFSET_STEP_MS) + 1)
        .map_err(|_| "offset width exceeds this platform".to_string())?;
    fit_samples
        .checked_mul(offset_width)
        .and_then(|fit| {
            recs.len()
                .checked_mul(480)
                .and_then(|report| fit.checked_add(report))
        })
        .ok_or_else(|| "analysis work estimate overflow".to_string())
}

/// The stamp offset of one track, minimising the sum of squared along-track errors of the STDDS
/// positions against the ADS-B truth (never the tracker) over offsets `-15 s` to `+15 s` in
/// 0.1 s steps. The samples are an even stride of at most [`FIT_MAX_SAMPLES`] of `recs`
/// (sorted by time); only samples whose truth is available and moving at every offset are used,
/// so every offset is scored on the same samples. Offsets are tried in the order 0, -0.1, +0.1,
/// -0.2, ... and replace the best only when strictly better, so a tie goes toward zero.
pub fn fit_offset(recs: &[&Rec], truth: &dyn Reference) -> Result<Fitted, FitMiss> {
    let steps = OFFSET_MAX_MS / OFFSET_STEP_MS;
    let width = (2 * steps + 1) as usize;
    let n = recs.len();
    let take = n.min(FIT_MAX_SAMPLES);
    let mut sums = vec![0.0f64; width];
    let mut used = 0usize;
    // Samples with a truth fix at every offset, moving or not.
    let mut covered = 0usize;
    'sample: for k in 0..take {
        let Some(rec) = recs.get(k * n / take) else {
            continue;
        };
        let (Some(addr), Some(t), Some(std)) = (rec.addr, rec.t_ms, stdds_fix(rec)) else {
            continue;
        };
        let mut along = Vec::with_capacity(width);
        let mut moving = true;
        for j in -steps..=steps {
            let Ok(tru) = truth.at(addr, t - j * OFFSET_STEP_MS) else {
                continue 'sample;
            };
            match direction(&tru)
                .and_then(|d| pair_err(&std, &tru, Some(d)))
                .and_then(|e| e.along_m)
            {
                Some(a) => along.push(a),
                None => moving = false,
            }
        }
        covered += 1;
        if !moving {
            continue;
        }
        for (s, a) in sums.iter_mut().zip(&along) {
            *s += a * a;
        }
        used += 1;
    }
    if used < FIT_MIN_SAMPLES {
        return Err(if take >= FIT_MIN_SAMPLES && covered < FIT_MIN_SAMPLES {
            FitMiss::NoTruthCoverage
        } else {
            FitMiss::TooFewSamples
        });
    }
    let mut best: Option<(i64, f64)> = None;
    for m in 0..=steps {
        for j in [-m, m] {
            if m == 0 && j != 0 {
                continue;
            }
            let Some(&c) = sums.get((j + steps) as usize) else {
                continue;
            };
            if best.is_none_or(|(_, bc)| c < bc) {
                best = Some((j, c));
            }
        }
    }
    let (j, _) = best.ok_or(FitMiss::TooFewSamples)?;
    Ok(Fitted {
        offset_ms: j * OFFSET_STEP_MS,
        at_edge: j.abs() == steps,
        samples: used,
    })
}

/// The result for one STDDS record.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub rec: Rec,
    /// Read at the record's own stamp.
    pub plain: Result<Sample, Unpaired>,
    /// The record's track offset, when fitted.
    pub offset_ms: Option<i64>,
    /// Segment index for its address/source/track key, when available.
    pub segment: Option<u32>,
    /// Read at the stamp minus the track's fitted offset.
    pub fitted: Result<Sample, Unpaired>,
}

/// Everything the report prints.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Analysis {
    pub rows: Vec<Row>,
    pub fits: Vec<TrackFit>,
    /// Rows excluded because a positive key occurs more than once at the same timestamp.
    pub ambiguous_timestamp_rows: u64,
    /// Descending timestamps in file/record arrival order, counted per key.
    pub nonmonotonic_arrivals: u64,
    /// Adjacent sorted updates whose flag-derived class changes within a key.
    pub class_churn_transitions: u64,
    /// Boundaries created by [`TRACK_REUSE_GAP_MS`].
    pub reuse_segments: u64,
}

/// Scores every record: at its own stamp, and at the stamp corrected by its track's fitted
/// offset. Rows are in the order of `recs`; fits are in track-key order.
pub fn analyse(recs: &[Rec], tracker: &dyn Reference, truth: &dyn Reference) -> Analysis {
    // TAIS has no generation identifier. Positive address, facility and track number form the
    // observed key; long silences split calibration segments. Class switches also split fits.
    // Duplicate key/timestamp rows are ambiguous and excluded rather than settled by file order.
    let mut histories: BTreeMap<TrackKey, Vec<&Rec>> = BTreeMap::new();
    for r in recs {
        if let Some((key, _)) = timed_fit_key(r) {
            histories.entry(key).or_default().push(r);
        }
    }
    let mut fits = Vec::new();
    let mut offsets: BTreeMap<(Class, TrackKey, u32), i64> = BTreeMap::new();
    let mut segments: BTreeMap<(TrackKey, i64), u32> = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    let mut ambiguous_timestamp_rows = 0u64;
    let mut nonmonotonic_arrivals = 0u64;
    let mut class_churn_transitions = 0u64;
    let mut reuse_segments = 0u64;
    for (key, history) in &mut histories {
        history.sort_by_key(|r| (r.t_ms, r.file, r.index));
        let mut by_time: BTreeMap<i64, Vec<&Rec>> = BTreeMap::new();
        for r in history.iter() {
            if let Some(t) = r.t_ms {
                by_time.entry(t).or_default().push(r);
            }
        }
        for (time, rows) in by_time.iter().filter(|(_, rows)| rows.len() > 1) {
            ambiguous.insert((*key, *time));
            ambiguous_timestamp_rows = ambiguous_timestamp_rows
                .saturating_add(u64::try_from(rows.len()).unwrap_or(u64::MAX));
        }
        let mut arrival: Vec<_> = history
            .iter()
            .copied()
            .filter(|r| r.t_ms.is_some_and(|t| !ambiguous.contains(&(*key, t))))
            .collect();
        arrival.sort_by_key(|r| (r.file, r.index));
        let mut prev_t: Option<i64> = None;
        let mut prev_file: Option<u32> = None;
        for r in arrival {
            if prev_file.is_some_and(|f| f != r.file) {
                prev_t = None;
            }
            if r.t_ms.is_some_and(|t| prev_t.is_some_and(|p| t < p)) {
                nonmonotonic_arrivals = nonmonotonic_arrivals.saturating_add(1);
            }
            prev_t = r.t_ms;
            prev_file = Some(r.file);
        }
        let mut segment = 0u32;
        let mut previous_time = None;
        let mut previous_class = None;
        let mut per_segment: BTreeMap<(Class, u32), Vec<&Rec>> = BTreeMap::new();
        for r in history.iter().copied() {
            let Some(t) = r.t_ms else { continue };
            if ambiguous.contains(&(*key, t)) {
                continue;
            }
            if previous_time.is_some_and(|p: i64| t.saturating_sub(p) > TRACK_REUSE_GAP_MS) {
                segment = segment.saturating_add(1);
                reuse_segments = reuse_segments.saturating_add(1);
            }
            if previous_class.is_some_and(|c| c != r.class) {
                class_churn_transitions = class_churn_transitions.saturating_add(1);
                segment = segment.saturating_add(1);
            }
            previous_time = Some(t);
            previous_class = Some(r.class);
            segments.insert((*key, t), segment);
            if r.pos.is_some() {
                per_segment.entry((r.class, segment)).or_default().push(r);
            }
        }
        for ((class, segment), mut rows) in per_segment {
            rows.sort_by_key(|r| (r.t_ms, r.file, r.index));
            let result = fit_offset(&rows, truth);
            if let Ok(f) = result {
                offsets.insert((class, *key, segment), f.offset_ms);
            }
            fits.push(TrackFit {
                class,
                key: *key,
                segment,
                result,
            });
        }
    }
    let rows = recs
        .iter()
        .map(|rec| {
            let segment = timed_fit_key(rec).and_then(|key| segments.get(&key).copied());
            let offset_ms = rec.addr.filter(|a| *a != 0).and_then(|a| {
                segment.and_then(|seg| {
                    offsets
                        .get(&(rec.class, (a, rec.src, rec.track_num), seg))
                        .copied()
                })
            });
            let (plain, fitted) = if timed_fit_key(rec).is_some_and(|key| ambiguous.contains(&key))
            {
                (
                    Err(Unpaired::AmbiguousTimestamp),
                    Err(Unpaired::AmbiguousTimestamp),
                )
            } else {
                match (rec.addr, rec.t_ms, rec.pos) {
                    (None, _, _) => (Err(Unpaired::NoAddress), Err(Unpaired::NoAddress)),
                    (Some(0), _, _) => (Err(Unpaired::ZeroAddress), Err(Unpaired::ZeroAddress)),
                    (_, None, _) | (_, _, None) => (
                        Err(Unpaired::NoTimeOrPosition),
                        Err(Unpaired::NoTimeOrPosition),
                    ),
                    (Some(_), Some(t), Some(_)) => {
                        let plain = evaluate(rec, t, tracker, truth);
                        let fitted = if !rec.src_present || rec.track_num.is_none() {
                            Err(Unpaired::MissingFitKey)
                        } else {
                            match offset_ms {
                                Some(o) => evaluate(rec, t - o, tracker, truth),
                                None => Err(Unpaired::NotFitted),
                            }
                        };
                        (plain, fitted)
                    }
                }
            };
            Row {
                rec: *rec,
                plain,
                offset_ms,
                segment,
                fitted,
            }
        })
        .collect();
    Analysis {
        ambiguous_timestamp_rows,
        rows,
        fits,
        nonmonotonic_arrivals,
        class_churn_transitions,
        reuse_segments,
    }
}
