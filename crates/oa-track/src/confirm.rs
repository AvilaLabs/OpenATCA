//! The large-move rule (RT-239). The position gate widens with the time since the track's last
//! update, so after a gap it passes one forged report of any plausible size. A report of a
//! confirmed track that passes the gate but lies farther than `confirm_move_m` from the
//! predicted position is applied at once, as without the rule, but the track keeps a snapshot of
//! its state from before it and marks every row it emits `O_MOVE_UNCONFIRMED` with a wider
//! uncertainty radius. The track's next report decides: it agrees with the moved track (the move
//! is confirmed), with the old path (the move is rolled back to the snapshot and the report
//! applied to it), or with neither or comes too late (the move stands). Core (pure): the
//! decision is a function of numbers only, so it is proved apart from the filter
//! (`tracker_proofs`).

use crate::params::{CONFIRM_MIN_SEP_NS, CONFIRM_WINDOW_INTERVALS};
use crate::types::TrackerConfig;

/// Whether a move of `miss_m` metres from the predicted position is a large one. A distance that
/// is not a number is large (fail closed).
pub(crate) fn move_is_large(miss_m: f64, move_m: f64) -> bool {
    miss_m.is_nan() || miss_m > move_m
}

/// The decision window of a track whose expected report interval is `expected_ns` (0: the
/// configuration's): four intervals, not under `confirm_window_ns` (the floor, 5 s: live 1 Hz
/// data keeps it) and not over the drop age. A thinned feed with a 7 s interval waits about 28 s
/// or more, long enough for its next report.
pub(crate) fn confirm_window_ns(cfg: &TrackerConfig, expected_ns: i64) -> i64 {
    let interval = if expected_ns > 0 {
        expected_ns
    } else {
        cfg.expected_interval_ns
    };
    interval
        .saturating_mul(CONFIRM_WINDOW_INTERVALS)
        .min(cfg.drop_after_ns)
        .max(cfg.confirm_window_ns)
}

/// How closely a deciding report `sep_ns` after the move must agree with a path, metres:
/// `confirm_agree_m` plus what a manoeuvre of `gate_manoeuvre_mps2` could add in that time
/// (`0.5 a t^2`), never over `confirm_move_m`. Only the agreement of two reports loosens with
/// time; the move that starts the rule is a fixed distance.
pub(crate) fn agree_limit_m(cfg: &TrackerConfig, sep_ns: i64) -> f64 {
    let s = sep_ns.max(0) as f64 / 1.0e9;
    (cfg.confirm_agree_m + 0.5 * cfg.gate_manoeuvre_mps2 * s * s).min(cfg.confirm_move_m)
}

/// What the track's next report does to an unconfirmed move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolve {
    /// Too soon after the move to be a second report (a copy, or a burst): nothing is decided.
    Wait,
    /// It agrees with the moved track: the move is confirmed.
    Confirm,
    /// It agrees with the old path and not with the moved track: roll back, then apply it.
    Rollback,
    /// It agrees with neither, or comes after the window: the move stands.
    Undecided,
}

/// The decision for a report at `now_ns` on a track whose move was applied at `start_ns`:
/// `cur_m` is its distance from the moved track's prediction and `old_m` from the old path's
/// (the snapshot's), at its time; `window_ns` the track's window. `Confirm` only with a report
/// at least the minimum separation after the move, within the window, that is within
/// [`agree_limit_m`] of the moved track and no farther from it than from the old path;
/// `Rollback` only likewise with the old path. Not a number agrees with nothing. Nothing here
/// depends on the time since the track's last update before the move or on its covariance: that
/// is the point.
pub(crate) fn resolve_decide(
    start_ns: i64,
    now_ns: i64,
    cur_m: f64,
    old_m: f64,
    window_ns: i64,
    cfg: &TrackerConfig,
) -> Resolve {
    let sep = now_ns.saturating_sub(start_ns);
    if sep < CONFIRM_MIN_SEP_NS {
        return Resolve::Wait;
    }
    if sep > window_ns {
        return Resolve::Undecided;
    }
    let limit = agree_limit_m(cfg, sep);
    if cur_m <= limit && (old_m.is_nan() || old_m >= cur_m) {
        Resolve::Confirm
    } else if old_m <= limit {
        Resolve::Rollback
    } else {
        Resolve::Undecided
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::DEFAULT_CONFIG;

    const S: i64 = 1_000_000_000;

    // REQ: OA-TRK-348
    #[test]
    fn the_decision_table() {
        let c = DEFAULT_CONFIG;
        let w = c.confirm_window_ns;
        let d = |sep_ns: i64, cur: f64, old: f64| {
            resolve_decide(100 * S, 100 * S + sep_ns, cur, old, w, &c)
        };
        assert_eq!(d(S / 4, 0.0, 2000.0), Resolve::Wait);
        assert_eq!(d(S, 10.0, 2000.0), Resolve::Confirm);
        assert_eq!(d(S, 2000.0, 10.0), Resolve::Rollback);
        assert_eq!(d(S, 2000.0, 2000.0), Resolve::Undecided);
        assert_eq!(d(S, f64::NAN, f64::NAN), Resolve::Undecided);
        // The limit is 302.5 m at 1 s: 303 m agrees with neither.
        assert_eq!(d(S, 302.0, 5000.0), Resolve::Confirm);
        assert_eq!(d(S, 303.0, 5000.0), Resolve::Undecided);
        // Both within the limit: the nearer one.
        assert_eq!(d(S, 100.0, 200.0), Resolve::Confirm);
        assert_eq!(d(S, 200.0, 100.0), Resolve::Rollback);
        // Window.
        assert_eq!(d(5 * S, 10.0, 2000.0), Resolve::Confirm);
        assert_eq!(d(5 * S + 1, 10.0, 2000.0), Resolve::Undecided);
    }

    // REQ: OA-TRK-347
    #[test]
    fn the_window_and_the_limit_follow_the_rate_and_the_gap() {
        let c = DEFAULT_CONFIG;
        assert_eq!(confirm_window_ns(&c, 0), 5 * S);
        assert_eq!(confirm_window_ns(&c, 7 * S), 28 * S);
        assert_eq!(confirm_window_ns(&c, 1000 * S), c.drop_after_ns);
        assert_eq!(agree_limit_m(&c, 0), 300.0);
        assert_eq!(agree_limit_m(&c, 1000 * S), 926.0);
    }
}
