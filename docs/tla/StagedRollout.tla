---------------------------- MODULE StagedRollout ----------------------------
(***************************************************************************)
(* Bounded staged software rollout policy.                                 *)
(*                                                                         *)
(* Primary keeps the prior trusted release. Shadow receives one candidate. *)
(* Comparison actions stand for abstract independent evidence; they do not  *)
(* model a comparator, release authority, deploy mechanism, or supervisor. *)
(* A primary fault before qualification may leave the service unavailable. *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS MaxComparisons, AllowPrematurePromotion

ASSUME MaxComparisons >= 1

Replicas == {"Primary", "Shadow"}
PriorRelease == "prior-v1"
CandidateRelease == "candidate-v2"
NoRelease == "none"
Releases == {PriorRelease, CandidateRelease, NoRelease}

VARIABLES release, healthy, serving, comparisons, qualified,
          evidenceValid, quarantined, unavailable

vars == <<release, healthy, serving, comparisons, qualified,
          evidenceValid, quarantined, unavailable>>

Init ==
    /\ release = [r \in Replicas |-> IF r = "Primary" THEN PriorRelease ELSE NoRelease]
    /\ healthy = [r \in Replicas |-> TRUE]
    /\ serving = [r \in Replicas |-> r = "Primary"]
    /\ comparisons = 0
    /\ qualified = FALSE
    /\ evidenceValid = FALSE
    /\ quarantined = FALSE
    /\ unavailable = FALSE

InstallCandidate ==
    /\ release["Shadow"] = NoRelease
    /\ healthy["Primary"]
    /\ healthy["Shadow"]
    /\ serving["Primary"]
    /\ release' = [release EXCEPT !["Shadow"] = CandidateRelease]
    /\ comparisons' = 0
    /\ qualified' = FALSE
    /\ evidenceValid' = TRUE
    /\ quarantined' = FALSE
    /\ UNCHANGED <<healthy, serving, unavailable>>

(* One bounded abstract concordant comparison against the trusted primary. *)
GoodComparison ==
    /\ release["Shadow"] = CandidateRelease
    /\ healthy["Primary"]
    /\ healthy["Shadow"]
    /\ evidenceValid
    /\ ~qualified
    /\ comparisons < MaxComparisons
    /\ comparisons' = comparisons + 1
    /\ UNCHANGED <<release, healthy, serving, qualified, evidenceValid,
                   quarantined, unavailable>>

Qualify ==
    /\ release["Shadow"] = CandidateRelease
    /\ healthy["Primary"]
    /\ healthy["Shadow"]
    /\ evidenceValid
    /\ comparisons = MaxComparisons
    /\ ~qualified
    /\ qualified' = TRUE
    /\ UNCHANGED <<release, healthy, serving, comparisons, evidenceValid,
                   quarantined, unavailable>>

Promote ==
    /\ release["Shadow"] = CandidateRelease
    /\ healthy["Shadow"]
    /\ evidenceValid
    /\ (qualified \/ AllowPrematurePromotion)
    /\ ~serving["Shadow"]
    /\ serving' = [serving EXCEPT !["Primary"] = FALSE, !["Shadow"] = TRUE]
    /\ unavailable' = FALSE
    /\ UNCHANGED <<release, healthy, comparisons, qualified, evidenceValid,
                   quarantined>>

(* A candidate failure or disagreement is contained on Shadow. *)
CandidateFailure ==
    /\ release["Shadow"] = CandidateRelease
    /\ healthy["Shadow"]
    /\ healthy' = [healthy EXCEPT !["Shadow"] = FALSE]
    /\ serving' = [serving EXCEPT !["Shadow"] = FALSE,
                                  !["Primary"] = healthy["Primary"]]
    /\ qualified' = FALSE
    /\ evidenceValid' = FALSE
    /\ quarantined' = TRUE
    /\ unavailable' = ~healthy["Primary"]
    /\ UNCHANGED <<release, comparisons>>

EvidenceDisagreement ==
    /\ release["Shadow"] = CandidateRelease
    /\ evidenceValid
    /\ evidenceValid' = FALSE
    /\ qualified' = FALSE
    /\ serving' = [serving EXCEPT !["Shadow"] = FALSE,
                                  !["Primary"] = healthy["Primary"]]
    /\ quarantined' = TRUE
    /\ unavailable' = ~healthy["Primary"]
    /\ UNCHANGED <<release, healthy, comparisons>>

PrimaryFailure ==
    /\ healthy["Primary"]
    /\ healthy' = [healthy EXCEPT !["Primary"] = FALSE]
    /\ serving' = [serving EXCEPT !["Primary"] = FALSE]
    /\ unavailable' = ~serving["Shadow"]
    /\ UNCHANGED <<release, comparisons, qualified, evidenceValid,
                   quarantined>>

Next ==
    \/ InstallCandidate
    \/ GoodComparison
    \/ Qualify
    \/ Promote
    \/ CandidateFailure
    \/ EvidenceDisagreement
    \/ PrimaryFailure

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ release \in [Replicas -> Releases]
    /\ healthy \in [Replicas -> BOOLEAN]
    /\ serving \in [Replicas -> BOOLEAN]
    /\ comparisons \in 0..MaxComparisons
    /\ qualified \in BOOLEAN
    /\ evidenceValid \in BOOLEAN
    /\ quarantined \in BOOLEAN
    /\ unavailable \in BOOLEAN

PriorReleaseUntouched == release["Primary"] = PriorRelease

QuarantineFallsBackWhenPriorHealthy ==
    quarantined /\ healthy["Primary"] => serving["Primary"] /\ ~unavailable

NoUnqualifiedCandidateServing ==
    serving["Shadow"] =>
        /\ release["Shadow"] = CandidateRelease
        /\ healthy["Shadow"]
        /\ qualified
        /\ evidenceValid
        /\ ~quarantined

AtMostOneServing == Cardinality({r \in Replicas : serving[r]}) <= 1

ServingHealthy == \A r \in Replicas : serving[r] => healthy[r]

UnavailableMatchesServing ==
    unavailable <=> ~\E r \in Replicas : serving[r]

=============================================================================
