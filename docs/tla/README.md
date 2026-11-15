# ReplayClock: a TLA+ model of the replay clock rules

`ReplayClock.tla` models the replay clock of `crates/oa-replay/src/clock.rs`
(`clock_observe`) together with the merge order of `crates/oa-replay/src/source.rs`
(`merge_streams`). It models the rules as decided for the next version of the clock, not
necessarily the text of `clock.rs` at the commit this file was written on:

* the first frame is accepted, and the "no clock" value as a frame time is held;
* a frame is accepted if it is within TOL of its own feed's last accepted frame or within TOL of
  the clock, and refutes only its own feed's candidate (all feeds' candidates if it is within TOL
  of the clock); accepted and before its own last frame is a regression (the clock stays),
  accepted and at or after the clock moves the clock, otherwise the clock keeps;
* lone-forward: if no other feed has ever had an accepted frame, a forward frame within GAP_MAX
  of its own last is accepted and the clock takes it;
* otherwise the frame is a candidate (held). A backward candidate continues a run if it is within
  TOL of the previous candidate, a forward one if it is at least the previous minus TOL and within
  GAP_MAX of it;
* a forward step needs CONFIRM consecutive candidates and no vote; a backward step needs CONFIRM
  candidates and backers_new >= backers_old (backers_new is 1 plus the other feeds with an open
  candidate in the same direction within TOL of the frame; backers_old is the other feeds that are
  seen, have no open candidate and whose last frame is within TOL of the clock). On a step the
  backers keep last = min(their candidate time, t), the other feeds are forgotten.

Decisions where the rule text left room: a candidate run that has reached CONFIRM but is refused
by the vote stays open at CONFIRM (the count saturates), and the next frame in the run asks again;
every candidate is dropped on a step; a feed that backs a step counts as having had an accepted
frame; a regression leaves the feed's last frame unchanged (under the invariant that a feed's last
frame is never after the clock, a frame before its last that is within TOL of the clock is also
within TOL of that last frame, so the two readings agree).

The numbers are small-integer stand-ins: TOL = 2, GAP_MAX = 8, CONFIRM = 3, times 0..20
(0..16 with three feeds). Streams are chosen nondeterministically at the start: a start time, steps
of 1 or 2, and at most one jump per stream (a regression of 3, or a pause of 4 or 9; 9 is beyond
GAP_MAX), up to 5 frames. Only the offset between the feeds' start times matters, so feed 0 starts
at 3 and feed 1 at 0..6.

Two environments, one constant `Merged`:

* **A (Merged = TRUE)**: the next frame delivered is the smallest head among the streams, ties by
  feed index, each stream in its own order.
* **B (Merged = FALSE)**: any feed delivers its next frame at any time (each stream still in its
  own order), so the interleaving is arbitrary.

## Properties

* `TypeInv`: every variable stays in its range (no overflow), a feed's last frame is never after
  the clock, a candidate count never passes CONFIRM and a saturated one is backward.
* `Behind` (property 2): no stream that still has frames and has not regressed within itself (a
  decreasing step among its frames up to and including its head) has its head more than TOL behind
  the clock.
* `Termination` (property 3a) and the absence of deadlock: TLC's deadlock check stays on, and the
  run ends with every stream consumed.
* `NoFreeze` (3b): when a feed resumes after a pause (a step of its own stream larger than TOL) and
  its next CONFIRM-1 frames each follow within TOL, then right after the CONFIRM-th of those frames
  (the resuming frame counted) the clock is at or past the resuming frame's time, provided no stream
  delivered a regression so far. `NeverResumed` exists only to show that this antecedent is
  reached (it is expected to fail).
* `NoPingPong` (property 4, as asked): steps <= number of within-stream jumps (a regression or a
  pause larger than TOL). `NoPingPong2` is twice that bound. `NoPingPongMerged` counts instead the
  deliveries that are more than TOL from the delivery before them in merge order.

## Configurations and results

TLC 2.19 (tla2tools v1.7.4, see `openatca-data/tools/tla/SOURCE.md`), Java 21, two workers, 3 GB,
on a shared machine with load around 40, so the times are slow.

| Configuration | Result | States generated / distinct | Time |
|---|---|---|---|
| `ReplayClock.cfg` (A, 2 feeds: TypeInv, Behind, NoFreeze, Termination) | all hold | 3,215,180 / 2,075,117, depth 11 | 13 min 27 s |
| `ReplayClock3.cfg` (A, 3 feeds, the middle feed may jump) | all hold | 2,311,704 / 1,407,555, depth 13 | 9 min 7 s |
| `ReplayClockScenario.cfg` (A, the earlier deadlock, see below) | all hold | 130 / 80 | 8 s |
| `ReplayClockNone.cfg` (A, a frame at the "no clock" value) | TypeInv holds | 4,032 / 2,583 | 8 s |
| `ReplayClock_NoPingPongMerged.cfg` (A) | holds | 3,215,180 / 2,075,117 | 3 min |
| `ReplayClock_NoPingPong.cfg` (A) | **fails** | 1,037,229 / 835,259 at the failure | 1 min 50 s |
| `ReplayClock_NoPingPong2.cfg` (A) | **fails** | 1,037,228 / 835,258 at the failure | 1 min 46 s |
| `ReplayClock_B_Behind.cfg` (B) | **fails** | 16,049 / 16,046 at the failure | 8 s |
| `ReplayClock_B_NoFreeze.cfg` (B) | **fails** | 888,197 / 381,190 at the failure | 55 s |
| `ReplayClock_B_NoPingPong.cfg` (B) | **fails** | 500,286 / 233,116 at the failure | 40 s |
| `ReplayClock_B_NoPingPongMerged.cfg` (B) | holds (but see below) | 3,641,866 / 1,218,082 | 2 min |

Run one with `java -Xmx3g -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC -workers 2 -config
ReplayClock.cfg ReplayClock.tla` from this directory.

### The earlier deadlock

`ScenarioChoices` pins the case found earlier: one feed's recording ends (for example
`<3,4,5>`) while the other resumes after a pause (`<3,4,5,6,14,15,16,17>`, a gap of 8, or
`<3,4,5,6,18,19,20>`, a gap of 12 beyond GAP_MAX, or a pause in the middle while the first feed is
still running). With the new rules the resuming feed's third candidate steps the clock forward
with no vote, so the clock is not frozen; `NoFreeze` holds in all of them, and `NeverResumed` fails
there as intended, so the check is not vacuous.

### Property 4 as stated fails in A: an offset between feeds is a step

`NoPingPong` (steps bounded by within-stream jumps) fails in the merged environment with no
within-stream jump at all. Trace: feed 0 is `<3>`, feed 1 is `<6,7,8>`. Merge order is 3, 6, 7, 8.
The 6 and 7 are candidates of feed 1 (it has no last frame, and 6 is more than TOL from the clock
at 3); the 8 confirms and the clock steps forward by 5, one forward step. Nothing in either stream
jumps; the jump is between the streams (a feed that starts, or whose other feed's recording ended,
more than TOL away). This is the intended behaviour (the rule text says a step is how the clock
follows such a gap) and not a modelling error, but the bound "steps <= within-stream jumps" is too
tight: a feed that starts late costs a step. The tight statement that does hold in A is
`NoPingPongMerged`: steps <= the number of deliveries that are more than TOL from the delivery
before them in merge order (which counts feed-start offsets and resumptions as well as pauses and
regressions). No rule gap was found in A.

### Why the code requires merged order

In B, `Behind`, `NoFreeze` and `NoPingPong` all fail (counterexamples are expected there):

* `Behind` fails at once: feed 0 `<3>` delivers first and the clock is 3, while feed 1's head is
  0, three behind and not a regression of feed 1. Without merge order a feed can be left behind.
* `NoFreeze` fails: feed 0 is `<3>`, feed 1 is `<2,11,12,13>`. Feed 1 delivers 2 and 11 (a
  candidate), then feed 0's late 3 arrives, is within TOL of the clock and refutes feed 1's
  candidate; 12 and 13 then give only two candidates and the clock stays at 3 although feed 1 has
  resumed. In merge order feed 0's 3 would have come before feed 1's 11.
* `NoPingPong` fails as in A, and worse: in B a feed far behind the other feeds produces
  backward steps with no jump in any stream (a feed running ahead of the others makes the next
  feed's frames look like a backward step).
* `NoPingPongMerged` holds in B too, but only because in B the deliveries themselves jump around,
  so the count is large; it says nothing about merge order and is not evidence for B.

## Limits

The checks are bounded: two feeds (three in one configuration), at most one jump per stream, five
frames, the small integer constants above. Streams with several jumps, a regression inside TOL,
a longer run or three jumping feeds are not covered. Real times are in nanoseconds with
TOL = 120 s; the model assumes only the order relations among TOL, GAP_MAX and the gaps, and these
are only sampled by the chosen jump sizes (-3, 4, 9, with steps of 1 or 2).
