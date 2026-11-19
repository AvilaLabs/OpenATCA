//! Pairing, the time-offset fit and the three error sets.
//!
//! A STDDS record is paired with the tracker and the ADS-B truth by Mode S address and time.
//! Each paired sample carries three error sets on the same sample: tracker minus STDDS, ADS-B
//! truth minus STDDS, tracker minus ADS-B truth. Along and cross track are taken against the
//! direction of the ADS-B truth velocity in all three, so the sets are comparable; cross is
//! positive to the right of the direction of travel.

use std::collections::BTreeMap;

use crate::model::{Class, Fix, Rec, Reference, Unpaired, MIN_SPEED_MPS};

/// Furthest the fitted stamp offset may be from zero, ms.
pub const OFFSET_MAX_MS: i64 = 15_000;
/// Step of the offset search, ms.
pub const OFFSET_STEP_MS: i64 = 100;
/// Most samples of one track used in its offset fit (an even stride over the track).
pub const FIT_MAX_SAMPLES: usize = 128;
/// Fewest samples a track needs to be fitted.
pub const FIT_MIN_SAMPLES: usize = 8;

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
        }),
        _ => Err(Unpaired::NotFinite),
    }
}

/// A STDDS track: one address under one producing facility and track number.
pub type TrackKey = (u32, [u8; 8], Option<u32>);

/// Why a track has no fitted offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitMiss {
    /// Fewer than [`FIT_MIN_SAMPLES`] samples have a moving truth fix at every offset searched.
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
    pub result: Result<Fitted, FitMiss>,
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
    'sample: for k in 0..take {
        let Some(rec) = recs.get(k * n / take.max(1)) else {
            continue;
        };
        let (Some(addr), Some(t), Some(std)) = (rec.addr, rec.t_ms, stdds_fix(rec)) else {
            continue;
        };
        let mut along = Vec::with_capacity(width);
        for j in -steps..=steps {
            let Ok(tru) = truth.at(addr, t - j * OFFSET_STEP_MS) else {
                continue 'sample;
            };
            let Some(a) = direction(&tru)
                .and_then(|d| pair_err(&std, &tru, Some(d)))
                .and_then(|e| e.along_m)
            else {
                continue 'sample;
            };
            along.push(a);
        }
        for (s, a) in sums.iter_mut().zip(&along) {
            *s += a * a;
        }
        used += 1;
    }
    if used < FIT_MIN_SAMPLES {
        return Err(FitMiss::TooFewSamples);
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
    /// Read at the stamp minus the track's fitted offset.
    pub fitted: Result<Sample, Unpaired>,
}

/// Everything the report prints.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Analysis {
    pub rows: Vec<Row>,
    pub fits: Vec<TrackFit>,
}

/// Scores every record: at its own stamp, and at the stamp corrected by its track's fitted
/// offset. Rows are in the order of `recs`; fits are in track-key order.
pub fn analyse(recs: &[Rec], tracker: &dyn Reference, truth: &dyn Reference) -> Analysis {
    let mut tracks: BTreeMap<TrackKey, Vec<&Rec>> = BTreeMap::new();
    for r in recs {
        if let (Some(a), Some(_), Some(_)) = (r.addr, r.t_ms, r.pos) {
            tracks.entry((a, r.src, r.track_num)).or_default().push(r);
        }
    }
    let mut fits = Vec::with_capacity(tracks.len());
    let mut offsets: BTreeMap<TrackKey, i64> = BTreeMap::new();
    for (key, v) in &mut tracks {
        v.sort_by_key(|r| (r.t_ms, r.file, r.index));
        let class = v.first().map_or(Class::Unknown, |r| r.class);
        let result = fit_offset(v, truth);
        if let Ok(f) = result {
            offsets.insert(*key, f.offset_ms);
        }
        fits.push(TrackFit {
            class,
            key: *key,
            result,
        });
    }
    let rows = recs
        .iter()
        .map(|rec| {
            let offset_ms = rec
                .addr
                .and_then(|a| offsets.get(&(a, rec.src, rec.track_num)))
                .copied();
            let (plain, fitted) = match (rec.addr, rec.t_ms, rec.pos) {
                (None, _, _) => (Err(Unpaired::NoAddress), Err(Unpaired::NoAddress)),
                (_, None, _) | (_, _, None) => (
                    Err(Unpaired::NoTimeOrPosition),
                    Err(Unpaired::NoTimeOrPosition),
                ),
                (Some(_), Some(t), Some(_)) => (
                    evaluate(rec, t, tracker, truth),
                    match offset_ms {
                        Some(o) => evaluate(rec, t - o, tracker, truth),
                        None => Err(Unpaired::NotFitted),
                    },
                ),
            };
            Row {
                rec: *rec,
                plain,
                offset_ms,
                fitted,
            }
        })
        .collect();
    Analysis { rows, fits }
}
