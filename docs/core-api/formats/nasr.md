# NASR public API proposal

**Tier:** Preview.

**Proposed public surface:** independent typed functions such as `parse_airports`, `parse_runways`, `parse_fixes`, `parse_navaids`, `parse_airways`, `parse_class_airspace`, and `parse_procedures`, accepting `BufRead`, source label, schema set, and explicit limits. Return `Parsed<T> { rows, row_errors, counts }`; reserve fatal `ParseError` for stream/schema/I/O/limit failures. Expose schema parsing/validation as a separate entry point, and never identify columns by positional index.

**Source-backed contract:** FAA NASR subscription CSV format as described by that subscription's data-structure CSV. The source does not pin one immutable NASR edition/cycle in the library contract; each data release's effective date and schema hashes must travel with a dataset bundle. Positions are derived from degrees/minutes/seconds and hemisphere and cross-checked against published decimal coordinate values. Coordinates currently use `f64` degrees. NASR effective date is a release date, not a row-event time.

**Bounds/refusals:** parser delegates CSV record, field, and file constraints to `oa-csv`; typed row issues are collected rather than silently dropped, with 10,000 rejected rows retained per file and further rejections counted. Schema violations are capped at 100,000 retained per file with dropped count. CSV syntax, missing columns, size limits, and I/O are fatal. Errors should retain source, physical line, column and typed reason. Preserve raw schema validation distinction from trimmed typed values.

**Gaps/stability:** subscription-specific schemas and cycle/version identity need a public data bundle convention; current API takes readers and likely assumes callers supply correct schema. No encoder. Stable evidence is not established. Preview only.
