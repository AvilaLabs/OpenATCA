# Reproducing the development ZAU comparison

`reproduce_zau.sh` is an offline, development-only comparator. It does not fetch archives,
select a release, certify an operational claim, or make a runtime product. Before an operator
creates a package, the coordinator must identify the canonical adsb.lol release tag and obtain
the complete release inventory from its published page. Record the canonical tag and each part's
exact published name, byte length, and digest; verify that every advertised part is present and
that the local bytes match. A plausible-looking subset is not a complete day. The wrapper checks
only the files named by the manifest and cannot establish that the manifest lists the whole
release. Keep the manifest, archive parts, ERAM export, and outputs private; the ERAM export
contains address/callsign-linked observations.

The package manifest is ASCII `reproduce-zau-manifest 1`, at most 64 KiB and 512 lines. Scalar
keys occur once, unknown keys are refused, and archive entries are unique. It carries day, source
commit, label, canonical release URL, archive part names/sizes/SHA-256, the exact clip arguments,
clip digest, ERAM export digest (and optional XZ digest), and expected `p002.txt`/`samples.csv`
hashes. The format and parser are intentionally narrow; clip arguments may contain only one each
of `--box`, `--from`, `--to`, and `--frame-ms`, with their bounded values. No shell fragment or
additional clip option is evaluated.

Resource limits are fixed before examining any case result: at most 16 archive parts, 8 GiB per
part and 64 GiB total; at most 512 MiB for the ERAM export/report, 8,000,000 rows, 100,000
flights, and 16 KiB per export line. The compressed input is capped independently of its claimed
unpacked size. The XZ reader has a 128 MiB decoder-memory limit, reads at most 64 KiB of compressed
input and emits at most 1 MiB of expanded output per step; it accepts one stream and refuses
truncation or trailing members. These caps bound whole-day workstation I/O and parsing; they are not thresholds
tuned to observed outcomes. The sidecar is capped at 4 KiB and the drop list at 1 MiB. Oversize,
incomplete, malformed, sealed, or digest-mismatched input is refused; partial decompression is
removed on failure.

`p002 --eram-export` requires explicit `--development`, refuses `--damage-report`, and continues
to use the existing sealed-path checks. The scorer's strict SFDPS, damage, ledger, and hold-out
validation paths are unchanged. `--allow-dirty` alone does not authorize export scoring.

The comparator uses the same recording-derived ERAM inputs and deterministic scorer configuration
as the source run. It checks package hashes and exact checkout commit before building, then compares
clip, normalized report, and CSV outputs. The normalized report masks only the scorer-binary hash;
the exact report hash is still shown. A byte-identical development replay demonstrates only that
the pinned source and supplied package reproduce those outputs under the stated normalization. It
does not establish a new independent recording match, truth completeness, or operational accuracy.
