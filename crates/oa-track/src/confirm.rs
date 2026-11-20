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

use crate::params::{CONFIRM_MIN_SEP_NS, CONFIRM_REPORTS, CONFIRM_WINDOW_INTERVALS};
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

/// A sentinel, not a time: the value of the old-path vote column while the move has no vote yet
/// (no report time is `i64::MIN`: times are applicability times of reports). Every reader tests
/// for it before it subtracts.
pub(crate) const NO_VOTE_NS: i64 = i64::MIN;

// The decision is written for two reports at a place (see `CONFIRM_REPORTS`).
const _: () = assert!(CONFIRM_REPORTS == 2);

/// What the track's next report does to an unconfirmed move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolve {
    /// Too soon after the move (or after the earlier vote) to be a further report (a copy, or a
    /// burst): nothing is decided.
    Wait,
    /// It agrees with the moved track: the move is confirmed.
    Confirm,
    /// It agrees with the old path, a second time, apart from the first: roll back, then apply it.
    Rollback,
    /// It agrees with the old path and not with the moved track, the first time: a vote, the move
    /// stands, marked (one report decides nothing, RT-273).
    OldVote,
    /// The old path has a snapshot but cannot be predicted at the report's time: that is not
    /// agreement with anything (RT-276); the move stands, marked, until the window ends.
    OldUnpredictable,
    /// It agrees with neither, or comes after the window: the move stands.
    Undecided,
}

/// What a deciding report is measured against: see [`resolve_decide`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seen {
    pub cur_m: f64,
    pub old_m: f64,
    pub has_old: bool,
    pub old_vote_ns: i64,
}

/// The decision for a report at `now_ns` on a track whose move was applied at `start_ns`:
/// `cur_m` is its distance from the moved track's prediction and `old_m` from the old path's
/// (the snapshot's), at its time; `has_old` whether the move has a snapshot (without one `old_m`
/// is not a number and the old path is unknown); `old_vote_ns` the time of the earlier report that
/// agreed with the old path ([`NO_VOTE_NS`] if none); `window_ns` the track's window. `Confirm`
/// only with a report at least the minimum separation after the move, within the window, that is
/// within [`agree_limit_m`] of the moved track and no farther from it than from the old path;
/// `Rollback` likewise with the old path, and only after an earlier such report at least the
/// minimum separation before (the same corroboration as confirming: two reports at a place). Not
/// a number agrees with nothing, and with a snapshot present an old path that cannot be predicted
/// confirms nothing either. Nothing here depends on the time since the track's last update before
/// the move or on its covariance: that is the point.
pub(crate) fn resolve_decide(
    start_ns: i64,
    now_ns: i64,
    seen: &Seen,
    window_ns: i64,
    cfg: &TrackerConfig,
) -> Resolve {
    let Seen {
        cur_m,
        old_m,
        has_old,
        old_vote_ns,
    } = *seen;
    let sep = now_ns.saturating_sub(start_ns);
    if sep < CONFIRM_MIN_SEP_NS {
        return Resolve::Wait;
    }
    if sep > window_ns {
        return Resolve::Undecided;
    }
    if has_old && old_m.is_nan() {
        return Resolve::OldUnpredictable;
    }
    let limit = agree_limit_m(cfg, sep);
    if cur_m <= limit && (!has_old || old_m >= cur_m) {
        Resolve::Confirm
    } else if has_old && old_m <= limit {
        if old_vote_ns == NO_VOTE_NS {
            Resolve::OldVote
        } else if now_ns.saturating_sub(old_vote_ns) >= CONFIRM_MIN_SEP_NS {
            Resolve::Rollback
        } else {
            Resolve::Wait
        }
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
        // `vote` is the time of the earlier old-path report, NO_VOTE_NS if none.
        let d = |sep_ns: i64, cur: f64, old: f64, vote: i64| {
            let seen = Seen {
                cur_m: cur,
                old_m: old,
                has_old: true,
                old_vote_ns: vote,
            };
            resolve_decide(100 * S, 100 * S + sep_ns, &seen, w, &c)
        };
        let none = NO_VOTE_NS;
        assert_eq!(d(S / 4, 0.0, 2000.0, none), Resolve::Wait);
        assert_eq!(d(S, 10.0, 2000.0, none), Resolve::Confirm);
        // One report on the old path is a vote, not a rollback; the second, apart, is.
        assert_eq!(d(S, 2000.0, 10.0, none), Resolve::OldVote);
        assert_eq!(d(2 * S, 2000.0, 10.0, 101 * S), Resolve::Rollback);
        assert_eq!(d(2 * S, 2000.0, 10.0, 102 * S - S / 4), Resolve::Wait);
        assert_eq!(d(S, 2000.0, 2000.0, none), Resolve::Undecided);
        assert_eq!(d(S, f64::NAN, f64::NAN, none), Resolve::OldUnpredictable);
        // The limit is 302.5 m at 1 s: 303 m agrees with neither.
        assert_eq!(d(S, 302.0, 5000.0, none), Resolve::Confirm);
        assert_eq!(d(S, 303.0, 5000.0, none), Resolve::Undecided);
        // Both within the limit: the nearer one.
        assert_eq!(d(S, 100.0, 200.0, none), Resolve::Confirm);
        assert_eq!(d(S, 200.0, 100.0, none), Resolve::OldVote);
        // Window.
        assert_eq!(d(5 * S, 10.0, 2000.0, none), Resolve::Confirm);
        assert_eq!(d(5 * S + 1, 10.0, 2000.0, none), Resolve::Undecided);
        // An old path that cannot be predicted confirms nothing, whatever the moved track says.
        assert_eq!(d(S, 10.0, f64::NAN, none), Resolve::OldUnpredictable);
        assert_eq!(d(5 * S + 1, 10.0, f64::NAN, none), Resolve::Undecided);
        // With no snapshot the old path is unknown, not unpredictable: the moved track decides.
        let e = |cur: f64| {
            let seen = Seen {
                cur_m: cur,
                old_m: f64::NAN,
                has_old: false,
                old_vote_ns: none,
            };
            resolve_decide(100 * S, 101 * S, &seen, w, &c)
        };
        assert_eq!(e(10.0), Resolve::Confirm);
        assert_eq!(e(2000.0), Resolve::Undecided);
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
