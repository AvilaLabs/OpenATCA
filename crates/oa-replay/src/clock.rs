//! The replay clock's step rules. Core (pure, B1): plain data and free functions, no I/O, no
//! system clock, no allocation.
//!
//! Replay time is the receive time of the frames, and the tables age their contents against it,
//! so one wrong receive time must not be able to move it. The rules, for a frame at time `t`
//! when the clock stands at `c` (`STEP` is [`CLOCK_STEP_NS`], 10 minutes):
//!
//! 1. **First frame.** The clock starts at `t`. Nothing is known yet that could say it is wrong;
//!    if it is, rule 4 rescues the clock once later frames disagree with it.
//! 2. **Ordinary.** `|t - c| <= STEP`: a frame at or after `c` moves the clock to `t`; a frame
//!    before it leaves the clock and is counted as a time regression. This is the whole of the
//!    behaviour for any recording whose frames are never more than `STEP` apart (the measured
//!    SFDPS feed has at most 0.5 s between frames), so such a recording replays exactly as it
//!    did before these rules.
//! 3. **Candidate.** `|t - c| > STEP`: the frame is processed at the old clock, counted as held
//!    ([`Clock::held`]; a frame that is behind the clock is also counted as a regression), and
//!    becomes a candidate step. The clock does not move.
//! 4. **Confirmation.** The step is confirmed by [`CLOCK_CONFIRM_FRAMES`] (3) consecutive
//!    candidates in the same direction whose times do not decrease. The third is applied at the
//!    new clock. A frame inside the tolerance, a candidate in the other direction or a candidate
//!    earlier than the previous one drops (or restarts) the candidate, so one wild frame, or
//!    scattered wild frames, never move the clock; the held count says how many there were.
//! 5. **Step.** On confirmation the clock takes the third frame's time and every age the tables
//!    hold is shifted by the same amount ([`Tick::Step`]), so a step of the recorder's clock,
//!    forward or back, neither evicts every flight nor leaves them stranded in the future: the
//!    time since each flight was last heard is what it was before the step. The eviction and
//!    checkpoint cadences are due at once, so a sweep and a checkpoint run at the new time. A real pause in a recording gets the same
//!    treatment, and flights then age from the pause rather than all being evicted at once.
//!
//! A wrong frame of any size is therefore refused, and a wrong frame within `STEP` moves the
//! clock by at most `STEP`, once. The price is on sparse feeds: frames more than `STEP` apart
//! (a replay of one slow feed alone, or a pause in a recording) are held until the third, so the
//! first two after the gap are processed at the old time, and the ages in the tables carry over
//! the gap instead of running through it.

use crate::table::NS_PER_S;

/// Largest difference between a frame's time and the clock accepted without confirmation:
/// 10 minutes.
pub const CLOCK_STEP_NS: i64 = 10 * 60 * NS_PER_S;
/// Consecutive candidates that confirm a step.
pub const CLOCK_CONFIRM_FRAMES: u32 = 3;

/// The clock's candidate state and counts. Plain data (A1).
// A7.2: cold struct, size guard waived (one per pipeline)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    /// Time of the latest candidate frame.
    pub cand_last_ns: i64,
    /// Consecutive candidates so far; 0 when there is none.
    pub cand_n: u32,
    /// Whether the candidates are behind the clock.
    pub cand_back: bool,
    /// Frames processed at the old clock because their time was refused (candidates that did
    /// not complete a confirmation).
    pub held: u64,
    /// Forward steps confirmed.
    pub forward_steps: u64,
    /// Backward steps confirmed.
    pub backward_steps: u64,
}

/// What a frame's time did to the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// The clock takes the frame's time.
    Advance,
    /// The frame is a little behind the clock: counted as a regression, the clock stays.
    Regress,
    /// The frame's time is refused for now (`back`: it is behind the clock); the clock stays.
    Held {
        /// Whether the frame is behind the clock.
        back: bool,
    },
    /// A step is confirmed: the clock takes the frame's time, and the tables shift their ages
    /// by `delta` (the new time minus the old, negative for a step back).
    Step {
        /// New clock minus old clock, ns.
        delta: i64,
    },
}

/// Applies the rules above for a frame at `t` with the clock at `now` (`i64::MIN` before the
/// first frame). The caller moves its clock for [`Tick::Advance`] and [`Tick::Step`].
pub fn clock_observe(c: &mut Clock, now: i64, t: i64) -> Tick {
    if now == i64::MIN {
        return Tick::Advance;
    }
    let back = t < now;
    let far = if back {
        now.saturating_sub(t) > CLOCK_STEP_NS
    } else {
        t.saturating_sub(now) > CLOCK_STEP_NS
    };
    if !far {
        c.cand_n = 0;
        return if back { Tick::Regress } else { Tick::Advance };
    }
    if c.cand_n > 0 && c.cand_back == back && t >= c.cand_last_ns {
        c.cand_n += 1;
    } else {
        c.cand_n = 1;
        c.cand_back = back;
    }
    c.cand_last_ns = t;
    if c.cand_n >= CLOCK_CONFIRM_FRAMES {
        c.cand_n = 0;
        if back {
            c.backward_steps += 1;
        } else {
            c.forward_steps += 1;
        }
        return Tick::Step {
            delta: t.saturating_sub(now),
        };
    }
    c.held += 1;
    Tick::Held { back }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    fn any_clock() -> Clock {
        let c = Clock {
            cand_last_ns: kani::any(),
            cand_n: kani::any(),
            cand_back: kani::any(),
            held: kani::any(),
            forward_steps: kani::any(),
            backward_steps: kani::any(),
        };
        // Reachable states: a confirmation resets the count before it reaches the limit, and
        // the three counters count frames, which cannot reach 2^64 - 1.
        kani::assume(c.cand_n < CLOCK_CONFIRM_FRAMES);
        kani::assume(c.held < u64::MAX);
        kani::assume(c.forward_steps < u64::MAX && c.backward_steps < u64::MAX);
        c
    }

    // REQ: OA-RPL-100
    #[kani::proof]
    fn clock_observe_total_and_confirms() {
        let mut c = any_clock();
        let before = c;
        let (now, t): (i64, i64) = (kani::any(), kani::any());
        let tick = clock_observe(&mut c, now, t);
        // The candidate count stays below the confirmation limit.
        assert!(c.cand_n < CLOCK_CONFIRM_FRAMES);
        if let Tick::Step { delta } = tick {
            // A step needs CLOCK_CONFIRM_FRAMES consecutive candidates: this frame is the
            // last of them, so there were at least CLOCK_CONFIRM_FRAMES - 1 before it.
            assert!(now != i64::MIN);
            assert!(before.cand_n >= CLOCK_CONFIRM_FRAMES - 1);
            assert_eq!(delta, t.saturating_sub(now));
            assert_eq!(c.cand_n, 0);
        }
        if before.cand_n == 0 && now != i64::MIN {
            assert!(!matches!(tick, Tick::Step { .. }));
        }
    }

    // REQ: OA-RPL-100
    #[kani::proof]
    fn clock_observe_three_candidates_before_a_step() {
        let mut c = any_clock();
        kani::assume(c.cand_n == 0);
        // Room for three more counts.
        kani::assume(c.held < u64::MAX - 3);
        let now: i64 = kani::any();
        kani::assume(now != i64::MIN);
        let (t1, t2, t3): (i64, i64, i64) = (kani::any(), kani::any(), kani::any());
        // The clock does not move between candidates (a refused frame leaves it alone).
        let a = clock_observe(&mut c, now, t1);
        let b = clock_observe(&mut c, now, t2);
        let was_step = matches!(a, Tick::Step { .. }) || matches!(b, Tick::Step { .. });
        assert!(!was_step);
        let d = clock_observe(&mut c, now, t3);
        if let Tick::Step { .. } = d {
            // Three frames all far from the clock, on one side, not going backward in time.
            assert!(matches!(a, Tick::Held { .. }) && matches!(b, Tick::Held { .. }));
        }
    }
}
