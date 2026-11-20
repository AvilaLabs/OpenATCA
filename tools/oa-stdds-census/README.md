# oa-stdds-census

Counts what the ADS-B flag on STDDS terminal radar tracks means. STARS publishes every track
update with a flag that says whether ADS-B contributed. If the flag is reliable, updates
flagged without ADS-B on a track that never carries ADS-B could be a reference independent of
ADS-B (UK-008, UK-009). An adsb=false update on a mixed track is not ADS-B-free, and adsb=true
updates are ADS-B-fused: their difference from ADS-B measures STARS latency and filtering, not
accuracy. This tool collects the evidence for and against the reading; it does not decide it.

```
oa-stdds-census DIR [--from S --to S] [--adsb CLIP.oaf [--gate-nm N] [--gate-ft N] [--max-gap-s S]]
```

- `DIR`: a STDDS recording directory (files `*.oaf` or `*.oaf.gz`, searched up to three levels
  down). Given a capture day directory that holds `rec/stdds` (the layout `run-window.sh` writes:
  `DAY/rec/<feed>/scds-<feed>-<UTC start>-<NNNN>.oaf.gz`, `DAY/log`), only `rec/stdds` is read, and only files named `scds-stdds-*`. A
  directory with `rec` but no `rec/stdds`, or named `rec`, is refused (exit 2).
- `--adsb`: an `.oaf` clip of the same day from `oa-adsb-clip`. Without it, only the census runs.
- `--gate-nm`, `--gate-ft`: the match gates of the position comparison, 1 NM and 500 ft by
  default. The horizontal gate is also tried at 0.5 and 2 NM.
- `--max-gap-s`: the widest gap between two ADS-B reports that is interpolated, 5 s by default
  (a wider bracket interpolates a manoeuvre away; the output counts matches by bracket width and the
  neighbouring reports left out as too far apart).
- `--from`, `--to`: a window of the UTC day in seconds, applied to both sides; the ADS-B side is
  widened by the bracket and the 60 s placebo shift. Both must be finite.

The run exits 2 if the STDDS and ADS-B time spans do not overlap at all, or if the window holds no
STDDS update. A partial overlap runs: the updates whose time lies outside the ADS-B clip's span are
counted apart per flag ("outside the ADS-B clip"), never as "no candidate" or "no ADS-B". The spans
are printed in UTC to the second, start rounded down and end rounded up: the span of each recording,
not of any aircraft. `--from` and `--to` take whole seconds (a fraction, or a value whose milliseconds
do not fit an i64, is a usage error); with a
window, an update with no time is outside it and is counted as such. It also exits 2, before the index is built, if the clip's
record count puts it past the 4 GB estimate (an estimate under stated assumptions, not a bound; see
OA-CEN-020). The environment variable `OA_STDDS_CENSUS_BUDGET_BYTES` replaces the 4 GB and is for
tests only.

Sealed hold-out data is never read. `DIR`, `--adsb`, every directory of the walk and every file
opened go through the shared guard `oa-sealed` (`crates/oa-sealed/sealed_paths.txt`), which
resolves links before it checks. A sealed path is refused before any byte is read: the message
names the path and the exit status is 2. The tool writes only to stdout and stderr and takes no
`--out`.

The tool runs on 25 November on the 24 November development day (the clip is the `oa-adsb-clip` output
for ADS-B day 2026.11.24):

```
oa-stdds-census /home/connoravila/Documents/Avila-Labs/openatca-data/scds/day-20261124 \
  --adsb CLIP-2026.11.24.oaf > census-20261124.txt
```

Exit status: 0 on success, 2 on a usage error, a refused path, a bad gate or window, or a memory
estimate above 4 GB; 3 if the frame reader stalls or a gzip file is damaged (the report is printed
first). The output is counts and distributions only.
No position, address, code, callsign or per-aircraft time of a real aircraft is printed; the only
text taken from the data is the facility source id.

## The census

Per update, split by the flag (true, false, absent; absent is never counted as false):

- the interval between consecutive updates of a track segment, with its distribution. STARS reuses
  track numbers, so a track key (facility, source and track number) is cut into track segments: a
  gap over `MAX_INTERVAL_MS` = 120 s between two updates, an update flagged new, or three backwards
  times in a row starts a new segment, and so does an update that follows one with no time. A coast,
  pseudo or frozen update takes part in this time state, so a coast keeps a segment alive across a
  gap between measured updates, since STARS kept the track. Every
  per-track figure (class, flag counts, changes, intervals, the cross-check) is taken per segment, and
  the report counts "track segments". A directory of several days never joins a segment across days:
  the same time of day a day later is far over the gap;
- how often the flag changes within a segment, and the intervals before, across and after a change;
- what identity the update carries: a non-zero Mode S address, a Mode 3/A code, code 1200, a
  callsign;
- the same by segment class (all false, all true, mixed) and by facility;
- updates flagged pseudo, frozen or coasting (each read only when present and true) are in the
  update totals but in no flag tally, class, change count, interval, identity share or the false
  shares (the adsb=false share of flagged updates, the per-facility one), so those agree with the
  class lines; they have their own counts per kind and flag (how many coasts carried adsb=false, how
  many adsb=true);
- track segments with no flagged update (only status updates, or only updates with no flag) are
  counted as "segments with no flagged update" and are in no class;
- an update with the same key, time, position and flags as one already seen, across files or within
  one, is dropped and counted ("duplicate updates dropped"); updates without a time or track number
  are not checked for duplicates;
- the report reconciles its counts and says "reconciles" or "DOES NOT RECONCILE" on each line: the
  input TAIS records against the updates, duplicates dropped, updates outside the window and updates
  with no time outside the window (with or without `--adsb`), and with `--adsb` the no-flag updates,
  the checked and not-checked updates per flag, and the placebo lookups per flag and shift (matched
  plus not matched plus outside the clip equals checked).

A track number reused within 120 s without the new flag is not detected and joins two aircraft.
When neither the message `src` nor the record's `recSTARSSrcID` is present the facility is printed as
"facility unknown".

## The position comparison

With `--adsb`, each update is compared with the ADS-B aircraft at the same place and time.

- The ADS-B side is gated exactly as the tracker and the truth are (`oa_truth::quality_check`
  with its default configuration), so only the aircraft's own ADS-B is used: TIS-B, which
  rebroadcasts the radar picture, multilateration and ADS-R are removed, and so are reports below
  the accuracy gates. Ground reports are removed after the gates. The output counts the clip's
  reports by source and what each gate removed.
- Each ADS-B aircraft is interpolated to the update's time between two reports at most 5 s
  apart. The ADS-B time is the receipt time (about 0.49 s fleet latency, UK-008), so the along-track
  time is the STARS latency minus the ADS-B latency. An update with exactly one aircraft inside the gates is a match; none and more than one
  are counted and not used.
- For matches, the difference STDDS minus ADS-B: horizontal, vertical, along and across the
  ADS-B track, and along-track as a time; along and across need 100 kt or more, like the time.
  Each is given per update and as the distribution of per-segment medians. A segment is a run of
  one track number that no gap over 120 s, new flag or resync has cut, so a reused track number
  is not one aircraft; a track segment is the unit of every per-track figure. adsb=false is headlined by track class (all false, mixed, all true);
  adsb=true is printed as ADS-B-fused, not independent of the truth.
- Updates flagged pseudo, frozen or coasting (a dead-reckoned position) are counted and not
  checked, with their own count.
- adsb=true updates that carry a Mode S address are also checked by address (that aircraft at that
  time, no gate), reported apart from the proximity matches.

Three counts must be read with every matched distribution:

- **The gate.** A match is inside the gate by construction, so the matched distribution is cut
  at the gate. The outcome counts at 0.5, 1 and 2 NM show how much lies beyond.
- **The placebo.** The comparison is run again with the STDDS time moved by 60 s each way. The
  placebo shares route structure with the real match (aircraft in trail about 60 s apart, slow or
  circling aircraft matching themselves), and it is not suppressed by the single-candidate rule the
  way a real chance match is. Its count over the real matches, printed next to the medians, is
  therefore a rough indication of unknown direction, not an estimate of the chance share (a ratio of
  counts, which can exceed 100%). A placebo lookup whose shifted time falls outside the clip's time
  span (no window, or near the clip's edges) is counted as "placebo outside the clip", never as a
  non-match.
- **Coverage.** The share of updates with any ADS-B aircraft within 30 NM separates an aircraft
  with no ADS-B from an area the clip does not cover.

The search is exact at any latitude and across the antimeridian. Updates above 85° and ADS-B
segments faster than 600 m/s are counted and not used, and the update totals reconcile with the
census counts line by line.

## First census (20 November 2026, recording in progress)

On the first 4.5 million updates from 142 facilities, 42% were flagged without ADS-B. On 98% of
those the Mode S address field is present but zero, while every update flagged with ADS-B has an
address. So the flag mostly means that no Mode S identity is attached to the track. 84% of the
updates flagged without ADS-B carry a Mode 3/A code, and 16% carry no code, address or callsign.
Updates flagged without ADS-B arrive at a median 13.8 s apart, against 9.1 s for the others, and
the flag changes within a track on 1.3% of consecutive pairs. None of this shows whether ADS-B
contributed to a position; that is what the position comparison is for.
