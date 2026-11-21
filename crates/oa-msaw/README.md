# oa-msaw

OpenATCA's minimum safe altitude warning (MSAW) evaluates a predicted aircraft path
against terrain, obstacles and approach surfaces. It issues a caution for a predicted
floor crossing and escalates to a warning when the crossing is within the configured
warning time. The crate is the deterministic core. The caller provides tracks, floor
samples and facility settings.

## How to run a cycle

1. Call `msaw_new` with a validated `MsawConfig` to allocate the corridor index, track
   table and cycle buffers.
2. Call `msaw_plan` with the timestamp and current tracks. Read the sample points
   through `msaw_points` and supply one `FloorSample` for each point in order.
3. Call `msaw_evaluate` with those samples and the cycle's inhibition and state
   tables. Read the alert log, skip log and published net state. Call `msaw_finish`
   to end open alerts when the input ends.

The required settings include warning times, clearances, corridor geometry, altitude
age limits and capacity bounds. An approach capture box applies to a track aligned
with its inbound course and wholly inside the box at the point being checked. Outside
applicable corridors, the terrain and obstacle floors use the configured clearances.
The minimum vectoring altitude is available for separate reporting.

When a point lacks terrain or obstacle coverage, the net reports degraded input and
continues to check covered points. Missing or stale aircraft altitude is reported per
track. The net degrades when the affected share exceeds the configured threshold.
An open alert is retained with a protection-lost reason while altitude is unavailable.

The forecast uses straight-line motion and the supplied vertical rate over a fixed
look-ahead. Fixed capacities bound the work per cycle. The settings in this repository
are for development and replay. See [M5 design](../../docs/M5_DESIGN.md) and
[requirements](../../REQUIREMENTS.md) for the design and requirements.
