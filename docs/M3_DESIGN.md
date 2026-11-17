# M3 — Surveillance tracking: design

Status: approved by Connor 2026-11-14.

## Goal

A multi-sensor tracker that beats every ESASSP 3 NM Recommended value and ERAM's measured
published track (M2) on the same traffic, with track swaps, false tracks and initiation delay
measured and published. SCOPE §7 M3; docs/BENCHMARKS.md §3.1; shortcomings LS-101 to LS-104,
LS-604 and LS-609.

## The input-rate problem, stated first

The adsb.lol history we use for M2 is thinned: about one position every 17 s per aircraft on the
2026-11-13 sample. That is fine as truth for ERAM's published track, but it cannot close M3:
ESASSP asks for a measurement interval of at most 4 s and an update probability of 99% at a data
age of 7 s. No tracker meets that from 17 s input, and none should be credited for coasting
through the gaps. So:

- **Development and every test below** run on adsb.lol replays and on simulated sensors (below).
- **The M3 exit numbers** need full-rate ADS-B (about 1 report per second): either the OpenSky
  research licence (application pending) or our own receiver. The receiver is no longer optional
  by the time M3 closes. It is not needed to start M3.

## 1. Inputs

| Sensor | Source now | Measurement model |
|---|---|---|
| ADS-B (ASTERIX 021, or adsb.lol via `oa-adsb`) | adsb.lol replay; own receiver later | position with covariance from NACp (95% bound / 2.448 per axis), velocity from NACv, barometric and geometric altitude, quality fields, time of applicability |
| Secondary radar (ASTERIX 048) | **simulated** from truth | range and azimuth noise, range and azimuth bias per radar, Mode C 100 ft quantisation, scan period, probability of detection, split and false plots |
| Multilateration | simulated, later | position covariance from geometry |

The radar error model's numbers (noise, bias, detection probability) are taken from a published
radar surveillance standard or test report and cited in BENCHMARKS.md before any result uses
them. No number is invented for it.

## 2. Truth for scoring, without circularity

Scoring a tracker that eats ADS-B against truth made from the same ADS-B is circular. Two
answers, used together:

1. **Split reports.** Each aircraft's reports are interleaved into two disjoint sets. The tracker
   sees one set; `oa-truth` builds truth from the other. Their independent noise is separable
   (truth's own error is measured by M2's leave-one-out). GNSS error correlated over seconds is
   shared between the sets; that limit is stated in every report.
2. **Simulated radar.** Radar plots are generated from truth with independent noise and bias, so
   a radar-only and a mixed-sensor tracker can be scored without any circularity.

Scoring reuses `oa-bench` unchanged: the tracker's output is fed in as the "legacy" stream. The
ESASSP indicators that M2 does not yet compute (R2 data age, R5 correlated errors, R6, R19, R20,
R21) are added to `oa-bench` as part of M3, so ERAM and OpenATCA are always scored by the same
code.

## 3. Tracker

**State and frame.** Per track, horizontal state in a local east-north plane centred on the
track's own last position and re-centred on every update, so the geometry is accurate anywhere
in the NAS without one projection's distortion. Vertical state is filtered separately.

**Motion models (IMM).** Horizontal: interacting multiple models with nearly-constant velocity
and coordinated turn (turn rate as a state). Vertical: level and climb/descent models, with
altitude quantisation (25 or 100 ft) treated as measurement noise of the matching uniform
variance. Model probabilities are output, so the display and the safety nets know when an
aircraft is turning or leaving its level.

**Time.** Every measurement is applied at its time of applicability, not its arrival time.
Out-of-order measurements inside a bounded window are applied by re-filtering from a stored
state (a ring of the last 12 measurements of the track and one anchor filter state, the state
after the oldest of them; the state after any newer entry is the anchor with the ring's
measurements applied again, bit for bit what the filter had); older ones are counted and dropped. Output is the track at each update and at a fixed
rate (1 s) for consumers.

The tracker clock is the newest time seen, decided by a pure gate (`time_gate`, proved with Kani). One
measurement may move it forward by at most the step limit (10 s, the out-of-order window; always below
the 60 s drop age, so one forged or faulty time cannot sweep the live tracks). A measurement farther
ahead is held (counted, not applied) until three in a row come from at least min(2, live sensors)
distinct sensors and min(3, live tracks) distinct addresses; with no live track, or only its own track and
sensor live, it steps at once. The far-ahead measurements of a run are kept (the last 8) and applied after the
step that confirms them: confirmation delays data, it does not lose it; a refuted run discards them (counted
as held).
If the clock is wrong-ahead, every real measurement is refused as late; a run of at least six such
refusals spanning the out-of-order window, from the same spread of sensors and addresses, steps the clock
back to the newest of them (sensors live only because of data ahead of that run do not count towards the
sensors it needs), drops the tracks built on the wrong clock and counts both. **A consumer must
expect that, after a step back, output rows carry earlier times than rows already emitted** (the time of
the output goes back once, by as much as the clock had been forged ahead); `time_steps_back` counts it.

**Association.**
- ADS-B reports are keyed by 24-bit address. The address is evidence, not proof: two aircraft on
  one address, a jump, or a position contradicting other sensors are integrity events (§4).
- Radar plots: global nearest neighbour over Mahalanobis-gated candidates, solved jointly per
  scan. A Mode S address on the plot is a hard constraint inside a wide gate (a gross mismatch is
  counted, not applied), and a track takes at most one plot per scan; a Mode A code alone never assigns
  a plot (LS-609).
- **A radar contradicting an address's track is flagged, not obeyed.** A Mode S plot outside the wide
  gate of its address's track changes no position (the address is evidence, not proof, and which of the
  two is wrong is not for the filter to say), but it sets a flag bit on the track's output rows
  (`O_RADAR_DISAGREES`, "a radar disagrees with this address's track") that stays while such a plot has
  been seen within the last three scans of any radar and clears after three scans without one. The
  marks are counted (`radar_disagree`, with `hard_mismatch`) and printed in the report. A spoofed ADS-B
  position that radar does not see is thus visible to a consumer, and an honest track never carries it
  while the radar agrees with it.
- Plots held for ambiguity are counted per track and per radar, so several radars overlapping a pair
  each hold their own plots. The coarse window of a plot is applied only to the tracks in the cells of a
  latitude and longitude grid (a quarter degree) that it can reach, in ascending track order, which
  finds exactly what testing every track finds; the candidates visited and the exact tests are charged to
  one work limit per bucket, and plots are gated for the radar that has spent least first, so a flood
  from one radar does not leave the others' plots ungated.
- Registration keeps up to 256 reference aircraft per radar (a configuration value). A reference
  keeps its slot while its aircraft makes residuals and gives it up after six scans without one; with
  every slot held a new aircraft is ignored and counted, so registration starts in busy airspace
  instead of replacing references faster than they can make the residuals they need.
- **Identity is held under ambiguity.** When two tracks share a gate, kinematics alone never swap
  their identities; a swap needs identity evidence (address). This is the direct answer to track
  jumps (LS-102, LS-604) and is tested with crossing and parallel traffic. A track whose last two plots
  were all held takes the next one on kinematics (counted): without that, two aircraft that stay together
  would never update their tracks. If the swap rate on the
  recorded day is not zero, JPDA for closely spaced radar-only targets is the next step.

**Unconfirmed identity.** Anonymous candidates keep separate source-local hypotheses; proximity
and Mode A never cancel them or suppress their free plots. Every row of a radar-only key carries
`O_IDENTITY_UNCONFIRMED`: it may describe the same aircraft as another key. These keys are not
aircraft identities. Input addresses cannot claim their reserved namespace. Hypothesis counts,
including possible duplicates, stay visible in scoring and capacity reports.

Initiation has a separate work allowance shared by all radars of a bucket: candidate visits,
address lookups, grid construction/query, candidate-index ordering and retirement are charged
before work. Exhaustion
preserves unvisited candidates and explicitly refuses unprocessed plots; it does not promise a
retry. One bounded refusal-accounting pass reports loss even with no allowance left. This is a
bound on initiation, not a complete work or allocation proof for the whole tracker.

**Initiation, coasting, drop.** Tentative to confirmed by M-of-N: 2 of 3 for ADS-B with a
plausible address and kinematics, 3 of 4 for radar only. Coast after missed expected updates,
flagged; drop after a time set from the recorded data per airspace (en route, terminal). Every
parameter is a named constant with the measurement behind it.

**Fusion.** Measurement-level: every sensor's measurements go into one track, which gives better
accuracy than fusing per-sensor tracks and keeps errors uncorrelated (ESASSP R5). Radar
registration: each radar's range and azimuth biases are estimated continuously against ADS-B
tracks of high integrity, and the estimates are published per radar. One reference is never trusted
alone: several distinct references must agree (median of the others), so a spoofed ADS-B track cannot
shift a radar, and a persistent shift of the agreed residuals (a realigned radar) restarts the estimate,
counted and published. Only reports the tracker accepted count toward a reference. Time is part of
the layer's input: advancing it with nothing pending still runs the tracker (coasting, dropping), and a
bucket once processed is closed to later inputs, which are counted late. The layer's bookkeeping is
bounded by the tracker's capacity (the association covers every live track up to the limit the layer
refuses a larger tracker at).

## 4. ADS-B integrity and spoofing

Checks, each producing a typed event and never silently dropping a target:
- kinematic plausibility by aircraft category (speed, acceleration, turn rate, climb rate);
- agreement with independent sensors (radar, multilateration) where they cover the target;
- barometric vs geometric altitude consistency against the weather (oa-wind temperature);
- duplicate addresses and identity changes;
- with our own receivers: received signal strength against range, and, with more than one
  receiver, time difference of arrival.

Injected attacks for the exit test: ghost aircraft, gradual position drift, replay of a real
aircraft's past track, and address duplication. Detection and false positives are reported as a
confusion matrix against the published bars (≥95% detection; false positives ≤1e-3).

## 5. Flight plan correlation

Track to plan by address (the plan's `aircraftAddress`, or the `ADSB_02M_52B` value SFDPS
carries on about 98% of tracked flights), then by flight ID, then by Mode A code together with
position consistent with the planned route. Never by code alone. Every uncorrelated track and
every conflict (two plans claiming one track, or one plan on two tracks) is listed, never hidden
(LS-603, LS-609).

## 6. Interfaces

ASTERIX category 021 (ADS-B), 048 (monoradar) and 062 (system tracks), in and out, as new
codecs written from the EUROCONTROL specifications. Fetching those specification PDFs is a
download that needs Connor's approval.

## 7. Performance

ERAM's stated capacity is 1,900 tracks and 64 radars per centre; twenty centres is about 38,000
tracks. At one ADS-B update per second per track plus radar, that is roughly 50,000 updates per
second. A two-model IMM update should take a few microseconds (an estimate, to be measured), so the budget should fit one core with room to
spare; gating uses a uniform grid index, and all tables are struct-of-arrays with fixed
capacity. Latency (measurement time to track output) is measured and published, not estimated.

**Memory.** A track is about 2.1 kB (the columns, the current filter state, one anchor filter
state and 12 stored measurements), so 38,000 tracks are about 78 MiB; keeping a full filter state
in every ring slot, as the first version did, was 8.9 kB a track, 323 MiB at that size. The
price of the anchor is one more filter application per update once the ring is full (measured in
the M3 report). The scoring harness does not hold the run: the tracker's output goes to column
files as it is produced and scoring reads it back one address range at a time (a range is closed
at 131,072 rows; an aircraft is never split), so harness memory is the largest range plus the paired
samples (about 60 bytes per paired sample, kept because the percentiles of T1 to T4 are exact
over all of them) and a histogram of the publication interval (whole milliseconds, exact). Peak
memory still grows with the run through the paired samples, at a few percent of the slope it had.

**Latency instrumentation.** Each measurement carries the stamps of the stages it passes: sensor
time of applicability, receive time (the frame's), and the shell's monotonic readings at frame
received, decoded, update called, update returned and output emitted. The clock is the shell's
and is injected (`oa_track::latency::Clock`; the tests use a fixed-step clock); the tracker never
sees it. Per-stage p50, p95, p99 and max are published with the per-update processing time. A
replay is not live: a recording's frame stamp is its first report's time and frames come from a
file, so replay latency is the code's cost per stage, not what a deployment would see.

## 8. Code

| Crate | Role |
|---|---|
| `oa-track` | IMM filters, association, initiation and drop, fusion, registration, integrity checks |
| `oa-sensorsim` | simulated radar and multilateration from truth, and injected spoofs |
| `oa-asterix` | ASTERIX 021, 048, 062 codecs |
| `oa-bench` | gains the ESASSP indicators listed in §2 |

The core is pure, as elsewhere: same inputs give the same tracks, byte for byte.

## Build order

1. `oa-track` on ADS-B only, scored with split reports against M2's ERAM numbers. **Built** (OA-TRK-001 to 029): `oa-track-run` scores on thinned input; see the report it writes for the numbers.
2. `oa-sensorsim` and radar fusion with registration. **Built** (OA-TRK-030 to 057): `oa-track-fusion` scores radar-only and radar plus ADS-B tracking against exact truth (the trajectories the plots were simulated from) beside two baselines on identical plots; see the report it writes for the numbers.
3. Integrity checks and the spoof test.
4. `oa-asterix`, after the specification download is approved.
5. The full ESASSP set on full-rate data, once OpenSky or the receiver provides it.

## Open questions for Connor

1. **Full-rate data for the exit.** The M3 exit needs OpenSky or our own receiver. Nothing is
   needed now; the decision is due before M3 closes.
2. **ASTERIX specifications.** Approve downloading EUROCONTROL's public ASTERIX category 021,
   048 and 062 specifications (PDFs, a few MB) when step 4 starts.
