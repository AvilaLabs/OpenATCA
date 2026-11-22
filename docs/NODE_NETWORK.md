# Node network design

Status: design, 2026-11-19. Nothing in this document is built yet except what it depends on: the
deterministic engine and the replay state hash. SCOPE.md design principle 8 states the principle;
this document is the design behind it. Every part is borrowed from technology that is already in
production elsewhere, and each section names where it comes from.

## 1. Why a node network

If the whole NAS fits on one laptop (M9), the network between facilities no longer exists to
divide the workload. It can exist for three other things:

- **Resilience.** Every facility can compute the national picture, so a facility can take over a
  neighbour's airspace without first building a picture of it.
- **Independent checking.** Facilities fed the same inputs are to reach the same state bit for bit,
  so a facility whose state differs is found within seconds. Today that holds only for the same
  build on the same kind of machine: the tracker crates are still `unsplit` (RULESET B9) and call
  the platform maths library, whose last bit can differ between CPUs and C library versions. It
  holds across machines once those crates are core (B7), which keeps them off platform maths.
- **Local survival.** A facility cut off from the shared network keeps working on its own sensors.

Control authority is not part of the network. Exactly one position is responsible for each
aircraft, and responsibility moves only by explicit handoff. No vote, quorum or consensus
protocol decides who controls an aircraft or sits in any real-time path.

### What exists today, and what is new

Redundancy and interfacility backup already exist, and the design is measured against them, not
against their absence:

- Each ERAM centre runs two functionally identical channels (FAA, ERAM programme page). The
  separate backup of the Host era, EBUS, was removed after a 2019 safety evaluation and not
  replaced (DOT OIG AV-2020-040).
- FAA Order JO 1900.47G (effective 30 April 2024) requires every facility to keep contingency
  plans that move airspace, frequencies, landlines and surveillance feeds to supporting
  facilities, with control maps and automation adaptations documented well enough that
  technicians can verify the supporting facility is capable. The order does not say the
  adaptations are prepared in advance and loaded.
- After the Chicago Center fire of 26 September 2014, the four neighbouring centres had to modify
  their site-specific ERAM software to take its airspace; the OIG found that ERAM "does not
  automatically enable one Center to control another Center's airspace", and that the lack of
  automatic flight-plan transfer kept some departures waiting 20 to 30 minutes (DOT OIG
  AV-2015-112). We found no public record of an ERAM capability added since then that changes
  this, which is not proof that none exists.

What the design adds is therefore narrow and testable: the automation for any airspace is
already running, verified and current at the supporting facility before it is needed, so a
transfer of responsibility needs no change to the supporting facility's automation; and machine
faults are found by comparing independently computed state.

## 2. What a node is

A node is the OpenATCA engine fed a wider set of inputs. The core does not change. Running
OpenATCA joins no network by default; joining is a separate, configured step, and credentials
decide which network a node may join.

| | Facility node | Observer node |
|---|---|---|
| Who | Authorised facilities, keys issued by the aviation authority | Anyone who opts in |
| Inputs | Surveillance and flight data from their sources, not another facility's output | Public ADS-B only |
| Purpose | Resilience and cross-checking for live operation | Public verification and large-scale testing |
| Effect on any aircraft | Advice to the people in charge | None |

Every node keeps its own record and publishes checkpoints. Nothing else is shared routinely.

### Independent inputs

A facility node takes its inputs from their sources (radar and ADS-B surveillance, flight plans
as filed and amended), never from another facility's automation output. The flight data that
SWIM publishes today (SFDPS) is produced by ERAM at each centre and distributed through each
centre's EDDS/HADDS (FAA, SFDPS page); a node fed by it loses exactly the data it needs when that
centre's automation fails. Research use of SWIM output for comparison is unaffected. Getting
flight data from its source is harder than getting surveillance, and is an open item
(docs/UNKNOWNS.md, UK-007).

### Three kinds of state

| State | What it is | How a node holds it |
|---|---|---|
| Surveillance | Where each aircraft is and how it is moving | Computed from surveillance inputs |
| Flight data | Route, amendments, assigned and interim altitudes, restrictions, coordination | Inputs to the engine: the filed plan plus every amendment and coordination message |
| Control | Instructions issued and acknowledged | Only as far as controllers enter them |

While OpenATCA runs in shadow beside ERAM, controller entries go into ERAM, and a node sees them
only through ERAM's output, so a shadow node's flight-data state is incomplete by construction.
Once OpenATCA is the operational system, every controller entry is an input to the deterministic
engine and is replicated with the rest. An instruction given by voice and never entered is held
by no automation, this one included; the takeover experiment measures how much of the control
state that leaves out.

### The core/shell boundary

The design depends on RULESET section B, which the gate checks mechanically (B7 to B9):

- **Comparison needs a pure core.** Nodes fed the same inputs reach the same state only if the
  core is a pure function of its inputs. A clock read or a randomly seeded hash map in the core
  would make healthy nodes disagree, and every mismatch alarm would be noise.
- **The boundary defines the record and separates the fault classes.** The values the shell
  hands to the core are the inputs a node logs and hashes; the core's state is what it hashes
  and compares. A difference in inputs is a data-path fault in the shell; the same inputs giving
  a different state is a compute fault (section 6).
- **It is what makes the defences against code faults practical.** Inputs entering the core one
  at a time through one boundary let the shell identify and quarantine the input that stops the
  core. Small pure functions are what formal proof, mutation testing and a narrow second
  implementation can cover. The safety monitor is itself a separate core module. A core with no
  operating-system dependence runs unchanged on different compilers, hardware and operating
  systems, which is what platform diversity needs (section 7).
- **It does not catch a wrong requirement.** A pure, proven, replicated core computes a wrong
  specification identically everywhere. Incident replays and review by controllers are the
  defence against that.

## 3. The record

Each node keeps an append-only Merkle tree over every input it consumed and every output it
produced, signed by the node. This is the log structure of Certificate Transparency (RFC 9162):
an inclusion proof shows a given entry is in the log, and a consistency proof shows a later log
extends an earlier one without rewriting it. An investigation can then rely on the record: an
entry cannot be removed or altered without the change being detectable.

An observer node can publish its log as static tiles, as the Tessera log implementation does, so
serving it needs no special server.

Optionally, a daily digest of a facility's log can be anchored in a public timestamping chain
(for example Bitcoin through OpenTimestamps), giving anyone proof that the record existed in that
form on that date.

## 4. Checkpoints

A checkpoint is a signed note in the C2SP format carrying: the node's identity, the log size and
root, the comparison window, a hash of the inputs consumed in the window and a hash of the
resulting state. It is a few hundred bytes. Nodes publish one per region every few seconds.

Checkpoints for a window are compared only after the tracker's late-arrival window has closed.
Reports are ordered by measurement time, not arrival time, so two nodes that received the same
reports in a different order still reach the same state once every report is in.

## 5. Peer witnessing

Every facility node acts as a witness for every other, following the C2SP tlog-witness protocol.
It keeps the last checkpoint it accepted from each peer, demands a consistency proof before
accepting the next, countersigns it, and keeps any refused request as evidence. A node therefore
cannot show one history to one neighbour and a different history to another. With known
membership this needs no outside witness network.

## 6. Comparing, isolating and rebuilding

For offline replay triage, `oa-replay --input-fingerprint` can emit a bounded ordered-input
digest alongside the existing state digest. It hashes validated metadata, replay stream identity,
and payload bytes after the frame and document sealed-window checks. It counts source framing
and gzip refusals, pipeline errors, and sealed drops separately. The result is partial if the
replay encounters a refusal or a drop, and incomplete if it stops on an error. The recorded
receive and monotonic timestamps are input fields. Compare complete local runs with the same
replay settings and stream grouping. A frame dropped by a document guard may still advance the
replay clock using its accepted receive time, so a partial digest cannot explain every state
difference. Peer authentication and complete delivery require separate evidence.

The input hash determines how a disagreement between peer checkpoints is classified.

- **Different inputs:** a data-path fault (loss, delay beyond the window, a feed problem). The
  node fetches what it missed and replays from the last agreed checkpoint.
- **Same inputs, different state:** a compute fault (hardware, memory corruption, a bad install).
  Large fleets show such faults are real: Google's and Meta's published studies of silent data
  corruption found CPUs that compute wrong answers without crashing, some of them consistently,
  so that re-running on the same machine reproduces the wrong answer. Only comparison with a
  different machine catches those.

Isolation follows the same rule as control: the people responsible decide, informed by the
evidence, once at least two peers that agree with each other confirm the mismatch. States are
never averaged; for a given set of inputs exactly one state is correct.

A node is rebuilt in the manner of Bitcoin Core's assumeutxo: it accepts a snapshot only if the
snapshot's hash matches a peer-countersigned checkpoint at that input position, replays its signed
input log from there, and runs in shadow until it has matched a set number of consecutive
checkpoints before it is trusted again.

### When the faulty machine is the one in use

A machine that keeps disagreeing on the same inputs is a hardware or installation fault, not a
dispute about the airspace: any other machine can replay those inputs and show the correct state.
What follows depends on whose machine it is.

- **Another facility's machine, or an observer's.** Nothing changes for this facility. The owner
  receives the evidence: which machine, since which checkpoint, and which tracks and fields
  differ, from the replay comparison (`oa-replay-diff`).
- **The machine feeding this facility's controllers.** Its picture is in doubt from the first
  failed comparison. Four things happen:
  1. **The controllers see the state, not a diagnosis.** A persistent indication that cannot be
     dismissed while it lasts ("PICTURE UNVERIFIED: PROCESSOR MISMATCH"), under the same rule the
     safety nets follow: a system always says what state it is in (docs/M5_DESIGN.md, rule 1).
  2. **Technicians receive the evidence.** It says how far the states differ, because one wrong
     field in one track and a whole picture diverging call for different urgency.
  3. **The picture moves to a machine that agrees with its peers.** That is the facility's other
     machine, or a neighbouring facility that already computes this airspace. Every facility
     therefore runs at least two machines, on different hardware where it can, so a mismatch
     never leaves it choosing between two pictures alone. The switch is logged, and the
     controllers see it happen.
  4. **With no machine that agrees, the facility steps down the fallback ladder (section 8)**, and
     each step is shown.

  The faulty machine is then rebuilt as above, and it serves no position until it has matched
  its peers for the set number of checkpoints.

Detection comes at the first comparison after the late-arrival window closes, so within seconds.
A machine that computes wrong without crashing is otherwise found only by fleet-wide testing,
which is how Google and Meta found theirs. The test is the fault-injection run in section 11: a
machine made to compute wrong on every cycle, with the time to detect, the time to a verified
picture on screen and the number of wrong frames shown measured.

## 7. What replication does not catch

Identical deterministic nodes agree on a shared bug, and they fail together on a shared poison
input. The UK's NATS lost its flight server in 2014 on a latent limit (151 where 193 was needed)
present in both its primary and its backup; Ariane 501's two inertial reference units ran the same
software and failed the same way. Agreement between nodes proves the software ran correctly, not
that it is correct, and not that its inputs were true.

Independently written versions of a program are not a cure. Knight and Leveson's 1986 experiment
rejected the hypothesis that separately written versions fail independently, and later
replications, including a 2026 study using AI coding agents in several languages, found the
same. Voting between versions still reduces failures on average, but by an amount that varies
and that cannot be multiplied out into a probability.

Code faults are therefore met with diversity, in this order of evidence:

1. **An independent safety monitor on every node.** A small, separately written checker bounds
   every output track against the raw measurements, physical limits, update ages and table
   limits. This is the Simplex runtime-assurance pattern: a simple, heavily verified component
   supervising a complex one. It shares no code with the tracker and is verified far more
   thoroughly.
2. **A raw-surveillance path** to the display that does not pass through the tracker.
3. **Platform diversity:** different compilers, hardware and operating systems across nodes, and
   staggered restarts, so a time-dependent fault (such as a counter that overflows after a fixed
   uptime) cannot strike every node at once.
4. **A second implementation of narrow, well-specified pieces only** (for example the safety
   nets), used as a differential-testing oracle, never counted as an independent vote.
5. **Version diversity during upgrades:** some nodes stay on the previous release, so a new
   release that behaves differently is caught by comparison.

Two further rules:

- **Poison input.** An input that crashes or stalls processing is quarantined and the node runs
  on without it. It is never reprocessed on restart, and it becomes a permanent test.
- **Limits at their maximum.** Every configured limit is tested at its maximum and one past it,
  and uptime is tested past every counter width, as a standing check. NATS 2014 passed full
  decision coverage; no test used the configured maximum.

## 8. Fallback ladder

1. Full picture.
2. Picture annotated by the safety monitor.
3. Raw surveillance only (the role the Host-era EBUS backup played for en route before its removal,
   and STARS's emergency service level plays for terminal).

Every step down is logged and counted.

## 9. Time

Comparison windows need clocks that agree. Each node takes authenticated network time (NTS,
RFC 8915) from several independent sources, carries it through outages on a local oscillator, and
checks its clock against the times in its peers' checkpoints. Where trustworthy time comes from
when GNSS is jammed or spoofed is open (docs/UNKNOWNS.md, UK-005); NTS does not stop delay
attacks.

## 10. Public observer network

Anyone can run an observer node on public ADS-B data: from an aggregator such as adsb.lol or from
their own receiver. Observer nodes on the same inputs publish checkpoints, and anyone can check
that they agree, which is public evidence that the published software computes what it claims.
Different hardware, operating systems and local traffic also make the observer network a large
test bed.

Observer nodes never connect to operational systems, never carry authority feeds, apply the LADD
block to anything they display or publish, and say on screen that they are not for navigation or
air traffic control. Operators gain a tracked picture of their own area and health statistics
for their own receiver; there is no token or payment.

## 11. Testing plan

- **Deterministic simulation of the shell.** The unmodified engine runs under a simulated network,
  disk and clock driven by a seed, with injected message loss, delay, partitions, clock skew,
  torn writes, bit flips, a node that sends different checkpoints to different peers, and a
  node whose processor computes wrong on every cycle (section 6). The
  comparison itself is the state checker, and every failure replays from its seed. This is the
  method of FoundationDB's simulator and TigerBeetle's VOPR.
- **Protocol model.** The bounded TLA+ slice in `docs/tla/RecoveryProtocol.tla` checks verified
  checkpoint acceptance, replay-watermark fencing and crash/restart recovery. Its README states
  the model bounds and limits. Isolation and rebuild remain outside this model's scope.
- **Proofs.** Kani proofs on the Merkle, signature-envelope and parsing code, as for the rest of
  the project (docs/VERIFICATION.md).
- **Monitor mutation study.** Bugs are planted in the tracker and the share the safety monitor
  catches is measured; the bugs it misses are reported as prominently as the ones it catches.
- **Unannounced takeover experiment (the headline test).** A facility's node is cut off without
  warning in a replay of real traffic. A neighbouring node must continue the affected airspace
  with its current surveillance and flight-data state and no change to its configuration.
  Measured: time to a working picture, flight-data state missing or wrong at takeover, entries a
  controller would have to re-make, and checks failed. Compared against the three architectures
  in section 12a, not only against 2014.
- **Incident replays.** Each real incident in docs/LEGACY_SHORTCOMINGS.md and the migration
  record is replayed or assessed: did the node setup prevent it, reduce it, or make no difference,
  and why. Among them: the Chicago Center fire (September 2014), the Los Angeles Center ERAM
  outage (April 2014), the NATS failures of 2014 and 2023, the loss of the remote radar feed for
  Philadelphia's Newark sectors (2025), and the 13 November 2026 network flood.

## 12. What may be claimed, and how it is measured

| Claim | Measured by | Against |
|---|---|---|
| Faulty machines are detected | Detection rate and time to detect under injected hardware faults | A single node with no peers |
| Code faults that identical nodes share are caught | The monitor mutation study: share caught and share missed | Replication without a monitor |
| A facility can take over a neighbour's airspace with no automation change | The unannounced takeover experiment | A prepared regional standby, and the current contingency procedure (JO 1900.47G) |
| Poison inputs do not stop the system | Every recorded crashing input replayed; time to quarantine | Restart-and-reprocess |
| False alarms are rare | Mismatches raised on fault-free replays and live shadow | Zero |

No availability figure or failure probability is derived from the number of nodes.

## 12a. Architectures compared

National replication at every facility is a hypothesis, adopted only if it beats the cheaper
options on these measurements:

| Architecture | What it is |
|---|---|
| Facility-specific | Today's pattern: each facility runs its own airspace; contingency plans move airspace to supporting facilities |
| Regional standby | Each facility also runs, continuously and verified, the airspace of the neighbours it would support |
| National | Every facility runs the whole NAS |

Compared on: the takeover experiment; failure scenarios a single neighbour cannot cover (two
adjacent facilities lost, a supporting facility itself degraded); fault identification, where
naming which node is wrong needs at least three nodes computing the same airspace and a regional
pair can only report that they disagree; and cost in distribution bandwidth, compute, the number
of sites holding national data (a security exposure), and configuration management.

## 12b. Storage by role

Processing is deterministic, so a node stores its inputs and the hashes of its outputs; outputs
are regenerated by replay. The checkpoint chain is kept permanently and is a few megabytes a
year. Raw inputs are kept for the retention period and then pruned, except windows tied to an
incident.

| Role | Storage needed (estimates) |
|---|---|
| Facility node | The retention period of national input; at about 10 GB a day, 45 days is under 0.5 TB |
| Archive (one or two nationally, if the authority wants long-term history) | A few TB a year uncompressed |
| Public observer, regional | A rolling day of its own area or less: a few GB |
| Public observer, national | A rolling day: about 5 to 10 GB |
| Developer or researcher | The recordings they choose: one national day is about 5 to 8 GB |

The national ADS-B figure is measured (1.32 GB for the four peak hours of 17 November 2026,
about 80 bytes a report); the radar share is estimated until radar is recorded. Unpruned, a node
would grow far faster than a Bitcoin node does; nothing in the design requires keeping
everything at every node.

## 13. Deliberately left out

- Consensus by vote or by computational work, and any blockchain. Membership is known and
  authenticated, so classical fault tolerance applies; proof-of-work exists for open membership.
- Byzantine agreement protocols (PBFT and its successors). Nodes compare results; they do not need
  to agree on a single input order. A crash-fault protocol is a held option only if a single
  agreed input order across facilities is ever required.
- Any new dependency in the core. Node features live in the shell, each added only when a
  milestone needs it.

## 14. Limits and open questions

- A transient fault that heals before the next checkpoint is missed; output comparison at a finer
  grain may be needed.
- Published fault-rate studies come from server fleets and define their rates differently; no
  probability of undetected corruption is quoted from them.
- Moving national inputs to every facility must respect the traffic-contract rule (SCOPE
  principle 2a); multicast distribution, not one copy per site.
- Taking over airspace also needs radio frequencies, radar feeds routed to the new site, and
  controllers trained on that airspace. The node design removes the automation and adaptation
  part of that problem only.
- Whether checkpoints for non-public data may be published at all is a policy and export
  question; facility checkpoints stay within the operational network unless the authority says
  otherwise.

## Sources

- RFC 9162, Certificate Transparency version 2.0; C2SP signed-note and tlog-witness
  specifications; Tessera (transparency-dev).
- RFC 8915, Network Time Security for NTP.
- Bitcoin Core, assumeutxo design document.
- J. Zhou et al., "FoundationDB: A Distributed Unbundled Transactional Key Value Store", SIGMOD
  2021; TigerBeetle, VOPR simulator and TigerStyle documentation.
- P. Hochschild et al., "Cores that don't count", HotOS 2021; H. Dixit et al. (Meta), "Silent Data
  Corruptions at Scale".
- J. Knight and N. Leveson, "An experimental evaluation of the assumption of independence in
  multiversion programming", IEEE Transactions on Software Engineering, 1986.
- L. Sha, "Using simplicity to control complexity", IEEE Software, 2001 (the Simplex architecture).
- UK CAA, NATS System Failure 12 December 2014, independent enquiry final report, May 2015; UK CAA,
  independent review of the 28 August 2023 flight plan processing failure (CAP2993).
- DOT Office of Inspector General audit reports on ERAM, including AV-2012-179 and AV-2020-040;
  AV-2015-112, "FAA's Contingency Plans and Security Protocols Were Insufficient at Chicago Air
  Traffic Control Facilities", 29 September 2015.
- FAA Order JO 1900.47G, Air Traffic Control Operational Readiness and Contingency Planning,
  effective 30 April 2024.
- FAA, En Route Automation Modernization (ERAM) and SWIM Flight Data Publication Service (SFDPS)
  programme pages.
