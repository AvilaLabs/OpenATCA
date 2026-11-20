# oa-shadow

Runs OpenATCA's tracker beside ERAM's published track on the same recorded traffic, offline. It
has two commands:

- `oa-shadow` reports where the two systems disagree.
- `oa-shadow screen` looks for aircraft whose ADS-B position is persistently late or offset.

ERAM's track comes from the FAA's SWIM SFDPS feed, recorded by `oa-scds`.

```
oa-shadow        --adsb clip.oaf --sfdps-dir dir --out dir
oa-shadow screen --adsb clip.oaf --sfdps-dir dir --out dir
```

Exit status: 0 on success, 1 on a failed run, 2 on a usage error or a sealed input path. Every
path is checked against the sealed hold-out list before anything is opened.

## The shadow comparison

A live shadow has no truth, so a difference is decided by the two systems' outputs alone. ADS-B
truth appears in one column only, added after the classification: which system was closer. It
never decides a class.

Both systems are limited to the ADS-B clip's box, so ERAM's tracks elsewhere in the country are
not counted as missing from OpenATCA. Flights are paired by Mode S address, or else by
transponder code and proximity. At each ERAM report time, OpenATCA's one-second output is
interpolated to that time and the two are compared. ERAM stamps positions on whole seconds, and
the along-track comparison allows for that.

Each difference falls into one class: a flight only one system has, a duplicate or contradicted
ERAM track, a gap in ERAM's reports, a position divergence (0.5 NM and 1 NM, cross-track and
along-track), an altitude disagreement, a beacon code or callsign mismatch, a start or drop
timing difference, or a coast that only one system shows. All thresholds are in one table
(`src/thresholds.rs`), and a value exactly at a threshold is not a difference.

The output is `events.csv` (one row per difference), `flights.csv` (one row per paired flight)
and `summary.txt` (counts per class, and every load count that was not zero).

## The ADS-B screen

OpenATCA's accuracy scores use ADS-B as truth, so an error shared by the truth and the tracker's
input cannot be seen in them (`docs/UNKNOWNS.md`, UK-008). The screen measures that error against
something independent of ADS-B: ERAM's track, which also uses radar.

At each ERAM report time it takes the aircraft's ADS-B position from the truth smoother and splits
the difference into along-track and cross-track parts. A late ADS-B report sits behind the
aircraft, so along-track offset = c − L × speed, where L is the latency in seconds (positive
means ADS-B is late) and c is a fixed offset.

A sample is used only where the comparison is clean. Each rule is a printed constant, and every
excluded report is counted under one reason, so the counts add up to ERAM's reports:

- the smoother is confident (95% radius 15 m or less);
- the aircraft is flying straight (turn rate 0.5°/s or less) and nearly level (500 ft/min or
  less);
- the two positions are within 2 km of each other (a larger gap is a wrong association or a
  glitch, not an ADS-B error);
- the flight is not a duplicate and its address is not contradicted.

It works in two stages:

1. **The fleet.** A fit of c and L over every aircraft with at least 30 samples spanning 10
   minutes. This is the common latency of the whole ADS-B path, including ERAM's own whole-second
   time stamps. The reference is the Theil-Sen fit, which outlying aircraft cannot pull. Least
   squares is printed beside it, with bootstrap intervals over aircraft.
2. **Each aircraft.** With c held at the fleet value, the aircraft's own latency is the median of
   its samples. Its cross-track offset is its median, less the fleet's. The interval comes from a
   seeded block bootstrap. An aircraft is screened only with at least 100 samples (10 blocks of
   10) spanning 10 minutes.

An aircraft is flagged:

- **LAG** when its latency exceeds the fleet's by more than 0.1 s, with the whole interval above
  that;
- **EARLY** in the same way below;
- **BIAS** when its cross-track offset exceeds the larger of three times its reported ADS-B
  accuracy bound (NACp) and the fleet's 95th percentile spread.

A flag must survive removing the sample that most supports it.

At constant speed an offset and a latency cannot be told apart, so LAG means an along-track
excess equivalent to 0.1 s or more. The LAG count is printed for the fleet reference, for both
ends of its interval, and for least squares, so the effect of the reference on the result is
visible.

The output is `aircraft.csv` (one row per aircraft, with addresses; not for publication) and
`summary.txt` (counts and distributions only).

### First result (15 November 2026, ZAU)

Of 891,293 ERAM reports, 16,591 passed every rule. Most of the rest fell where the archived
ADS-B traces leave more than 10 s between reports, so the smoother had no state.

- **The fleet.** 91 aircraft entered the fleet fit, which gave a latency of 0.49 s (95% interval
  0.41 to 0.56 s) and an offset of −20 m.
- **Individual aircraft.** Only 5 aircraft had enough samples to be screened individually. None
  was flagged.

One day of these recordings is too thin to say how common a persistent ADS-B error is. Full-rate
ADS-B and longer ERAM recordings will settle it.
