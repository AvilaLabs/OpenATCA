# M5 — Safety nets: design

Status: approved by Connor 2026-11-19, confirmed 2026-11-20. Built: step 1 (net state, inhibition objects and their
display indications, `oa-safety` and `oa-display`) and step 2 (the close-encounter record,
`oa-encounter` and `tools/oa-encounter`). The rest is not built yet.

## Goal

Conflict alert, minimum safe altitude warning (MSAW), area proximity warning and approach path
monitoring that miss no alert the legacy logic would give, give fewer nuisance alerts, and never
fail silently. SCOPE §7 M5; shortcomings LS-301 and LS-302.

The second half of that sentence is new. The accidents in which an investigator cited the ATC
alerting all come down to how the alert behaved, not to whether one existed:

- **Überlingen, 2002.** The conflict alert was off in a maintenance fallback mode and nothing told
  the controller (BFU recommendation 10/2004: show when it is unavailable, make the warning
  impossible to miss, sound it until acknowledged).
- **Korean Air 801, Guam, 1997.** The MSAW was inhibited on purpose to stop nuisance alarms; the
  NTSB named that inhibition and its poor management as contributing.
- **DCA, 2025.** Conflict alerts were common at DCA and mostly nuisance, and every alert looked and
  sounded the same whatever its severity (NTSB AIR-26-02, finding 48, recommendation A-26-20).
- **JAL 907 and 958, 2001.** The alert came two minutes late because the lead aircraft was turning.
- **Gol 1907, 2006.** The transponder went off, the altitude vanished from the controller's display,
  and nothing alerted.

So every safety net here is designed from the start around five rules, before any geometry.

## The five rules

1. **A safety net always says what state it is in.** Each net publishes its state every cycle:
   running, degraded (with the reason and the area affected), inhibited (with the inhibitions in
   force), or unavailable. Every position shows that state, and a net that is not fully running is a
   persistent indication that cannot be dismissed while it lasts. The display never shows "no
   alerts" when the truth is "no alerting". (BFU 10/2004.)
2. **An inhibition is an object, never a setting.** It has an area, an altitude band, a start, an
   end no later than a maximum set in adaptation, a reason and the name of whoever set it. It is
   drawn on the display, logged, reviewed when it expires, and can be replayed to show every alert
   it suppressed. No inhibition is permanent or silent, and there is no way to switch a net off for
   an area except by such an object. (Guam.)
3. **Alerts are graded.** Each alert carries a severity from the predicted time to loss of
   separation and the predicted miss distance, and each grade looks and sounds different. The
   aural alert repeats until acknowledged. The nuisance rate is measured and published per facility
   and per grade. (A-26-20.)
4. **Missing data is an alert, not a gap.** A track that loses its altitude still takes part in
   conflict alert in the horizontal plane, marked "altitude unknown", and the loss itself raises an
   alert. A coasting or uncertain track takes part with its uncertainty, never dropped. (Gol, and
   9/11 for the case of a transponder turned off.)
5. **Work is bounded and the bound is visible.** Every net counts its work against a named budget
   (RULESET C7). If a cycle cannot evaluate every pair or every track, it evaluates them in a stated
   priority order and the net's state becomes "degraded: N pairs not evaluated", under rule 1. It
   never skips work silently.

## 1. Inputs

- **Tracks** from `oa-track`: position and velocity with covariance, flags (coasting, uncertain,
  radar-only, identity unconfirmed, radar disagrees), barometric altitude, vertical rate.
- **Altitude reference.** Below the transition level, pressure altitude is turned into indicated
  altitude with the nearest altimeter setting from `oa-awc`, which already refuses a stale setting.
  A refused setting puts MSAW into "degraded" for that area under rule 1.
- **Terrain and obstacles.** The conservative `oa-tiff` queries (`*_conservative`, every cell that
  touches the volume, bounded between samples) and `oa-dof`. The plain queries are never used for a
  warning.
- **Minimum vectoring and instrument altitudes** from `oa-mva`.
- **Airspace.** Separation minima, alerting volumes, special-use airspace from `oa-airspace`
  (NASR), and temporary restrictions from `oa-notam`.
- **Intent** (later). Cleared altitude and route from flight data (M4), and the selected altitude
  from ADS-B, which about one airborne report in five carries in the ZAU data. The nets work without
  it and use it only to sharpen a prediction, never to suppress an alert. The ADS-B selected heading,
  roll angle and track rate were tested for turn prediction and do not help (§2).

## 2. Conflict alert

- **Candidates.** Pairs are found with the spatial grid the tracker already has (`oa-track`
  coarse grid), within the look-ahead plus the minima plus each track's uncertainty, counted
  against the pair budget.
- **Prediction.** Straight-line prediction first. Then turn-aware prediction for a track whose turn
  is under way, and later for a turn the flight plan predicts (M4). JAL 907 is the test: a turning
  aircraft must not delay the alert.
- **What ADS-B intent gives (measured 2026-11-19, ZAU, 17 November 2026).** Roll angle and track
  rate are almost never present during a turn (63 of 454,000 turning reports). The selected heading
  is present before about a quarter of turn starts, but using it made position error worse, most
  likely because the selected heading is not the heading flown while the flight management system
  steers. Restricted to autopilot on and lateral navigation off, it did no harm but reached under 1%
  of turn starts. None of these signals is used. The experiment is kept on the tag
  `archive/intent-null-2026-11-19`.
- **Uncertainty.** The predicted miss distance is judged with each track's covariance, so a poor
  track widens the alert rather than hiding it.
- **Minima.** From adaptation, per volume: JO 7110.65 §5-5-4 gives 3 NM in terminal airspace within
  40 NM of the antenna and 5 NM en route, and §4-5-1 gives 1,000 ft vertically up to FL410 in RVSM
  airspace (both to be checked against the current edition before any value is used).
  Local volumes (reduced minima on parallel approaches, helicopter routes) are adaptation objects.
- **Look-ahead and warning time.** Set from the EUROCONTROL STCA specification and from published
  ERAM and STARS values where they exist, each value cited in BENCHMARKS.md before any result uses
  it. No number is chosen here.
- **Severity grades.** At least two: "loss predicted" and "loss imminent or current". The
  boundaries come from the same sources and are tuned only on development data.

## 3. Minimum safe altitude warning

- Predicted vertical path against the higher of the conservative terrain and obstacle height plus
  clearance, and the MVA or MIA of the sector, over the look-ahead.
- Approach and departure areas use the published procedure altitudes from CIFP.
- Guam is the test case: a recorded approach that descends early must alert, and an inhibition
  over the area must be visible, time-limited and replayable.

## 4. Area proximity warning and approach path monitoring

Built after conflict alert and MSAW, on the same five rules. Area proximity uses special-use and
temporary restricted airspace. Approach path monitoring uses CIFP final approach paths and
glidepaths.

## 5. A record of close encounters

Every pair that comes within the alerting volume is logged whether or not it alerts, with its
geometry. That gives the facility the record of close-proximity events the NTSB found missing before
DCA (AIR-26-02, findings 63 to 66, A-26-37). It also gives every nuisance alert a classification.
Public output is aggregated, with the LADD list and the observer-network privacy rules applied
(NODE_NETWORK §10). This record can be built before the alerts, from tracks alone.

Built as `oa-encounter` (`tools/oa-encounter/README.md`). On the development hour (ZAU, 13 November
2026, 18:00Z) it found 2,501 encounters. 1,559 were on the surface and 439 involved a coasting or
uncertain track. Of the 503 airborne encounters between healthy tracks, 57 were aircraft on levels
1,000 ft apart, inside the volume only through 25 ft altitude steps. The other 446 had none in the
closest class, and the 28 in the next class were light aircraft in traffic patterns at
general-aviation fields. 145 of the airborne encounters near 1,000 ft involve a climb or descent.
Telling a level-off 1,000 ft from another aircraft from a climb that does not stop needs the
cleared altitude, which comes with flight data (M4). Until then they stay in the record as
converging.

## 6. Evaluation

The exit criteria in SCOPE §7 M5 stand. Added here:

- **A legacy-style baseline.** A straight-line STCA, with no uncertainty and no grading, written by
  us from the EUROCONTROL specification's description and labelled as ours, never as ERAM's. Every
  result is reported against it on identical data.
- **Recorded traffic.** Every recorded loss of separation and low-altitude event must alert. Every
  nuisance alert is counted, classified and reported per hour per position, because a rate per hour
  is what controllers live with (DCA).
- **Warning time.** Reported as a distribution, not a mean, against the baseline.
- **Encounter models.** Monte Carlo over the MIT LL models (have, BSD-2-Clause) for missed and
  nuisance alerts and the risk ratio.
- **Reference independence.** Truth for recorded traffic is built from ADS-B, which shares its
  errors with the tracker (UK-008). Alert results are reported separately on the independent subsets
  (STDDS radar-only tracks) once they exist.
- **The five rules have their own tests.** A net taken down in a test shows "unavailable" on every
  position. An expired inhibition stops suppressing. A track losing altitude raises its alert. A
  budget overrun shows "degraded" with the count.

## 7. Proofs

The geometric core of each net (closest approach, the comparison with minima, the terrain clearance
test) is proved correct and shown free of overflow and division by zero, in the DAIDALUS manner.
That means bounded Kani proofs on the implementation, and a written proof of the method checked
against the implementation's tests. NASA's DAIDALUS (Apache-2.0) is run as an outside oracle on the
same geometries; it is not copied in.

## 8. Build order

1. Net state and inhibition objects, and their display indications (rules 1 and 2). Display work on
   `oa-display`, before any alert exists.
2. The close-encounter record (§5), from tracks.
3. Conflict alert: candidates under budget, straight-line prediction, uncertainty, minima from
   adaptation.
4. The legacy-style baseline, and the evaluation harness on recorded traffic and on the encounter
   models.
5. Severity grades.
6. Turn-aware prediction: turns under way, then flight-plan turn points once M4 exists.
7. MSAW, on conservative terrain, obstacles and MVA.
8. Area proximity warning; approach path monitoring.

## 9. Open for Connor

- Approval of this design, especially the five rules: they make the nets stricter about showing
  their own state than ERAM and STARS are documented to be, and that adds display work to M5.
- Controller input on the grades and their sound (NATCA, or controllers we can reach). Until then
  the grades are logic and counts only, and their presentation is an M8 question.
