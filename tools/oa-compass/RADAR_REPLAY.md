# CAT048 radar-only replay

The shell command below reads one ordered CAT048 Ed 1.32 stream, tracks it with the radar-only
path in `oa-track`, and writes CAT062 Ed 1.21 for OpenATS COMPASS. It does not accept ADS-B or
ERAM inputs:

```text
oa-compass radar --cat048 <file.ast> --date <YYYY.MM.DD> --out <new-dir> \
  --site-lat-deg <deg> --site-lon-deg <deg> --site-height-m <ellipsoid-m> \
  --scan-period-ms <ms>
```

`--date` anchors the first time-of-day to UTC midnight of that date. One rollover is accepted only
when the prior I048/140 time is at or after 18:00 and the next is at or before 06:00. The adjusted
times must be nondecreasing and span no more than 24 hours. Output CAT062 files are separated by
UTC date because I062/070 is day-relative. The scan epoch is the supplied date's UTC midnight; no
CAT034 north-marker timing is inferred.

The command uses the explicitly named `MSSR_EUROCONTROL_1997` profile: 70 m range sigma, 0.08°
azimuth sigma, zero range quantum and 100 ft Mode C quantum. These are a documented model profile,
not calibration of the input sensor. Site latitude/longitude and antenna height above the ellipsoid
are required. Scan period is required and bounded to 100–600,000 ms. The profile is held fixed; no
whole-day estimate or future-data calibration is performed. A single-radar run disables relative
registration estimation.

The stream is read through the sealed-path guard, one ASTERIX block at a time (maximum block size
65,535 bytes). The full input is capped at 1 GiB, 10,000,000 records, and 10,000,000 exported
CAT062 rows. The output root must not already exist: the command refuses existing paths and never
changes their permissions. On Unix, it creates the new output root and each day directory with
mode `0700`, and creates new ASTERIX/metadata files with mode `0600`. Other platforms use their
native filesystem access controls. Exactly one CAT048 SAC/SIC source is allowed. Any CAT034 block refuses the whole run
with an unsupported-category message;
CAT034 is not silently ignored or decoded. Any other category also refuses. Records without
I048/140 time refuse because their ordering cannot be established. Records without I048/040 polar
position are skipped and counted. I048/090 flight level is used only when its V/G flags are clear.
Missing or invalid flight level is counted separately from conversion refusals. The command
supplies no site-height or user-selected fallback for a plot: the existing fusion path may use the
estimated height of an associated track; if no such estimate is available, the plot is refused for
missing height. The report retains both the input-condition count and actual conversion refusals.
This initial command intentionally has no height fallback option. Valid CAT048 flight level is
used as the model's ellipsoidal height; pressure-to-geometric altitude difference and atmospheric
refraction remain unmodelled.

The output uses SAC 255/SIC 14, a project-selected test pair, not a live allocation. Each UTC-day
folder contains `radar.ast`, `data_sources.json`, and a README with profile and refusal/loss counts.
The exporter reads CAT062 back before committing. Handled input-order violations, CAT034, malformed
blocks, sealed dates, output overflow, bounded fusion-work caps, and tracker refusals remove the
uncommitted output. Abrupt process or host termination can leave a private partial output tree;
cleanup is not guaranteed for crashes, and the command refuses to reuse an existing output root.
Inspect and remove any interrupted output before starting a new run.

This command has synthetic codec/rollover coverage only. It does not establish real-radar accuracy,
CAT034 handling, or equivalence to the separate dense synthetic fusion stress workload.
