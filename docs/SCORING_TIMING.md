# Scoring timing profiles

`p002` keeps its existing interval-derived timing when `--timing-profile` is absent. That path
continues to compute the set-A measurement interval distribution, use its median for the expected
interval and its 90th percentile for the initiation interval, and use the existing one-second
fallback when no usable interval exists. Its output text and CSV remain unchanged.

For explicitly marked development runs, `--timing-profile fixed-10-30-120-v1` selects a frozen
engineering profile:

| Setting | Value |
| --- | ---: |
| Expected measurement interval | 10 seconds |
| Initiation interval | 30 seconds |
| Track drop age | 120 seconds |
| Tracker output capacity | 1,048,576 rows |

The fixed branch does not call `interval_stats` and does not use the reference or ERAM/SFDPS
cadence to select tracker parameters. It changes only the three timing fields and preserves the
scoring output capacity and all other base configuration values. The 120-second drop age was
already the scoring path's fixed value (`oa_truth::LEG_GAP_MS`); it was not estimated from each
day's input. The profile names it explicitly so its complete timing configuration is recorded.

The profile is accepted only with `--development`. It is refused with frozen or candidate commit
bindings, `--combine`, and the export-only `--export-eram` mode. The separate `--eram-export`
scoring input remains a normal development input. This profile does not change any tracker core
defaults or frozen/candidate scoring semantics.

When selected, `p002.txt` includes machine-readable `SCORING_*` lines naming the profile, source,
all three nanosecond values, output capacity, and the fact that interval statistics were not
computed. The ordinary interval-derived path adds no lines, preserving its previous report bytes.

The source includes a synthetic regression that uses two measurement histories identical through
a cutoff and with different later cadences. It checks fixed-profile configuration equality and
compares real tracker rows through the cutoff, while confirming that the legacy interval-derived
configurations diverge. The cutoff is before both runs' final input, so final flush output is not
mistaken for prefix output. The regression is source-only until run by the designated coordinator.

## Optional publication grid

An explicit development run may also select `--oa-publication-grid sfdps-v1`. After replay,
the scorer rounds only OpenATCA tick latitude and longitude to the nearest whole arcsecond,
then renders that grid point to six decimal degrees before the existing E7 representation.
The extra rendering step matches the inspected SFDPS development messages. It rounds the raw east and
north velocity components independently to whole knots before the existing deciknot adapter.
M5 still uses the vector magnitude; this option does not round scalar ground speed.

The option changes the scoring view of the output. It leaves tracker state, timestamps,
altitude, the reference, ERAM and passthrough inputs unchanged. Pairing and metric algorithms
remain the same; rounded values can affect eligibility. Invalid positions and unavailable
velocity pairs are counted, and a finite velocity beyond the adapter's representable range
refuses the run. Reports and CSV comments identify the selected grid. Strict, combine and
export-only modes refuse this option, and the default output has no added grid annotation.

The run protocol determines whether this is a predeclared comparison or an exploratory
sensitivity analysis. Selecting this option does not make a development result a hold-out claim.
