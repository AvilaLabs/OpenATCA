# oa-stdds

Parser for the FAA's STDDS (SWIM Terminal Data Distribution System) messages, as received
through SCDS and recorded by `oa-feed`. It is a shell-class crate, like its siblings `oa-fixm` and
`oa-tbfm`: every function depends only on the parsed document and the bytes it came from, with
no I/O and no clock (RULESET B1, B2). The crate's only I/O is its example.

## What it reads

| Module | Messages | Depth |
|---|---|---|
| `kind` | every STDDS message | Classified by root element and namespace. A message of no known kind is counted with its root name and namespace, never an error. |
| `tais` | TAIS `TATrackAndFlightPlan`: STARS terminal radar tracks with their flight plans | Parsed in full into `TaisRecord`, with every field in a fixed unit named in the field. This includes the fusion and sensor flags and the coast flag, which say how independent of ADS-B each track is. |
| `smes` | SMES (ASDE-X) Cat11 `positionReport`: airport surface tracks | Parsed to the same level, including the runway the target is associated with. |
| `status` | the service-status messages | Service name and state. |
| — | Cat10 `mlatReport` and `adsbReport`, the FIXM-mediated forms, APDS, ISMC, RVR and TDES | Classified and counted only. |

The element names come from the FAA's public FIXM-mediated STDDS data overview, through its
mapping tables (REQUIREMENTS.md OA-STD-001). The parser was first written without a real
message, then checked against the first recorded day. Every element of the TAIS track and flight
plan and of the SMES position report now parses, apart from runway text with damaged encoding,
which is refused.

## What is not yet confirmed

That check confirms the element names, the text forms and the value ranges. It does not confirm
**units**. A field whose doc comment still says ASSUMED is confirmed only by comparing its values
with an independent source, such as the same aircraft's ADS-B, and nothing downstream trusts it
before then. `tools/oa-stdds-ref` and `tools/oa-stdds-census` are where that comparison happens.

## Interface rules (`docs/INTERFACES.md`)

- A field that is present but unreadable is a counted field error that names the field and the
  byte offset. The record is kept with that field absent, and nothing is defaulted.
- A message whose structure is unreadable is refused whole.
- The output has a stated capacity, and reaching it is an error, never a truncation.
- Unknown elements are counted, never fatal (`stats`).
- No floating point is used: every value is a scaled integer.

## Example

```
cargo run -p oa-stdds --example stdds_summary -- FILE.oaf[.gz]
```

This prints per-kind counts, parse errors and per-field presence for a recorded feed. It prints
no position, identity, time or other message value: real STDDS data is reported as counts only.
Paths into the sealed hold-out data are refused (`oa-sealed`).
