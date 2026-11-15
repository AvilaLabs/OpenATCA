//! The replay clock's step rules. Core (pure, B1): plain data and free functions, no I/O, no
//! system clock, no allocation.
//!
//! Replay time is the receive time of the frames, and the tables age their contents against it,
//! so one wrong receive time must not be able to move it. Each feed keeps its own record (the
//! time of its last accepted frame and its own candidate step), because the feeds' receive times
//! can differ by a constant (their recorders' clocks, their delivery delays) and one feed's
//! times are only comparable with that feed's. The rules, for a frame at time `t` of feed `f`
//! when the clock stands at `c` (`TOL` is [`CLOCK_TOLERANCE_NS`], 2 minutes):
//!
//! 1. **First frame.** The clock starts at `t`. Nothing is known yet that could say it is wrong;
//!    if it is, rule 4 rescues the clock once later frames disagree with it.
//! 2. **Ordinary.** The frame is within `TOL` of the feed's own last accepted frame (`own`), or
//!    within `TOL` of the clock. Then it is accepted: it becomes the feed's last frame, any
//!    candidate steps are dropped (a frame within `TOL` of the clock refutes every feed's
//!    candidate; one that only follows its own feed refutes that feed's), and the clock takes `t`
//!    if `t > c`. A frame before `own` is a time regression of its feed: it is counted, the feed's
//!    last frame stays and the clock stays. A frame behind the clock but not before its own feed's
//!    last frame is a feed that runs behind the others, not a regression: it is processed at the
//!    clock and nothing is counted. This is the whole of the behaviour for any recording whose
//!    frames are never more than `TOL` apart in each feed's own time (the measured SFDPS feed has
//!    at most 0.5 s between frames), so such a recording replays exactly as it did before these
//!    rules, and a clock moved by an accepted frame is never moved by more than `TOL` at once.
//! 3. **Candidate.** Otherwise the frame is far from both: it is processed at the old clock,
//!    counted as held ([`Clock::held`]; a frame that is behind the clock is also counted as a
//!    regression), and becomes a candidate step of its feed. The clock does not move.
//! 4. **Confirmation.** A feed's step is confirmed by [`CLOCK_CONFIRM_FRAMES`] (3) consecutive
//!    candidates of that feed in the same direction whose times do not decrease. The third is
//!    applied at the new clock. A frame that is not a candidate, a candidate in the other
//!    direction or one earlier than the previous one drops (or restarts) the candidate, so one
//!    wild frame, or scattered wild frames, never move the clock; the held count says how many
//!    there were. A feed that is steadily more than `TOL` behind the others is held for the same
//!    reason and never confirms, as long as the others' frames keep refuting it.
//! 5. **Step.** On confirmation the clock takes the third frame's time and every age the tables
//!    hold is shifted by the same amount ([`Tick::Step`]), so a step of the recorder's clock,
//!    forward or back, neither evicts every flight nor leaves them stranded in the future: the
//!    time since each flight was last heard is what it was before the step. Every other feed's
//!    last frame is forgotten, so its next frames are compared with the new clock. The eviction
//!    and checkpoint cadences are due at once, so a sweep and a checkpoint run at the new time. A
//!    real pause in a recording gets the same treatment, and flights then age from the pause
//!    rather than all being evicted at once. The pipeline marks everything last heard before the
//!    step as stale until it is heard again, because its age no longer says how long ago that was.
//!
//! # The tolerance
//!
//! Frames of the SFDPS feed carry up to 60 s of report time each (a flight's report may be a
//! minute older than the frame that delivers it), and the feeds' clocks can differ by a few
//! seconds; 120 s is twice the largest spread seen in that feed. It is also what bounds a forged
//! time that is accepted without confirmation: 2 minutes once, where a 10-minute allowance let a
//! single frame age out flights whose age limit is 10 minutes, and a few such frames age out
//! every flight. Frames more than `TOL` apart in a feed (a replay of one slow feed alone, or a
//! pause in a recording) are held until the third, so the first two after the gap are processed
//! at the old time, and the ages in the tables carry over the gap instead of running through it.

use crate::pipeline::FEED_COUNT;
use crate::table::NS_PER_S;

/// Largest difference between a frame's time and its feed's last frame, or the clock, accepted
/// without confirmation: 2 minutes (see the module documentation).
pub const CLOCK_TOLERANCE_NS: i64 = 120 * NS_PER_S;
/// Consecutive candidates that confirm a step.
pub const CLOCK_CONFIRM_FRAMES: u32 = 3;

/// One feed's record. Plain data (A1).
// A7.2: cold struct, size guard waived (one per feed)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedClock {
    /// Time of the feed's last accepted frame; meaningful only if `seen`.
    pub last_ns: i64,
    /// Whether the feed has an accepted frame to compare with.
    pub seen: bool,
    /// Time of the latest candidate frame.
    pub cand_last_ns: i64,
    /// Consecutive candidates so far; 0 when there is none.
    pub cand_n: u32,
    /// Whether the candidates are behind the clock.
    pub cand_back: bool,
}

/// The clock's per-feed state and counts. Plain data (A1).
// A7.2: cold struct, size guard waived (one per pipeline)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    /// Each feed's record, indexed by the feed.
    pub feeds: [FeedClock; FEED_COUNT],
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
    /// The frame is behind the clock because its feed runs behind the others; it is processed at
    /// the clock, which stays. Nothing is counted.
    Keep,
    /// The frame is before its own feed's last frame: counted as a regression, the clock stays.
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

fn within(a: i64, b: i64) -> bool {
    a.saturating_sub(b).saturating_abs() <= CLOCK_TOLERANCE_NS
}

/// Applies the rules above for a frame of feed `feed` at `t` with the clock at `now` (`i64::MIN`
/// before the first frame). The caller moves its clock for [`Tick::Advance`] and [`Tick::Step`].
pub fn clock_observe(c: &mut Clock, feed: usize, now: i64, t: i64) -> Tick {
    let feed = feed.min(FEED_COUNT - 1);
    let Some(own) = c.feeds.get(feed).copied() else {
        return Tick::Held { back: t < now };
    };
    if now == i64::MIN {
        if let Some(f) = c.feeds.get_mut(feed) {
            f.last_ns = t;
            f.seen = true;
        }
        return Tick::Advance;
    }
    let near_clock = within(t, now);
    let near_own = own.seen && within(t, own.last_ns);
    if near_own || near_clock {
        // Drop the candidates this frame refutes.
        for (i, f) in c.feeds.iter_mut().enumerate() {
            if near_clock || i == feed {
                f.cand_n = 0;
            }
        }
        if near_own && t < own.last_ns {
            return Tick::Regress;
        }
        if let Some(f) = c.feeds.get_mut(feed) {
            f.last_ns = t;
            f.seen = true;
        }
        return if t >= now { Tick::Advance } else { Tick::Keep };
    }
    let back = t < now;
    let mut confirmed = false;
    if let Some(f) = c.feeds.get_mut(feed) {
        if f.cand_n > 0 && f.cand_back == back && t >= f.cand_last_ns {
            f.cand_n += 1;
        } else {
            f.cand_n = 1;
            f.cand_back = back;
        }
        f.cand_last_ns = t;
        if f.cand_n >= CLOCK_CONFIRM_FRAMES {
            f.cand_n = 0;
            confirmed = true;
        }
    }
    if confirmed {
        if back {
            c.backward_steps += 1;
        } else {
            c.forward_steps += 1;
        }
        // The other feeds' times were compared with the old clock; they are compared with the new.
        for (i, f) in c.feeds.iter_mut().enumerate() {
            f.seen = i == feed;
            f.cand_n = 0;
        }
        if let Some(f) = c.feeds.get_mut(feed) {
            f.last_ns = t;
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
