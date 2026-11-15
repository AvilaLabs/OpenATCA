---------------------------- MODULE ReplayClock ----------------------------
(***************************************************************************)
(* The OpenATCA replay clock rules (crates/oa-replay/src/clock.rs,          *)
(* clock_observe), in the form decided for the per-feed candidate rules     *)
(* with backer voting and the lone-forward rule, together with the merge    *)
(* order that feeds it (source.rs, merge_streams).                          *)
(*                                                                          *)
(* Times are small integers.  NONE (-1) is the "no clock yet" value.  The   *)
(* environment is a set of finite per-feed streams, chosen                 *)
(* nondeterministically at the start.  With Merged = TRUE the next frame    *)
(* delivered is the smallest head among the streams (ties by feed index);   *)
(* with Merged = FALSE any feed may deliver its next frame at any time      *)
(* (each stream still in its own order).                                    *)
(***************************************************************************)
EXTENDS Naturals, Integers, Sequences, FiniteSets, TLC

CONSTANTS
    NF,         \* number of feeds, feeds are 0..NF-1
    TOL,        \* tolerance
    GAP_MAX,    \* largest forward gap a candidate run or the lone-forward rule allows
    CONFIRM,    \* consecutive candidates that confirm a step
    MaxT,       \* times are in 0..MaxT (NONE below that)
    Merged,     \* TRUE: merge order; FALSE: arbitrary interleaving
    Choices     \* Choices[f]: the set of streams feed f may have

NONE == -1
Feeds == 0..(NF - 1)

Min(a, b) == IF a < b THEN a ELSE b
Within(a, b) == a - b <= TOL /\ b - a <= TOL

(***************************************************************************)
(* Stream generators: a start time, then deltas; normal deltas (within      *)
(* TOL, "mostly increasing") and at most mj jump deltas (pauses forward,    *)
(* regressions back).  Every time stays in lo..MaxT.                        *)
(***************************************************************************)
RECURSIVE SumTo(_, _)
SumTo(ds, k) == IF k = 0 THEN 0 ELSE SumTo(ds, k - 1) + ds[k]

Gen(lo, starts, maxLen, normD, jumpD, mj) ==
    UNION { { st \in { [i \in 1..n |-> s + SumTo(ds, i - 1)] :
                        ds \in { d \in [1..(n - 1) -> normD \cup jumpD] :
                                   Cardinality({ i \in DOMAIN d : d[i] \in jumpD }) <= mj } } :
                \A i \in 1..n : st[i] >= lo /\ st[i] <= MaxT }
            : s \in starts, n \in 1..maxLen }

Jumps(s) == Cardinality({ i \in 1..(Len(s) - 1) : s[i + 1] - s[i] > TOL \/ s[i + 1] < s[i] })

VARIABLES
    clock,      \* the clock, NONE before the first frame
    seen,       \* seen[f]: feed f has a last accepted frame to compare with
    last,       \* last[f]: time of that frame
    cn,         \* cn[f]: consecutive candidates of f (0: none; saturates at CONFIRM)
    cl,         \* cl[f]: time of f's latest candidate
    cb,         \* cb[f]: f's candidates are behind the clock
    ever,       \* ever[f]: feed f has ever had an accepted frame
    stream,     \* stream[f]: the recorded frame times of feed f (fixed at the start)
    pos,        \* pos[f]: frames of f delivered so far
    lastDel,    \* feed of the latest delivery (-1 before any)
    prevT,      \* time of the latest delivered frame of any feed (ghost; NONE before any)
    mjumps,     \* deliveries more than TOL from the delivery before them (ghost)
    fsteps,     \* forward steps confirmed (ghost)
    bsteps      \* backward steps confirmed (ghost)

clk == <<clock, seen, last, cn, cl, cb, ever, fsteps, bsteps>>
vars == <<clock, seen, last, cn, cl, cb, ever, fsteps, bsteps, stream, pos, lastDel, prevT, mjumps>>

(***************************************************************************)
(* clock_observe for a frame of feed f at time t.                           *)
(***************************************************************************)
Frame(f, t) ==
    LET others == Feeds \ {f}
        nearClock == clock # NONE /\ Within(t, clock)
        nearOwn == seen[f] /\ Within(t, last[f])
        back == t < clock
        loneOK == \A g \in others : ~ever[g]
        cont == /\ cn[f] > 0 /\ cb[f] = back
                /\ IF back THEN Within(t, cl[f])
                   ELSE t >= cl[f] - TOL /\ t - cl[f] <= GAP_MAX
        n == IF cont THEN Min(cn[f] + 1, CONFIRM) ELSE 1
        votersNew == { g \in others : cn[g] > 0 /\ cb[g] = back /\ Within(cl[g], t) }
        backNew == 1 + Cardinality(votersNew)
        backOld == Cardinality({ g \in others : seen[g] /\ cn[g] = 0 /\ Within(last[g], clock) })
        confirmed == n = CONFIRM /\ (back => backNew >= backOld)
    IN \/ \* The "no clock" value as a frame time is held.
          /\ t = NONE
          /\ UNCHANGED clk
       \/ \* First frame.
          /\ t # NONE /\ clock = NONE
          /\ clock' = t
          /\ seen' = [seen EXCEPT ![f] = TRUE]
          /\ last' = [last EXCEPT ![f] = t]
          /\ ever' = [ever EXCEPT ![f] = TRUE]
          /\ UNCHANGED <<cn, cl, cb, fsteps, bsteps>>
       \/ \* Ordinary: within TOL of the feed's own last or of the clock.
          /\ t # NONE /\ clock # NONE /\ (nearOwn \/ nearClock)
          /\ cn' = [g \in Feeds |-> IF nearClock \/ g = f THEN 0 ELSE cn[g]]
          /\ cl' = [g \in Feeds |-> IF nearClock \/ g = f THEN 0 ELSE cl[g]]
          /\ cb' = [g \in Feeds |-> IF nearClock \/ g = f THEN FALSE ELSE cb[g]]
          /\ IF nearOwn /\ t < last[f]
             THEN \* Regress: the feed's last and the clock stay.
                  UNCHANGED <<clock, seen, last, ever>>
             ELSE /\ seen' = [seen EXCEPT ![f] = TRUE]
                  /\ last' = [last EXCEPT ![f] = t]
                  /\ ever' = [ever EXCEPT ![f] = TRUE]
                  /\ clock' = IF t >= clock THEN t ELSE clock   \* Advance or Keep
          /\ UNCHANGED <<fsteps, bsteps>>
       \/ \* Lone-forward: no other feed has ever had an accepted frame.
          /\ t # NONE /\ clock # NONE /\ ~(nearOwn \/ nearClock)
          /\ seen[f] /\ t > last[f] /\ t - last[f] <= GAP_MAX /\ loneOK
          /\ clock' = IF t >= clock THEN t ELSE clock
          /\ last' = [last EXCEPT ![f] = t]
          /\ seen' = seen
          /\ ever' = [ever EXCEPT ![f] = TRUE]
          /\ cn' = [cn EXCEPT ![f] = 0]
          /\ cl' = [cl EXCEPT ![f] = 0]
          /\ cb' = [cb EXCEPT ![f] = FALSE]
          /\ UNCHANGED <<fsteps, bsteps>>
       \/ \* Candidate, confirmed: a step.
          /\ t # NONE /\ clock # NONE /\ ~(nearOwn \/ nearClock)
          /\ ~(seen[f] /\ t > last[f] /\ t - last[f] <= GAP_MAX /\ loneOK)
          /\ confirmed
          /\ clock' = t
          /\ seen' = [g \in Feeds |-> g = f \/ g \in votersNew]
          /\ last' = [g \in Feeds |-> IF g = f THEN t
                                      ELSE IF g \in votersNew THEN Min(cl[g], t) ELSE 0]
          /\ ever' = [g \in Feeds |-> ever[g] \/ g = f \/ g \in votersNew]
          /\ cn' = [g \in Feeds |-> 0]
          /\ cl' = [g \in Feeds |-> 0]
          /\ cb' = [g \in Feeds |-> FALSE]
          /\ fsteps' = IF back THEN fsteps ELSE fsteps + 1
          /\ bsteps' = IF back THEN bsteps + 1 ELSE bsteps
       \/ \* Candidate, held.
          /\ t # NONE /\ clock # NONE /\ ~(nearOwn \/ nearClock)
          /\ ~(seen[f] /\ t > last[f] /\ t - last[f] <= GAP_MAX /\ loneOK)
          /\ ~confirmed
          /\ cn' = [cn EXCEPT ![f] = n]
          /\ cl' = [cl EXCEPT ![f] = t]
          /\ cb' = [cb EXCEPT ![f] = back]
          /\ UNCHANGED <<clock, seen, last, ever, fsteps, bsteps>>

Pending(f) == pos[f] < Len(stream[f])
NextT(f) == stream[f][pos[f] + 1]
AllDone == \A f \in Feeds : ~Pending(f)

RECURSIVE Prod(_)
Prod(k) == IF k = 0 THEN { <<>> }
           ELSE { Append(p, s) : p \in Prod(k - 1), s \in Choices[k - 1] }
StreamSets == { [i \in Feeds |-> p[i + 1]] : p \in Prod(NF) }

Init ==
    /\ clock = NONE
    /\ seen = [f \in Feeds |-> FALSE]
    /\ last = [f \in Feeds |-> 0]
    /\ cn = [f \in Feeds |-> 0]
    /\ cl = [f \in Feeds |-> 0]
    /\ cb = [f \in Feeds |-> FALSE]
    /\ ever = [f \in Feeds |-> FALSE]
    /\ stream \in StreamSets
    /\ pos = [f \in Feeds |-> 0]
    /\ lastDel = -1
    /\ prevT = NONE
    /\ mjumps = 0
    /\ fsteps = 0
    /\ bsteps = 0

IsNext(f) ==
    /\ Pending(f)
    /\ \/ ~Merged
       \/ \A g \in Feeds : Pending(g) =>
            (NextT(f) < NextT(g) \/ (NextT(f) = NextT(g) /\ f <= g))

Deliver(f) ==
    /\ IsNext(f)
    /\ Frame(f, NextT(f))
    /\ pos' = [pos EXCEPT ![f] = pos[f] + 1]
    /\ lastDel' = f
    /\ prevT' = NextT(f)
    /\ mjumps' = IF prevT # NONE /\ ~Within(NextT(f), prevT) THEN mjumps + 1 ELSE mjumps
    /\ UNCHANGED stream

Done == AllDone /\ UNCHANGED vars

Next == (\E f \in Feeds : Deliver(f)) \/ Done

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

(***************************************************************************)
(* Properties                                                               *)
(***************************************************************************)

\* 1. Types and ranges: nothing leaves the modelled ranges; the candidate count never passes
\* CONFIRM; a feed's last frame is never after the clock; a forward candidate is never
\* saturated (it would have stepped).
TypeInv ==
    /\ clock \in NONE..MaxT
    /\ seen \in [Feeds -> BOOLEAN]
    /\ ever \in [Feeds -> BOOLEAN]
    /\ last \in [Feeds -> 0..MaxT]
    /\ cn \in [Feeds -> 0..CONFIRM]
    /\ cl \in [Feeds -> 0..MaxT]
    /\ cb \in [Feeds -> BOOLEAN]
    /\ pos \in [Feeds -> Nat]
    /\ \A f \in Feeds : pos[f] <= Len(stream[f])
    /\ fsteps \in Nat /\ bsteps \in Nat
    /\ \A f \in Feeds : seen[f] => ever[f]
    /\ \A f \in Feeds : seen[f] => (clock # NONE /\ last[f] <= clock)
    /\ \A f \in Feeds : cn[f] = CONFIRM => cb[f]

\* 2. No stream is left behind the clock by more than TOL, unless it has regressed within
\* itself (a decreasing step among its frames up to and including its head).
RegrUpToHead(f) == \E i \in 1..pos[f] : i < Len(stream[f]) /\ stream[f][i + 1] < stream[f][i]
Behind ==
    \A f \in Feeds :
        Pending(f) => (RegrUpToHead(f) \/ clock <= NextT(f) + TOL)

\* 3. Progress.  (a) Termination is the temporal property below.  (b) No permanent freeze:
\* when a feed resumes after a pause (a forward step of its own stream larger than TOL) and
\* its next CONFIRM-1 frames each follow within TOL, then right after the CONFIRM-th of those
\* frames (the resuming frame counted), the clock is at or past the resuming frame's time,
\* provided no stream has regressed among the frames delivered so far.
NoRegrDelivered ==
    \A g \in Feeds : ~(\E i \in 1..(pos[g] - 1) : stream[g][i + 1] < stream[g][i])
Resumed(f, r) ==
    /\ lastDel = f
    /\ pos[f] = r + CONFIRM - 1
    /\ stream[f][r] - stream[f][r - 1] > TOL
    /\ \A i \in r..(r + CONFIRM - 2) :
         stream[f][i + 1] - stream[f][i] >= 0 /\ stream[f][i + 1] - stream[f][i] <= TOL
    /\ NoRegrDelivered
NoFreeze ==
    \A f \in Feeds : \A r \in 2..Len(stream[f]) :
        Resumed(f, r) => clock >= stream[f][r]
\* Sanity check that NoFreeze is not vacuous: this one is expected to fail (a resume is
\* reached) in every configuration that has a pause.
NeverResumed ==
    \A f \in Feeds : \A r \in 2..Len(stream[f]) : ~Resumed(f, r)

Termination == <>AllDone

\* No ping-pong, counted per stream: steps against the within-stream jumps (a regression, or
\* a forward pause larger than TOL).  Both NoPingPong and NoPingPong2 (twice the bound) fail
\* in the merged order, because a feed that starts far from the others costs a step without
\* any jump inside a stream; see README.md.
RECURSIVE SumJ(_)
SumJ(S) == IF S = {} THEN 0
           ELSE LET f == CHOOSE x \in S : TRUE IN Jumps(stream[f]) + SumJ(S \ {f})
NJumps == SumJ(Feeds)
NoPingPong == fsteps + bsteps <= NJumps
NoPingPong2 == fsteps + bsteps <= 2 * NJumps
\* The same bound counted in the merged delivery order: steps against the deliveries that are
\* more than TOL from the delivery before them (this includes a feed that starts, or
\* resumes, far from the others).
NoPingPongMerged == fsteps + bsteps <= mjumps

(***************************************************************************)
(* Stream choices used by the configurations.                              *)
(***************************************************************************)
\* Two feeds, at most one jump per stream, up to 5 frames.  Only the offset between the feeds'
\* starts matters, so feed 0 starts at 3 and feed 1 at 0..6 (offsets -3..3).
Choices2 == [f \in Feeds |->
    IF f = 0 THEN Gen(0, {3}, 5, {1, 2}, {-3, 4, 9}, 1)
    ELSE Gen(0, 0..6, 5, {1, 2}, {-3, 4, 9}, 1)]
\* Two feeds, smaller sets for the arbitrary-order runs.
Choices2B == [f \in Feeds |->
    IF f = 0 THEN Gen(0, {3}, 4, {1, 2}, {-3, 4, 9}, 1)
    ELSE Gen(0, {0, 2, 3, 5}, 4, {1, 2}, {-3, 4, 9}, 1)]
\* Three feeds: the middle feed may jump, the others are jump-free.
Choices3 == [f \in Feeds |->
    IF f = 0 THEN Gen(0, {3}, 4, {1, 2}, {}, 0)
    ELSE IF f = 1 THEN Gen(0, 2..4, 4, {1, 2}, {-3, 4, 9}, 1)
    ELSE Gen(0, {1, 3, 5}, 4, {1, 2}, {}, 0)]
\* The earlier deadlock: one feed's recording ends, the other resumes after a pause (a gap
\* within GAP_MAX, one beyond it, and one feed continuing past the pause of the other).
ScenarioChoices == [f \in Feeds |->
    IF f = 0 THEN { <<3, 4, 5>>, <<3, 4, 5, 6, 7>> }
    ELSE { <<3, 4, 5, 6, 14, 15, 16, 17>>, <<3, 4, 5, 6, 18, 19, 20>>,
           <<3, 4, 5, 6, 7, 8, 16, 17, 18, 19>> }]
\* The "no clock" value as a frame time (A, tiny).
NoneChoices == [f \in Feeds |-> Gen(-1, -1..1, 3, {1, 2}, {}, 0)]
=============================================================================
