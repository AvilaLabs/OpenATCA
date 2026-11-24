# Regional session increment

Proposed implementation, 2026-11-23. This is an offline integration increment, not a production
authority service or an operational qualification claim.

## Result

Add `tools/oa-regional-session`, a shell CLI that processes a bounded, ordered regional input
timeline. Each admitted prefix supplies one flight/aircraft identity and one issue cutoff to
flight state, tracker prediction, route forecast, conflict alerts, feed status and the scope.
Freeze outputs before admitting later input. Never use final flight state or future tracker
history to construct earlier frames.

Start with a checked, public synthetic input format and an executable example containing two
aircraft, an amendment, altitude clearance, offer/accept/take handoff, source loss and recovery.
Make the same runner usable on explicitly supplied normalized inputs. Raw recording ingestion
is a later adapter if it cannot be completed and checked in this increment. Do not describe
normalized synthetic evidence as a replay of FAA recordings.

An older source amendment delivered late must not roll back newer source intent. Route
selection considers only available inputs at the cutoff and reports contradictory revisions,
missing geometry, unsupported clearances and ambiguous identity explicitly. Route geometry is
supplied evidence; an independent route converter remains outside this slice.

## Proposed dependency approval

RULESET F1 requires Connor's approval even for links to existing workspace crates. No external
crate is proposed. The new shell tool may use the following existing workspace crates, with
the stated reason written beside each dependency in its Cargo.toml. Remove unused links.

| Workspace crate | Reason |
| --- | --- |
| oa-flight | Authoritative flight state, clearance and handoff operations |
| oa-flight-sfdps | Normalize supplied SFDPS messages if the recording adapter fits this slice |
| oa-track | Actual surveillance updates and issue-time predictions |
| oa-legacy-traj | Existing route forecast and cutoff provenance |
| oa-ca | Actual conflict cycles and alert transitions |
| oa-msaw | Actual terrain-warning evaluation where adaptation is supplied |
| oa-safety | Feed availability, net status and visible degradation |
| oa-display | Plain display input types and ownership values |
| oa-scope-replay | Existing scope construction and raster evidence |
| oa-airspace | Scope adaptation model |
| oa-replay | Bounded receipt-ordered recording merge, if raw ingestion fits |
| oa-feed | Recording reader, if raw ingestion fits |
| oa-adsb | Surveillance decoder, if raw ingestion fits |
| oa-xml | Bounded XML parsing, if raw ingestion fits |
| oa-fixm | SFDPS event records, if raw ingestion fits |
| oa-geo | Checked geographic conversion |
| oa-math | Deterministic arithmetic and canonical floats |
| oa-ieee | Exactly specified numeric conversion operations if needed |
| oa-sha256 | Bind input, adaptation and frame bytes to evidence |
| oa-sealed | Refuse sealed inputs before opening them |

## Ownership and checks

Three bounded implementation lanes: session runner; source-order trajectory selection and
flight-view adapter; failure scenarios. Coordinator freezes their interfaces, reviews each
change against the ruleset, owns all local builds, and integrates onto the existing coordinator
branch. No new remote branches. No new dependency is added before F1 approval.

Acceptance requires independently calculated fixture answers, positive full-identifier joins,
no future input leakage, explicit counts for every refusal and omission, retained ownership
until TAKE, CA inputs equal to the same-time displayed predictions, and deterministic output
from repeated execution. Fresh-session reconstruction from an admitted prefix must reproduce
the uninterrupted suffix, including flight state, trajectory/refusal, alerts, warnings and
frame hashes. Source-loss warnings and coast/reacquisition are checked independently.

Process death/publication fencing, if implemented, must reject old generations and duplicate
sequences. A local replay/reconstruction test does not establish production multi-host failover.
Report which boundary was actually exercised.

Run the existing complete promotion gate serially under the memory guard. Push reviewed,
validated source to both `main` and `resilience`; keep private evidence and sealed days private.
