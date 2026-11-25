# ARINC 424 CIFP public API proposal

**Tier:** Preview.

**Proposed public surface:** `parse_reader(reader, source, Limits) -> Result<Cifp, ParseError>` and `parse_record(line, text) -> Result<Record, RecordError>`, plus typed queries for airports, runways, procedures, airways, airspace and MORA. Keep source names and line locations in errors. Put all caller-adjustable caps in an explicit `Limits` value; keep a documented `Limits::standard()` profile once reviewed. Avoid promising a generic ARINC file parser: the supported profile is FAA CIFP.

**Source-backed contract:** ARINC 424-18, FAA `FAACIFP18`; fixed-width 132-character records, 1-based columns, header-declared cycle with record cycles no later than header cycle. Current decoded sections include PA/PG/D/DB/EA/PC/PI/ER/PD/PE/PF/UC/UR/AS. Unsupported sections are counted/skipped. Coordinates are decoded to decimal degrees (`f64`) and altitude conversion returns feet; cycle fields represent FAA CIFP cycle identity, not timestamps.

**Bounds/refusals:** 256-byte line cap, 2,000,000 records, 16 continuations per primary, 16 header records, 1,024 fixes per airway, 256 routes per procedure, 256 legs per route, 4,096 boundary points, 1,000 stored record errors. Errors carry source and 1-based line; field failures carry 1-based column span. Fatal framing/limit/cycle/header issues stop the parse; record-level decode errors are retained and counted. Public API should make `errors_dropped` visible and avoid calling a result “clean” if any errors were dropped.

**Gaps/stability:** current implementation covers a CIFP subset and skips other record classes; it is not a general ARINC 424 encoder. Source tests cover fixtures/layouts and cross-checks, but the full Stable evidence set has not been established. Preview until unsupported-section inventory, edition exceptions, cross-arch results, fuzz/mutation evidence and public vectors are complete.
