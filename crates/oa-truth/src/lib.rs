#![forbid(unsafe_code)]
//! oa-truth: reference trajectories from ADS-B, the truth side of the M2 benchmark
//! (docs/M2_DESIGN.md section 1). Class `split` (B7). Core (pure, B1): the smoother, the
//! quality rules, the leave-one-out evaluation, the queries ([`truth_at`], [`truth_hash_into`])
//! and the data types. Shell: [`build_truth`], whose output is a function of its input but
//! which carries a global test-hook counter and names `oa_adsb` (see `shell/mod.rs`), the [`clip`] reader and the `oa-truth-check` binary
//! (both read files; the binary also reads the clock and writes stdout).
//!
//! # What it does
//!
//! 1. **Selection.** Reports must carry NACp >= 8, NIC >= 7, SIL 3 and, where present, ADS-B
//!    version 2 ([`DEFAULT_CONFIG`]). The quality fields are printed by readsb on a minority of
//!    rows, so each is carried forward within an aircraft for at most 60 s; a row with no value
//!    in that age is not truth. Rows with readsb's stale flag (position older than 20 s, the
//!    only fault flag a trace row has), rows whose source is not ADS-B (MLAT, TIS-B, ADS-R and
//!    the rest carry no meaningful NACp), duplicate times (same-time reports that disagree beyond their NACp radii, or more than 16 distinct), reports of the reserved addresses
//!    (all zeros and all ones, under `invalid`) and kinematic jumps are removed. A jump is a
//!    displacement the speed rule (`oa_adsb::jump_verdict`) calls impossible: over the hard cap of
//!    the emitter category, or inconsistent with the aircraft's own ground speed and the gap. Every
//!    removed row is counted under one reason ([`REMOVE_NAMES`]) in [`TruthStats`], so
//!    `rows_in = rows_kept + removed` always balances.
//! 2. **Legs.** An aircraft (the 24-bit address plus `oa_adsb::NON_ICAO`, the same key as the
//!    ADS-B table in oa-replay, so non-ICAO addresses stay separate) splits into legs at
//!    readsb's new-leg flag, at a gap of more than 120 s between kept reports, and (counted
//!    separately) after repeated jump rejections or when the leg leaves the frame's range.
//! 3. **Smoother.** Per leg, in a local east-north frame about the leg's first report
//!    (`frame.rs`; oa-geo has no local frame), a fixed-interval Rauch-Tung-Striebel smoother
//!    with a nearly-constant-acceleration model (`smooth.rs`). The measurement noise per report
//!    is its NACp bound as a 95 % radius, converted to a per-axis sigma.
//! 4. **No extrapolation.** [`truth_at`] answers only inside a leg and only where a real report
//!    lies within 10 s on both sides of the query time (a report exactly at the time counts on
//!    both sides). Otherwise it says why ([`TruthQuery`]).
//! 5. **Accuracy, measured.** Leave-one-out: every fifth report of every leg is hidden, the leg
//!    is smoothed again without them and each hidden report is predicted from the rest under
//!    the same bracket rule. [`LooStats`] has the residual distribution (horizontal, cross-track,
//!    along-track, altitude) and how often the predicted 95 % uncertainty was honest. A truth
//!    sample is refused ([`TruthQuery::TooUncertain`]) when its 95 % horizontal radius exceeds
//!    50 m.
//! 6. **Altitude.** Barometric altitude is not smoothed: linear interpolation between the two
//!    bracketing reports, absent if either lacks it.
//!
//! The gates are named constants ([`MIN_NACP`] and the rest) copied into [`TruthConfig`]; the
//! leave-one-out result decides whether they stay.
//!
//! # Determinism
//!
//! Same inputs give the same [`Truth`] and [`TruthStats`], bit for bit. What guarantees it:
//! reports are put in the total order (address, time, input index) before anything else, so
//! input order does not matter; there are no hash maps, threads or randomness; every sum and
//! matrix product is written in a fixed order (`linalg.rs`) and the compiler does not fuse or
//! reorder floating-point operations; time differences are exact `i64` nanoseconds converted
//! once with a division; the leave-one-out hidden set is by position in the leg. The only
//! platform dependence is the C library's `sin`, `cos`, `atan2` and `asin` in the frame, which
//! can differ in the last bit between libm versions; one binary on one machine is bit exact,
//! and [`truth_hash_into`] is the digest that shows it.
//!
//! # Bounds and refusal
//!
//! Input is bounded by [`MAX_REPORTS`]; columns of different lengths and an invalid
//! [`TruthConfig`] are refused with [`TruthError`] (E5). A row that fails a rule is removed and
//! counted, never passed on and never dropped silently. Cold, offline code (A7.2, C6): the
//! truth model is a struct-of-arrays, one column per field, with no hot per-row struct.

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

pub use crate::core::quality::{
    nacp_epu_m, nacv_epu_mps, quality_check_row, quality_eval_row, Held, QualityRow, QualityState,
    QualityVerdict, HELD_NONE,
};
pub use crate::core::query::{
    aircraft_addrs, leg_addr, leg_count, leg_end_ms, leg_end_ns, leg_reports, leg_start_ms,
    leg_start_ns, legs_of, truth_at, truth_hash_into,
};
pub use crate::core::smooth::{sigma95_h_m, SIGMA95};
pub use crate::core::types::*;
pub use crate::shell::build::{build_truth, pair_evals_this_thread};
pub use crate::shell::clip;
pub use crate::shell::quality::{quality_check, quality_eval};
