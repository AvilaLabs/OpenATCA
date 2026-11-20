//! Every tunable of the tracker as a named constant. Each says what will set it from data. The
//! values are first choices from the physics of airliners, adjusted on the synthetic worlds of
//! the tests and after a first look at the 2026-11-14 ZAU hour (the process noises, the turn rate
//! limits); that hour is also the scoring clip and there is no held-out set yet, so the scored
//! numbers are not out-of-sample. The mean normalised innovation squared in the report shows
//! whether the noise model is honest (2.0 expected).

/// Nanoseconds per second.
pub const NS_PER_S: i64 = 1_000_000_000;

// ---- Measurement noise ----------------------------------------------------------------

/// Multiplier on the per-axis position sigma derived from NACp. NACp is a 95 % bound, so 1.0 is
/// conservative against real GNSS error. Set from the tracker's own normalised innovations and
/// from the leave-one-out calibration of oa-truth.
pub const POS_SIGMA_SCALE: f64 = 1.0;
/// NACv assumed for a velocity report when none was printed within the carry age: 1 (95 % bound
/// 10 m/s), the loosest level that still bounds the error. Set from the NACv distribution of the
/// clip.
pub const DEFAULT_NACV: u8 = 1;
/// Per-axis velocity noise from the NACv bound (DO-260B table 2-22: 10, 3, 1 and 0.3 m/s as 95 %
/// radii) divided by `oa_truth::SIGMA95`, by NACv; index 0 is unused. The behaviour before the
/// calibrated table: far looser than the measured error, so kept for the tests that pin
/// outputs bit for bit and for comparison.
pub const NACV_BOUND_VEL_SIGMA_MPS: [f64; 5] = [
    f64::INFINITY,
    10.0 / oa_truth::SIGMA95,
    3.0 / oa_truth::SIGMA95,
    1.0 / oa_truth::SIGMA95,
    0.3 / oa_truth::SIGMA95,
];
/// Per-axis velocity noise by NACv, m/s (1 sigma), all flight conditions; index 0 is unused. Calibrated on the
/// 2026-11-14 ZAU clip (`velocity_diag`, set A reported velocity against the set-B reference
/// velocity at each report's time, east and north components): the robust sigma of NACv k is
/// the 95th percentile of the absolute component error divided by 1.96, the larger of east and
/// north, over every sample with a reference (all flight conditions, not steady flight only).
/// NACv 1: 296,284 samples, east 2.1959, north 2.1733; NACv 2: 930,908, 1.8628 and 1.8602;
/// NACv 3: 1,056, 1.8436 and 1.7313; NACv 4: 5,224, 2.7502 and 2.7294 (the steady-flight values
/// are 0.46 to 0.71, medians of the absolute error 0.30 to 0.35). Every NACv had at least 1,000
/// samples, so each takes its own estimate. The reference carries its own error, which is large
/// in turns and accelerations (the smoother lags a manoeuvre), so these are conservative
/// upper bounds on the error of the reported velocity; NACv 4 is above NACv 1 because the
/// reference error and the manoeuvring share of that group, not the transponder, dominate it.
pub const NACV_VEL_SIGMA_ALL_CONDITIONS_MPS: [f64; 5] = [f64::INFINITY, 2.196, 1.863, 1.844, 2.750];
/// The default per-axis velocity noise by NACv, m/s (1 sigma); index 0 is unused: the
/// steady-flight values of the same calibration (95th percentile of the absolute component error
/// over steady-flight samples, divided by 1.96; 0.6142, 0.5938, 0.5209, 0.7070). The
/// all-conditions table above puts the reference's own manoeuvre lag into the noise and weakens
/// the filter in steady flight; manoeuvres are covered by the gate's allowance
/// ([`GATE_MANOEUVRE_MPS2`]) instead. Chosen on the 2026-11-14 clip (position RMS 50.3 m with
/// gate 400, 30.8 m with A5), validated on 2026-11-15 (58.3 m current, 32.0 m with A5).
pub const NACV_VEL_SIGMA_MPS: [f64; 5] = [f64::INFINITY, 0.6142, 0.5938, 0.5209, 0.707];
/// Barometric altitude quantisation of ADS-B reports, feet (Q bit set, 25 ft). adsb.lol rows do
/// not say which encoding the transponder used; Mode C radar passes 100.
pub const BARO_QUANT_FT: u16 = 25;
/// Altitude noise besides quantisation, feet (1 sigma). Set from the residuals of level flight.
pub const ALT_BASE_SIGMA_FT: f64 = 5.0;
/// Vertical rate noise, feet per second (1 sigma): ADS-B reports it in 64 ft/min steps and
/// barometric and inertial rates disagree. Set from the residual of reported rate against the
/// altitude difference rate.
pub const VRATE_SIGMA_FPS: f64 = 2.0;

// ---- Horizontal motion ----------------------------------------------------------------

/// Nearly-constant-velocity model: white acceleration density, m^2/s^3. 1.0 is generous for
/// airliners (0.05 gave a mean innovation of 2.1 on the clip but lagged accelerations and the
/// low-speed traffic of the ZAU hour); it keeps the covariance conservative (mean 1.3). Set from
/// the straight-flight acceleration spectrum of the full-rate ADS-B data.
pub const H_Q_CV: f64 = 1.0;
/// Coordinated-turn model: white acceleration density, m^2/s^3. Set the same way, in turns.
pub const H_Q_CT: f64 = 1.0;
/// Coordinated-turn model: turn rate random walk, (rad/s)^2 per second. Set from how fast
/// observed turn rates change (standard rate is 0.052 rad/s).
pub const H_Q_OMEGA: f64 = 3.0e-6;
/// Floor of the turn rate standard deviation entering the turn model, rad/s (about 2.3 deg/s):
/// a turn can start at any time, and without a floor the mixed prior from the straight model
/// (turn rate exactly zero) would never let the turn model discover one.
pub const CT_OMEGA_STD_FLOOR: f64 = 0.04;
/// Largest turn rate magnitude the turn model keeps, rad/s (29 deg/s: helicopters and aircraft
/// in tight orbits at low speed are in the ZAU hour; an airliner never exceeds 3).
pub const OMEGA_MAX: f64 = 0.5;
/// How far the reported velocity lags the aircraft's, seconds: in a turn the reported track is
/// behind the track of the positions. Measured as the reference's track minus the reported one over
/// the reference's turn rate, median over the reports of aircraft in the scoring grid in turns of
/// 0.25 deg/s or more (`turn_diag`): 2026-11-15 1.01, 0.99, 0.92 and 0.74 s in the bins 0.25-1, 1-2,
/// 2-3 and over 3 deg/s; 2026-11-19 0.96, 1.00, 0.91 and 0.84 s. With it the cross-track p95 of the
/// common samples fell from 45.4 to 34.8 m (2026-11-15) and from 41.2 to 30.4 m (2026-11-19), the
/// median from 1.9 to 1.7 m on both; 1.5 s was worse than 1.0 s on 2026-11-15.
pub const VEL_LAG_S: f64 = 1.0;
/// Mean time the straight model persists, seconds. Set from the distribution of straight
/// segment lengths in the recorded day.
pub const H_TAU_CV_S: f64 = 100.0;
/// Mean time the turn model persists, seconds. Set from the distribution of turn durations.
pub const H_TAU_CT_S: f64 = 40.0;
/// Initial probability of the straight model.
pub const H_MU0_CV: f64 = 0.9;
/// Floor of a model probability, so a model is never locked out.
pub const MU_FLOOR: f64 = 1.0e-3;
/// Velocity standard deviation assumed when the first report has none, m/s.
pub const H_PRIOR_VEL_STD: f64 = 150.0;

// ---- Vertical motion ------------------------------------------------------------------

/// Level model: altitude random walk density, ft^2/s. Set from altitude residuals in level
/// flight.
pub const V_Q_LEVEL: f64 = 1.0;
/// Climb/descent model: white vertical acceleration density, ft^2/s^3. Set from the vertical
/// rate change spectrum in climbs and descents.
pub const V_Q_CLIMB: f64 = 1.0;
/// Mean time the level model persists, seconds. Set from level segment durations.
pub const V_TAU_LEVEL_S: f64 = 60.0;
/// Mean time the climb/descent model persists, seconds. Set from climb and descent durations.
pub const V_TAU_CLIMB_S: f64 = 60.0;
/// Initial probability of the level model.
pub const V_MU0_LEVEL: f64 = 0.8;
/// Vertical rate standard deviation assumed when the first report has none, ft/s.
pub const V_PRIOR_RATE_STD_FPS: f64 = 30.0;

// ---- Gating ---------------------------------------------------------------------------

/// A position innovation with a normalised squared distance above this is not applied
/// (chi-square, 2 degrees of freedom, tail 1e-6). Set from the innovation distribution.
pub const GATE_CHI2: f64 = 27.6;
/// A velocity innovation with a normalised squared distance above this is dropped and the
/// position of the same measurement is applied alone (chi-square, 2 degrees of freedom, tail
/// 1e-3, 99.9 %). A report with a good position and a bad velocity (a stale or default-NACv
/// velocity, 39 m/s RMS outliers on the 2026-11-14 clip) must not move the track's velocity once
/// the velocity noise is the calibrated one.
pub const VEL_GATE_CHI2_STRICT: f64 = 13.815_510_557_964_274;
/// The default velocity gate: chi-square 400 (20 sigma). With the steady-flight noise the strict
/// gate above drops velocities of a manoeuvring aircraft; 400 still drops the 39 m/s outliers
/// and was slightly better on the 2026-11-14 clip, and on 2026-11-15.
pub const VEL_GATE_CHI2: f64 = 400.0;
/// The default manoeuvre allowance of the position gate, m/s^2: variance (0.5 a dt^2)^2 added to
/// the innovation's, so a track that missed reports in a turn is not refused. Variant A5 of the
/// 2026-11-14 experiments (position RMS 50.3 to 30.8 m, jump_rejected and resyncs cut by about
/// 85 %), chosen before the 2026-11-15 validation was seen; a forged 2 km report is accepted at
/// 10 s of gap instead of 12 s.
pub const GATE_MANOEUVRE_MPS2: f64 = 5.0;
/// A report that passes the position gate but lies farther than this, metres, from the track's
/// predicted position is applied but marked unconfirmed, with the state from before it kept for a
/// rollback, until the track's next report decides it (RT-239). A fixed distance: it does not
/// scale with the gap or the covariance, which is what lets the widening gate pass a single
/// forged report after a gap. 926 m is 0.5 NM, the radius of [`COAST_MAX_R95_M`].
pub const CONFIRM_MOVE_M: f64 = 926.0;
/// A later report agrees with a path (the moved track's or the old one's) if it lies within this,
/// metres, of that path's prediction at its time, growing with the time since the move by the
/// manoeuvre allowance and never over [`CONFIRM_MOVE_M`]: the two reports must agree more
/// closely than the move they decide.
pub const CONFIRM_AGREE_M: f64 = 300.0;
/// The floor of the time, ns, within which the next report decides a move (see
/// [`CONFIRM_WINDOW_INTERVALS`]); a report past the window leaves the move standing (undecided,
/// counted).
pub const CONFIRM_WINDOW_NS: i64 = 5 * NS_PER_S;
/// The window follows the track's report rate: this many expected report intervals, with
/// [`CONFIRM_WINDOW_NS`] as the floor and the drop age as the ceiling. Live 1 Hz data keeps the
/// floor; the thinned clips (7 s median interval) have about 28 s for the second report.
pub const CONFIRM_WINDOW_INTERVALS: i64 = 4;
/// The deciding report is at least this long, ns, after the move: a copy of the same message,
/// or a burst within one squitter, is not a second report.
pub const CONFIRM_MIN_SEP_NS: i64 = NS_PER_S / 2;
/// How many reports at one place decide a move, either way (RT-273): the move's own report counts
/// for the moved place, so one more report there confirms it, and the old path needs two reports
/// of its own, at least [`CONFIRM_MIN_SEP_NS`] apart, to roll it back (one report, the case of a
/// forger who replays the old place, or of a genuine relocation seen by one stray plot, decides
/// nothing). The rule is written for two: the moved side needs one more, the old side keeps one
/// earlier vote (`const` assertion in `confirm`).
pub const CONFIRM_REPORTS: u8 = 2;
/// How many unconfirmed moves can hold a rollback snapshot at once (2200 bytes each): 64, 141 kB.
/// Measured on the two development days (the 2026-11-14 day to choose, 2026-11-15 as the
/// check): the most tracks with an unconfirmed move at once was 7 on 2026-11-14 and
/// 7 on 2026-11-15, so 64 is about nine times the largest peak seen; a move is open
/// for the window of its track (5 s at 1 Hz, 28 s on a 7 s feed) unless its track goes silent,
/// when it stays until the track is dropped (120 s). What a forger can force: one gate-passing
/// far report on each of many confirmed aircraft addresses at once, each holding a snapshot until
/// the track's next report decides it or the track ends. With the pool full the move is still
/// applied and marked, only without a snapshot, so such a flood costs rollback and never the
/// mark (`move_no_snapshot` counts it); it needs a live confirmed track and a gap of 10 s or more
/// on it for every slot taken, and it cannot grow memory, which is allocated once.
pub const CONFIRM_POOL: usize = 64;
/// An altitude innovation above this many standard deviations is not applied.
pub const ALT_GATE_SIGMAS: f64 = 10.0;
/// This many rejections in a row re-initialise the filter at the new report (the track the
/// comparisons were anchored to was probably the bad one). The value of `oa_truth::JUMP_RESYNC`.
pub const RESYNC_AFTER: u8 = oa_truth::JUMP_RESYNC as u8;
/// A measurement more than this ahead of the tracker clock does not move the clock until a run
/// of them from enough distinct sensors and addresses confirms it (`crate::time_gate`); until
/// then it is counted and not applied. Equal to the out-of-order window and below the drop age:
/// no single measurement may move the clock by as much as the age at which a silent track is
/// dropped, or one forged or faulty report would sweep every live track and make every real
/// measurement after it late.
pub const CLOCK_STEP_LIMIT_NS: i64 = OOO_WINDOW_NS;
/// Consecutive far-ahead measurements that confirm a step of the clock forward (and, doubled, the
/// late refusals that confirm a step back).
pub const CLOCK_STEP_CONFIRM: u32 = 3;
/// Allowance added to the plausibility distance, metres (`oa_truth::JUMP_SLACK_M`).
pub const PLAUSIBLE_SLACK_M: f64 = oa_truth::JUMP_SLACK_M;

// ---- Time, ordering, output -----------------------------------------------------------

/// Output rate for consumers: one set of tracks per tick, ns (1 s).
pub const TICK_NS: i64 = NS_PER_S;
/// A measurement older than the newest time seen by more than this is counted and dropped; one
/// inside it is applied by re-filtering from stored state, ns. Set from the measured
/// out-of-order depth of the feeds.
pub const OOO_WINDOW_NS: i64 = 10 * NS_PER_S;
/// Updates stored per track for re-filtering. Bounds the re-filter depth in updates (as
/// [`OOO_WINDOW_NS`] bounds it in time); at 1 Hz it covers the window.
pub const RING_DEPTH: usize = 12;

// ---- Track management -----------------------------------------------------------------

/// Reports needed to confirm an ADS-B track (the M of M-of-N).
pub const INIT_M: u8 = 2;
/// Window of reports the M must fall in (the N of M-of-N). The window in time is N expected
/// intervals.
pub const INIT_N: u8 = 3;
/// Expected interval between reports of one aircraft, ns: ADS-B squitters position at 2 Hz and
/// the receiver network delivers about 1 per second. Set from the recorded interval
/// distribution of the input (the scoring run measures it from the thinned set).
pub const EXPECTED_INTERVAL_NS: i64 = NS_PER_S;
/// A confirmed track is flagged coasting when its newest update is older than this many
/// expected intervals.
pub const COAST_AFTER_INTERVALS: i64 = 3;
/// A track with no update for this long is dropped, ns. Set per airspace from the recorded gap
/// distribution (en route, terminal).
pub const DROP_AFTER_NS: i64 = 60 * NS_PER_S;
/// A row (tick or update, coasting or live) whose 95% horizontal uncertainty radius is over this,
/// metres, carries `O_UNCERTAIN`: 926 m (0.5 NM), the size ESASSP uses for a large position error
/// (correlated errors over 926 m, BENCHMARKS R5). The row is published all the same, and counted
/// in `rows_uncertain`: a live track is never absent from the output, and a consumer decides what
/// to show of an uncertain one. (It withheld coasting rows past this radius before, OA-TRK-338.)
pub const COAST_MAX_R95_M: f64 = 926.0;
/// An update after more than this long, ns, since the track's previous applied update carries
/// `O_REACQUIRED` (15 s: more than five expected intervals, past a coast).
pub const REACQUIRE_GAP_NS: i64 = 15 * NS_PER_S;
/// The 95% radius, metres, a row of a track with an unconfirmed move gets when its widening
/// cannot be computed (RT-239). 500 km: larger than any sector this tracker serves, and finite
/// through every consumer (a covariance of the largest `f32` made the fusion window width
/// saturate a counter and overflow its sum).
pub const FAIL_CLOSED_R95_M: f64 = 500_000.0;
/// The 95% quantile of the chi-squared distribution with two degrees of freedom.
pub const CHI2_2_95: f64 = 5.991_464_547_107_979;
/// Capacity: simultaneous tracks. ERAM states 1,900 per centre; 4,096 is two times that.
pub const MAX_TRACKS: usize = 4096;
/// Capacity of the output table between drains. At one row per track per second plus one per
/// update, 65,536 rows is about 16 s of a full centre.
pub const OUT_CAPACITY: usize = 65_536;
/// Capacity of the track log (one entry per track that ended or was flushed).
pub const LOG_CAPACITY: usize = 1 << 20;

// ---- Comparison baseline --------------------------------------------------------------

/// White acceleration densities of the single straight-model baseline (modes 1, 2 and 3 of
/// `TrackerConfig::h_mode`), m^2/s^3: the interacting pair's own straight value, a value for
/// manoeuvring targets and one tuned for standard-rate turns. Used only to measure what the
/// interacting pair gains.
pub const CV_ONLY_Q: [f64; 3] = [H_Q_CV, 0.5, 5.0];

/// The process noise of baseline mode `mode` (1 to 3).
pub fn cv_only_q(mode: u8) -> f64 {
    CV_ONLY_Q
        .get(usize::from(mode).saturating_sub(1))
        .copied()
        .unwrap_or(H_Q_CV)
}
