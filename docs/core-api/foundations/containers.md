# Compression and archive containers

## `oa-deflate` — writer

`Compressor::new(level)` owns fixed preallocated state; levels 1 through 9 are accepted and
out-of-range values are clamped (default 6). `write(input, callback)`, `flush(callback)`,
`finish(callback)`, `reset()` and `level()` are public. Raw DEFLATE is emitted to the caller's
callback; `GzipWriter` wraps it in a gzip member with `write`, sync `flush` and `finish`. The caller
owns the destination and I/O errors. Callback failure aborts and leaves compressor state
unspecified; discard or reset it. The compressor documents a 32 KiB window, 256 KiB input staging,
bounded hash/token/output arrays and no post-construction allocation. It provides no compression
ratio or output-size guarantee.

## `oa-zip` — ZIP reader, inflater, gzip reader, CRC

`Archive::parse(bytes)` / `parse_with_limits(bytes, Limits)` retain a borrowed archive and parsed
directory; `entries()`, `extract(name)` and `extract_entry(entry)` return entry metadata or
uncompressed bytes. `read_archive_file(path)` is an explicit shell helper with a size limit.
ZIP supports stored and DEFLATE entries. `Entry` exposes name, method, flags, CRC, compressed and
uncompressed sizes, local offset and ZIP64 status. Unsafe paths are listed but extraction refuses
them; no extraction-to-filesystem API exists.

The current bounded ZIP name profile is UTF-8/ASCII only: general-purpose bit 11 requires valid
UTF-8, while names without bit 11 must be ASCII until a legacy decoding table is supported. Invalid
high-bit names are refused instead of being lossy-normalized, preventing distinct raw names from
aliasing through replacement characters. This is a Preview profile choice, not CP437 support.

Default archive bounds: 10,000 entries; 256 MiB per output entry; 1 GiB total output; ratio 200
after a 1 MiB exemption; 4,096-byte names; 1 GiB archive; EOCD search 65,557 bytes. `Limits`
allows tighter limits. Inflate has a fixed 32 KiB history window and caller-provided output/ratio
limits; errors include truncation, reserved block type, bad LEN/NLEN or Huffman table/code/symbol,
too-far distance, output cap and ratio cap. Streaming `Inflater` takes chunks, reports total in/out,
sync output and finished state. `GzipReader` accepts concatenated members, has a 64 KiB header-field
cap and reports truncation summary rather than disguising it as clean completion. `crc32` and
`crc32_update` are pure byte functions.

## `oa-tar` — streaming archive reader

`TarReader::new(parts: Vec<R>, Limits)` concatenates split `Read` sources into one stream.
`next_entry()` yields `Entry(Header)`, `End`, or `Truncated`; `read_data(&mut [u8])` yields bytes,
entry end or truncation; `read_entry_to_vec` is a convenience whose caller must bound the output.
Formats include ustar/V7, GNU long names/base-256, and bounded PAX path/size handling. It does not
write/extract files and returns stored names without path-safety policy.

Defaults: entry up to 8 GiB; name up to 4,096 bytes; a single `next_entry` may consume at most 64
metadata records and 1 MiB total metadata, with each metadata record at most 64 KiB. `TarError`
reports bad header/checksum/number/name, size/name/meta/work bounds, I/O with stream offset, or a
reader already failed. EOF before the two-block archive end marker is explicit truncation.

**Tier/stability proposal.** L0 container codecs, but keep writer and untrusted-input readers
separate. Current tests/fixtures cover format cases and adversarial archives; source inspection
does not establish RFC 1951/1952, PKZIP or POSIX/GNU conformance across all edge cases. Complete
§8 with published-standard vectors, 24-hour fuzz, reviewed mutation outcomes, Kani where integer
and byte parsing warrants it, no-panic review, cross-architecture results, language examples and
published safe vectors. Explicitly qualify ZIP64/data descriptors, gzip truncation/concatenation,
tar extension semantics and archive-wide resource accounting. APIs consult no wall clock, trust
seal, filename policy or external state; all input bytes, readers, callbacks and limits are
explicit. A digest or archive CRC is not authenticity.
