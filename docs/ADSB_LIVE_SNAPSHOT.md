# Bounded adsb.lol development snapshots

`oa-adsb-snapshot` decodes one adsb.lol V2 regional JSON response from stdin. It writes one
OAF2 MESSAGE frame carrying the existing ADS3 payload to stdout; counters go to stderr. The
capture adapter `tools/oa-stdds-ref/capture_adsb_lol.py` retains each exact response, frame, local
wall and monotonic receipt, request parameters, decoder/capture source hashes, and a hashed
completion manifest. It requires a new output directory below `openatca-private`, uses the shared
sealed-data guard for every input/output path, and writes `COMPLETE` last. A partial run has no
`COMPLETE` marker and retains an `INCOMPLETE.json` reason.

Synthetic input example (not recorded data):

```json
{
  "ac": [{
    "hex": "a1b2c3", "type": "adsb_icao", "seen_pos": 1.2,
    "lat": 41.9, "lon": -87.9, "alt_baro": 12000,
    "alt_geom": 12200, "gs": 250.5, "track": 359.9,
    "baro_rate": -640, "geom_rate": -700,
    "nic": 8, "nac_p": 9, "nac_v": 2, "sil": 2,
    "sil_type": "perhour", "version": 2, "gva": 2,
    "sda": 1, "nic_baro": 1, "mlat": [], "tisb": []
  }],
  "ctime": 1795557600000, "msg": "No error",
  "now": 1795557600000, "ptime": 1, "total": 1
}
```

The API schema defines `now`/`ctime` as Unix milliseconds, `seen_pos` as seconds since the last
position update, latitude/longitude as decimal degrees, altitude as feet, ground speed as knots,
track as true degrees, and rates as feet per minute. The decoder estimates the report time as
`now - seen_pos`; it does not replace missing age with zero, clamp future report times, or fall
back to `lastPosition`. Exact source age precision is preserved in the raw response. This timestamp
is a snapshot-derived estimate with cache, age-quantization, and transport uncertainty. Do not
use it as precision benchmark truth without a separate prospective accuracy study.

The response envelope must have matching `total` and `ac` counts and `msg == "No error"`. The
decoder rejects duplicate object keys, malformed envelope timestamps, oversized bodies, and
oversized row sets. Per-aircraft rows without a current `lat`/`lon`, an address, a valid `seen_pos`,
or a known source type are separately counted or refused. Rows older than the configured age cap,
outside the default ZAU box, at reserved addresses, or inside a compiled sealed time window do not
reach the OAF payload. It counts the source `type` distribution and whether `mlat`/`tisb` fields
are present; those flags are not evidence of independent radar. An unknown or non-ADS-B source is
never relabeled as ADS-B.

There are no callsign or proximity joins. Ground state is set only by `alt_baro: "ground"`; a
missing altitude remains missing. Quality values are retained only when within the crate's known
field ranges; absent or malformed values are not filled in. The raw response and per-snapshot
stderr summary preserve the exact source inputs and exclusions. Every JSON object is capped at
128 members and the structural walk is depth/node bounded. The envelope's local receipt, server
`now`, and `ctime` are checked against sealed windows before the aircraft array is decoded;
individual estimated position times are also checked. The `mlat` and `tisb` identity arrays must
be present and well formed on position rows. A direct ADS-B type paired with nonempty
MLAT/TIS-B-derived fields is counted as a source conflict and excluded from reports, so it
cannot be treated as independent ADS-B truth.

The official OpenAPI schema checked on 2026-11-24 explicitly includes `mlat` and `tisb` in
`V2Response_AcItem.required`; this requirement is for that V2 endpoint, not every readsb JSON feed.

The capture wrapper has fixed ceilings: 300 seconds, polling no faster than every 5 seconds, 60
responses, 1 MiB per response, 64 MiB cumulative response bytes, 256 MiB OAF bytes, 15-second
maximum request timeout, and 30 seconds maximum position age. It makes one current METAR request
to the official Aviation Weather Center API and retains its response and HTTP receipt metadata.
It allows at most 32 METAR station identifiers. Its adsb.lol query is fixed to the documented
ZAU coverage circle (42.5 N, 88.5 W, 250 nm), and the decoder applies the fixed 38.5–46.5 N,
83.5–93.5 W box; rows outside that box are excluded. Cohort bounds require explicit timestamps
with exact millisecond precision. Output creation and hashing use the shared sealed-data guard,
and the decoder executable is hashed and run through the same verified open file. It does not
score data or publish FAA-origin SWIM recordings. Keep all resulting material private.

The schema and units above follow adsb.lol's official [V2 OpenAPI schema](https://api.adsb.lol/api/openapi.json)
and [open data API documentation](https://www.adsb.lol/docs/open-data/api). The capture uses the
public ODbL source; the official API documentation currently lists a 500-request daily allowance
without a key and documents HTTP 429/`Retry-After` on quota excess. This adapter makes no automatic
retry after network or quota errors.
