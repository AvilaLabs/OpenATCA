# oa-nam

The North American Common Coordination interface between area control centres (ACC to ACC): the
message protocol ERAM speaks with the Canadian and Mexican centres, and the session layer it runs
over. Standard library only, no dependencies, no `unsafe`, no panics, no clock, no I/O. A system
built on it is meant to be indistinguishable from ERAM at the wire.

Implemented against:

* NAM Common Coordination Interface Control Document, NAS-IC-21009205, **Revision G**, 15 December
  2022 (messages, fields, error codes, workload).
* ERAM / NAM Non-US ACC System Interface Control Document, NAS-IC-82422100, and its Interface
  Requirements Document, NAS-IR-82422100 (transport framing, session states and messages).

Nothing from any other source is used. Where the documents disagree or are silent, the choice made
is in "Ambiguities and choices" below. **Every item in that section is marked for review**: it is a
place where this crate decided something the ICD did not. Validation runs under a `Profile`
(see "Validation profiles").

## What is implemented

| Layer | Module | Content |
|---|---|---|
| Messages | `model`, `field`, `schema`, `message` | All 25 messages of the Rev G core set (Part II Table 2): FPL CHG EST CNL CPL MOD ABI MIS IRQ IRS TRQ TRS ASM RTI RTU RLA RTA POI PLA POA POJ TOC AOC LAM LRM. Fields 03, 07, 08, 09, 10, 13, 14, 15, 16, 18, 22, 31, 32 and their elements, validated one by one. |
| Codec | `message::{parse, parse_tolerant, encode}` | One table of message definitions drives both directions. `encode` refuses an invalid value, and re-parses its own output and compares it with the input. |
| Transport | `frame` | The four-hex-digit length header, 17 to 1800 byte messages. |
| Session | `session` | INACTIVE / ACTIVATING / ACTIVE / DEACTIVATING as the pure function `step(config, session, event, now) -> (session, actions)`; message numbers 000 to 999; unanswered-message table; ASM keep-alive; IRQ retry; TRQ/TRS. |
| Errors | `error`, `lrm` | Every refusal carries the Table A-1 code and field a receiver would answer with in an LRM (`lrm::lrm`). The code tables (Table A-1, 1 to 91; Attachment 3, 92 to 94; Attachment 2, 99 to 116) are in `lrm::LRM_TABLE`. |
| Workload | `workload` | The ICD's rate, size and latency figures as data (below). |

Classification (RULESET B1):

| Module | Class | Why |
|---|---|---|
| `session` | core (pure) | `step(config, session, event, now)` is a function of its inputs, allocates nothing, fixed named capacities; checked under a counting allocator (C5, `validation/oa-alloc-check`, fixture `nam_session_activate_traffic_retry_full_deactivate`). |
| `message`, `field`, `model`, `schema`, `frame`, `lrm`, `error`, `workload` (the codec) | shell, input-format adapter (D1, C6) | Pure functions of bytes and values, no clock and no I/O, but they allocate per message (`Vec`, `String`), so they are not real-time core and are not held to C1 or C3. Their structs are cold (A7.2). |

A shell supplies bytes from a socket and the time, runs the codec, feeds the session the parsed
headers, and sends what the actions say.

### Round trip

* `encode(parse(b)) == b` for every `b` that `parse` accepts (checked on 60,000 mutated messages
  and in the fuzz target).
* `parse(encode(m)) == m` for every `m` that `encode` accepts (enforced inside `encode`).
* Where the ICD allows more than one spelling, the model stores which one, so the round trip holds
  (the Field 22 indicator `07` versus `7`). Everything else has one canonical form.

### Strict and tolerant

`parse` accepts only the canonical wire form. The ICD's own printed examples contain en dashes and
spaces beside hyphens (typography, not wire text). `parse_tolerant` drops a space next to a hyphen
and returns how many it dropped; the tests parse every printed example both ways. The tolerance is
spacing only; every field rule still applies.

### Not implemented

* The messages outside the core set (Part II Table 2 lists them; Attachments 1 to 7 add
  per-partner variants). The mnemonic of any other message is refused with Table A-1 code 60.
* The ICD's Field 18 `NAT` and ICAO 2012 versus 2008 format switch (codes 56, 67 to 70) are not
  modelled: the 2012 format is the only one parsed (Rev G deleted the old one, Table A-1 code 69).
* Semantic checks that need flight-plan or database state (duplicate ACID, active FPL, aerodrome
  and navaid existence, RVSM status, STS/ values: codes 7, 8, 80 to 83, 88, 90). The codec checks
  syntax, including Field 10 and its agreement with Field 18 (ambiguity 7).
* The transport itself (TCP over VPN, the load-balancer health words): the shell's job.
* The ICD v4 PDF's session diagram is blank in the copy available; Figure 3-3 of the IRD is
  implemented instead.

## Revision in operation

Which revision ERAM actually runs on this interface is **unconfirmed**. A 2023 ICAO NACC paper
reports the US operating on NAM ICD "Version E" (15 April 2016), not G. Rev G (15 December 2022)
is implemented as the default because it is the current text, and `Profile::RevE` covers the
differences below, so a Rev E partner can be validated against. Rev E and Rev G were compared
text against text. Part I (units), the field definitions of Part II section 2 and Part III are the
same apart from the two items marked (a) and (b); the differences that matter to a partner are:

| Item | Rev E | Rev G | In `Profile::RevE` |
|---|---|---|---|
| ASM 03(c) (Table 15) | a, b, c | a, b | reference required |
| POI 03(c) (Table 22) | a, b (examples have none) | a, b, optional c | reference refused |
| POA (Table 24, 3.7.3.1, B.1.8.5) | 07 a, b, c; acknowledgement, "requires no response" | 07 a (SSR optional here); LAM or LRM expected | SSR required; no response expected |
| POJ (Table 25, 3.7.4.1, B.1.8.6) | 03 a, b, c and Field 18 reason (`RMK/00/text` or `0`); no response | 07 a, 13 a, 16 a, 31 a, b optional c; LAM or LRM expected | Field 18 form; no response expected |
| Field 32(e) (a) | "ICAO format for a level" (metric allowed) | `S` and `M` not permitted | metric accepted (the field is then 38 bytes) |
| LRM error code width (b) | two digits | two or three digits | codes up to 116 are not used by Rev E |
| Error code 89 | "Deleted, duplicated message" | Unused | not emitted either way |
| Attachment 2 additions (block altitude, unknown Field 32, RTA 31(c), IRQ number) | none | in Attachment 2 | none, as `Core` |
| IRQ resend number (B.1.6) | same number | same number | same number |

The session machine differs only by `expects_response_in` (a POA or POJ is not left pending in
Rev E). Rev E examples with defects: the POA example is printed with the mnemonic `PLA`, and the
POJ example `-RMK /00/Unable PO Request` has a space inside the indicator; neither parses as
printed. The two Rev E POJ forms written canonically (`RMK/00/Unable PO Request`, `0`) are tested.

What a Rev E partner would still need that this crate does not do: the Rev E Attachments (the
per-partner variants, as for Rev G, are not implemented); a partner that sends Rev E POI
messages with a 03(c) or Rev G POJs is refused under `RevE`, by design. Nothing else in Rev G is
required of a Rev E sender, so a receiver that wants to be liberal can try `RevE`, then the
default, and log which matched.

## Validation profiles

`Profile::Core` is Part II of Rev G alone; `Profile::RevE` is Part II of Rev E alone (see "Revision in operation"). `Profile::EramAttachment2` (the default, since the
target partner is ERAM) adds what Attachment 2 agrees for the ERAM and NAV CANADA interface. The
entry points are `parse`, `parse_tolerant` and `encode` (default profile) and `parse_with`,
`parse_tolerant_with`, `encode_with` (explicit). The differences:

| Rule | `Core` (and `RevE`) | `EramAttachment2` | Source |
|---|---|---|---|
| Field 14(c) block altitude `AnnnBmmm`, `FnnnBmmm` | refused | accepted | Att. 2 2.6 |
| Field 32(c), (d) unknown (`N9999`, `99999`) | refused | accepted | Att. 2 2.15 |
| RTA Field 31(c) | refused | CPDLC contact `frequency/name/function`, up to 28 characters | Att. 2 2.17 |
| Field 31(c) of POI, PLA, POA, POJ, RLA | 20 alphanumeric | 20 alphanumeric | Part II 2.12 |
| IRQ retry number (`Config::for_profile`) | same number | new number | B.1.6, Att. 2 2.10 |
| LRM code for a bad message number | 4 | 57 | Att. 2 Table 2-2 |
| LRM code for a bad 03(c) in RTI, RTA, RLA, RTU | 5 | 57 | Att. 2 Table 2-2 |
| LRM field column for codes 17, 63 to 66 | Table A-1 | Table 2-2 (`lrm::lrm_field`) | Att. 2 Table 2-2 |

Attachment 2 also says the FAA sets Field 18 of TRQ and TRS to `0` (2.11, 2.12) and that no ASM
carries 03(c) (2.13.2). Those bind the sender; the parser accepts both a `0` and an `RMK/` Field
18 in a TRQ or TRS because the Part II examples carry `RMK/`. The 2.14 and 2.15 rules about not
sending RTI and RTU without a beacon code or altitude, 2.16 (no RTU from the FAA to NAV CANADA) and
2.8 (route start) need flight state or direction; they are the shell's.

## Ambiguities and choices (for review)

Every item is a place where this crate decided something the ICD did not, or where the documents
disagree. Items 1 to 7 are decisions of the coordinator after an independent review.

1. **POA example is misprinted.** Part II 3.7.3.3 prints `DLH499/ KSLC`: a slash and a space where a
   hyphen belongs. Rev F prints `DLH499/A4222-KSLC-...`, so Rev G's text is a half-edited leftover
   after Table 24 dropped the SSR elements of 07. Both parsers refuse the printed text; the
   corrected Rev G form (as in the POJ example) and the Rev F form are the vectors. Tests
   `poa_1_printed_with_a_defect_is_refused_and_the_corrected_form_parses` and
   `poa_with_the_rev_f_ssr_form_parses`.
2. **CHG: 13(b) and Field 18 optional only for the sole ACID change.** Table 4 lists both as
   required. The ICD's own example of an ACID change (3.1.2.3, Field 22 `07/` alone) has neither.
   They are optional exactly when Field 22 is one `07/` amendment and required otherwise
   (`message::post_checks`). The same inconsistency is in Rev F.
3. **POA 07(b)/(c) (SSR) optional; POJ has none.** Rev G lists 07 a only for both (Table 24, 25).
   Rev F listed more SSR elements for the POA (the copy read shows `a, b,` with the remainder cut
   off); a POA from a Rev F sender is accepted. A POJ with an SSR is refused.
4. **Field 22 indicator spelling.** The ICD prints `07/AAL73` (CHG) and `9/B744/H` (ABI). Both are
   accepted and the spelling is kept in the model.
5. **Field 22 rules.** In CHG and MOD a change of 07(a) is the only amendment (3.1.2.3). In ABI
   fields 9 and 15 are present (Table 9) and only 8, 9, 10, 15, 18 can be amended, each once, in
   any order (an earlier version required ascending order; the ICD does not).
6. **Example spacing and dashes.** See "Strict and tolerant". 16 of the 36 printed examples carry
   en dashes or spaces beside hyphens and need the tolerant parser. (CNL 13(b) optional is not an
   ambiguity: Table 6 says it is implemented as optional "to preclude rejection" when no EOBT is
   available.)
7. **Field 10 syntax** (codes 64, 65, 66, 71 to 76, 84, 86, 91). The ICD defers the values to ICAO
   Doc 4444 and says "all elements will be checked for syntax and correctness" (Att. 6 2.7). The
   rules implemented are this crate's reading of Doc 4444 (2012) and are marked for review:
   * 10(a): `A..D F..I K L N O R S T..Z`, `E1..E3`, `J1..J7`, `M1..M3`, `P1..P9`; no repeats (71);
     `N` alone and `S` not with `V`, `O` or `L` (75); anything else 73.
   * 10(b): `N` alone, `A`, `C`, at most one Mode S letter of `E H I L P S X`, ADS-B `B1 B2 U1 U2 V1
     V2` (65), ADS-C `D1 G1` (66), no repeats (72), at most one Mode S letter (76), else 74.
   * 10 against 18, only when the one message carries both: `N` with COM/, NAV/ or DAT/ is 64;
     PBN/ without `R` is 86; `R` without PBN/, `Z` without COM/, NAV/ or DAT/, and `G` without
     NAV/ is 91 (Att. 2 Table 2-2: "R, G, Z with no specification in field 18"). That `G` needs
     NAV/ in particular is an assumption. The ICD's own MOD example has NAV/ with no `Z`, so
     NAV/ without `Z` is accepted.
   * More than eight PBN values is 84, a malformed one 77.
   Not checkable without state, so not implemented: whether the aircraft really has what it
   declares, RVSM status against 15 (81), STS/ element validity (83), and the checks against an
   existing flight plan (duplicate ACID, active FPL, aerodrome and navaid existence: 7, 8, 80, 82,
   88, 90).
8. **IRQ received in INACTIVE.** Rev G B.1.6 says everything received when the interface is not
   initialised is ignored "except for IRQ"; the IRD Figure 3-3 and ERAM ICD 3.2.2.4 ignore
   everything in INACTIVE but the Activate Request. Default follows B.1.6: the IRQ is surfaced as
   `Action::InitRequested` (no IRS, since the system has not sent its own IRQ and so is "not
   ready"; no state change). `Config::inactive_ignores_irq = true` gives the IRD behaviour.
9. **Re-initialisation keeps pending messages.** On ACTIVE to ACTIVATING (a TRQ received, or an
   unanswered ASM) unanswered messages stay pending and time out normally, also while ACTIVATING;
   no document says they are abandoned. On INACTIVE (`ConnectionLost`, TRQ/TRS exchange, deactivate)
   they are abandoned and counted, because the numbering context ends.
10. **Wrong sender or receiver.** A message whose 03(b) names a unit other than the partner (code 1)
    or us (code 2) is counted (`Session::rejected`) and answered with `Action::SendLrm` (field 3;
    supporting text "MESSAGE REJECTED, MANUAL COORDINATION REQUIRED" in ERAM, Att. 2 Table 2-2),
    except in INACTIVE (ignored, counted) and except an LRM, which is never answered with an LRM.
11. **An LRM that answers nothing pending** (an RTU, an ASM, a timed-out message) is delivered as
    `Action::UnmatchedLrm` with its reference, not dropped (Table B-1 note). A LAM, RLA or PLA for
    nothing is still discarded as unmatched; a response whose 03(c) names other units is
    `ForeignReference`.
12. **Session timer values are not in the ICD** ("an adaptable time", "on the order of seconds").
    Defaults: ASM after 10 s of silence, 10 s to answer it, 10 s IRS timeout, 10 s TRS timeout, 60 s
    response timeout, no resends, no IRQ retry limit (the "Max IRQ Retries" of Figure 3-3 is
    optional). Dead-partner detection is then 20 s, inside the 60 s of NAS-IR-82422100 3.2.2.2-b
    (which also covers recovery, so the shell's reconnect time adds to it).
13. **IRQ retry number.** Rev G B.1.6 says a resent IRQ keeps its number; Attachment 2 2.10 states
    that on the ERAM interface it increments. The IRD is silent. `Profile::Core` keeps it,
    `Profile::EramAttachment2` increments (`Config::for_profile`); an IRS to any of the last eight
    IRQs is accepted either way.
14. **Connection loss** is not in Figure 3-3. `Event::ConnectionLost` goes to INACTIVE; the shell
    raises Activate when the connection returns (NAS-IR-82422100 3.2.2.6-c). Pending responses are
    abandoned and counted, and the inbound sequence baseline resets so the first message on the
    reconnected session is not compared with a message from the lost connection.
15. **Response matching.** A LAM answers anything but RTI and POI; an RLA answers an RTI; a PLA
    answers a POI; an LRM answers anything. RTU expects no response (Table B-1).
16. **Sequence numbers** of received messages are tracked only for messages the session
    processes (ACTIVE), not for ones ignored in other states.
17. **Metric levels (`S`, `M`) and speeds (`K`).** Accepted only where Part II marks the field as
    allowing metric forms; a Mach speed (`M084`) and a metric altitude (`M0840`) are told apart by
    the digit count. Field 32 never allows `S`/`M` (Att. 2 2.15).
18. **Levels are strict.** `A` below 18,000 ft and `F` from 18,000 ft (Part I 3.4), so `A180` and
    `F179` are refused.
19. **`VFR` as 15(b) is refused.** ICAO allows it, the crate requires a level: flights are IFR at
    the boundary (Att. 1 2.4, Att. 2 2.9).
20. **Free text in Field 18 containing ` XYZ/`** (three or four capitals and a slash after a space)
    is read as a new indicator, as the ICD's grammar makes it; a remark meant to contain that is
    refused or mis-split. The round trip is unaffected.
21. **Functional address charset.** 07(a) of a MIS may be `/` and one to six upper-case
    alphanumerics; the ICD gives the length but not the character set.
22. **RTU rate discrepancy between the two ICDs.** See the workload table.
23. **RTA 31(c)** presence (always when accepting, never when retracting) cannot be told from the
    message alone, so only its form is checked; presence is the shell's.
24. **Outstanding message numbers are unique.** The 000–999 sequence wraps, but a new ATM message
    that expects a response is refused with `Refusal::MessageNumberInUse` if its next number is
    still pending. The caller can retry after the response or timeout; the refused request consumes
    no number. This keeps transport response matching unambiguous. A LAM/other logical response is
    transport bookkeeping only; it does not change flight-sector ownership, which belongs to the
    application state machine.

## Workload and latency (from the ICDs)

Per flight, NAS-IC-21009205 Rev G Part III 6.3 Table 29 (planning estimates; sizes include an
estimated 50 bytes of communications header):

| Message | Per flight | Average bytes | Applies to |
|---|---|---|---|
| FPL | 1 | 240 | near-border departure |
| CHG | 0.5 | 160 | near-border departure |
| EST | 1 | 120 | near-border departure |
| MOD | 0.5 | 120 | near-border and other departures |
| CPL | 1 | 250 | non near-border departure |
| CNL | 0.01 | 100 | every flight |
| RTI | 1 | 150 | every flight |
| RTU | 5 (one per 6 s for 30 s) | 140 | every flight |
| RTA | 1 | 110 | every flight |
| POI / PLA / POA / POJ | 0.01 each | 250 / 110 / 80 / 100 | every flight |
| TOC / AOC | 1 each | 150 / 80 | every flight |
| ABI | 1 | 250 | every flight |
| MIS | 0.1 | 130 | every flight |
| LAM / RLA | one per message answered | 80 | |
| LRM | on error | 100 | |

Peak rates, NAS-IC-82422100 Table 3-IV (CAATS design workload), messages per minute:

| Message | One ARTCC | All En Route | Average bytes |
|---|---|---|---|
| CPL | 4.88 | 9.06 | 383 |
| MOD | 3.50 | 6.50 | 407 |
| FPL | 2.26 | 4.20 | 240 |
| CHG | 1.13 | 2.10 | 210 |
| EST | 2.12 | 3.94 | 164 |
| CNL | 0.07 | 0.13 | 155 |
| RTI | 7.00 | 13.00 | 316 |
| RLA | 6.65 | 12.35 | 124 |
| RTA | 7.00 | 13.00 | 277 |
| RTU | 92.40 | 171.60 | 309 |
| LAM | 19.92 | 36.99 | 124 |
| LRM | 1.40 | 2.60 | 149 |

The RTU peak is the RTI peak times 13.2 (every 10 s for 2.2 minutes per handoff, the table's
footnote). **Discrepancy:** the NAM ICD assumes 5 RTUs per flight (every 6 s for 30 s); the ERAM
ICD assumes 13.2 per handoff. `workload` carries both; the session's pending table does not depend
on RTU because it expects no response.

Latency, availability and size bounds:

| Bound | Value | Source |
|---|---|---|
| LAM or LRM after a message | within 60 s, 99% of the time | NAM ICD Part III 6.1 |
| RTI / RTA / RTU transit | under 6 s | NAM ICD Part III 6.1 |
| Detect and recover from failure | 60 s | NAS-IR-82422100 3.2.2.2-b |
| End-to-end availability goal | 0.9999 | NAS-IC-82422100 3.1.1 |
| One-way latency, ERAM to NESG, 95th percentile | 225 ms | NAS-IC-82422100 3.1.1 |
| One-way latency, NESG to NESG, 95th percentile | 50 ms | NAS-IC-82422100 3.1.1 |
| Message size | 17 to 1800 bytes | NAS-IC-82422100 3.2.2.3.1 |
| Simultaneous peers | 4 | NAM ICD Part III 6.3 |

## Bounds

A message is 17 to 1800 bytes; a route holds a bounded number of elements; at most 128 messages are
unanswered at once (2.4 times the peak rate for the 60 s response window) and `step` returns at
most 8 actions. A full table refuses a new message and numbers nothing.

## Tests

* `tests/vectors.rs`: every example message printed in Part II, cited by section and page.
* `tests/malformed.rs`: one defect per field, with the reason and field expected, and the framing
  refusals.
* `tests/roundtrip.rs`: byte-exact round trip on seeds and 60,000 mutations, arbitrary bytes.
* `tests/session.rs`: every transition of Figure 3-3, timeouts, numbering and wrap, bookkeeping,
  bounds, the LRM table and the workload figures, wrong-unit refusal, unmatched LRMs.
* `tests/review.rs`: the fixes from the independent review: CHG optionality, Field 10, profiles,
  LRM code mapping.
* Fuzz target `nam` (`fuzz/`, docs/FUZZING.md), seeded with the examples.
