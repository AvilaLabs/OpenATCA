//! The replay clock's step rules. Core (pure, B1): plain data and free functions, no I/O, no
//! system clock, no allocation.
//!
//! Replay time is the receive time of the frames, and the tables age their contents against it,
//! so one wrong receive time must not be able to move it. Each feed keeps its own record (the
//! time of its last accepted frame and its own candidate step), because the feeds' receive times
//! can differ by a constant (their recorders' clocks, their delivery delays) and one feed's
//! times are only comparable with that feed's.
//!
//! # Precondition: merged receive order
//!
//! [`clock_observe`] assumes its frames arrive in the order [`crate::merge_streams`] emits them:
//! the minimum head across the streams by receive time (every caller, the replay and the bench
//! input, goes through it). That order is what already prevents one stream from dragging the
//! clock ahead of the others: a stream whose times run ahead does not get its frames processed
//! early, they wait in the merge until every other stream with frames left has passed those
//! times, so while another stream still has frames the clock is never ahead of that stream's next
//! frame. Frames of one stream that go backwards are not reordered by the merge (file order is
//! kept), so within a stream the rules below still see regressions. Calling [`clock_observe`] in
//! an arbitrary order voids this: a feed whose frames are presented early can move the clock
//! ahead of the others without limit, one `TOL` at a time.
//!
//! # The rules
//!
//! For a frame at time `t` of feed `f` when the clock stands at `c` (`TOL` is
//! [`CLOCK_TOLERANCE_NS`], 2 minutes; `GAP` is [`FEED_GAP_MAX_NS`], 30 minutes):
//!
//! 1. **First frame.** The clock starts at `t`. Nothing is known yet that could say it is wrong;
//!    if it is, rule 4 rescues the clock once later frames disagree with it.
//! 2. **Ordinary.** The frame is within `TOL` of the feed's own last accepted frame (`own`),
//!    within `TOL` of the clock, or a lone-forward frame (rule 3). Then it is accepted: it becomes
//!    the feed's last frame, that feed's candidate step is dropped (no other feed's candidate is:
//!    a step is decided by the vote of rule 5, not by one feed's frames refuting the others),
//!    and the clock takes `t` if `t >= c`. A frame before `own` is a time regression of its
//!    feed: it is counted, the feed's last frame stays and the clock stays. A frame behind the
//!    clock but not before its own feed's last frame is a feed that runs behind the others, not
//!    a regression: it is processed at the clock and nothing is counted. For any recording whose
//!    frames are never more than `TOL` apart in each feed's own time (the measured SFDPS feed has
//!    at most 0.5 s between frames) this is the whole of the behaviour, and a clock moved by an
//!    accepted frame is never moved by more than `TOL` at once, except as rule 3 says.
//! 3. **Lone-forward.** If no other feed has ever had an accepted frame in this run (a
//!    single-feed recording), the feed has an accepted frame, and the frame is later than it by
//!    at most `GAP`, it is accepted and the clock takes it: a gap in a one-feed recording is time
//!    that passed, and nothing else could say otherwise. A larger gap stays a candidate (a pause)
//!    and is confirmed as a step.
//! 4. **Candidate.** Otherwise the frame is far from both: it is processed at the old clock,
//!    counted as held ([`Clock::held`]; a frame that is behind the clock is also counted as a
//!    regression), and becomes a candidate step of its feed. The clock does not move. A candidate
//!    continues its feed's run when it is on the same side of the clock and, behind the clock,
//!    within `TOL` of the previous candidate either way (so frames a little out of order, as two
//!    decoder threads deliver them, still count), or, ahead of it, not more than `TOL` before the
//!    previous candidate and not more than `GAP` after it (a slow feed after a pause). Any other
//!    frame restarts the run, so scattered wild frames never move the clock.
//! 5. **Confirmation.** A run of [`CLOCK_CONFIRM_FRAMES`] (3) candidates ahead of the clock is a
//!    step with no vote: in merged order a forward candidate means no other stream has a frame
//!    between the clock and its time (the merge would have emitted it first), so the other feeds
//!    are silent over the gap, not in disagreement. A run behind the clock is a step only if the
//!    other feeds do not contradict it, because feeds whose last frame is within `TOL` of the
//!    clock have actually spoken at the current time. The new time is backed by the feed itself and
//!    by every other feed with an open candidate on the same side within `TOL` of this frame
//!    (`new`); the old clock is backed by every other feed that has an accepted frame, no
//!    candidate and a last frame within `TOL` of the clock (`old`). A backward step needs
//!    `new >= old`: the tie goes to the earlier time because a clock that is ahead evicts and
//!    refuses data, and one that is behind only delays it. A step that fails the vote is held and
//!    counted in [`Clock::steps_outvoted`]; the run stays at three so the feed's next candidate
//!    asks again.
//! 6. **Step.** On a confirmed run the clock takes the third frame's time and every age the tables
//!    hold is shifted by the same amount ([`Tick::Step`]), so a step of the recorder's clock,
//!    forward or back, neither evicts every flight nor leaves them stranded in the future: the
//!    time since each flight was last heard is what it was before the step. The feeds that backed
//!    the new time keep their records (each one's last frame is its candidate or the new clock,
//!    whichever is earlier, so it is never after the clock), every other
//!    feed's last frame is forgotten, so its next frames are compared with the new clock. The
//!    eviction and checkpoint cadences are due at once, so a sweep and a checkpoint run at the new
//!    time. A real pause in a recording with no monotonic time gets the same treatment (rule 7 is
//!    how a recording with one avoids it), and flights then age from the pause rather than all
//!    being evicted at once. The pipeline marks everything last heard before
//!    the step as stale until it is heard again, because its age no longer says how long ago that
//!    was.
//!
//! 7. **Monotonic time.** A recording made with a monotonic time per frame (an `OAF2` frame, see
//!    `docs/FEED_FRAMES.md`) lets rules 3 to 6 tell time that passed from a step of the wall
//!    clock. For a frame that would be a candidate, [`clock_observe_mono`] splits the wall time
//!    elapsed since the feed's last accepted frame exactly in two: the monotonic time elapsed
//!    between the same two frames is **time that passed**, and the rest (wall elapsed minus
//!    monotonic elapsed, the **step**) is a step of the recorder's clock. A step within `TOL`
//!    is noise (a frame that waited in the recorder's queue before it was stamped, a few
//!    seconds of skew) and is ignored: then, if the frame is ahead of the clock, the gap is a
//!    **real gap**. The frame is accepted at once ([`Tick::RealGap`]), the clock takes its time
//!    and no age is shifted, so every row is silent for the whole gap and expires under its own
//!    age limit. A step over `TOL` goes through rules 4 to 6 as it always did (the run of three
//!    candidates, the vote), and when it confirms the clock takes the frame's time but the ages
//!    are shifted by the step alone ([`Tick::Step`]), no more than the clock moved: the time
//!    that passed ages every row, so a gap that also holds a step expires what it should. A
//!    pure step (the monotonic time elapsed is a second) shifts by all but that second. The
//!    two times are comparable only for two frames of the same stream (one recorder run: the
//!    monotonic time of different streams has different origins and is never compared, and a
//!    stream's reference is dropped for a feed whose record a step forgot) and a monotonic time
//!    that did not go back (one that did is a recorder restart). A frame with no monotonic
//!    time, a feed with no comparable earlier frame, or a restart is not classified: rules 4 to
//!    6 apply with the shift the whole difference of the clock, and a recording with no
//!    monotonic time at all behaves exactly as before. Each decision is counted and folded into
//!    [`Clock::domain_log`], which the state digest covers.
//!
//! # What the rules defend against, and what they do not
//!
//! They defend against a time regression inside a stream, a step of the recorder's clock (all
//! feeds jump together: each feed's run confirms, the first to reach three finds the others'
//! candidates backing it), a feed whose recorder is skewed against the majority, a stream that
//! ends early (the others keep the clock), and a single-feed recording with gaps. They cannot
//! check a feed alone: in a single-feed recording one wild forward frame less than `GAP` after
//! the previous one is accepted, because a gap and a glitch cannot be told apart with one feed.
//! Two feeds that are both sparse (gaps over `TOL` in each) are never near their own last frame or
//! the clock once both are known: the first to speak is alone while the other has had no accepted
//! frame, so the clock follows it (rule 3); then each feed's run of three forward candidates is a
//! step, so the clock moves in steps, trailing the newest frame by up to a run of frames, and the
//! tables' ages are shifted at each step, not run.
//!
//! # The tolerance
//!
//! Frames of the SFDPS feed carry up to 60 s of report time each (a flight's report may be a
//! minute older than the frame that delivers it), and the feeds' clocks can differ by a few
//! seconds; 120 s is twice the largest spread seen in that feed. It is also what bounds a forged
//! time that is accepted without confirmation: 2 minutes once, where a 10-minute allowance let a
//! single frame age out flights whose age limit is 10 minutes, and a few such frames age out
//! every flight. Frames more than `TOL` apart in a feed (a pause in a recording) are held until
//! the third, so the first two after the gap are processed at the old time, and the ages in the
//! tables carry over the gap instead of running through it.

use super::mono_split;
use crate::core::feed::FEED_COUNT;
use crate::core::table::NS_PER_S;

/// Largest difference between a frame's time and its feed's last frame, or the clock, accepted
/// without confirmation: 2 minutes (see the module documentation).
pub const CLOCK_TOLERANCE_NS: i64 = 120 * NS_PER_S;
/// Consecutive candidates that confirm a step.
pub const CLOCK_CONFIRM_FRAMES: u32 = 3;
/// Largest forward gap in a single-feed recording that is taken as time that passed (30 minutes),
/// and the largest a forward candidate may be after the previous one.
pub const FEED_GAP_MAX_NS: i64 = 30 * 60 * NS_PER_S;

/// One feed's record. Plain data (A1).
// A7.2: cold struct, size guard waived (one per feed)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedClock {
    /// Time of the feed's last accepted frame; meaningful only if `seen`.
    pub last_ns: i64,
    /// Whether the feed has an accepted frame to compare with.
    pub seen: bool,
    /// Whether the feed has had an accepted frame at any time in this run (a step clears `seen`
    /// of the other feeds, not this).
    pub ever: bool,
    /// Time of the latest candidate frame.
    pub cand_last_ns: i64,
    /// Consecutive candidates so far, at most [`CLOCK_CONFIRM_FRAMES`]; 0 when there is none.
    pub cand_n: u32,
    /// Whether the candidates are behind the clock.
    pub cand_back: bool,
    /// Whether `ref_wall_ns` and `ref_mono_ns` are the wall and monotonic time of the feed's last
    /// accepted frame (false until a frame with a monotonic time is accepted, and after one
    /// without).
    pub ref_ok: bool,
    /// Wall time of the last accepted frame that had a monotonic time.
    pub ref_wall_ns: i64,
    /// Its monotonic time.
    pub ref_mono_ns: u64,
    /// The stream it came from: monotonic times of different streams are never compared.
    pub ref_stream: u32,
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
    /// Backward runs of three candidates refused by the vote of the other feeds (each is also in
    /// `held`).
    pub steps_outvoted: u64,
    /// Gaps classified as time that passed (rule 7).
    pub real_gaps: u64,
    /// Steps confirmed that the monotonic time showed to be steps of the recorder's clock.
    pub mono_steps: u64,
    /// Steps confirmed for a frame that has a monotonic time that could not be compared (no
    /// earlier frame of the feed, or the monotonic time went back).
    pub unclassified_steps: u64,
    /// A running hash of every time-domain decision (its kind, the wall time elapsed and the
    /// monotonic time elapsed, in order); 0 while there has been none, so a recording with no
    /// monotonic time hashes as before.
    pub domain_log: u64,
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
    /// by `delta`.
    Step {
        /// The step, ns: new clock minus old clock (negative for a step back), less the
        /// monotonic time that passed in the gap if the frame had one (rule 7).
        delta: i64,
    },
    /// The frame follows a gap that the monotonic time shows was real: the clock takes the
    /// frame's time and no age is shifted (rule 7).
    RealGap,
}

/// How a frame's wall time splits against the monotonic time, against the feed's last accepted
/// frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Domain {
    /// No monotonic time, or nothing comparable to it.
    Unknown,
    /// The wall time elapsed is the monotonic time elapsed plus `step_ns`.
    Split {
        wall_ns: i64,
        mono_ns: i64,
        step_ns: i64,
    },
}

fn classify(own: &FeedClock, t: i64, stream: u32, mono: Option<u64>) -> Domain {
    match mono_split::split(
        own.ref_ok,
        own.ref_stream,
        own.ref_wall_ns,
        own.ref_mono_ns,
        t,
        stream,
        mono,
    ) {
        Some((wall_ns, mono_ns, step_ns)) => Domain::Split {
            wall_ns,
            mono_ns,
            step_ns,
        },
        None => Domain::Unknown,
    }
}

/// Folds one decision into the log (FNV-1a over the kind and the two elapsed times).
fn log_decision(c: &mut Clock, kind: u8, wall_ns: i64, mono_ns: i64) {
    let mut h = if c.domain_log == 0 {
        0xcbf2_9ce4_8422_2325
    } else {
        c.domain_log
    };
    for b in [kind]
        .into_iter()
        .chain(i64::to_le_bytes(wall_ns))
        .chain(i64::to_le_bytes(mono_ns))
    {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    // 0 is "no decision yet".
    c.domain_log = h.max(1);
}

/// Records the frame as the feed's reference for rule 7 (or clears it, for a frame with no
/// monotonic time).
fn set_ref(f: &mut FeedClock, t: i64, stream: u32, mono: Option<u64>) {
    f.ref_ok = mono.is_some();
    f.ref_wall_ns = t;
    f.ref_mono_ns = mono.unwrap_or(0);
    f.ref_stream = stream;
}

fn within(a: i64, b: i64) -> bool {
    a.saturating_sub(b).saturating_abs() <= CLOCK_TOLERANCE_NS
}

/// Whether a forward candidate at `t` continues a run whose latest candidate is at `prev`.
fn forward_continues(t: i64, prev: i64) -> bool {
    t >= prev.saturating_sub(CLOCK_TOLERANCE_NS) && t.saturating_sub(prev) <= FEED_GAP_MAX_NS
}

/// Applies the rules above for a frame of feed `feed` at `t` with the clock at `now` (`i64::MIN`
/// before the first frame). The caller moves its clock for [`Tick::Advance`] and [`Tick::Step`].
///
/// The frames must come in merged receive order ([`crate::merge_streams`]); in an arbitrary order
/// the guarantee that one feed cannot drag the clock ahead of the others is void (see the module
/// documentation).
pub fn clock_observe(c: &mut Clock, feed: usize, now: i64, t: i64) -> Tick {
    clock_observe_mono(c, feed, now, t, 0, None)
}

/// [`clock_observe`] for a frame that may carry the recorder's monotonic time (`mono`, ns since
/// the recorder started) and the stream it comes from (`stream`: monotonic times are compared
/// only within one). With `None` it is [`clock_observe`] exactly; with a time it also applies
/// rule 7.
pub fn clock_observe_mono(
    c: &mut Clock,
    feed: usize,
    now: i64,
    t: i64,
    stream: u32,
    mono: Option<u64>,
) -> Tick {
    let feed = feed.min(FEED_COUNT - 1);
    let Some(own) = c.feeds.get(feed).copied() else {
        return Tick::Held { back: t < now };
    };
    if t == i64::MIN {
        // The "no frame yet" value is never a frame time: as the clock it would make the next
        // frame of any time a first frame, accepted without confirmation.
        c.held += 1;
        return Tick::Held { back: true };
    }
    if now == i64::MIN {
        if let Some(f) = c.feeds.get_mut(feed) {
            f.last_ns = t;
            f.seen = true;
            f.ever = true;
            set_ref(f, t, stream, mono);
        }
        return Tick::Advance;
    }
    let near_clock = within(t, now);
    let near_own = own.seen && within(t, own.last_ns);
    let alone = !c.feeds.iter().enumerate().any(|(i, g)| i != feed && g.ever);
    let lone_forward =
        alone && own.seen && t > own.last_ns && t.saturating_sub(own.last_ns) <= FEED_GAP_MAX_NS;
    if near_own || near_clock || lone_forward {
        if let Some(f) = c.feeds.get_mut(feed) {
            f.cand_n = 0;
        }
        if near_own && t < own.last_ns {
            return Tick::Regress;
        }
        if let Some(f) = c.feeds.get_mut(feed) {
            f.last_ns = t;
            f.seen = true;
            f.ever = true;
            set_ref(f, t, stream, mono);
        }
        return if t >= now { Tick::Advance } else { Tick::Keep };
    }
    let back = t < now;
    let domain = classify(&own, t, stream, mono);
    if let (
        false,
        Domain::Split {
            wall_ns,
            mono_ns,
            step_ns,
        },
    ) = (back, domain)
    {
        // Rule 7: a gap whose step is within the tolerance is time that passed.
        if step_ns.saturating_abs() <= CLOCK_TOLERANCE_NS {
            if let Some(f) = c.feeds.get_mut(feed) {
                f.last_ns = t;
                f.seen = true;
                f.ever = true;
                f.cand_n = 0;
                set_ref(f, t, stream, mono);
            }
            c.real_gaps += 1;
            log_decision(c, 1, wall_ns, mono_ns);
            return Tick::RealGap;
        }
    }
    let mut ready = false;
    if let Some(f) = c.feeds.get_mut(feed) {
        let continues = f.cand_n > 0
            && f.cand_back == back
            && if back {
                within(t, f.cand_last_ns)
            } else {
                forward_continues(t, f.cand_last_ns)
            };
        if continues {
            f.cand_n = (f.cand_n + 1).min(CLOCK_CONFIRM_FRAMES);
        } else {
            f.cand_n = 1;
            f.cand_back = back;
        }
        f.cand_last_ns = t;
        ready = f.cand_n >= CLOCK_CONFIRM_FRAMES;
    }
    if ready {
        // The vote: who backs the new time, who backs the old clock.
        let backs_new =
            |g: &FeedClock| g.cand_n > 0 && g.cand_back == back && within(g.cand_last_ns, t);
        let mut new = 1u32;
        let mut old = 0u32;
        for (i, g) in c.feeds.iter().enumerate() {
            if i == feed {
                continue;
            }
            if backs_new(g) {
                new += 1;
            } else if g.seen && g.cand_n == 0 && within(g.last_ns, now) {
                old += 1;
            }
        }
        if !back || new >= old {
            // The ages shift by the whole move of the clock, less the monotonic time that passed
            // in it (never more than the move, so no age shrinks).
            let mut delta = t.saturating_sub(now);
            match domain {
                Domain::Split {
                    wall_ns,
                    mono_ns,
                    step_ns,
                } if step_ns.saturating_abs() > CLOCK_TOLERANCE_NS => {
                    c.mono_steps += 1;
                    log_decision(c, 2, wall_ns, mono_ns);
                    delta = delta.min(step_ns);
                }
                Domain::Unknown if mono.is_some() => {
                    c.unclassified_steps += 1;
                    log_decision(c, 3, 0, 0);
                }
                _ => {}
            }
            if back {
                c.backward_steps += 1;
            } else {
                c.forward_steps += 1;
            }
            // The backers keep their records; the other feeds' times were compared with the old
            // clock, they are compared with the new. A feed that did not take this frame has no
            // monotonic reference any more: it was taken before the step.
            for (i, g) in c.feeds.iter_mut().enumerate() {
                if i == feed {
                    g.last_ns = t;
                    g.seen = true;
                    g.ever = true;
                    set_ref(g, t, stream, mono);
                } else if backs_new(g) {
                    // At most the new clock, so every feed with a record is at or before it.
                    g.last_ns = g.cand_last_ns.min(t);
                    g.seen = true;
                    g.ever = true;
                    g.ref_ok = false;
                } else {
                    g.seen = false;
                    g.ref_ok = false;
                }
                g.cand_n = 0;
            }
            return Tick::Step { delta };
        }
        c.steps_outvoted += 1;
    }
    c.held += 1;
    Tick::Held { back }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    fn any_feed() -> FeedClock {
        let f = FeedClock {
            last_ns: kani::any(),
            seen: kani::any(),
            ever: kani::any(),
            cand_last_ns: kani::any(),
            cand_n: kani::any(),
            cand_back: kani::any(),
            ref_ok: kani::any(),
            ref_wall_ns: kani::any(),
            ref_mono_ns: kani::any(),
            ref_stream: kani::any(),
        };
        // Reachable: the count stops at the limit (a refused step leaves it there).
        kani::assume(f.cand_n <= CLOCK_CONFIRM_FRAMES);
        f
    }

    fn any_clock() -> Clock {
        let c = Clock {
            feeds: [any_feed(), any_feed(), any_feed(), any_feed(), any_feed()],
            held: kani::any(),
            forward_steps: kani::any(),
            backward_steps: kani::any(),
            steps_outvoted: kani::any(),
            real_gaps: kani::any(),
            mono_steps: kani::any(),
            unclassified_steps: kani::any(),
            domain_log: kani::any(),
        };
        // The counters count frames, which cannot reach 2^64 - 1.
        kani::assume(c.held < u64::MAX && c.steps_outvoted < u64::MAX);
        kani::assume(c.real_gaps < u64::MAX && c.mono_steps < u64::MAX);
        kani::assume(c.unclassified_steps < u64::MAX);
        kani::assume(c.forward_steps < u64::MAX && c.backward_steps < u64::MAX);
        c
    }

    /// The clock state the pipeline keeps: every feed with an accepted frame has its last frame
    /// at or before the clock (`now`), and before the first frame no feed has one.
    fn consistent(c: &Clock, now: i64) -> bool {
        c.feeds
            .iter()
            .all(|f| !f.seen || (now != i64::MIN && f.last_ns <= now))
    }

    /// Whether some feed other than `own` has ever had an accepted frame.
    fn others_ever(c: &Clock, own: usize) -> bool {
        (0..FEED_COUNT).any(|i| i != own && c.feeds[i].ever)
    }

    // REQ: OA-RPL-100
    #[kani::proof]
    fn clock_observe_total_and_confirms() {
        let mut c = any_clock();
        let before = c;
        let feed: usize = kani::any();
        let (now, t): (i64, i64) = (kani::any(), kani::any());
        let tick = clock_observe(&mut c, feed, now, t);
        let own = feed.min(FEED_COUNT - 1);
        // Every candidate count stays at or below the confirmation limit.
        assert!(c.feeds.iter().all(|f| f.cand_n <= CLOCK_CONFIRM_FRAMES));
        if let Tick::Step { delta } = tick {
            // A step needs CLOCK_CONFIRM_FRAMES consecutive candidates of one feed: this frame
            // is the last of them, so that feed had at least CLOCK_CONFIRM_FRAMES - 1 before it.
            assert!(now != i64::MIN);
            assert!(before.feeds[own].cand_n >= CLOCK_CONFIRM_FRAMES - 1);
            assert_eq!(delta, t.saturating_sub(now));
            assert!(c.feeds.iter().all(|f| f.cand_n == 0));
        }
        if before.feeds[own].cand_n == 0 {
            assert!(!matches!(tick, Tick::Step { .. }));
        }
        // Only this frame's feed gains candidates; the others' counts never grow.
        for i in 0..FEED_COUNT {
            if i != own {
                assert!(c.feeds[i].cand_n <= before.feeds[i].cand_n);
            }
        }
    }

    // REQ: OA-RPL-101
    #[kani::proof]
    fn clock_advances_by_at_most_the_tolerance_unless_the_feed_is_alone() {
        let mut c = any_clock();
        let feed: usize = kani::any();
        let (now, t): (i64, i64) = (kani::any(), kani::any());
        kani::assume(consistent(&c, now));
        let own = feed.min(FEED_COUNT - 1);
        let alone = !others_ever(&c, own);
        let tick = clock_observe(&mut c, feed, now, t);
        // An accepted frame moves a running clock forward by at most the tolerance, or by at most
        // the gap limit when no other feed has ever had an accepted frame.
        if now != i64::MIN && tick == Tick::Advance {
            assert!(t >= now);
            let by = i128::from(t) - i128::from(now);
            let limit = if alone {
                FEED_GAP_MAX_NS
            } else {
                CLOCK_TOLERANCE_NS
            };
            assert!(by <= i128::from(limit));
            // A frame that moved a lone feed by more than the tolerance was lone-forward.
            kani::cover!(alone && by > i128::from(CLOCK_TOLERANCE_NS));
        }
        // The state stays consistent with the clock the caller now holds.
        let next = match tick {
            Tick::Advance | Tick::Step { .. } => t,
            _ => now,
        };
        assert!(consistent(&c, next));
    }

    // REQ: OA-RPL-188
    #[kani::proof]
    fn clock_step_needs_three_candidates_and_the_vote() {
        let mut c = any_clock();
        let before = c;
        let feed: usize = kani::any();
        kani::assume(feed < FEED_COUNT);
        let now: i64 = kani::any();
        kani::assume(now != i64::MIN);
        let t: i64 = kani::any();
        let tick = clock_observe(&mut c, feed, now, t);
        let back = t < now;
        // The vote as the rules state it, from the state before the frame.
        let mut new = 1u32;
        let mut old = 0u32;
        for i in 0..FEED_COUNT {
            if i == feed {
                continue;
            }
            let g = before.feeds[i];
            if g.cand_n > 0 && g.cand_back == back && within(g.cand_last_ns, t) {
                new += 1;
            } else if g.seen && g.cand_n == 0 && within(g.last_ns, now) {
                old += 1;
            }
        }
        if let Tick::Step { .. } = tick {
            assert!(before.feeds[feed].cand_n >= CLOCK_CONFIRM_FRAMES - 1);
            if back {
                // A backward step is voted: ties go to the earlier time.
                assert!(new >= old);
                assert_eq!(c.backward_steps, before.backward_steps + 1);
            } else {
                // A forward step needs the candidates and nothing else.
                assert_eq!(c.forward_steps, before.forward_steps + 1);
            }
        }
        // A refused step is a held backward frame, counted, with the run kept at the limit and
        // the vote against it; no forward step is ever refused.
        if c.steps_outvoted > before.steps_outvoted {
            assert!(matches!(tick, Tick::Held { back: true }));
            assert!(back);
            assert_eq!(c.feeds[feed].cand_n, CLOCK_CONFIRM_FRAMES);
            assert!(new < old);
        }
        if !back && before.feeds[feed].cand_n >= CLOCK_CONFIRM_FRAMES - 1 {
            // Three forward candidates in a row are a step, whoever else has spoken.
            let g = before.feeds[feed];
            let continues = g.cand_n > 0 && !g.cand_back && forward_continues(t, g.cand_last_ns);
            let lone = !others_ever(&before, feed) && g.seen && t > g.last_ns;
            let accepted = within(t, now) || (g.seen && within(t, g.last_ns)) || lone;
            if continues && !accepted && t != i64::MIN {
                assert!(matches!(tick, Tick::Step { .. }));
            }
        }
        kani::cover!(matches!(tick, Tick::Step { .. }) && !back);
        kani::cover!(matches!(tick, Tick::Step { .. }) && back);
        kani::cover!(c.steps_outvoted > before.steps_outvoted);
    }

    // REQ: OA-RPL-100
    #[kani::proof]
    fn clock_observe_three_candidates_before_a_step() {
        let mut c = any_clock();
        let feed: usize = kani::any();
        kani::assume(feed < FEED_COUNT);
        kani::assume(c.feeds[feed].cand_n == 0);
        // Room for three more counts.
        kani::assume(c.held < u64::MAX - 3);
        let now: i64 = kani::any();
        kani::assume(now != i64::MIN);
        let (t1, t2, t3): (i64, i64, i64) = (kani::any(), kani::any(), kani::any());
        // The clock does not move between candidates (a refused frame leaves it alone).
        let a = clock_observe(&mut c, feed, now, t1);
        let b = clock_observe(&mut c, feed, now, t2);
        let was_step = matches!(a, Tick::Step { .. }) || matches!(b, Tick::Step { .. });
        assert!(!was_step);
        let d = clock_observe(&mut c, feed, now, t3);
        if let Tick::Step { .. } = d {
            // Three frames of one feed, all far from the clock, on one side, in time order
            // (behind the clock within the tolerance of each other; ahead, not going back by more
            // than the tolerance or forward by more than the gap limit).
            assert!(matches!(a, Tick::Held { .. }) && matches!(b, Tick::Held { .. }));
            assert_eq!(t1 < now, t3 < now);
            if t3 < now {
                assert!(within(t1, t2) && within(t2, t3));
            } else {
                assert!(forward_continues(t2, t1) && forward_continues(t3, t2));
            }
        }
    }
}
