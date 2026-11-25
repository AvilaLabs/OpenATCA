# ADS-B tools

`oa-adsb` reads and writes ADS-B history clips. The `surface_counts` example is an offline,
counts-only-by-default diagnostic for a verified private archive manifest; it does not classify
runway or taxiway occupancy and is not a runtime safety product.

## Optional private position export

The existing invocation remains count-only:

```text
cargo run --release -p oa-adsb --example surface_counts -- <private.complete> <private-counts.txt> <source-commit>
```

To opt in to a raw observation export, append `--position-csv <private-path>`. The CSV contains
transponder addresses and precise positions, so keep it private. It includes every valid-position
observation accepted by the archive filter's manifest bbox and UTC window, regardless of ground
flag. The fixed header is:

```text
addr,t_ns,lat_e7,lon_e7,ground,stale,direct_adsb,source
```

Addresses are lowercase hexadecimal; times are Unix nanoseconds; coordinates are signed integer
degrees times 10^7. The three flags are `0` or `1`; `source` is the numeric `u8` index into readsb
`addrtype_t` order. When CSV is enabled, the `.source` sidecar records the exact mapping in
`position_csv_source_index_map`, keyed by `position_csv_source_index_order`.
The mapping also labels `255` as `none_no_source_column` for legacy traces; those rows have
`direct_adsb=0`.

The CSV path must be a new path distinct from the count output, its `.source` sidecar, the
completion manifest, and every archive part. The writer caps the CSV at 256 MiB before each write
and uses the existing 10,000,000 positioned-report limit. On I/O or capacity errors it removes its
uncommitted file; if trace entries were refused, the sidecar labels the CSV
`partial_refused_trace_files_present`, never `complete`. Downstream consumers must require a
`complete` status and verify the recorded CSV SHA-256 and row count before treating the export as a
complete census. The existing address/time-bin summary is bounded to 1,000,000 entries, about 115
addresses observed in every 10-second bin across a full day; sparse stationary traces consume fewer
entries. The report denominator remains position-bearing address/time bins, not all aircraft.
