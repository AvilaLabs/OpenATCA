---------------------------- MODULE RecoveryProtocol ----------------------------
(***************************************************************************)
(* Bounded model of a narrow checkpoint recovery and publication fence.   *)
(*                                                                        *)
(* The external authority supplies generations. It advances strictly;    *)
(* this model has no election, quorum, transfer, or authority inference.   *)
(* A checkpoint is abstractly verified when its peer identity and          *)
(* authentication predicates hold and its generation/watermark bind to   *)
(* the publication.                                                       *)
(* No cryptographic primitive or real storage format is modelled.         *)
(*                                                                        *)
(* Two nodes exchange bounded messages. Pending messages may be delayed,   *)
(* duplicated, or delivered after a generation change or crash. Watermarks *)
(* stand for replay progress and must strictly advance on acceptance.     *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    MaxGeneration,
    MaxWatermark,
    MaxMessages,
    RequireVerified,
    FaultInjection

ASSUME MaxGeneration >= 2
ASSUME MaxWatermark >= 2
ASSUME MaxMessages >= 2

Nodes == {"A", "B"}
Peer(n) == IF n = "A" THEN "B" ELSE "A"
Slots == 1..MaxMessages
Generations == 1..MaxGeneration
Watermarks == 1..MaxWatermark
NoValue == 0

VARIABLES
    authorityGen,
    online,
    serving,
    activeGen,
    activeWM,
    activeVerified,
    durableGen,
    durableWM,
    durableVerified,
    everGen,
    everWM,
    used,
    msgFrom,
    msgTo,
    msgGen,
    msgWM,
    msgBinds,
    msgAuthenticated

vars == <<authorityGen, online, serving, activeGen, activeWM, activeVerified,
          durableGen, durableWM, durableVerified, everGen, everWM, used,
          msgFrom, msgTo, msgGen, msgWM, msgBinds, msgAuthenticated>>

CheckpointVerified(s) ==
    /\ msgFrom[s] = Peer(msgTo[s])
    /\ msgBinds[s]
    /\ msgAuthenticated[s]

CanAccept(n, s) ==
    /\ used[s]
    /\ msgTo[s] = n
    /\ online[n]
    /\ msgGen[s] = authorityGen
    /\ msgGen[s] >= everGen[n]
    /\ msgWM[s] > everWM[n]
    /\ (IF RequireVerified THEN CheckpointVerified(s) ELSE TRUE)

Init ==
    /\ authorityGen = 1
    /\ online = [n \in Nodes |-> TRUE]
    /\ serving = [n \in Nodes |-> FALSE]
    /\ activeGen = [n \in Nodes |-> NoValue]
    /\ activeWM = [n \in Nodes |-> NoValue]
    /\ activeVerified = [n \in Nodes |-> FALSE]
    /\ durableGen = [n \in Nodes |-> NoValue]
    /\ durableWM = [n \in Nodes |-> NoValue]
    /\ durableVerified = [n \in Nodes |-> FALSE]
    /\ everGen = [n \in Nodes |-> NoValue]
    /\ everWM = [n \in Nodes |-> NoValue]
    /\ used = [s \in Slots |-> FALSE]
    /\ msgFrom = [s \in Slots |-> "A"]
    /\ msgTo = [s \in Slots |-> "A"]
    /\ msgGen = [s \in Slots |-> NoValue]
    /\ msgWM = [s \in Slots |-> NoValue]
    /\ msgBinds = [s \in Slots |-> FALSE]
    /\ msgAuthenticated = [s \in Slots |-> FALSE]

(* A node republishes only its currently serving, already verified state. *)
PublishCurrent(from, to, s) ==
    /\ from \in Nodes
    /\ to \in Nodes \ {from}
    /\ s \in Slots
    /\ serving[from]
    /\ activeVerified[from]
    /\ ~used[s]
    /\ used' = [used EXCEPT ![s] = TRUE]
    /\ msgFrom' = [msgFrom EXCEPT ![s] = from]
    /\ msgTo' = [msgTo EXCEPT ![s] = to]
    /\ msgGen' = [msgGen EXCEPT ![s] = activeGen[from]]
    /\ msgWM' = [msgWM EXCEPT ![s] = activeWM[from]]
    /\ msgBinds' = [msgBinds EXCEPT ![s] = TRUE]
    /\ msgAuthenticated' = [msgAuthenticated EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<authorityGen, online, serving, activeGen, activeWM,
                   activeVerified, durableGen, durableWM, durableVerified,
                   everGen, everWM>>

(* Abstract external offer: includes valid, unauthenticated, or mismatched
   checkpoint metadata. This is the bounded fault-injection surface. *)
NetworkOffer(from, to, s, g, w, binds, authenticated) ==
    /\ FaultInjection
    /\ from \in Nodes
    /\ to \in Nodes
    /\ s \in Slots
    /\ g \in Generations
    /\ w \in Watermarks
    /\ binds \in BOOLEAN
    /\ authenticated \in BOOLEAN
    /\ ~used[s]
    /\ used' = [used EXCEPT ![s] = TRUE]
    /\ msgFrom' = [msgFrom EXCEPT ![s] = from]
    /\ msgTo' = [msgTo EXCEPT ![s] = to]
    /\ msgGen' = [msgGen EXCEPT ![s] = g]
    /\ msgWM' = [msgWM EXCEPT ![s] = w]
    /\ msgBinds' = [msgBinds EXCEPT ![s] = binds]
    /\ msgAuthenticated' = [msgAuthenticated EXCEPT ![s] = authenticated]
    /\ UNCHANGED <<authorityGen, online, serving, activeGen, activeWM,
                   activeVerified, durableGen, durableWM, durableVerified,
                   everGen, everWM>>

Duplicate(s, fresh) ==
    /\ s \in Slots
    /\ fresh \in Slots
    /\ s # fresh
    /\ used[s]
    /\ ~used[fresh]
    /\ used' = [used EXCEPT ![fresh] = TRUE]
    /\ msgFrom' = [msgFrom EXCEPT ![fresh] = msgFrom[s]]
    /\ msgTo' = [msgTo EXCEPT ![fresh] = msgTo[s]]
    /\ msgGen' = [msgGen EXCEPT ![fresh] = msgGen[s]]
    /\ msgWM' = [msgWM EXCEPT ![fresh] = msgWM[s]]
    /\ msgBinds' = [msgBinds EXCEPT ![fresh] = msgBinds[s]]
    /\ msgAuthenticated' = [msgAuthenticated EXCEPT ![fresh] = msgAuthenticated[s]]
    /\ UNCHANGED <<authorityGen, online, serving, activeGen, activeWM,
                   activeVerified, durableGen, durableWM, durableVerified,
                   everGen, everWM>>

(* A consumed slot carries no state. Forget its metadata regardless of the verdict. *)
ClearSlot(s) ==
    /\ used' = [used EXCEPT ![s] = FALSE]
    /\ msgFrom' = [msgFrom EXCEPT ![s] = "A"]
    /\ msgTo' = [msgTo EXCEPT ![s] = "A"]
    /\ msgGen' = [msgGen EXCEPT ![s] = NoValue]
    /\ msgWM' = [msgWM EXCEPT ![s] = NoValue]
    /\ msgBinds' = [msgBinds EXCEPT ![s] = FALSE]
    /\ msgAuthenticated' = [msgAuthenticated EXCEPT ![s] = FALSE]

Accept(n, s) ==
    /\ n \in Nodes
    /\ s \in Slots
    /\ CanAccept(n, s)
    /\ serving' = [serving EXCEPT ![n] = TRUE]
    /\ activeGen' = [activeGen EXCEPT ![n] = msgGen[s]]
    /\ activeWM' = [activeWM EXCEPT ![n] = msgWM[s]]
    /\ activeVerified' = [activeVerified EXCEPT ![n] = CheckpointVerified(s)]
    /\ durableGen' = [durableGen EXCEPT ![n] = msgGen[s]]
    /\ durableWM' = [durableWM EXCEPT ![n] = msgWM[s]]
    /\ durableVerified' = [durableVerified EXCEPT ![n] = CheckpointVerified(s)]
    /\ everGen' = [everGen EXCEPT ![n] = msgGen[s]]
    /\ everWM' = [everWM EXCEPT ![n] = msgWM[s]]
    /\ ClearSlot(s)
    /\ UNCHANGED <<authorityGen, online>>

Reject(n, s) ==
    /\ n \in Nodes
    /\ s \in Slots
    /\ used[s]
    /\ msgTo[s] = n
    /\ online[n]
    /\ ~CanAccept(n, s)
    /\ ClearSlot(s)
    /\ UNCHANGED <<authorityGen, online, serving, activeGen, activeWM,
                   activeVerified, durableGen, durableWM, durableVerified,
                   everGen, everWM>>

Receive(s) ==
    /\ s \in Slots
    /\ used[s]
    /\ IF CanAccept(msgTo[s], s)
          THEN Accept(msgTo[s], s)
          ELSE Reject(msgTo[s], s)

AdvanceAuthority(g) ==
    /\ g \in Generations
    /\ g > authorityGen
    /\ authorityGen' = g
    /\ serving' = [n \in Nodes |-> FALSE]
    /\ activeGen' = [n \in Nodes |-> NoValue]
    /\ activeWM' = [n \in Nodes |-> NoValue]
    /\ activeVerified' = [n \in Nodes |-> FALSE]
    /\ UNCHANGED <<online, durableGen, durableWM, durableVerified,
                   everGen, everWM, used, msgFrom, msgTo, msgGen, msgWM,
                   msgBinds, msgAuthenticated>>

Crash(n) ==
    /\ n \in Nodes
    /\ online[n]
    /\ online' = [online EXCEPT ![n] = FALSE]
    /\ serving' = [serving EXCEPT ![n] = FALSE]
    /\ activeGen' = [activeGen EXCEPT ![n] = NoValue]
    /\ activeWM' = [activeWM EXCEPT ![n] = NoValue]
    /\ activeVerified' = [activeVerified EXCEPT ![n] = FALSE]
    /\ UNCHANGED <<authorityGen, durableGen, durableWM, durableVerified,
                   everGen, everWM, used, msgFrom, msgTo, msgGen, msgWM,
                   msgBinds, msgAuthenticated>>

Restart(n) ==
    /\ n \in Nodes
    /\ ~online[n]
    /\ online' = [online EXCEPT ![n] = TRUE]
    /\ serving' = [serving EXCEPT ![n] =
          (durableGen[n] = authorityGen /\ durableVerified[n] /\ durableWM[n] >= everWM[n])]
    /\ activeGen' = [activeGen EXCEPT ![n] =
          IF durableGen[n] = authorityGen /\ durableVerified[n] /\ durableWM[n] >= everWM[n]
          THEN durableGen[n] ELSE NoValue]
    /\ activeWM' = [activeWM EXCEPT ![n] =
          IF durableGen[n] = authorityGen /\ durableVerified[n] /\ durableWM[n] >= everWM[n]
          THEN durableWM[n] ELSE NoValue]
    /\ activeVerified' = [activeVerified EXCEPT ![n] =
          (durableGen[n] = authorityGen /\ durableVerified[n] /\ durableWM[n] >= everWM[n])]
    /\ UNCHANGED <<authorityGen, durableGen, durableWM, durableVerified,
                   everGen, everWM, used, msgFrom, msgTo, msgGen, msgWM,
                   msgBinds, msgAuthenticated>>

Next ==
    \/ \E g \in Generations : AdvanceAuthority(g)
    \/ \E from \in Nodes : \E to \in Nodes : \E s \in Slots : PublishCurrent(from, to, s)
    \/ \E from \in Nodes : \E to \in Nodes : \E s \in Slots : \E g \in Generations :
          \E w \in Watermarks : \E binds \in BOOLEAN :
          \E authenticated \in BOOLEAN : NetworkOffer(from, to, s, g, w, binds, authenticated)
    \/ \E s \in Slots : \E fresh \in Slots : Duplicate(s, fresh)
    \/ \E s \in Slots : Receive(s)
    \/ \E n \in Nodes : Crash(n)
    \/ \E n \in Nodes : Restart(n)

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ authorityGen \in Generations
    /\ online \in [Nodes -> BOOLEAN]
    /\ serving \in [Nodes -> BOOLEAN]
    /\ activeGen \in [Nodes -> 0..MaxGeneration]
    /\ activeWM \in [Nodes -> 0..MaxWatermark]
    /\ activeVerified \in [Nodes -> BOOLEAN]
    /\ durableGen \in [Nodes -> 0..MaxGeneration]
    /\ durableWM \in [Nodes -> 0..MaxWatermark]
    /\ durableVerified \in [Nodes -> BOOLEAN]
    /\ everGen \in [Nodes -> 0..MaxGeneration]
    /\ everWM \in [Nodes -> 0..MaxWatermark]
    /\ used \in [Slots -> BOOLEAN]
    /\ msgFrom \in [Slots -> Nodes]
    /\ msgTo \in [Slots -> Nodes]
    /\ msgGen \in [Slots -> 0..MaxGeneration]
    /\ msgWM \in [Slots -> 0..MaxWatermark]
    /\ msgBinds \in [Slots -> BOOLEAN]
    /\ msgAuthenticated \in [Slots -> BOOLEAN]
    /\ \A s \in Slots : used[s] => msgGen[s] \in Generations /\ msgWM[s] \in Watermarks

NoUnverifiedServing ==
    \A n \in Nodes : serving[n] => activeVerified[n] /\ durableVerified[n]

NoStaleServing ==
    \A n \in Nodes : serving[n] => activeGen[n] = authorityGen

OutputAtHighWater ==
    \A n \in Nodes : serving[n] => activeGen[n] = everGen[n] /\ activeWM[n] = everWM[n]

DurableAtHighWater ==
    \A n \in Nodes : durableGen[n] = everGen[n] /\ durableWM[n] = everWM[n]

(* Action-level safety: the persistent high-water marks never decrease. *)
Nondecreasing ==
    \A n \in Nodes : everGen'[n] >= everGen[n] /\ everWM'[n] >= everWM[n]

NoRegression == [][Nondecreasing]_<<everGen, everWM>>

=============================================================================
