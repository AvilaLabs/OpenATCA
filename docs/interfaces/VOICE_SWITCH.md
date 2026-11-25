# Voice-system integration design

**Status: DRAFT — research and boundary design only.** No voice-system interface, adapter, connection, recorder access, operational integration, or qualification is implemented by this document.

## Purpose and acceptance boundary

Controller voice communications are a separate, safety-critical system boundary. A voice switch provides access to air-to-ground (A/G) and ground-to-ground (G/G) communications; it is not an OpenATCA data feed by default. Being installed at a facility that uses a voice switch does not establish system integration.

The relevant operational acceptance criterion requires a deployment that includes legacy surveillance, flight-data processing, and voice communications. This design and the public references cited below do not demonstrate that deployment. A passive display of voice-system availability, if authorized and supported by a current interface, must not be represented as satisfying the criterion by itself.

## What the public references establish

### Historical VSCS and ACCC interfaces

FAA Order 6690.4, the 1993 VSCS Project Implementation Plan, describes a historical VSCS end-state. Section 33 says that a VSCS-to-Area Control Computer Complex (ACCC) processor interface could exchange digital data to coordinate ARTCC position-equipment configuration. It identifies NAS-IC-21024201 Part I for that interface and Part II for the common-console interface through the Voice Communication Equipment (VCE). The plan describes the processor interface as a product-improvement requirement intended after initial deployment.

The same order identifies historical VSCS interfaces to radios, trunks, backup A/G equipment, recording equipment, a coded time source, Network Management and Control Equipment (NMCE), and a maintenance processor. It also names configuration artifacts: VP 26, the site-by-site map-generation report for A/G and G/G assignments and capabilities per operational position; ATR-320, the VSCS site-configuration policy; and VP 41, the VSCS Interface Control Document (ICD). Section 57 says its list of contract deliverables is incomplete and directs readers to the VSCS contract for the full list.

These are documented historical design and document-number facts. They do not prove that a given interface was installed at any particular site, remains present in a current system release, uses the same data semantics, or is available to an external application. They do not authorize OpenATCA to connect. The current site baseline and controlling ICD revisions must be identified by the responsible system and facility authorities.

### Current program context

The FAA Voice Switch & Recorder (VS&R) public program overview describes legacy analog voice switches and a modernization program for IP-based Voice Communication Systems (VCS). It describes planned dynamic resource reconfiguration and an Air-to-Ground Protocol (APC) that interfaces with legacy analog and IP switches. This is program-level context; it does not define an external API, current site deployment, ERAM/STARS exchange, third-party access, or an OpenATCA interface.

FAA controller procedures assign operational radio and interphone duties among team positions and describe constraints associated with VSCS split functionality. Those procedures establish operational responsibilities, not a machine-readable position/sector/frequency schema. Fault-report examples that name sector, side, frequency, or calling/called location are maintenance reporting semantics, not proof that those values are exposed through a digital interface.

The FAA public order listing identifies JO 6690.3D as an active VSCS maintenance order. That listing is not an ICD and does not supply a current system or site interface definition.

## Unknowns that block interface selection

Before selecting an integration, the responsible authorities must identify:

1. The actual voice system, release, and site configuration in scope: VSCS, ETVS, VCS, or another system and facility variant.
2. The current interface inventory and controlling ICDs for the selected voice and automation systems, including revisions, changes, connected-system list, topology, security boundary, and permitted operating modes.
3. Whether any read-only status exchange is authorized, and its approved endpoint, transport, identity model, message fields, timing, update rate, authentication, and failure behavior.
4. The authoritative source for controller position, sector/side, console, radio resource, frequency, assignment, and configuration version. The historical map artifacts show that position-resource configuration is site-specific; they do not define a present external data contract.
5. What the acceptance authority considers successful voice-system “integration” and the evidence required to claim it in an operational deployment.

Until these questions are resolved, no system protocol or connection is selected. Do not reverse engineer an interface, connect to a voice network, infer an interface from network reachability, or treat an old ICD number as current approval.

## Safety boundary

The initial posture is that controllers continue to operate the approved voice system independently. OpenATCA must not select frequencies or radios, key or unkey a transmitter, assert PTT, route or mute calls, assign voice channels to positions, reconfigure voice equipment, or transmit ATC instructions. Controller operational authority and voice-equipment configuration/maintenance authority remain with their designated roles.

Do not capture, replay, transcribe, analyze, or associate voice audio with flight records. The FAA identifies the National Voice Recorder as a legally acceptable recorder; it is a separate system boundary and is out of scope.

If a current, authorized read-only status interface is later established, implement it in a separate shell adapter, not in the reusable core. Keep each observation attributed to its source, source time, receive time, voice-system identity, and configuration version when the ICD defines them. Do not infer PTT, frequency, assignment, or voice health from missing fields or display labels. Do not infer that a selected resource can transmit.

## Position, resource, and time semantics

- **Position and sector:** Sector, controller position, and voice position are not interchangeable identities by implication. Bind mappings to the facility and exact configuration version only when the controlling interface and configuration authority define those keys. Missing, duplicate, stale, or conflicting mappings remain unknown or ambiguous; do not silently relabel them.
- **Frequency and radio:** Treat frequency and radio selection as configuration and operator action, never as a control request from OpenATCA. Do not derive them from sector labels, public frequency plans, or a stale map.
- **PTT and audio:** No public source reviewed establishes an OpenATCA-readable PTT/key/unkey event. Keep transmit control and audio out of scope. Any future state indication requires explicit source-of-truth, timing, latency, quality, and failure semantics in the approved ICD.
- **Time:** The historical VSCS plan identifies a coded time source interface and describes synchronization/timestamping. That does not establish current clock accuracy or cross-system alignment. Preserve source time and receive time separately and use only the time contract approved for the selected interface.
- **Authority:** A mismatch between voice status and OpenATCA's flight or sector view is a conflict, not a condition to resolve by guesswork. Preserve the values and timestamps from both sources.

## Failure isolation

If an approved read-only interface is eventually authorized, missing, stale, malformed, or contradictory data must be reported as unknown/unavailable, never as healthy. The adapter must fail independently and must not interrupt voice calls, surveillance processing, flight-data processing, or controller operation. No new controller-facing alert should be introduced until its meaning, priority, stale threshold, and workload impact are accepted by the responsible operational authority.

The failure model must match the selected current system. Analog VSCS assumptions must not be carried over to a newer IP voice system without evidence from its ICD and acceptance plan. Any active operation—including resource selection, PTT, routing, reconfiguration, or failover—is a separate safety and security case and is not proposed here.

## Verification and operational evidence

Before any integration claim, an approved plan should identify the interface and site baseline, verify read-only behavior and source identity, demonstrate unknown/stale/conflict handling, and show that adapter failure leaves the independent voice service usable. Any operational deployment claim additionally needs facility-approved acceptance records, system safety/security review, operator and maintainer procedures, and field evidence for the designated voice system alongside the surveillance and flight-data integrations.

No test plan, thresholds, or integration results are claimed here. A successful parser or lab demonstration would not establish current site compatibility or operational acceptance.

## Public references

- FAA, [Voice Switch & Recorder Program](https://www.faa.gov/air_traffic/technology/cinp/vsrprogram) — current program overview and modernization context.
- FAA, [Order 6690.4, VSCS Project Implementation Plan](https://www.faa.gov/documentLibrary/media/Order/6690.4.pdf), 1993 — historical system interfaces, configuration artifacts, and implementation plan; not a current ICD.
- FAA, [JO 7110.65, Team Position Responsibilities](https://www.faa.gov/air_traffic/publications/atpubs/ATC_html/chap2_section_10.html) — controller position procedures.
- FAA, [Facility Administration procedures, Chapter 2](https://www.faa.gov/air_traffic/publications/atpubs/foa_html/chap2_section_2.html) — facility position and equipment status procedures.
- FAA, [Orders and Notices listing for AJW-154](https://www.faa.gov/regulations_policies/orders_notices/index.cfm/go/document.list/?appliedFacets=%7B%22officenumber%22%3A%22AJW-154%22%7D) — public status listing for current VSCS maintenance orders.
