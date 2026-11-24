# `oa-regional-session`

This offline runner composes the actual flight table, tracker, independent route converter,
shared trajectory baseline, observed-traffic conflict alert, safety status and raster scope.
Every completed frame has one receipt cutoff. Forecast intent never feeds or suppresses the
observed-traffic conflict detector. Inputs are explicitly normalized; raw FAA adapters and a
windowed controller workstation are separate integration work.

```text
oa-regional-session run --input examples/synthetic-session.tsv --synthetic-model --out NEW_DIR
oa-regional-session run --input FILE --snapshot ADAPTATION --out NEW_DIR
oa-regional-session run --input examples/synthetic-msaw.tsv --synthetic-model --synthetic-msaw --out NEW_DIR
oa-regional-session run --input FILE --snapshot ADAPTATION --msaw-inputs MSAW_MANIFEST --out NEW_DIR
oa-regional-session stage --input FILE --out NEW_DIR
```

The first command exercises two-report tracker confirmation, two aircraft, route forecasts,
controller handoff offer/accept/take, unauthorized and authorized altitude requests, feed loss,
reacquisition, a failed route amendment and an explicit assigned-altitude clear. The fixture
and its two-fix adaptation are public synthetic inputs. The older `synthetic-normalized.tsv`
is an input-staging example; its route text is not the converter's supported grammar.

Run an installed standalone executable. Cargo normally hardlinks its build output into `deps/`;
copy that binary to a new installation path before running it. Executable provenance hashing
uses the same guard that refuses multiply-linked inputs, with no exception for build artifacts.

`stage` validates/group-counts input and labels its output `STAGED_INPUT_ONLY`. `run` executes
product cores and writes fixed-point flight/trajectory evidence and PPM scope frames, settings,
a manifest of input/adaptation/executable/settings/frame hashes, and `COMPLETE` last. A run is
closed only when `COMPLETE` matches the manifest digest and every listed file's hash matches.
Existing output directories are refused. An interrupted run is incomplete; reconstruct state
from the original admitted prefix into a new directory. This is recomputation, not restoration
of an in-memory checkpoint. The separate `oa-scope-replay` publisher supports verified raw-frame
publication resume; that feature is not implicitly a regional runtime checkpoint.

Source qualification is external to this runner: the publication gate records the exact clean
source commit alongside the executable and output hashes. This runner alone does not establish
that an executable was built from a claimed source commit. Frame evidence uses source-versioned
formatting and is reproducible for a fixed executable, not a general wire API.

The development envelope is explicit: 32 retained flight records and 32 simultaneous tracks; 512 admitted frames;
whole-second positive receipt cutoffs separated by at most 60 seconds; 100,000 input events;
256 events per receipt; 256 retained route revisions and 256 cursors across the session; 16 MiB
normalized input; 768 bytes per row; 256 bytes per filed route/cursor text; 64 MiB adaptation.
Capacity and work exhaustion refuse explicitly. Offline shell allocation is permitted; this
composition makes no real-time allocation or deadline qualification claim.

Each frame retains completed CA alert episodes and altitude-loss events as well as currently
open alerts. The core logs are drained only after that evidence is copied. Their payload budget
is one cycle: 496 completed episodes (47,616 bytes) and 32 altitude losses (1,024 bytes), rather
than a completed-alert allocation multiplied by the 512-frame session limit. Log-overflow
counters remain in the cycle statistics; end-of-input open episodes remain explicitly open.

The scope is 800×600, centred on 41°N, 87°W at 180 NM. The centre view is `ZAU1`. Normalized
positions here mean airborne traffic; ground/surface classification is not inferred. The CA
settings (1,300 m horizontal, 300 ft vertical, 120 s lookahead) are synthetic development values,
not operational FAA separation minima. Other safety nets display unavailable state until they
have actual evaluated inputs. Transport receipt and accepted observation freshness are separate
evidence; a live transport cannot make an old or rejected measurement current.

MSAW is evaluated only when explicitly configured. `--synthetic-msaw` uses a public flat
500-ft terrain model, 500-ft clearance and synthetic weather in the 40–42°N, 88–86°W test box;
it requires `--synthetic-model`. The synthetic descending-aircraft example exercises caution
and warning with the actual core. It does not qualify operational terrain coverage or tuning.
`--msaw-inputs` reads a bounded UTF-8 manifest beside its referenced files:

```text
OA-MSAW-INPUTS<TAB>1
CONFIG<TAB>path/to/config.json
TERRAIN<TAB>path/to/terrain-directory
DOF<TAB>path/to/obstacles.zip
METAR<TAB>actual_file_receipt_unix_ms<TAB>path/to/metar.json
```

CONFIG is required; TERRAIN is optional and unique; at most 32 DOF and 32 METAR files are
accepted. Each weather file must have actually arrived by the first session cutoff; report
observation times also pass causal admission. Missing terrain, obstacles, weather and chart
coverage remain explicit. Automatic CIFP corridor loading and controller-authorized MSAW
inhibition/service requests are not yet bridged and configured requests are refused.
`msaw-data.txt` records supplied settings, source hashes, receipt stamps and loader limits,
bound by the closed run manifest; frame evidence records the exact terrain tiles used.

The current MSAW alert markers use the matching current tracker position and retain latched
episode severity. A lost-altitude alert or failed provider is labelled protection lost; no
current position means an unlocated count, never a marker at the alert's historical origin.
Warning, caution, lost, off-map and unlocated counts remain in the scope legend. A terrain
reader failure isolates MSAW and leaves tracking, observed conflict alerting and scope running;
resuming MSAW requires fresh prefix reconstruction. This display has no aural alert or alert
acknowledgement workflow yet.

## Protocol and identity

The UTF-8 file starts with `OA-RS<TAB>1`. All fields are tab separated:

```text
v1  receipt_ns  source_ns  sequence  source  GUFI_HEX  ADDRESS_HEX  CENTRE  KIND  payload...
```

GUFI is the full 128-bit value; zero explicitly means unknown for raw positions and feed status.
Unbound non-address flight updates may carry address zero until a positive address binding arrives.
Positions, bindings, commands and routes require a positive aircraft address. Address includes the non-ICAO bit where present; centre is four
ASCII bytes. Source cannot follow receipt. Paths and both timestamps pass `oa-sealed`; sealed
time envelopes are refused before identity/body parsing. Receipts increase between groups.
Within one receipt the categories are surveillance, flight, route, weather, adaptation; sequence
numbers increase per category. Older source stamps remain visible and cannot silently replace
newer source state. Flight/trajectory timestamps require exact whole milliseconds.

`POS` carries lat/lon in 1e-7 degrees, optional altitude feet (`-` absent), east/north velocity
mm/s, and optional vertical rate ft/min. `FLIGHT` carries authoritative upstream plan, address,
identity, clearance and ownership updates. Its bootstrap `OWNER` operation is trusted normalized
source state, not controller authority. `FEED` carries `UP`/`DOWN` and a reason code.

`FILED_ROUTE` carries a 12-byte plan ID, a legacy meter label (not used to guess an active leg),
and the converter's explicit identity-qualified grammar, for example
`FIX:FIXA:K1:1:- DCT:FIXB:K1:1:-`. General ICAO route parsing remains open. Legacy `ROUTE`
coordinates are counted as refused for forecasting because they cannot establish complete
identity/path/cursor semantics. The newest eligible failed or changed-plan route blocks fallback.

`CURSOR` carries plan ID, SHA-256 of the exact filed route text, and one qualified `FIX:...` token.
It explicitly identifies the active next fix; there is no nearest-fix progress inference. The
adapter binds it to the complete converted payload digest, including adaptation and constraints.
Only supported geometry is admitted. Both altitude and speed restrictions survive conversion;
IAS is never treated as groundspeed. The baseline still lacks wind, aircraft performance,
forward uncertainty and altitude-crossing feasibility; these remain explicit output limitations.

`COMMAND` carries plan ID, expected target generation, displayed frame sequence, local actor ID,
and a typed operation/value. Development actor IDs 1 and 2 map to `SECTOR1` and `SECTOR2`.
Operations are `ASSIGNED`, `UNASSIGNED`, `INTERIM`, `CLEAR_INTERIM`, `HO_OFFER`, `HO_ACCEPT`,
`HO_TAKE`, `HO_CANCEL`. Handoff values name stable unit IDs. A request must match the exact
visible flight/address/centre/plan/generation and a fresh track; the bridge then rechecks the
authoritative flight and actor's ownership/offer authority. Ambiguous, stale or unauthorized
requests are recorded as refusals. This local actor registry is not network authentication.

The controller interaction core is independent of the flight table. Its queue acceptance is
request acceptance, not authority transfer. A valid handoff acceptance still leaves the original
owner in place until a valid take. A level observation never teleports to a requested altitude.
