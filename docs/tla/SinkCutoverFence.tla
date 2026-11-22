-------------------------- MODULE SinkCutoverFence --------------------------
(***************************************************************************)
(* Bounded model of a single output sink during one externally authorized  *)
(* generation change. The sink must stop old output and persist its minimum *)
(* generation before external authority grants the replacement.            *)
(*                                                                         *)
(* Authority is an external input, not elected or inferred here. A fence   *)
(* acknowledgement is emitted only after durableMinGen is raised. Loss of  *)
(* that acknowledgement delays the grant and may leave the sink stopped.  *)
(*                                                                         *)
(* The dequeue guard reads only sink-local durableMinGen and durableGrant, *)
(* never authorityGen. authorityGen is used only by external actions and   *)
(* as the model checker oracle for an emission's generation.              *)
(* No clock, lease, consensus, real storage, network, or aircraft control  *)
(* behavior is modeled.                                                    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANT RequireFenceAck
ASSUME RequireFenceAck \in BOOLEAN

Epochs == {1, 2}
NoGen == 0

VARIABLES authorityGen, online, partitioned,
          transferRequested, requestDelivered,
          ackPending, ackReceived,
          durableMinGen, durableGrantGen, durableEvidenceGen,
          serving, queued, queuedGen,
          lastOutputGen, lastOutputAuthority, lastOutputMin

vars == <<authorityGen, online, partitioned,
          transferRequested, requestDelivered,
          ackPending, ackReceived,
          durableMinGen, durableGrantGen, durableEvidenceGen,
          serving, queued, queuedGen,
          lastOutputGen, lastOutputAuthority, lastOutputMin>>

Init ==
    /\ authorityGen = 1
    /\ online = TRUE
    /\ partitioned = FALSE
    /\ transferRequested = FALSE
    /\ requestDelivered = FALSE
    /\ ackPending = FALSE
    /\ ackReceived = FALSE
    /\ durableMinGen = 1
    /\ durableGrantGen = 1
    /\ durableEvidenceGen = 1
    /\ serving = TRUE
    /\ queued = FALSE
    /\ queuedGen = NoGen
    /\ lastOutputGen = NoGen
    /\ lastOutputAuthority = NoGen
    /\ lastOutputMin = NoGen

SetPartition(value) ==
    /\ value \in BOOLEAN
    /\ value # partitioned
    /\ partitioned' = value
    /\ UNCHANGED <<authorityGen, online, transferRequested, requestDelivered,
                   ackPending, ackReceived, durableMinGen, durableGrantGen,
                   durableEvidenceGen, serving, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

RequestTransfer ==
    /\ authorityGen = 1
    /\ ~transferRequested
    /\ transferRequested' = TRUE
    /\ UNCHANGED <<authorityGen, online, partitioned, requestDelivered,
                   ackPending, ackReceived, durableMinGen, durableGrantGen,
                   durableEvidenceGen, serving, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

DeliverRequest ==
    /\ transferRequested
    /\ ~requestDelivered
    /\ online
    /\ ~partitioned
    /\ requestDelivered' = TRUE
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   ackPending, ackReceived, durableMinGen, durableGrantGen,
                   durableEvidenceGen, serving, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

(* Stop old output and persist the fence before creating an acknowledgement. *)
PersistFence ==
    /\ requestDelivered
    /\ online
    /\ durableMinGen = 1
    /\ durableMinGen' = 2
    /\ serving' = FALSE
    /\ ackPending' = TRUE
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackReceived, durableGrantGen,
                   durableEvidenceGen, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

LoseAck ==
    /\ ackPending
    /\ ackPending' = FALSE
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackReceived, durableMinGen,
                   durableGrantGen, durableEvidenceGen, serving, queued,
                   queuedGen, lastOutputGen, lastOutputAuthority,
                   lastOutputMin>>

RepeatAck ==
    /\ requestDelivered
    /\ online
    /\ durableMinGen = 2
    /\ ~ackPending
    /\ ~ackReceived
    /\ ackPending' = TRUE
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackReceived, durableMinGen,
                   durableGrantGen, durableEvidenceGen, serving, queued,
                   queuedGen, lastOutputGen, lastOutputAuthority,
                   lastOutputMin>>

DeliverAck ==
    /\ ackPending
    /\ online
    /\ ~partitioned
    /\ durableMinGen = 2
    /\ ackPending' = FALSE
    /\ ackReceived' = TRUE
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, durableMinGen, durableGrantGen,
                   durableEvidenceGen, serving, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

GrantReplacement ==
    /\ transferRequested
    /\ authorityGen = 1
    /\ (IF RequireFenceAck THEN ackReceived ELSE TRUE)
    /\ authorityGen' = 2
    /\ UNCHANGED <<online, partitioned, transferRequested,
                   requestDelivered, ackPending, ackReceived,
                   durableMinGen, durableGrantGen, durableEvidenceGen,
                   serving, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

DeliverGrant ==
    /\ authorityGen = 2
    /\ online
    /\ ~partitioned
    /\ durableGrantGen = 1
    /\ durableGrantGen' = 2
    /\ durableEvidenceGen' = 2
    /\ serving' = (durableMinGen <= 2)
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackPending, ackReceived,
                   durableMinGen, queued, queuedGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

QueueCurrentOutput ==
    /\ serving
    /\ ~queued
    /\ durableGrantGen = durableEvidenceGen
    /\ durableGrantGen >= durableMinGen
    /\ queued' = TRUE
    /\ queuedGen' = durableGrantGen
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackPending, ackReceived,
                   durableMinGen, durableGrantGen, durableEvidenceGen,
                   serving, lastOutputGen, lastOutputAuthority,
                   lastOutputMin>>

(* A single queued output may arrive late. Its admission uses only local,
   durable sink state; the authority value is not an input to this guard. *)
DequeueOutput ==
    /\ queued
    /\ online
    /\ queuedGen = durableGrantGen
    /\ queuedGen >= durableMinGen
    /\ queuedGen = durableEvidenceGen
    /\ serving
    /\ lastOutputGen' = queuedGen
    /\ lastOutputAuthority' = authorityGen
    /\ lastOutputMin' = durableMinGen
    /\ queued' = FALSE
    /\ queuedGen' = NoGen
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackPending, ackReceived,
                   durableMinGen, durableGrantGen, durableEvidenceGen,
                   serving>>

RejectQueuedOutput ==
    /\ queued
    /\ online
    /\ (queuedGen # durableGrantGen
        \/ queuedGen < durableMinGen
        \/ queuedGen # durableEvidenceGen
        \/ ~serving)
    /\ queued' = FALSE
    /\ queuedGen' = NoGen
    /\ UNCHANGED <<authorityGen, online, partitioned, transferRequested,
                   requestDelivered, ackPending, ackReceived,
                   durableMinGen, durableGrantGen, durableEvidenceGen,
                   serving, lastOutputGen, lastOutputAuthority,
                   lastOutputMin>>

Crash ==
    /\ online
    /\ online' = FALSE
    /\ serving' = FALSE
    /\ queued' = FALSE
    /\ queuedGen' = NoGen
    /\ ackPending' = FALSE
    /\ UNCHANGED <<authorityGen, partitioned, transferRequested,
                   requestDelivered, ackReceived, durableMinGen,
                   durableGrantGen, durableEvidenceGen,
                   lastOutputGen, lastOutputAuthority, lastOutputMin>>

Restart ==
    /\ ~online
    /\ online' = TRUE
    /\ serving' = (durableGrantGen = durableMinGen
                   /\ durableEvidenceGen = durableGrantGen)
    /\ UNCHANGED <<authorityGen, partitioned, transferRequested,
                   requestDelivered, ackPending, ackReceived,
                   durableMinGen, durableGrantGen, durableEvidenceGen,
                   queued, queuedGen, lastOutputGen, lastOutputAuthority,
                   lastOutputMin>>

Next ==
    \/ \E value \in BOOLEAN : SetPartition(value)
    \/ RequestTransfer
    \/ DeliverRequest
    \/ PersistFence
    \/ LoseAck
    \/ RepeatAck
    \/ DeliverAck
    \/ GrantReplacement
    \/ DeliverGrant
    \/ QueueCurrentOutput
    \/ DequeueOutput
    \/ RejectQueuedOutput
    \/ Crash
    \/ Restart

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ authorityGen \in Epochs
    /\ online \in BOOLEAN
    /\ partitioned \in BOOLEAN
    /\ transferRequested \in BOOLEAN
    /\ requestDelivered \in BOOLEAN
    /\ ackPending \in BOOLEAN
    /\ ackReceived \in BOOLEAN
    /\ durableMinGen \in Epochs
    /\ durableGrantGen \in Epochs
    /\ durableEvidenceGen \in Epochs
    /\ serving \in BOOLEAN
    /\ queued \in BOOLEAN
    /\ queuedGen \in {NoGen} \cup Epochs
    /\ lastOutputGen \in {NoGen} \cup Epochs
    /\ lastOutputAuthority \in {NoGen} \cup Epochs
    /\ lastOutputMin \in {NoGen} \cup Epochs

(* A durable fence precedes every external replacement grant. *)
NoGrantBeforeFence == durableMinGen >= authorityGen

(* Serving relies only on sink-local durable grant and matching evidence. *)
ServingHasLocalEvidence ==
    serving => durableGrantGen = durableMinGen
               /\ durableEvidenceGen = durableGrantGen

(* The most recent admitted output was current and above the sink's local
   durable minimum at the instant it was emitted. *)
NoStaleOutput ==
    lastOutputGen = NoGen
    \/ (lastOutputGen = lastOutputAuthority
        /\ lastOutputGen >= lastOutputMin)

=============================================================================
