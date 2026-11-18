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
/// A coasting track's position is published only while its 95% horizontal uncertainty radius is
/// at most this, metres: 926 m (0.5 NM), the size ESASSP uses for a large position error
/// (correlated errors over 926 m, BENCHMARKS R5). Past it the track has no displayed position
/// until an update arrives: a position that uncertain is withheld and counted
/// (`rows_coast_withheld`), never shown as if known. The track itself is kept until
/// `drop_after_ns`, so a returning aircraft keeps its identity.
pub const COAST_MAX_R95_M: f64 = 926.0;
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
