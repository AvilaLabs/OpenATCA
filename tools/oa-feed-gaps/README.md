# oa-feed-gaps

Decides whether a recorded day's data is good enough to score, before anything is scored. This is
the damage check of the comparison protocol: P-006 section 6, as amended by P-007. It reads one
day's SFDPS recordings, the ADS-B clip, and the clip tool's own log, and gives one verdict: CLEAN
or DAMAGED.

```
oa-feed-gaps --sfdps-dir <dir> --adsb <clip.oaf> --clip-log <clip stderr> \
    --from 2026-11-19T00:17:00Z --to 2026-11-19T23:44:00Z --out <report>
```

## What it checks

**Feed gaps.** For each feed it takes the times in the window, sorted:
- SFDPS by the recorder's receive time;
- ADS-B by report time.
Then it measures every interval between consecutive times, including from the window start to the
first time and from the last time to the window end. An interval over 600 s is an outage. It is not
damage by itself, because a gap removes truth for every system alike. The interval, and the 300 s
after it, become an **excluded interval**: the scorer removes it from every system.

**Damage.** The day is DAMAGED when:
- the excluded intervals, merged, total more than 3,600 s;
- the SFDPS reader reports any error;
- the ADS-B clip has a damaged frame;
- the clip log shows a refused trace, or does not say the archive was complete;
- the clip log is not the log of this clip: its `kept` count must equal the reports in the clip;
- a feed has no message in the window at all.

## Output

The report lists, for each feed:
- the files read;
- the message count;
- the first and last times;
- the largest interval;
- every interval over 600 s.

Then come one `exclude <from> <to>` line per merged excluded interval, at nanosecond resolution,
which is what the scorer reads, and finally the verdict.

The exit status is 0 for CLEAN, 1 for DAMAGED and 2 for a usage or read error. The report is
written to a new temporary file, synced, and renamed into place; an existing report is never
overwritten. Every input goes through the sealed-data guard, so the tool cannot read a hold-out day
the seal still covers.

## First results

- **2026-11-15, development day: CLEAN.** The largest interval is 2 s in either feed.
- **2026-11-19, tune day: CLEAN.** ADS-B has one 1,005 s outage, from 23:24:42Z, in the archive
  itself; SFDPS has no interval over 24 s. That outage, with its margin and cut at the window's
  end, is the one excluded interval: 1,158 s.

Under P-006 as first written, that one archive outage would have voided the whole day. P-007 was
written to handle it before any hold-out data was read.
