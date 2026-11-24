# Shared issue-time trajectory baseline

`oa-legacy-traj::core::trajectory` is a pure, fixed-capacity component. Its current API is
`issue_trajectory(&TrajectoryIssueInput) -> TrajectoryIssueOutput` plus the independent evidence
selector `select_revision_evidence(key, cutoff_ms, candidates) -> EvidenceSelection`. It is an
implemented component baseline; it is not yet the shared operational trajectory consumed by the
other tools.

## Input contract

The caller freezes the issue-time view and supplies plain values. The core does not read a clock,
choose or mutate flight state, allocate, call back into the shell, or select a route as part of
`issue_trajectory`.

`FlightKey` is the full `gufi: [u8; 16]` (the 128 GUFI bits in network byte order),
`aircraft_address: u32`, `centre: [u8; 4]`, and `plan_id: [u8; 12]`. A valid key has a
nonzero GUFI and aircraft address, address at most
`0x01ff_ffff`, and padded printable centre and plan identity. The revision selector identifies
the queried flight by GUFI, aircraft address and reporting centre. Plan ID is revision state,
not a filter: a newer source-time plan change participates in selection. The shell must compare
the selected full key, including plan ID, with the current flight view and refuse a mismatch; it
must not fall back to an older matching plan.

Times are signed Unix milliseconds. The caller supplies `cutoff_ms`; no timestamp is inferred.
For route and cursor evidence, `source_ms <= available_ms <= cutoff_ms` is required. The selected
route carries its complete key, source and availability times, message ID and payload SHA-256 in
`RouteProvenance`. The cursor repeats the full flight key and binds to that route payload hash.
It also carries the exact full `next_fix` identity and its own source, availability and message
ID. Cursor evidence must be available by the cutoff and match both the issue key and route hash.

`CurrentTrack` carries both `state_ms` (the time coordinates and motion already represent) and
`newest_source_ms` (the newest actual measurement supporting that state), plus availability,
message ID and aircraft address. Its required ordering is
`newest_source_ms <= state_ms <= available_ms <= cutoff_ms`; state may be at the cutoff even when
the underlying measurement is older. The caller supplies `max_track_age_ms`; the core checks
`cutoff_ms - newest_source_ms` with checked subtraction. It refuses a future state, invalid
coordinate, mismatched aircraft address, negative age policy, stale track or nonpositive/missing
groundspeed explicitly. The caller must provide the already-emitted tracker state; the forecast
does not extrapolate the tracker state to cutoff a second time.

`OwnerProvenance` retains the owner state, full eight-byte owner unit and source, availability
and message stamps. Unknown ownership is represented as unknown and flagged; the core does not
invent a unit. `Clearance` retains a typed `ClearanceForm` (`Absent`, `Exact`, `Window`,
`AltitudeAtFix`, `Block`, `Vfr`, or `Unsupported`) and its altitude values and evidence stamps.
An absent form is represented by zero evidence stamps. Unsupported or unavailable forms are
reported as such; the component does not coerce them into a scalar altitude.

`RoutePoint` is a 60-byte bounded per-route input record (A3 exception documented in source), with a
complete `FixIdentity`: length-tagged `ident[12]`, `region[4]`, length-tagged `scope[12]`, and
class. Empty scope is a valid explicit identity. Coordinates are signed degrees × 1e7; altitude
constraints are feet; speed bounds and measured groundspeed are millimetres per second.
Constraint kind, path geometry and speed reference are typed. Speed reference distinguishes
groundspeed, indicated airspeed and unknown. Only an explicitly groundspeed-referenced constraint
can be compared with observed groundspeed. Unsupported route constraints are declared and
degraded or refused, never silently reinterpreted.

The route slice may contain at most 256 points. The named checked forecast-work budget is
`MAX_FORECAST_WORK = 23,216` units at that maximum route size. The active cursor is required:
the core resolves its exact full fix identity uniquely in the filed route, then forecasts only
the suffix beginning there. It does not infer progress from proximity, route order, or the ETA
target. Missing and ambiguous cursor matches, a future cursor, or a cursor bound to another key
or payload hash are explicit refusals. Points before the active fix are counted as ignored.
Route-end extrapolation is refused.

## Outputs and limits

Every issue returns exactly three `HorizonOutput` records at 300,000, 600,000 and 1,200,000 ms
(5, 10 and 20 minutes) after `track.state_ms`. The issue cutoff gates which inputs are knowable;
it is not the epoch from which horizons are added when the tracker state is older. The ETA is
also measured from `track.state_ms`. Position is degrees × 1e7; unavailable positions use
`NO_POSITION_E7 = i32::MIN`, not a plausible coordinate. Altitude is feet, groundspeed is mm/s,
and predicted time is Unix milliseconds. Each horizon has its own availability, refusal and
flags. The optional ETA request is one exact full `FixIdentity`; its output retains that identity
and an ETA timestamp. A missing target, unsupported path or exhausted route does not produce an
invented ETA.

The current motion model follows supported great-circle route geometry at the supplied constant
observed groundspeed and uses constant observed vertical rate. A known exact-level or valid
window clearance may capture vertical motion at a boundary only when motion approaches/crosses
that boundary; the model does not teleport a level aircraft or reverse motion that moves away
from it. Static inverted altitude windows are refused only when the associated clearance evidence
is eligible at the cutoff; future or otherwise unavailable clearance payload values cannot affect
route feasibility or suppress the horizontal forecast. Unavailable clearance leaves altitude
unavailable and the horizon explicitly degraded. A valid altitude-crossing feasibility solver
along the route/profile is **not implemented**. Published route altitude constraints are reported
as unsupported by this baseline rather than treated as a solved vertical profile. Unsupported
geometry, gaps, route end and invalid inputs have named refusal/degraded states. A vector gap
crossed by a horizon remains flagged and degrades that horizon; ETA across a gap is refused. A
repeated exact target identity in the active suffix is refused as an ambiguous ETA target rather
than selecting the first occurrence.

The output flags disclose that wind, aircraft performance and forward uncertainty are not
modelled. In particular, constant groundspeed is not a wind-corrected airspeed forecast, and no
confidence interval or confidence score is invented. This component has no operational-service
qualification and makes no operational-safety or legacy-superiority claim.

`select_revision_evidence` is separately bounded to 256 candidates with a checked worst-case
`MAX_EVIDENCE_SCAN_WORK = 1,597,456` units. It considers only evidence
available by cutoff, gates future availability before examining other row fields, validates the
eligible set before selecting, and refuses malformed eligible evidence rather than falling back
to an older route. Source time selects across plan IDs. Different payload or plan values at the
same source time conflict; exact duplicates resolve deterministically by earliest availability
then message ID. Foreign GUFI, address and centre counts are disjoint, and over-capacity reports
all candidates as unexamined. The returned selected key and provenance must still be checked
against the frozen flight view by the shell.

The fixed output layout is guarded in source: `FlightKey` is 36 bytes, route provenance and
revision evidence are 96 bytes, active cursor is 128 bytes, each horizon output is 40 bytes, and
the issue output (including exactly three horizons) is 328 bytes. These guards describe the
current 64-bit ABI layout and do not imply an external wire format.
Required alignment bytes are explicit `reserved` arrays rather than implicit padding. Callers
zero them when constructing inputs; nonzero reserved bytes in eligible evidence are refused.
Future-available evidence remains gated before payload validation.

## Integration boundary

`oa-airspace::core::route::convert_route` is a separate bounded converter. It accepts caller-
normalized route steps and writes a fixed SoA route with exact identities and supported geometry;
it is not a general filed-route-string parser and does not infer the active leg. An adapter still
has to bind converter output and payload hash to a frozen flight view, supply the explicit active
cursor, and translate into this baseline's `RoutePoint` slice. The shared trajectory component
does not itself link the route converter, flight table, tracker, weather, conflict probe,
conformance monitor or display.

The planned regional runtime links have not been composed, and no combined regional session
runner is claimed to work. The earlier arrival study and its behavior remain unchanged.
