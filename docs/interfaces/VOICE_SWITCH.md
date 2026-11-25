# Voice-system integration boundary

**Status:** Proposal, under review.

## Decision

OpenATCA will consume voice status through a **read-only, process-isolated adapter**. The adapter owns translation into a versioned normalized contract; OpenATCA consumes that contract over a local Unix-domain socket authenticated with operating-system peer credentials. When the adapter and OpenATCA run on separate hosts, use mutually authenticated TLS 1.3 with service identities. In both deployments, the adapter has no voice-control path or credentials, and the voice system remains independently usable if the adapter or OpenATCA fails.

The adapter stays outside the reusable flight-data/display core. OpenATCA accepts only the configured adapter identity; network reachability or a valid identity alone does not grant voice-system authority. The adapter may connect only to an endpoint and identity approved for the selected site and release.

This boundary exposes status, not voice control. It does not by itself demonstrate operational voice integration alongside surveillance and flight-data processing.

## Ownership and direction

Use separate authorities for separate facts; do not reconcile mismatches by silently choosing one value.

| Information | Authoritative owner | Proposed flow and use |
|---|---|---|
| Voice-system identity, reported service/resource state, and any reported PTT state | The selected voice system, only for fields its approved interface actually defines | Voice system → adapter → clearly source-attributed status view. Absence of a field means unknown; it does not mean idle, healthy, or unkeyed. |
| Facility, position, sector/side, console, radio-resource and frequency assignments; configuration revision | Facility-approved configuration authority or the voice-system configuration service explicitly named by the controlling ICD | Read-only configuration input to the adapter. Never infer a mapping from a sector name, public frequency list, local UI label, or stale configuration. Configuration changes become effective only with an explicit version/epoch from the authority. |
| Surveillance tracks, flight plans, controller commands and operational decisions | Their existing approved OpenATCA/source boundaries and, operationally, the controller | No voice adapter writes into these domains. A voice status and a flight/sector view that disagree are shown as a conflict with provenance and time. |
| Transmit selection, PTT/key/unkey, audio, call routing, mute, reconfiguration and failover | Approved voice equipment and its authorized operators/maintainers | No OpenATCA command path. These actions remain outside this proposal. |

Position, sector, controller position, console, and voice position are distinct identifiers unless the approved configuration explicitly maps them. Frequency assignments are configuration facts, not commands. OpenATCA will not infer that a selected resource is able to transmit or that a reported PTT indication proves audio reached a receiver.

## Process and data contract proposal

The adapter is a separate process with a one-way input boundary and a bounded output queue. It does not share mutable voice-system state with the core, expose a reverse command channel, or hold credentials usable for voice control. The adapter-to-OpenATCA envelope is the following OpenATCA-owned normalized contract. It is not the vendor-to-adapter protocol:

| Envelope field | Meaning |
|---|---|
| `contract_version` | Explicit major/minor version. Unknown major versions are rejected. Unknown optional fields in a supported minor version are ignored; required fields and their semantics are fixed by the selected contract version. |
| `source_id` | Authenticated voice-service identity, checked against the approved allowlist. |
| `epoch` | Adapter-session generation identifier; a new epoch invalidates sequence continuity from the prior generation. |
| `sequence` | Monotone per-source, per-epoch sequence number used for duplicate, gap, and replay detection. |
| `event_time` and `received_time` | Source timestamp and local receipt timestamp retained separately, with explicit time basis/quality from the ICD. |
| `configuration_version` | Version of the authoritative position/resource mapping used for any assignment. Missing or unknown version prevents assignment joins. |
| `observations[]` | Bounded, typed, source-reported status values with explicit validity and quality. No inferred values. |

The adapter emits a heartbeat at 1 Hz on this boundary, including its contract version, adapter epoch, upstream-feed state, and time of last valid source input. OpenATCA marks the feed unavailable after three consecutive missed heartbeats or three seconds without valid source input, clears current voice-status validity, and restores it only after a valid heartbeat and fresh source snapshot in the current epoch. Three seconds tolerates one missed interval for scheduling or delivery jitter while bounding stale-status display. These are OpenATCA design targets; revise them only if measured interface cadence/latency or an approved operating requirement shows they are unsuitable.

For the normalized boundary, cap a decoded message at 64 KiB, 128 observations per message, and a 256-message queue per adapter. These caps bound allocation and queue growth. Exceeding a cap rejects the whole message and increments a bounded diagnostic counter; it does not truncate and present a partial message as complete.

On a single host, authenticate the adapter with OS peer credentials on the Unix-domain socket and authorize its configured service identity. Across hosts, authenticate both service identities with TLS 1.3 mutual authentication and apply the same allowlist authorization. Authentication and authorization are separate; a valid but unapproved identity is rejected. OpenATCA user identity/session is not voice-system authority.

Within an adapter epoch, an identical repeated sequence is a duplicate and is ignored; a reused sequence with different content is a protocol-integrity error and invalidates the affected source state. Older sequences are rejected as replay. A forward sequence gap is recorded and source status becomes incomplete until a fresh source snapshot restores a known baseline. Epoch changes start a new sequence domain; data from a prior epoch cannot overwrite the new one.

## Mismatch, partition, and loss behavior

| Condition | Adapter behavior | Operator-facing behavior |
|---|---|---|
| Authentication, authorization, schema, or version failure | Reject input; retain only bounded reason counters; close/reconnect only under the approved retry policy | Show “voice status unavailable” and the failure category. Never show a green/healthy default. |
| Duplicate sequence with identical content | Ignore as duplicate | No state change; diagnostic counter only. |
| Sequence reuse with different content, replay, gap, invalid epoch, or stale source time | Mark that source state unknown/incomplete; do not merge across epochs | Show stale/incomplete state and last accepted source/receive times. Do not imply voice service failure or success. |
| Position/resource/configuration mismatch or conflicting sources | Preserve both source-attributed observations; refuse the join | Show “mapping conflict” or “mapping unknown”; do not pick the newest or most convenient value silently. |
| Network partition, process crash, queue full, or adapter restart | Drop no event silently; count bounded losses, terminate/clear uncertain state, reconnect only per the approved policy | Show unavailable/unknown. The approved voice equipment and its normal controls continue independently. |
| Reported service loss or reported PTT state | Display only if the interface defines the value, scope, time, and quality; distinguish source report from measured end-to-end service | Label the state as reported by the named source. A missing PTT field is “not provided,” not “not keyed.” |

Staleness is measured using source time only when its clock basis and quality are valid; receipt time remains available for diagnosing delivery delay. The three-second target above governs feed availability; if source time quality is invalid, display observations as time-unverified and do not use them as current status. Confirm or revise the target against measured cadence, latency, clock uncertainty, and operational review before implementation. Adapter loss must not interrupt, delay, or alter voice calls.

No new alarm, sound, or controller-priority alert is proposed. The first UI should be a passive, clearly labeled status panel with source, configuration version, state age/quality, and unavailable/conflict conditions. A status label must not imply control authority, end-to-end audio quality, or controller action. Any future alert would need a separately accepted meaning, priority, threshold, workload assessment, and failure behavior.

## Verification strategy

Before connecting to any real system, build a deterministic simulator and contract test harness from the approved interface description. Use synthetic identities and states only. The harness should cover:

1. Supported and unsupported contract versions, schema bounds, malformed messages, unknown enumerations, and the byte/item/queue caps.
2. Authentication and authorization separately, including an authenticated but unapproved source.
3. Duplicate, out-of-order, replayed, conflicting-same-sequence, missing-sequence, epoch-reset, and snapshot-recovery cases.
4. Stale/future timestamps, invalid clock quality, separate event/receipt time, and configuration-version changes.
5. Missing, duplicate, and conflicting position/resource mappings; ensure no field or identifier is silently substituted.
6. Partition, restart, queue exhaustion, malformed input, and adapter shutdown while a separate mock voice service remains callable and unchanged.
7. UI state for unknown, stale, conflicting, reported loss, and reported PTT; confirm no green default, control action, audio path, or unsupported end-to-end claim.

After contract testing with synthetic data, validate against the approved system simulator or test endpoint for the exact site/release/configuration. Measure timing and loss behavior and demonstrate independent voice operation under adapter failure. Complete facility/system authority review and field acceptance before making an operational integration claim.

## Evidence and decision rationale

- FAA Order 6690.4 (1993) §33 describes a historical VSCS-to-ACCC processor interface and identifies NAS-IC-21024201 Parts I and II. It describes historical design intent, not proof of current installation, availability, or authorization.
- The same order names historical interfaces and configuration artifacts including VP 26, ATR-320, and VP 41; it says its deliverable list is incomplete. These references support the need for a site-specific configuration and current controlling documents, not use of those artifacts as an assumed current contract.
- The FAA VS&R program overview describes modernization context and planned capabilities. It is not an external API or site deployment specification.
- FAA controller procedures establish position responsibilities and facility practices, not a machine-readable voice mapping or external data contract.
- The FAA public order listing identifies current maintenance-order status but is not an interface control document.

The design choice is a read-only, process-isolated adapter with explicit provenance and fail-unknown display behavior. Evidence that would change it includes an approved ICD requiring a different safe boundary or an authority-approved architecture that specifies another process boundary.

## References

- FAA, [Voice Switch & Recorder Program](https://www.faa.gov/air_traffic/technology/cinp/vsrprogram) — program-level modernization context.
- FAA, [Order 6690.4, VSCS Project Implementation Plan](https://www.faa.gov/documentLibrary/media/Order/6690.4.pdf) (1993) — historical interface and configuration references, not a current ICD.
- FAA, [JO 7110.65, Team Position Responsibilities](https://www.faa.gov/air_traffic/publications/atpubs/ATC_html/chap2_section_10.html) — controller position procedures.
- FAA, [Facility Administration procedures, Chapter 2](https://www.faa.gov/air_traffic/publications/atpubs/foa_html/chap2_section_2.html) — facility position and equipment status procedures.
- FAA, [Orders and Notices listing for AJW-154](https://www.faa.gov/regulations_policies/orders_notices/index.cfm/go/document.list/?appliedFacets=%7B%22officenumber%22%3A%22AJW-154%22%7D) — public order listing, not a system interface definition.

## Limits and open questions

The public references do not identify the current KORD/ARTCC voice system, software release, interface inventory, connected-system topology, site resource configuration, or a third-party read-only status API. They do not establish whether current VSCS, ETVS, or VCS exposes position/resource assignments, PTT, health, or configuration version to OpenATCA. The vendor-to-adapter protocol, endpoint, source identity, field encodings, time semantics, update cadence, and recovery/snapshot behavior therefore remain unknown; no vendor wire format is proposed here.

Next, the responsible engineering authority must provide the controlling ICD and site/release baseline, and identify the authorized test endpoint. The system/security owners must confirm read-only scope, source identity, permitted fields, and mapping authority. Then compare the adapter’s normalized field map, timestamp/epoch translation, queue caps, and heartbeat targets against the ICD and measured normal/peak traffic. Facility operations must define the intended status use and acceptance evidence. If the approved interface cannot support a bounded read-only feed, retain the independent voice boundary without an adapter.
