# `oa-demo-scenario-prep`

This private-shell tool reads only the pinned public FAA CIFP archive and the selected public
737 type-certificate source. It refuses an unpinned CIFP file or parser errors. It never reads
recordings, SWIM data, or facility adaptation.

## List composable KORD route selectors

In an approved guarded execution slot, run the built executable to list exact selectors for
resolver-accepted route assemblies. It composes STAR transition, common and runway routes, then
approach transition and final routes. A nonmatching STAR exit and approach entry are joined only
by a clearly labeled synthetic DCT segment:

```text
/path/to/guarded/oa-demo-scenario-prep --list-route-chains --cifp /private/path/CIFP_261029.zip
```

The command emits public CIFP selectors, source hashes, and an explicit `--synthetic-direct` marker
when needed. It does not write an asset. Fixed limits bound route indexing, candidate-chain checks
and output size; if a limit is exceeded it refuses without printing a partial list. Resolver
continuity does not prove that the 737 scenario materializes; run the selected tuple through the
normal prep command to check all scenario constraints.

When only one deterministic selector is needed, `--first-route-chain --cifp ZIP` follows the same
pinned-input and work limits, checks that the STAR runway transition matches the approach-final
runway, and applies the same bounded route-motion validation used by materialization before
stopping at the first accepted source-order composition. Its output reports refusal counts for
the inspected prefix and explicitly says the complete route population was not enumerated. The
exhaustive `--list-route-chains` mode retains its fixed candidate cap and refuses the entire list
if it is exceeded; it never silently truncates. The exhaustive listing validates selector
resolution and runway compatibility, but does not claim motion feasibility.

`--list-candidates` remains a lower-level exact-endpoint listing. `--diagnose-candidates` prints
the underlying endpoint and refusal details when neither listing finds a route.

If the complete-route list is empty, the bounded diagnostic mode emits each KORD STAR/approach
selector, first/last fix, path-terminator sequence, route-resolution result, and exact STAR-last to
approach-first join count. It does not invent a connector or alter CIFP geometry:

```text
/path/to/guarded/oa-demo-scenario-prep --diagnose-candidates --cifp /private/path/CIFP_261029.zip
```

The diagnostic uses the same pinned archive/parser and refuses parser errors, work limits, or
output limits without printing a partial result.

## Prepare the browser asset

The normal command requires the exact candidate selector tuple from the listing, the caller's
SHA-256 of the official JCAB TCDS No. 38 Rev. 12 PDF, and private output destination configured
in the source. Use `--outputs oasd` to produce only the OASD v1 browser asset and its private
provenance record. The source pins `CIFP_261029.zip` by its known official SHA-256 and requires the
entry `FAACIFP18`; do not substitute a newer CIFP cycle without a separately reviewed source
change. The OASD whole-file SHA-256 reported by prep must be carried into the WASM build's
`OA_DEMO_SCENARIO_ASSET_SHA256`; the selected reviewed source commit is supplied separately as
`OA_F2_SOURCE_COMMIT`.
