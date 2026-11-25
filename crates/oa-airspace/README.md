# `oa-airspace` route conversion

`core::route::convert_route` expands a caller-normalized, fixed-capacity route against one
`Model`. The caller supplies up to `MAX_ROUTE_STEPS` route steps and owns the returned fixed
`MAX_ROUTE_POINTS` struct-of-arrays result. Conversion performs no allocation. Each model row
examined is charged before access against `MAX_ROUTE_WORK`; malformed accessed columns/spans,
ambiguous identifiers, unsupported constraints, and exhausted budgets produce explicit refusals.

`RouteStep::Fix` resolves one complete identity and emits an isolated anchor. `RouteStep::Direct`
records an explicit filed direct connector; consecutive fixes are connected only when this token
was supplied. `RouteStep::Airway` expands a contiguous NASR airway sequence from the exact entry
fix to the exact exit fix, in the stored forward direction, subject to the planned altitude and available MEA
constraints. Airway gaps and unresolved border crossings are refused. `RouteStep::Procedure`
requires the exact airport identifier and region, procedure kind and identifier, CIFP route type,
and transition identity. Reverse airway travel is explicitly refused: the current adaptation
model does not retain opposite GPS MEAs or opposite MEA directions. A missing forward GPS MEA
does not establish that reverse travel has no GPS restriction.

CIFP procedures currently accept only IF, TF, and DF path terminators, with decoded altitude,
speed, and turn constraints retained on their points. Course, distance/time, arc, vector, unresolved,
and other unsupported path/constraint forms are refused; no geometry is inferred for them. Source
path terminators and point identities are retained in output. A failed conversion returns an empty
new route, allowing a caller to retain its prior good route as stale display data without using it
for a current forecast.

Procedure references must identify an airport. Missing or inverted decoded altitude bounds are
refused; populated altitude fields without a supported description are not silently discarded.

The `oa-adapt route` command accepts normalized identity-qualified instructions:

```text
oa-adapt route --snapshot region.snap --steps 'FIX:ALPHA:K3:1:- DCT:BRAVO:K3:1:-'
```

For a pinned adaptation package, first create a new package directory from a validated snapshot:

```text
oa-adapt package --snapshot region.snap --out-root package-2026-11 --release cycle-2026-11 \
  --facility ZAU --change-ref CHG-123
```

The command reports a manifest SHA-256 pin. Provision that value independently, then pass it to
the verified route path:

```text
oa-adapt route --package-root package-2026-11 --expected-manifest-sha256 <64-hex-pin> \
  --steps 'FIX:ALPHA:K3:1:- DCT:BRAVO:K3:1:-'
```

Verified route mode checks the package on each invocation and decodes the exact bytes it hashes.
The pin is an integrity comparison; it does not authenticate who issued the package. The legacy
`--snapshot` route form remains a development path and labels its adaptation status unverified.

`FIX` and `DCT` fields are identifier, region, identity class and scope (`-` means absent).
`AWY` fields are designator, location, four entry-fix fields, four exit-fix fields and planned
altitude in feet. `PROC` fields are airport, airport region, procedure kind, procedure identifier,
CIFP route type and transition (`-` for a common transition). Labels are retained in full or
refused; no nearby fix is substituted. Output includes snapshot/instruction hashes, conversion
counts and fixed-point coordinates with constraints and continuity flags. Refused conversions
produce counts and no partial point list.

This is a normalized instruction grammar, not a general ICAO filed-route-string parser. Preferred
routes are not expanded. The caller must supply the active route leg separately to a predictor;
this converter does not infer flight progress. Conversion scans have an explicit 1,048,576-record
budget, including exact identity lookup across unsorted caller-built models.
