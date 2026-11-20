# oa-stdds-ref

Scores the OpenATCA tracker, and the ADS-B truth it is scored against, against STARS terminal
radar tracks as published on STDDS (TAIS). It is the first reference in the project that is not
built from the tracker's own input.

```
oa-stdds-ref --adsb <clip.oaf> --out <dir> <stdds.oaf[.gz]>...
```

The tool runs the tracker on the ADS-B clip, builds the truth from the same clip (as `vs_eram`
does), reads the STDDS recordings and writes `stdds_ref.txt` and `stdds_ref.csv` into `--out`.

## Why there are classes

STARS in fusion mode uses ADS-B as one of its sensors, so a STDDS track can follow the same
ADS-B reports our tracker and our truth are built from. A score against such a track says little
about absolute accuracy. So every STDDS record gets a class from its own flags,
and nothing is pooled across classes:

| Class | Rule |
|---|---|
| pseudo, frozen or coasting | `track/pseudo`, `track/frozen` or `track/coast` present and true. No live sensor position; reported apart. |
| adsb=false | `track/adsb` present and false |
| adsb=true | `track/adsb` present and true |
| adsb absent | `track/adsb` not in the record |

A flag is read only when it is present: an absent flag is never taken as false. The class names
state the flag's value, not what it means. That adsb=false marks a radar-only track is a
hypothesis, which the terminal radar census (`tools/oa-stdds-census`) tests.

## What is computed

Within each class, on the same paired samples, there are three error sets: tracker against STDDS,
ADS-B truth against STDDS, and tracker against ADS-B truth. If STDDS follows ADS-B, the second set
is small. That set is the diagnostic.

- **Pairing** is by Mode S address only. A record with no address, time or position is counted
  and never paired.
- **Time.** The STDDS time is the track's `mrtTime`, in Unix milliseconds UTC. Tracker rows are
  interpolated linearly across gaps of at most 5 s, and extrapolated by at most 1 s past their
  ends; each extrapolation is flagged and counted. The truth is never extrapolated.
- **Time offset.** Each track's STDDS time stamps are also fitted against the truth over ±15 s in
  0.1 s steps, with ties going toward zero. An offset at either edge is flagged. A track with
  fewer than 8 usable samples is not fitted, and the report gives the reason: no truth over the
  window, or too few samples.
- **Errors.** Along-track, cross-track (positive to the right) and horizontal error, in metres,
  are computed in an azimuthal-equidistant frame centred on the reference point. They are joined
  by the altitude difference (feet) and the velocity difference. Percentiles are nearest-rank over
  absolute values; the signed mean is the bias. Non-finite values are counted and excluded.

## Units not yet confirmed

The STDDS units are assumed, not confirmed: altitude in feet, and velocity in m/s east and north
(`crates/oa-stdds/src/tais.rs`). The report states this in its header. Each class prints two unit
checks, and marks "units suspect" when they disagree:
- the median STDDS altitude against the median truth altitude (more than a factor of 10 apart);
- the median STDDS-to-truth speed ratio (outside 0.8 to 1.25).

The tracker altitude is barometric when the ADS-B report carries one, and geometric otherwise.
The row does not record which.

## Output and hold-out data

- The output holds counts and aggregates only: no address, callsign, track number, time or
  position. The CSV gives each sample's file and record index, class, status and errors.
- The summary carries the SHA-256 of every input and a digest over both outputs. For the same
  inputs given by the same paths, the output is byte-identical.
- An existing output file is refused, never overwritten.
- Every input and output path goes through the shared hold-out guard (`oa-sealed`): it is checked
  before anything is opened or created, and verified on the opened file. A sealed path exits 2,
  naming it.

## Limits

- A track whose records change class is split into one track per class, so short pieces may go
  unfitted.
- Only TAIS terminal tracks are scored, not ASDE-X surface tracks.
