//! The large-move hold (RT-239). A report that passes the position gate but lies farther than
//! `confirm_move_m` from the track's predicted position is not applied: the track keeps it as
//! its one pending report, and a later report that agrees with it confirms both. The gate
//! widens with the time since the track's last update, so after a gap it would pass one forged
//! report of any plausible size; this distance is fixed and does not. Core (pure): the decision
//! is a function of numbers only, so it is proved apart from the filter (`tracker_proofs`).

use crate::params::CONFIRM_MIN_SEP_NS;
use crate::types::{Measurement, TrackerConfig};

/// A track's pending report. Fixed size: one slot per track, no allocation.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(crate) struct Pending {
    pub m: Measurement,
    /// 1 if `m` is held.
    pub valid: u8,
}
// Budget: the 72-byte measurement and a flag, 80 with alignment.
const _: () = assert!(core::mem::size_of::<Pending>() == 80);

/// What to do with a report that passed the position gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Move {
    /// A small move: apply it as always.
    Apply,
    /// Keep it as the pending report (replacing any); `expired` if the replaced one was stale.
    Hold { expired: bool },
    /// It agrees with the pending report: apply the pending one, then it.
    Confirm,
}

/// Whether a move of `miss_m` metres from the predicted position is held. A distance that is
/// not a number is held (fail closed).
pub(crate) fn move_is_large(miss_m: f64, move_m: f64) -> bool {
    miss_m.is_nan() || miss_m > move_m
}

/// Whether the pending report (time `pend_t_ns`) can no longer be confirmed at `now_ns`: older
/// than the window, or older than the track's last applied update (the track moved on without
/// it, so applying it would run time backwards).
pub(crate) fn pending_stale(pend_t_ns: i64, last_ns: i64, now_ns: i64, window_ns: i64) -> bool {
    pend_t_ns < last_ns || now_ns.saturating_sub(pend_t_ns) > window_ns
}

/// The decision for a report at `rep_t_ns` that passed the gate, `miss_m` from the predicted
/// position and `agree_m` from the pending report propagated to its time (not a number when
/// there is no pending report or it could not be computed). `Confirm` only when the move is
/// large, a pending report exists and is not stale, the report is at least the minimum
/// separation after it, and `agree_m` is within `confirm_agree_m`. Nothing here depends on the
/// time since the track's last update or on its covariance: that is the point.
pub(crate) fn confirm_decide(
    pending_t_ns: Option<i64>,
    last_ns: i64,
    rep_t_ns: i64,
    miss_m: f64,
    agree_m: f64,
    cfg: &TrackerConfig,
) -> Move {
    if !move_is_large(miss_m, cfg.confirm_move_m) {
        return Move::Apply;
    }
    let Some(pt) = pending_t_ns else {
        return Move::Hold { expired: false };
    };
    if pending_stale(pt, last_ns, rep_t_ns, cfg.confirm_window_ns) {
        return Move::Hold { expired: true };
    }
    if rep_t_ns.saturating_sub(pt) >= CONFIRM_MIN_SEP_NS && agree_m <= cfg.confirm_agree_m {
        Move::Confirm
    } else {
        Move::Hold { expired: false }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::DEFAULT_CONFIG;

    const S: i64 = 1_000_000_000;

    // REQ: OA-TRK-347, OA-TRK-348
    #[test]
    fn the_decision_table() {
        let c = DEFAULT_CONFIG;
        let d = |pend: Option<i64>, last: i64, t: i64, miss: f64, agree: f64| {
            confirm_decide(pend, last, t, miss, agree, &c)
        };
        // Small moves are applied, whatever the rest says.
        assert_eq!(d(None, 0, 100 * S, 926.0, f64::NAN), Move::Apply);
        assert_eq!(d(Some(0), 0, 2 * S, 10.0, 0.0), Move::Apply);
        // A large move with no pending report, or a not-a-number distance, is held.
        assert_eq!(
            d(None, 0, 100 * S, 927.0, f64::NAN),
            Move::Hold { expired: false }
        );
        assert_eq!(
            d(None, 0, 100 * S, f64::NAN, 0.0),
            Move::Hold { expired: false }
        );
        // Confirmed: pending 1 s before, agreeing.
        assert_eq!(
            d(Some(40 * S), 20 * S, 41 * S, 2000.0, 299.0),
            Move::Confirm
        );
        // Not agreeing, too soon, too late, superseded by an applied update.
        assert_eq!(
            d(Some(40 * S), 20 * S, 41 * S, 2000.0, 301.0),
            Move::Hold { expired: false }
        );
        assert_eq!(
            d(Some(40 * S), 20 * S, 41 * S, 2000.0, f64::NAN),
            Move::Hold { expired: false }
        );
        assert_eq!(
            d(Some(40 * S), 20 * S, 40 * S + S / 4, 2000.0, 0.0),
            Move::Hold { expired: false }
        );
        assert_eq!(
            d(Some(40 * S), 20 * S, 40 * S + S / 2, 2000.0, 0.0),
            Move::Confirm
        );
        assert_eq!(d(Some(40 * S), 20 * S, 45 * S, 2000.0, 0.0), Move::Confirm);
        assert_eq!(
            d(Some(40 * S), 20 * S, 45 * S + 1, 2000.0, 0.0),
            Move::Hold { expired: true }
        );
        assert_eq!(
            d(Some(40 * S), 41 * S, 42 * S, 2000.0, 0.0),
            Move::Hold { expired: true }
        );
    }
}
