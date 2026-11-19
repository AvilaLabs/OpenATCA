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
- **Independent checking.** Facilities fed the same inputs reach the same state bit for bit, so a
  facility whose state differs is found within seconds.
- **Local survival.** A facility cut off from the shared network keeps working on its own sensors.

Control authority is not part of the network. Exactly one position is responsible for each
aircraft, and responsibility moves only by explicit handoff. No vote, quorum or consensus
protocol decides who controls an aircraft or sits in any real-time path.

## 2. What a node is

A node is the OpenATCA engine fed a wider set of inputs. The core does not change. Running
OpenATCA joins no network by default; joining is a separate, configured step, and credentials
decide which network a node may join.

| | Facility node | Observer node |
|---|---|---|
| Who | Authorised facilities, keys issued by the aviation authority | Anyone who opts in |
| Inputs | The authority's surveillance and flight data feeds | Public ADS-B only |
| Purpose | Resilience and cross-checking for live operation | Public verification and large-scale testing |
| Effect on any aircraft | Advice to the people in charge | None |

Every node keeps its own record and publishes checkpoints. Nothing else is shared routinely.

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

A checkpoint that differs from its peers is classified by its input hash first:

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
3. Raw surveillance only (the role ERAM's EBUS and STARS's emergency service level play today).

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
  torn writes, bit flips and a node that sends different checkpoints to different peers. The
  comparison itself is the state checker, and every failure replays from its seed. This is the
  method of FoundationDB's simulator and TigerBeetle's VOPR.
- **Protocol model.** A TLA+ model of the checkpoint, isolation and rebuild protocol.
- **Proofs.** Kani proofs on the Merkle, signature-envelope and parsing code, as for the rest of
  the project (docs/VERIFICATION.md).
- **Monitor mutation study.** Bugs are planted in the tracker and the share the safety monitor
  catches is measured; the bugs it misses are reported as prominently as the ones it catches.
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
| A facility can take over a neighbour's airspace | Replayed takeovers: time to a working picture, hand edits needed | The weeks of the 2014 Chicago fire |
| Poison inputs do not stop the system | Every recorded crashing input replayed; time to quarantine | Restart-and-reprocess |
| False alarms are rare | Mismatches raised on fault-free replays and live shadow | Zero |

No availability figure or failure probability is derived from the number of nodes.

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
- DOT Office of Inspector General audit reports on ERAM, including AV-2012-179 and AV-2020-040.
