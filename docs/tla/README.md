# ReplayClock: a TLA+ model of the replay clock

`ReplayClock.tla` models how replay decides the current time from the feeds' frame times:
`clock_observe` in `crates/oa-replay/src/core/clock.rs`, fed in the order `merge_streams` in
`crates/oa-replay/src/shell/source.rs` delivers frames. TLC checks the model exhaustively within
small bounds. The result is that, with merged delivery, the clock never leaves a feed
behind, never freezes when a feed resumes and always finishes. With arbitrary delivery it
does none of those, which is why replay depends on the merge.

## The rules modelled

* The first frame is accepted. A frame whose time is the "no clock" value is held.
* A frame is accepted when it is within TOL of its own feed's last accepted frame, or within
  TOL of the clock. It cancels its own feed's pending candidate; a frame within TOL of the
  clock cancels every feed's candidate. An accepted frame before its feed's last is a
  regression and leaves the clock alone. One at or after the clock moves the clock to it.
* A feed that is the only one ever accepted may move forward by up to GAP_MAX from its own
  last frame, and the clock follows it.
* Any other frame is a candidate and is held. A backward candidate continues a run if it is
  within TOL of the previous candidate. A forward one continues a run if it is no more than
  TOL before the previous candidate and no more than GAP_MAX after it.
* Forward: CONFIRM candidates in a run step the clock, with no vote.
* Backward: CONFIRM candidates step the clock only if the feeds backing the new time are at
  least as many as those backing the old one; a tie keeps the earlier time. The new time's
  backers are this feed plus every other feed with an open candidate in the same direction,
  within TOL of the frame. The old time's backers are the feeds that have been seen, have no
  open candidate and whose last frame is within TOL of the clock.
* On a step, each backer's last frame becomes the earlier of its candidate and the new time,
  all candidates are dropped, and the other feeds start over.

Where the code leaves a choice, the model takes these readings:
* A run that reaches CONFIRM but loses the vote stays open with its count held at CONFIRM,
  and its next frame asks again.
* A feed that backs a step counts as having had an accepted frame.
* A regression leaves the feed's last frame unchanged. A feed's last frame is never after the
  clock, so a frame before it that is within TOL of the clock is also within TOL of the last
  frame, and the two readings agree.

## Bounds

The constants are small integers: TOL = 2, GAP_MAX = 8, CONFIRM = 3. Times run 0..20, or
0..16 with three feeds.

TLC picks every combination of streams with up to 5 frames each:
* steps of 1 or 2;
* at most one jump per stream: a regression of 3, or a pause of 4 or 9 (9 is past GAP_MAX);
* feed 0 starts at 3 and feed 1 anywhere in 0..6, since only the offset between them matters.

The code uses TOL = 120 s, CONFIRM = 3 and GAP_MAX = 30 min, in nanoseconds. The model keeps
only the order between TOL, GAP_MAX and the gap sizes, and the chosen jumps sample that
order; they do not cover it.

The constant `Merged` selects how frames are delivered:
* **A (`Merged = TRUE`)**, as replay does it. The next frame is the smallest head among the
  streams, ties going to the lower feed index.
* **B (`Merged = FALSE`)**. Any feed may deliver its next frame at any time, each stream
  still in its own order.

## Properties

* `TypeInv`:
  * every variable stays in range;
  * a feed's last frame is never after the clock;
  * a candidate count never passes CONFIRM, and only a backward count stays at CONFIRM.
* `Behind`: no stream that still has frames is more than TOL behind the clock, unless the
  stream itself went backwards somewhere up to and including its next frame.
* `Termination`, plus TLC's deadlock check: every run ends with every stream consumed.
* `NoFreeze`: a feed resumes after a pause longer than TOL, and its next CONFIRM − 1 frames
  each follow within TOL. Once the CONFIRM-th of those frames has arrived, counting the
  resuming frame, the clock is at or past the resuming frame's time. This is required only if
  no stream has gone backwards so far.
  * `NeverResumed` must fail. It shows that this case is actually reached, so `NoFreeze` is
    not true vacuously.
* `NoPingPong`: the number of clock steps is at most the number of jumps inside the streams
  (regressions, plus pauses longer than TOL). `NoPingPong2` allows twice that.
* `NoPingPongMerged`: the number of clock steps is at most the number of deliveries more than
  TOL away from the delivery before them, in delivery order.

## Results

TLC 2.19 (tla2tools v1.7.4, recorded in `openatca-data/tools/tla/SOURCE.md`), Java 21, two
workers, 3 GB heap. The machine was heavily loaded, so the times are slow.

| Configuration | Result | States generated / distinct | Time |
|---|---|---|---|
| `ReplayClock.cfg`: A, 2 feeds; TypeInv, Behind, NoFreeze, Termination | all hold | 3,215,180 / 2,075,117, depth 11 | 13 min 27 s |
| `ReplayClock3.cfg`: A, 3 feeds, only the middle one may jump | all hold | 2,311,704 / 1,407,555, depth 13 | 9 min 7 s |
| `ReplayClockScenario.cfg`: A, one recording ends while another resumes | all hold | 130 / 80 | 8 s |
| `ReplayClockNone.cfg`: A, a frame at the "no clock" value | TypeInv holds | 4,032 / 2,583 | 8 s |
| `ReplayClock_NoPingPongMerged.cfg`: A | holds | 3,215,180 / 2,075,117 | 3 min |
| `ReplayClock_NoPingPong.cfg`: A | **fails** | 1,037,229 / 835,259 at the failure | 1 min 50 s |
| `ReplayClock_NoPingPong2.cfg`: A | **fails** | 1,037,228 / 835,258 at the failure | 1 min 46 s |
| `ReplayClock_B_Behind.cfg`: B | **fails** | 16,049 / 16,046 at the failure | 8 s |
| `ReplayClock_B_NoFreeze.cfg`: B | **fails** | 888,197 / 381,190 at the failure | 55 s |
| `ReplayClock_B_NoPingPong.cfg`: B | **fails** | 500,286 / 233,116 at the failure | 40 s |
| `ReplayClock_B_NoPingPongMerged.cfg`: B | holds; meaningless in B, see below | 3,641,866 / 1,218,082 | 2 min |

To run one, go to this directory and run:

```
java -Xmx3g -XX:+UseParallelGC -cp /path/to/tla2tools.jar tlc2.TLC -workers 2 -config ReplayClock.cfg ReplayClock.tla
```

### One recording ends while another resumes

`ScenarioChoices` fixes the streams for a case that froze an earlier version of the rules,
in which forward steps were voted on. In that case one feed's recording ends, for example
`<3,4,5>`, while the other resumes after a pause. The resuming stream is one of these:
* `<3,4,5,6,14,15,16,17>`, a gap of 8;
* `<3,4,5,6,18,19,20>`, a gap of 12, past GAP_MAX;
* a pause in the middle while the first feed is still running.

Under the current rules the resuming feed's third candidate steps the clock forward, so the
clock does not freeze. `NoFreeze` holds in every variant, and `NeverResumed` fails, as it
should.

### Why NoPingPong fails in A, and why that is correct

The counterexample has no jump inside either stream. Feed 0 is `<3>` and feed 1 is
`<6,7,8>`, so frames arrive in the order 3, 6, 7, 8:
* 6 and 7 are candidates for feed 1, which has no accepted frame and is more than TOL from
  the clock at 3;
* 8 is the third candidate, and the clock steps forward by 5.

The jump is between the streams: a feed that starts more than TOL from the others. Following
such a gap with one step is what the rules are for, so the per-stream bound is simply too
tight, even doubled. `NoPingPongMerged`, which also counts these gaps between feeds, holds.
TLC found no rule that misbehaves in A.

### Why replay needs merged order

In B, three of the properties fail:
* **`Behind`.** Feed 0 delivers its 3 first and the clock is 3, while feed 1's next frame is
  0. Feed 1 is three behind without having gone backwards itself.
* **`NoFreeze`.** Feed 0 is `<3>` and feed 1 is `<2,11,12,13>`.
  * Feed 1 delivers 2, then 11, which becomes a candidate.
  * Feed 0's 3 arrives late. It is within TOL of the clock, so it cancels feed 1's candidate.
  * 12 and 13 make only two candidates, and the clock stays at 3 although feed 1 has resumed.
  * Merged order would have delivered the 3 before the 11.
* **`NoPingPong`.** It fails here as in A, and also through backward steps: a feed running
  ahead makes the next feed's frames look like a step back, with no jump in any stream.

`NoPingPongMerged` holds in B only because arbitrary delivery itself jumps around, which
makes its bound large. It says nothing about B.

## Limits

The checks cover:
* two feeds, or three in one configuration;
* at most one jump per stream;
* five frames per stream;
* the small constants above.

They do not cover streams with several jumps, a regression smaller than TOL, longer streams,
or three feeds that all jump.

The model follows the rules in `core/clock.rs`; no tool checks that the two stay in step.

## RecoveryProtocol: checkpoint recovery and stale-publication fence

`RecoveryProtocol.tla` is a bounded protocol model for the checkpoint recovery and stale
publication rejection named in `docs/NODE_NETWORK.md` section 11. It models two replicas and an
external generation value. Only an explicit `AdvanceAuthority` action can raise that value; the
model has no election, quorum, transfer protocol, or way to infer authority from peer messages.

A publication carries a peer identity, generation, replay watermark, and abstract checkpoint
metadata. The `CheckpointVerified` predicate stands for the combined result of peer identity,
authentication, and binding the checkpoint's generation and watermark to the publication. It is
not a signature, parser, hash, Merkle, or on-disk format model. The network can hold a bounded
message, delay delivery, duplicate it into another slot, or deliver it after an authority change
or receiver restart.
`NetworkOffer` injects valid and invalid offers so the model can check refusal behavior. A crash
clears runtime serving state but preserves durable and high-water values; restart serves only a
verified durable checkpoint for the currently authorized generation.

The finite configuration uses generations 1..2, watermarks 1..2, two nodes, and two message
slots. It checks:

* `NoRegression`: accepted generation and watermark high-water marks never decrease.
* `NoUnverifiedServing`: an unverified or mismatched checkpoint cannot become serving state.
* `NoStaleServing`: serving state uses the currently authorized generation, including after
  crash/restart.
* `OutputAtHighWater` and `DurableAtHighWater`: serving and durable state equal the node's
  retained accepted high-water marks; replayed duplicates cannot replace them.
* `TypeOK`: variables stay within the finite model bounds.

Run the intended fence configuration from this directory with:

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config RecoveryProtocol.cfg RecoveryProtocol.tla
```

`RecoveryProtocol_WeakFence.cfg` sets `RequireVerified = FALSE`. It is an intentionally weakened
control: the `NoUnverifiedServing` invariant should find a counterexample when an invalid network
offer is accepted. This control is included to check that the verification invariant is
non-vacuous; it is not a model of an implementation option. Run it with:

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config RecoveryProtocol_WeakFence.cfg RecoveryProtocol.tla
```

TLC 2.19 with Java 21, two workers and a 1 GB heap checked the two configurations.

| Configuration | Result | States generated / distinct | Time |
|---|---|---|---|
| `RecoveryProtocol.cfg` | All invariants and `NoRegression` hold, depth 10 | 3,024,309 / 574,600 | 36 s |
| `RecoveryProtocol_WeakFence.cfg` | `NoUnverifiedServing` fails in a three-state trace | 603 / 591 at failure | <1 s |

The weak trace offers an unverified checkpoint and admits it at the next receive step. In the
strong model, an external authority advance revokes serving in the same abstract transition, and
acceptance stores the durable state atomically with publication. The check is bounded to the states
and transitions above. Dissemination of authority changes, real storage durability, cryptographic
verification, and operational availability require separate evidence.

## StagedRollout: bounded candidate qualification and rollback

`StagedRollout.tla` models one candidate release across two replicas. The primary keeps the prior
trusted release while the candidate is installed only on the shadow. Two concordant comparison
steps are abstract independent evidence; `Qualify` and `Promote` are separate actions so a primary
failure can occur after the last comparison but before qualification or promotion. A candidate
failure or evidence disagreement quarantines the candidate and restores service to the prior
release only when the primary remains healthy. If the primary fails during probation, the model
allows an explicit unavailable state and cannot qualify the candidate without the trusted
comparison peer. A later evidence revocation removes a promoted candidate from service; the prior
release resumes only if it is healthy.

The finite bound is one rollout, two replicas, one prior release, one candidate and two required
comparison steps. `StagedRollout.cfg` checks type, untouched fallback, qualification-before-serving,
quarantine fallback, single-serving, healthy-serving and explicit-unavailability invariants.
Both configurations allow terminal faults as stuttering states. Its action graph contains
the positive path `InstallCandidate`, two `GoodComparison` actions, `Qualify`, then `Promote`, as
well as both orderings of primary failure and qualification and candidate/evidence failure before
or after promotion. No fairness or measured-liveness claim is made.

Run the strong model from this directory with:

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config StagedRollout.cfg StagedRollout.tla
```

`StagedRollout_WeakPromotion.cfg` is the non-vacuity control. It permits promotion before the
qualification threshold while checking the same invariants; TLC is expected to produce a short
counterexample to `NoUnqualifiedCandidateServing` (install, then premature promote):

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config StagedRollout_WeakPromotion.cfg StagedRollout.tla
```

TLC 2.19 with Java 21 and two workers checked both configurations. The strong model generated 45
states, found 24 distinct states at depth 7, and satisfied all invariants. The weak model produced
a three-state counterexample. It installed the candidate and promoted it with zero comparisons.

The model takes comparison results as abstract inputs and applies a single rollout policy.
Release signing, comparison, deployment, health monitoring and operational rollback require
implementation and separate evidence. When the primary fails before qualification, the model
marks service unavailable. M6's availability goal requires measured performance under faults.

## SinkCutoverFence: durable output-sink cutover

`SinkCutoverFence.tla` models one output sink and one externally supplied generation change from
1 to 2. The strong protocol requires the authority to receive acknowledgement that the sink
stopped old output and persisted its minimum generation before granting generation 2.
A lost or delayed acknowledgement can leave the sink unavailable and the transfer pending.

Output admission uses only the sink's local durable minimum, locally delivered durable grant, and
matching local evidence. `NoStaleOutput` compares each admitted output against the external
generation at the instant of emission. Restart reconstructs service from the durable minimum,
grant and matching evidence. A stale queued output is rejected after the fence, including after
delivery of the replacement grant.

The model holds at most one queued output. It checks `NoGrantBeforeFence`,
`ServingHasLocalEvidence`, `NoStaleOutput` and `TypeOK`. The strong state graph reaches an admitted
generation-2 output. An unreachable sink can block the grant, so a runtime that bypasses the wait
would need independent proof of isolation.

Run the intended model from this directory with:

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config SinkCutoverFence.cfg SinkCutoverFence.tla
```

`SinkCutoverFence_WeakGrant.cfg` removes the acknowledgement prerequisite. In its counterexample,
the external authority grants generation 2 before fencing, and the sink then queues and emits
generation 1 using its still-current local evidence.

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config SinkCutoverFence_WeakGrant.cfg SinkCutoverFence.tla
```

TLC 2.19 with Java 21, two workers and a 1 GB heap ran both configurations.

| Configuration | Result | States generated / distinct |
|---|---|---|
| `SinkCutoverFence.cfg` | All invariants hold, complete depth 11 | 345 / 102 |
| `SinkCutoverFence_WeakGrant.cfg` | `NoStaleOutput` fails in a five-state trace | 103 / 52 at failure |

The model assumes durable fence state survives restart and the external authority observes the
acknowledgement rule. It covers publication at one sink. Storage faults, acknowledgement
authentication, multiple sinks, and fairness need separate checks.

## ApplyAckRecovery: lost acknowledgement and retry after worker restart

`ApplyAckRecovery.tla` models one sender-local key (generation, source, sequence), two worker
incarnations, one good digest, one conflicting digest, one initial application and at most one
retry. The sender does not receive the first acknowledgement. Worker 1 crashes after application,
worker 2 restarts, and the sender retries the same key and digest. The abstract downstream effect
count is bounded to 0..2 and attempt count to 0..1. The strong model's fair `AcceptedRetryPath`
property requires acceptance of the retry, delivery of its acknowledgement, and a single total
effect.

With `AtomicCommit = TRUE`, applying the effect and recording its idempotency decision are one
transition. A same-key, same-digest retry is accepted without another effect; a same-key,
different-digest probe is refused. The strong config checks at-most-once effect, effect/decision
coupling, identity binding, collision refusal, and the fair positive path to an accepted retry.
`AtomicCommit = FALSE` is a deliberately weak control: the effect and durable decision are separate
transitions. A crash can occur after the first effect and lost acknowledgement but before the
decision. The replacement can then accept the retry and produce a second effect.

TLC 2.19 with Java 21, two workers, and a 1 GB heap checked both configurations. The atomic
configuration passed its invariants and the fair retry property at 13 generated states and
11 distinct states, with depth 9. Its state graph reached both a delivered retry acknowledgement
and a refused same-key digest collision. The split configuration violated `AtMostOnce` at
15 generated states and 14 distinct states. The seven-state counterexample applied the first
effect, lost the acknowledgement, crashed before recording the durable decision, restarted, and
applied a second effect for the same key.

Run the strong model from this directory with:

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config ApplyAckRecovery.cfg ApplyAckRecovery.tla
```

Run the intentionally weak control with:

```sh
java -cp /path/to/tla2tools.jar tlc2.TLC -config ApplyAckRecovery_WeakCommit.cfg ApplyAckRecovery.tla
```

This finite model assumes an atomic durable idempotency record plus downstream effect in the strong
variant. It does not establish that a real storage system can provide that atomicity, that the
effect can be rolled back or queried, or that an ACK is authenticated. Digest strings stand for
content identities; no hash or wire format is implemented. The model has no retries beyond the
single retry, no generation authority changes, clock, network service, external publication,
operational policy, or measured liveness claim.
