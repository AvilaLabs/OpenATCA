---------------------------- MODULE ApplyAckRecovery ----------------------------
(***************************************************************************)
(* Bounded model of an application whose acknowledgement is lost, followed  *)
(* by retry on a replacement worker. This models one sender-local identity  *)
(* and an abstract irreversible downstream effect; it is not a live sink.   *)
(*                                                                         *)
(* The strong variant commits the idempotency decision and effect together. *)
(* The weak variant makes the effect and durable decision separate actions,*)
(* allowing a crash after the effect and lost ACK but before the decision.  *)
(* No cryptography, durable store, real output, or supervisor is modelled.  *)
(***************************************************************************)
EXTENDS Naturals, TLC

CONSTANT AtomicCommit
ASSUME AtomicCommit \in BOOLEAN

Workers == {"worker-1", "worker-2"}
Digests == {"none", "good-digest", "other-digest"}
NoDigest == "none"
GoodDigest == "good-digest"
OtherDigest == "other-digest"

(* One explicit sender-local generation/source/sequence identity. A retry
   keeps this complete key; the collision probe changes only its digest. *)
SenderKey == [generation |-> 4, source |-> "source-A", sequence |-> 1]
NoKey == [generation |-> 0, source |-> "none", sequence |-> 0]

VARIABLES
    currentWorker,
    workerUp,
    requestKey,
    requestDigest,
    requestPending,
    attempt,
    ledgerKey,
    ledgerDigest,
    effectCount,
    effectUncommitted,
    ackPending,
    ackLost,
    senderAck,
    retryAccepted,
    collisionKey,
    collisionDigest,
    collisionRefused

vars == <<currentWorker, workerUp, requestKey, requestDigest, requestPending,
          attempt, ledgerKey, ledgerDigest, effectCount, effectUncommitted,
          ackPending, ackLost, senderAck, retryAccepted, collisionKey,
          collisionDigest, collisionRefused>>

TypeOK ==
    /\ currentWorker \in Workers
    /\ workerUp \in BOOLEAN
    /\ requestKey \in {SenderKey}
    /\ requestDigest \in Digests
    /\ requestPending \in BOOLEAN
    /\ attempt \in 0..1
    /\ ledgerKey \in {NoKey, SenderKey}
    /\ ledgerDigest \in Digests
    /\ effectCount \in 0..2
    /\ effectUncommitted \in BOOLEAN
    /\ ackPending \in BOOLEAN
    /\ ackLost \in BOOLEAN
    /\ senderAck \in BOOLEAN
    /\ retryAccepted \in BOOLEAN
    /\ collisionKey \in {NoKey, SenderKey}
    /\ collisionDigest \in Digests
    /\ collisionRefused \in BOOLEAN

Init ==
    /\ currentWorker = "worker-1"
    /\ workerUp = TRUE
    /\ requestKey = SenderKey
    /\ requestDigest = GoodDigest
    /\ requestPending = TRUE
    /\ attempt = 0
    /\ ledgerKey = NoKey
    /\ ledgerDigest = NoDigest
    /\ effectCount = 0
    /\ effectUncommitted = FALSE
    /\ ackPending = FALSE
    /\ ackLost = FALSE
    /\ senderAck = FALSE
    /\ retryAccepted = FALSE
    /\ collisionKey = NoKey
    /\ collisionDigest = NoDigest
    /\ collisionRefused = FALSE

StrongFirstApply ==
    /\ AtomicCommit
    /\ workerUp
    /\ currentWorker = "worker-1"
    /\ requestPending
    /\ attempt = 0
    /\ requestKey = SenderKey
    /\ requestDigest = GoodDigest
    /\ ledgerKey = NoKey
    /\ ledgerDigest = NoDigest
    /\ effectCount = 0
    /\ requestPending' = FALSE
    /\ effectCount' = 1
    /\ ledgerKey' = SenderKey
    /\ ledgerDigest' = GoodDigest
    /\ effectUncommitted' = FALSE
    /\ ackPending' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   attempt, ackLost, senderAck, retryAccepted, collisionKey,
                   collisionDigest, collisionRefused>>

WeakFirstApply ==
    /\ ~AtomicCommit
    /\ workerUp
    /\ currentWorker = "worker-1"
    /\ requestPending
    /\ attempt = 0
    /\ requestKey = SenderKey
    /\ requestDigest = GoodDigest
    /\ ledgerKey = NoKey
    /\ ledgerDigest = NoDigest
    /\ effectCount = 0
    /\ requestPending' = FALSE
    /\ effectCount' = 1
    /\ effectUncommitted' = TRUE
    /\ ackPending' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   attempt, ledgerKey, ledgerDigest, ackLost, senderAck,
                   retryAccepted, collisionKey, collisionDigest,
                   collisionRefused>>

FirstApply == StrongFirstApply \/ WeakFirstApply

LoseAck ==
    /\ ackPending
    /\ attempt = 0
    /\ ~senderAck
    /\ ackPending' = FALSE
    /\ ackLost' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   requestPending, attempt, ledgerKey, ledgerDigest,
                   effectCount, effectUncommitted, senderAck, retryAccepted,
                   collisionKey, collisionDigest, collisionRefused>>

Crash ==
    /\ workerUp
    /\ currentWorker = "worker-1"
    /\ effectCount = 1
    /\ ackLost
    /\ workerUp' = FALSE
    /\ effectUncommitted' = FALSE
    /\ UNCHANGED <<currentWorker, requestKey, requestDigest, requestPending,
                   attempt, ledgerKey, ledgerDigest, effectCount, ackPending,
                   ackLost, senderAck, retryAccepted, collisionKey,
                   collisionDigest, collisionRefused>>

Restart ==
    /\ ~workerUp
    /\ currentWorker = "worker-1"
    /\ currentWorker' = "worker-2"
    /\ workerUp' = TRUE
    /\ UNCHANGED <<requestKey, requestDigest, requestPending, attempt,
                   ledgerKey, ledgerDigest, effectCount, effectUncommitted,
                   ackPending, ackLost, senderAck, retryAccepted, collisionKey,
                   collisionDigest, collisionRefused>>

SendRetry ==
    /\ workerUp
    /\ currentWorker = "worker-2"
    /\ ackLost
    /\ ~senderAck
    /\ attempt = 0
    /\ ~requestPending
    /\ requestPending' = TRUE
    /\ attempt' = 1
    /\ requestDigest' = GoodDigest
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, ledgerKey,
                   ledgerDigest, effectCount, effectUncommitted, ackPending,
                   ackLost, senderAck, retryAccepted, collisionKey,
                   collisionDigest, collisionRefused>>

StrongAcceptRetry ==
    /\ AtomicCommit
    /\ workerUp
    /\ currentWorker = "worker-2"
    /\ requestPending
    /\ attempt = 1
    /\ requestKey = SenderKey
    /\ requestDigest = GoodDigest
    /\ ledgerKey = SenderKey
    /\ ledgerDigest = GoodDigest
    /\ ~senderAck
    /\ requestPending' = FALSE
    /\ retryAccepted' = TRUE
    /\ ackPending' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   attempt, ledgerKey, ledgerDigest, effectCount,
                   effectUncommitted, ackLost, senderAck, collisionKey,
                   collisionDigest, collisionRefused>>

WeakAcceptRetryWithoutDecision ==
    /\ ~AtomicCommit
    /\ workerUp
    /\ currentWorker = "worker-2"
    /\ requestPending
    /\ attempt = 1
    /\ requestKey = SenderKey
    /\ requestDigest = GoodDigest
    /\ ledgerKey = NoKey
    /\ ledgerDigest = NoDigest
    /\ effectCount = 1
    /\ ~senderAck
    /\ requestPending' = FALSE
    /\ effectCount' = 2
    /\ effectUncommitted' = TRUE
    /\ retryAccepted' = TRUE
    /\ ackPending' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   attempt, ledgerKey, ledgerDigest, ackLost, senderAck,
                   collisionKey, collisionDigest, collisionRefused>>

WeakAcceptRetryWithDecision ==
    /\ ~AtomicCommit
    /\ workerUp
    /\ currentWorker = "worker-2"
    /\ requestPending
    /\ attempt = 1
    /\ requestKey = SenderKey
    /\ requestDigest = GoodDigest
    /\ ledgerKey = SenderKey
    /\ ledgerDigest = GoodDigest
    /\ effectCount = 1
    /\ ~senderAck
    /\ requestPending' = FALSE
    /\ retryAccepted' = TRUE
    /\ ackPending' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   attempt, ledgerKey, ledgerDigest, effectCount,
                   effectUncommitted, ackLost, senderAck, collisionKey,
                   collisionDigest, collisionRefused>>

RetryApply == StrongAcceptRetry \/ WeakAcceptRetryWithoutDecision
               \/ WeakAcceptRetryWithDecision

WeakCommitDecision ==
    /\ ~AtomicCommit
    /\ workerUp
    /\ effectUncommitted
    /\ ledgerKey = NoKey
    /\ ledgerDigest = NoDigest
    /\ ledgerKey' = SenderKey
    /\ ledgerDigest' = GoodDigest
    /\ effectUncommitted' = FALSE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   requestPending, attempt, effectCount, ackPending, ackLost,
                   senderAck, retryAccepted, collisionKey, collisionDigest,
                   collisionRefused>>

(* The collision probe is independent of accepting and acknowledging a valid retry. *)
RejectDigestCollision ==
    /\ AtomicCommit
    /\ workerUp
    /\ currentWorker = "worker-2"
    /\ attempt = 1
    /\ requestKey = SenderKey
    /\ ledgerKey = SenderKey
    /\ ledgerDigest = GoodDigest
    /\ ~collisionRefused
    /\ collisionKey' = SenderKey
    /\ collisionDigest' = OtherDigest
    /\ collisionRefused' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   requestPending, attempt, ledgerKey, ledgerDigest,
                   effectCount, effectUncommitted, ackPending, ackLost,
                   senderAck, retryAccepted>>

DeliverRetryAck ==
    /\ ackPending
    /\ attempt = 1
    /\ retryAccepted
    /\ ~senderAck
    /\ ackPending' = FALSE
    /\ senderAck' = TRUE
    /\ UNCHANGED <<currentWorker, workerUp, requestKey, requestDigest,
                   requestPending, attempt, ledgerKey, ledgerDigest,
                   effectCount, effectUncommitted, ackLost, retryAccepted,
                   collisionKey, collisionDigest, collisionRefused>>

Next == FirstApply \/ LoseAck \/ Crash \/ Restart \/ SendRetry \/ RetryApply
        \/ WeakCommitDecision \/ RejectDigestCollision \/ DeliverRetryAck

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(FirstApply)
    /\ WF_vars(LoseAck)
    /\ WF_vars(Crash)
    /\ WF_vars(Restart)
    /\ WF_vars(SendRetry)
    /\ WF_vars(RetryApply)
    /\ WF_vars(WeakCommitDecision)
    /\ WF_vars(RejectDigestCollision)
    /\ WF_vars(DeliverRetryAck)

AtMostOnce == effectCount <= 1
EffectHasDecision == (effectCount > 0) => ledgerKey = SenderKey /\ ledgerDigest = GoodDigest
IdentityBound ==
    /\ requestKey = SenderKey
    /\ (ledgerDigest = NoDigest <=> ledgerKey = NoKey)
    /\ (collisionRefused => collisionKey = SenderKey)
DigestCollisionRefused ==
    collisionRefused
    => /\ collisionDigest = OtherDigest
       /\ ledgerDigest = GoodDigest
       /\ effectCount = 1
AcceptedRetryPath == <>(retryAccepted /\ senderAck /\ effectCount = 1)

=============================================================================
