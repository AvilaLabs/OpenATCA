# oa-stdds-census

Counts what the ADS-B flag on STDDS terminal radar tracks means. STARS publishes every track
update with a flag that says whether ADS-B contributed. If the flag is reliable, the updates
flagged without ADS-B are a reference independent of ADS-B, and comparing them with the same
aircraft's ADS-B measures how far ADS-B itself is off (UK-008, UK-009). This tool collects the
evidence for and against that reading; it does not decide it.

```
oa-stdds-census DIR [--from S --to S] [--adsb CLIP.oaf [--gate-nm N] [--gate-ft N]]
```

- `DIR`: a STDDS recording directory (files `*.oaf` or `*.oaf.gz`, searched up to three levels
  down). Given a capture day directory that holds `rec/stdds` (the layout `run-window.sh` writes:
  `DAY/rec/<feed>/<feed>-<UTC start>-<NNNN>.oaf.gz`, `DAY/log`), only `rec/stdds` is read; the
  other feeds' recordings are skipped.
- `--adsb`: an `.oaf` clip of the same day from `oa-adsb-clip`. Without it, only the census runs.
- `--gate-nm`, `--gate-ft`: the match gates of the position comparison, 1 NM and 500 ft by
  default. The horizontal gate is also tried at 0.5 and 2 NM.
- `--from`, `--to`: a window of the UTC day in seconds, applied to both sides.

Sealed hold-out data is never read. `DIR`, `--adsb`, every directory of the walk and every file
opened go through the shared guard `oa-sealed` (`crates/oa-sealed/sealed_paths.txt`), which
resolves links before it checks. A sealed path is refused before any byte is read: the message
names the path and the exit status is 2. The tool writes only to stdout and stderr and takes no
`--out`.

Run on the 24 November development day, on 25 November (the clip is the `oa-adsb-clip` output
for ADS-B day 2026.11.24):

```
oa-stdds-census /home/connoravila/Documents/Avila-Labs/openatca-data/scds/day-20261124 \
  --adsb CLIP-2026.11.24.oaf > census-20261124.txt
```

Exit status: 0 on success, 2 on a usage error, a refused path, a bad gate or window, or a memory
estimate above 4 GB; 3 if the frame reader stalls. The output is counts and distributions only.
No position, address, code, callsign or per-aircraft time of a real aircraft is printed; the only
text taken from the data is the facility source id.

## The census

Per update, split by the flag (true, false, absent; absent is never counted as false):

- the interval between consecutive updates of a track, with its distribution. A track is one
  facility, source and track number; a gap over 120 s, an update flagged new or three backwards
  times in a row starts it again;
- how often the flag changes within a track, and the intervals before, across and after a change;
- what identity the update carries: a non-zero Mode S address, a Mode 3/A code, code 1200, a
  callsign;
- the same by track class (all false, all true, mixed) and by facility.

A track number reused within 120 s without the new flag is not detected and joins two aircraft.

## The position comparison

With `--adsb`, each update is compared with the ADS-B aircraft at the same place and time.

- The ADS-B side is gated exactly as the tracker and the truth are (`oa_truth::quality_check`
  with its default configuration), so only the aircraft's own ADS-B is used: TIS-B, which
  rebroadcasts the radar picture, multilateration and ADS-R are removed, and so are reports below
  the accuracy gates. Ground reports are removed after the gates. The output counts the clip's
  reports by source and what each gate removed.
- Each ADS-B aircraft is interpolated to the update's time between two reports at most 10 s
  apart. An update with exactly one aircraft inside the gates is a match; none and more than one
  are counted and not used.
- For matches, the difference STDDS minus ADS-B: horizontal, vertical, along and across the
  ADS-B track, and along-track as a time for aircraft at 100 kt or more. Each is given per update
  and as the distribution of per-track medians, for adsb=false and for adsb=true as the control.

Three counts must be read with every matched distribution:

- **The gate.** A match is inside the gate by construction, so the matched distribution is cut
  at the gate. The outcome counts at 0.5, 1 and 2 NM show how much lies beyond.
- **Chance matches.** The comparison is run again with the STDDS time moved by 60 s each way.
  Matches in those passes are unrelated aircraft that happen to be inside the gate; their rate is
  the floor under the real one.
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
