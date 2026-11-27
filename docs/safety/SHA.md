# OpenATCA System Hazard Analysis

Status: draft 1, 2026-11-27. Owner: Avila Labs. Not reviewed or approved by the FAA.
Source analysed: `AvilaLabs/OpenATCA` at commit c15b5ca. Data: `docs/safety/analysis/sha.csv`.

The SSHA looks inside each component. The System Hazard Analysis (SHA) looks at the connections
between them. It covers the interfaces between OpenATCA components, and between OpenATCA and the
NAS systems that feed it: what happens when data crossing an interface is lost, late, repeated,
reordered, corrupted, too much, or from the wrong source.

## 1. Method

1. **Interfaces.** There are 34:
   - 14 external interfaces and data sources: ASTERIX surveillance, STDDS, SFDPS, TBFM, TFMS,
     FNS/NOTAM, IWXXM weather, METAR altimeter settings, GRIB2 weather, NAM coordination,
     adaptation ingest, issued adaptation at startup, recording, and the display;
   - 20 internal boundaries, where a pure core function's output passes to the shell code that
     acts on it.

   Each is cited by function and `file:line`, with its interface document.
2. **Failure modes.** Each interface is examined in seven ways:
   - loss;
   - delay;
   - duplicate;
   - out of order;
   - corruption;
   - overload;
   - wrong source or spoofed.
3. **For each pair**, the analysis records the detection, the response and the hazards, or NONE
   with the reason.

That gives 238 rows: every interface in every mode.

## 2. Results

130 of the 238 interface and failure-mode pairs have no detection. Every one of those 130 also
has no response.

| Failure mode | Pairs with no detection and no response |
|---|---:|
| Wrong source or spoofed | 33 |
| Loss | 24 |
| Duplicate | 23 |
| Delay | 21 |
| Out of order | 21 |
| Corruption | 6 |
| Overload | 2 |
| **Total** | **130** |

Two things stand out.

1. **Only NAM checks where its data came from.** On 33 of the 34 interfaces, no check was found
   for a message from the wrong source, or a forged one. The exception is NAM, the
   facility-to-facility coordination link, which has detection for every failure mode.
2. **Corruption is well covered; sequence is not.** Only 6 pairs have no corruption detection.
   Loss, duplicates, delay and reordering are much less often detected, especially at the
   internal boundaries. At those interfaces, no check was found that would stop a well-formed
   message that is repeated, late or out of order.

Coverage by interface:

| Interface | Undetected modes (of 7) |
|---|---:|
| NAM | 0 |
| SFDPS, TBFM, TFMS, FNS/NOTAM | 1 each (wrong source) |
| METAR altimeter settings, recording | 3 each |
| ASTERIX, display, GRIB2 weather | 4 each |
| STDDS, IWXXM weather, adaptation ingest, issued adaptation at startup | 5 each |
| Internal boundaries (20) | 1 to 6 each; most have 4 or 5 |

## 3. Safety requirements this analysis sets

These are proposed, as in the SSHA.

1. **Source authentication at every external interface.** Each external input is checked against
   its expected source. Where the source system cannot sign or authenticate its data, this is
   recorded as a stated limit for that interface, and the controls of item 4 carry the hazard.
   That check belongs to the security plan (row E1) as well as to safety.
2. **Sequence and freshness at every interface.** Each message carries a sequence or source time
   that the receiver checks. Gaps, repeats and reordering are counted and, above a stated rate,
   reported to Tech Ops.
3. **Internal boundaries pass source time with the data.** This is the same requirement as SSHA
   requirement 1, applied where core output enters the shell.
4. **Cross-checks between independent sources.** Where two sources cover the same aircraft, such
   as ADS-B and radar, disagreement beyond a stated bound is flagged. This is the main defence
   against a wrong or spoofed source that cannot be authenticated.
5. **Overload is reported.** When an input exceeds its bound, the excess is counted and the
   function reports that it is degraded.

## 4. Limits

- Each interface is analysed for the seven failure modes. Timing between interfaces, such as two
  feeds drifting against each other, is covered only through the delay and out-of-order modes.
- External systems are described from their interface documents. Their internal failure behaviour
  is not analysed.
- Hazards and severities are as in the SSHA: 20 coarse hazards, severities as candidates, no
  likelihood yet.
- No FAA review of this analysis is recorded.
