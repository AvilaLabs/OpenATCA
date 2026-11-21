# oa-msaw

The `oa-msaw` command replays a development ADS-B clip through the tracker and the
minimum safe altitude warning net. It records warnings, cautions, advisories, input
gaps and timing for the run.

## Run a development replay

```sh
cargo run --release --locked -p oa-msaw-run --bin oa-msaw -- \
  --adsb development.oaf --config tools/oa-msaw/testdata/dev.cfg --out msaw-results \
  --terrain terrain-dir --dof obstacles.zip --cifp FAACIFP18.zip \
  --metar metar-dir --mva mva-dir --sfdps development-sfdps-dir
```

Use the [example settings](testdata/dev.cfg) as a starting point. They describe a
development test and contain no operational facility settings. `--terrain` supplies
3DEP tiles, `--dof` supplies obstacle records and `--metar` supplies altimeter
observations. The run reports degraded input when it lacks required floor data or
usable altimeter settings. `--cifp` supplies runway geometry and procedure data.
Set `cifp_area_margin_nm` to generate approach capture boxes and runway strips for
the clip's area. Use `cifp_airport` entries for airports that need departure corridors
as well.

`--sfdps` enables flight association and VFR eligibility. Without it, every
evaluated track is eligible for an alert. `--mva` supplies the chart reference for
separate below-MVA reporting. To limit replay, pass both `--from-s` and `--to-s`
as seconds after the first measurement.

## Read the results

- `alerts.csv` records caution and warning episodes. `advisory.csv` records violations
  for VFR traffic excluded by the eligibility rules.
- `skipped.csv` records coverage and tracking gaps. `below_mva.csv` records tracks
  predicted below the chart altitude without an alert-floor violation.
- `inhibitions.csv` records configured inhibitions and the alerts they suppressed.
- `summary.txt` records input coverage, net state, alert counts and compute use.
  Add `--diagnose` for a breakdown of the terrain, obstacles and path estimates
  behind the alerts.

The round 14 ZAU development replay recorded 763 alerts in one hour, of which 524
reached warning and 239 remained caution. It counted 587 episodes after grouping
alerts on the same track within 20 seconds. The project has no measured ERAM or
STARS alert count for that hour. The straight-line descent forecast can raise alerts
for aircraft that subsequently level off or land.
