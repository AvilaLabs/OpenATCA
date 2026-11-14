# Interface discipline

Status: adopted 2026-11-14. Applies to every adapter from M1 onward: anything that receives
from, or sends to, a system we do not control.

## Why

The systems OpenATCA connects to are older, slower, differently built and not ours to fix. Large
ATC outages rarely come from one cause. They come from a **trigger** (a burst, an unusual
message, a version mismatch, a maintenance action) meeting an **amplifier** (retry storms, state
dumps on reconnect, fan-out, identical backups, timeouts that turn slowness into failure).

| Event | Trigger | Amplifier |
|---|---|---|
| Los Angeles Center ERAM, 2014-04-30 | One flight plan with an unusual high-altitude route | The system computed conflicts across a vast area and exhausted memory |
| UK NATS flight plan processing, 2023-08-28 | A flight plan with two different waypoints sharing a name | The backup received the same plan and failed the same way |
| FAA NOTAM system, 2023-01-11 | A damaged database file | The backup was synchronised from the damaged primary |
| FAA nationwide ground stop, 2026-11-13 | New equipment feeding more data into old routers (officials' account; root cause not yet published) | Inconsistent data spread to radar and communications across facilities |

Retry and reconnect storms can keep a system overloaded after the trigger is gone. The research
literature calls these metastable failures (Bronson, Aghayev, Charapko and Zhu, HotOS 2021).

Our software being correct is not enough. It must never be the trigger or the amplifier for a
system next to it, and it must not be taken down by one.

## Rules for sending

- **IF-S1 Traffic contract.** Every outbound interface has a written contract: maximum message
  rate, maximum burst, maximum message size, and the message types and versions we may send.
  The adapter enforces it internally, smoothing bursts into evenly paced output. Exceeding the
  contract is impossible by construction, not merely unlikely.
- **IF-S2 Contracts come from measurement.** Limits are set below what the receiving system has
  been observed to handle in shadow mode, never from a guess about its capacity.
- **IF-S3 No retry storms.** Retries back off exponentially with random spread, are capped in
  number, and count against the traffic contract.
- **IF-S4 No resynchronisation dumps.** After a reconnect, state is resent at the contract rate,
  most important first. A reconnect must never look like a flood.
- **IF-S5 Send conservatively.** Use the oldest message form the receiver is known to accept,
  stay within the sizes and field usage it has already seen, and introduce anything new only
  after the legacy system has handled it in shadow mode.
- **IF-S6 Bounded fan-out.** One input produces a bounded number of output messages and a bounded
  amount of work, stated per message type.

## Rules for receiving

- **IF-R1 Bounded work per input.** Every message type has a stated maximum cost in time and
  memory. An input that would exceed it is rejected and reported, never processed partly.
- **IF-R2 Poison-message quarantine.** If an input causes a component to fail or stall, that input
  is recorded and held back when the component restarts, so it cannot fail the restarted
  component or the backup again.
- **IF-R3 Redundancy that does not share a blind spot.** A backup either runs independently built
  logic for the same function, or does not receive an input until the primary has processed it
  successfully. One input must never be able to take out both.
- **IF-R4 Slow is not dead.** Each feed has measured normal latency and rate. Slowness is reported
  as degradation, separately from loss, and timeouts come from measurements, not defaults.
- **IF-R5 Refuse and flag.** Invalid or inconsistent data is refused and flagged, never passed on
  (RULESET E5). A source disagreeing with independent sources is quarantined, not averaged in.
- **IF-R6 Health is an output.** Every feed continuously reports rate, latency, sequence gaps,
  duplicates, reordering and agreement with independent sources, and alarms on trends before
  outright loss.

## Before connecting to anything real

- **IF-P1 Passive characterisation.** Every legacy interface is first observed only: real rates,
  bursts, sizes, timing and error behaviour, recorded for replay.
- **IF-P2 Emulators with bad behaviour.** Each legacy interface gets an emulator that reproduces
  its recorded behaviour and its failure modes: slow reads, stalls, disconnects, garbage,
  duplicate and reordered messages, and lower capacity than observed. Every adapter is tested
  against it in replay, including deliberate overload, and must never make a degraded neighbour
  worse.
- **IF-P3 Replay of real incidents.** Each event in the table above, and each new one, becomes a
  replay scenario our system must pass: it neither causes nor amplifies the failure and keeps
  its own outputs consistent.
