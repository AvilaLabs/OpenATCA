# OpenATCA requirements register

Every requirement has a stable ID. Every requirement must be verified by at least one test that
carries the tag `// REQ: <ID>` on the line above the test function. `oa-trace` fails the build if
a requirement has no test, or a test names an ID that does not exist.

IDs are never reused. A withdrawn requirement stays in the register marked WITHDRAWN.

## M0 — Airspace foundation

### General

- **OA-GEN-001** All crates forbid `unsafe` code.
- **OA-GEN-002** No crate depends on any external crate, including dev-dependencies.
- **OA-GEN-003** Every parser rejects an input that exceeds a stated limit (record length, field
  length, record count, string length) with an error naming the source, line and limit.
- **OA-GEN-004** No parser panics on any input. Arbitrary byte input produces either a value or
  an error.
- **OA-GEN-005** Every struct held in quantity as an array element asserts its exact size at
  compile time, and every index type is four bytes with one shared `NONE` sentinel (RULESET A4,
  A7).

### CSV (oa-csv)

- **OA-CSV-001** Parses RFC 4180: quoted fields, doubled quotes inside quoted fields, CRLF and
  LF line endings, embedded newlines in quoted fields.
- **OA-CSV-002** Fields are accessed by header name; a missing column is an error naming it.
- **OA-CSV-003** Errors report the 1-based physical line number of the offending record.
- **OA-CSV-004** Rejects a record whose field count differs from the header's.

### Geodesy (oa-geo)

- **OA-GEO-001** `LatLon` rejects latitude outside [-90, 90], longitude outside [-180, 180],
  and non-finite values.
- **OA-GEO-002** Converts degrees/minutes/seconds/hemisphere to decimal degrees, rejecting
  minutes or seconds outside [0, 60).
- **OA-GEO-003** Solves the WGS-84 inverse problem (distance, initial and final azimuth) with
  error below 1 mm against published reference values (GeographicLib test set).
- **OA-GEO-004** Solves the WGS-84 direct problem with error below 1 mm against the same
  reference values.
- **OA-GEO-005** Handles nearly antipodal and coincident points without failing to converge.
- **OA-GEO-006** Point-in-polygon for polygons whose edges are geodesics, correct for points on
  either side of every edge of the ZAU boundary.
- **OA-GEO-007** Computes the maximum separation between a geodesic and a rhumb line joining the
  same two points.

### NASR (oa-nasr)

- **OA-NASR-001** Validates each NASR CSV file against the FAA's `*_CSV_DATA_STRUCTURE.csv`:
  every column present, maximum lengths, numeric types, nullability. Violations are collected,
  not fatal, and reported with file, line and column.
- **OA-NASR-002** Parses ARTCC boundaries (ARB) into ordered point sequences per facility and
  stratum (HIGH, LOW, and FIR/CTA/UTA types).
- **OA-NASR-003** Parses fixes, navaids, airports, runways and runway ends with positions
  computed from DMS and checked against the published decimal value.
- **OA-NASR-004** Parses airways with ordered segments, minimum enroute altitudes and gap flags.
- **OA-NASR-005** Parses STAR and DP route structure with transitions and airport/runway
  associations.
- **OA-NASR-006** Parses class airspace designations per airport.

### CIFP (oa-arinc424)

- **OA-424-001** Parses the CIFP header record and the cycle of every data record. Records carry
  the cycle of their last change, so earlier cycles are expected; a record whose cycle is later
  than the header's is rejected. (Amended 2026-11-14: the original text, "rejecting a file that
  mixes cycles", contradicted how the FAA codes CIFP.)
- **OA-424-002** Parses airport (PA), runway (PG), VHF navaid (D), NDB (DB), terminal NDB (PN),
  enroute waypoint (EA), terminal waypoint (PC) and localizer/glide slope (PI) records.
  (Amended 2026-11-14: PN added; approach legs name terminal NDBs.)
- **OA-424-003** Parses airway (ER) records into ordered fixes with route type and altitudes.
- **OA-424-004** Parses SID (PD), STAR (PE) and approach (PF) records into ordered legs with
  path-terminator, fix, altitude and speed constraints, keeping continuation records attached
  to their primary record.
- **OA-424-005** Parses class B/C/D airspace (UC) and special use airspace (UR) into boundary
  sequences including arcs and their centres and radii.
- **OA-424-006** Parses Grid MORA (AS) records.
- **OA-424-007** Every field position used is verified by a test against a real CIFP record.
- **OA-424-008** A field outside its character class or range (bearings and courses over 360
  degrees, magnetic variation over 180, section and subsection codes, fix identifiers, the
  IFR flag, the header production flag) is an error naming the field.

### Airspace model (oa-airspace)

- **OA-ASP-001** Builds the model for one ARTCC and cycle from NASR and CIFP.
- **OA-ASP-002** Selects every object inside the ARTCC's HIGH or LOW boundary plus a
  configurable buffer, and every procedure of every airport inside.
- **OA-ASP-003** Reports every point present in both sources whose positions differ by more
  than 1 m, with the distance.
- **OA-ASP-004** Reports every reference to a point that cannot be resolved.
- **OA-ASP-005** Reports, per boundary, the maximum separation between geodesic and rhumb-line
  interpretations of its edges.
- **OA-ASP-006** Serializes the model deterministically: identical inputs produce a
  byte-identical snapshot and identical SHA-256.
- **OA-ASP-007** Diffs two snapshots, listing added, removed and changed objects with the
  changed fields.

- **OA-ASP-008** The selection area's bounding filter never excludes a point that is inside the
  area or its buffer: it is correct for areas and volumes crossing the antimeridian and includes
  the poleward bulge of every geodesic edge. A boundary that cannot be turned into an outline is
  an error, never read as "does not touch".

- **OA-ASP-009** A source record that is incomplete or out of range in a way that would change
  its meaning is a build error naming the record, never a stored default: an airspace circle or
  arc without a usable radius, a procedure leg course outside 0 to 360 degrees.
- **OA-ASP-010** An airway leg ends at the next row only when that row belongs to the same
  airway, lies further along it and starts at the point the leg names as its end; otherwise the
  leg has no end point and the report lists it. A gap flag is stored with its leg.
- **OA-ASP-011** A NASR airport and a CIFP airport with the same identifier are merged only
  within 1 NM (`MAX_AIRPORT_PAIR_M`); a farther pair is two objects, listed in the report as a
  disagreement, and the report states the cap.
- **OA-ASP-012** The snapshot writer enforces the limits the reader enforces (ARTCC length,
  file size), so every file it writes can be read back; a model whose column values are out of
  range (coordinates, altitudes, bearings, codes) fails validation.
- **OA-ASP-013** The `oa-adapt` command line rejects unknown, duplicate and valueless flags,
  empty values and malformed identifiers, and parses SHA-256 values as exactly 64 hexadecimal
  digits.
- **OA-ASP-014** A NASR navaid is unique by its own key (id, type, city, state) and is never
  dropped because its derived (ident, region) equals another's: where identities would
  collide, the NASR navaids among them take a scope (the airport of a terminal NDB, else
  state and city). A NASR navaid pairs with a CIFP navaid (D/V, D/B or P/N) only when it is
  the same ident and class and the nearest, within 1 NM (`MAX_PAIR_M`); two CIFP candidates in
  the cap, or one CIFP navaid wanted by two NASR navaids, pair nobody and are reported; a
  farther same-ident navaid is reported with its distance. A CIFP reference resolves by its
  own ident, region, section and airport, never to another station's point.

### Traceability (oa-trace)

- **OA-TRC-001** Lists every requirement without a test and every test tag naming an unknown
  requirement, and exits non-zero if either list is non-empty.

### Hashing (oa-sha256)

- **OA-SHA-001** Computes SHA-256 per FIPS 180-4, matching every NIST CAVP SHA-256 byte-oriented
  test vector (short, long and Monte Carlo messages).
- **OA-SHA-002** Hashes streamed input incrementally with the same result as one-shot hashing,
  for every split point of the test messages.
- **OA-SHA-003** Matches the straightforward reference implementation on 10,000 deterministic
  pseudo-random messages of 0 to 2,000 bytes, hashed one-shot and in chunks at random split points.
- **OA-SHA-004** Reports throughput in MB/s at 1 KiB, 64 KiB and 8 MiB (release builds only,
  informational, no assertion on speed) alongside the system `sha256sum` when present.

### Archives (oa-zip)

- **OA-ZIP-001** Reads ZIP archives (central directory, local headers, stored and deflate
  entries), rejecting encrypted, multi-disk and ZIP64-beyond-limits archives with a clear error.
- **OA-ZIP-002** Inflates DEFLATE streams per RFC 1951 (stored, fixed and dynamic Huffman
  blocks), verifying each entry's CRC-32 and uncompressed size.
- **OA-ZIP-003** Enforces limits on entry count, uncompressed size per entry and total, and
  compression ratio, so a crafted archive cannot exhaust memory or disk.
- **OA-ZIP-004** A ZIP64 extra field must hold exactly the values whose central-directory field
  is 0xFFFFFFFF, once; any other combination is rejected. An archive entry cannot be built
  outside the parser.

- **OA-ZIP-005** The streaming inflater (`Inflater`) takes input in pieces of any size and gives
  exactly the output of the one-shot `inflate` for every chunking from 1 byte up, for stored,
  fixed and dynamic blocks; it reports the bytes it consumed and the end of the stream, so bytes
  after the stream are left to the caller; and `inflate` is built on it. Output reaches the sink
  in runs of at most one window, and an error from the sink is returned unchanged.
- **OA-ZIP-006** The streaming inflater and the gzip reader keep hard limits: a total output cap
  (across all members for gzip) and a compression-ratio cap above a small exempt size, each its
  own error; a gzip bomb stops at the cap with an error and the good prefix delivered.
- **OA-ZIP-007** The streaming inflater and gzip reader allocate only when created: a fixed
  32 KiB window, three Huffman tables and fixed-size arrays, and no growable collection. The
  window and the tables keep their address and size through pushes, resets and members. (A
  counting allocator needs an `unsafe` impl, which RULESET A0 forbids in this crate, so the check
  is on the buffers themselves.)
- **OA-ZIP-008** The gzip reader parses every header form (FEXTRA, FNAME, FCOMMENT and FHCRC in
  any combination), verifies CRC-32 and ISIZE, and reads concatenated members, identically for
  every chunking from 1 byte up and in agreement with the system `gzip` (skipped if missing).
- **OA-ZIP-009** Each kind of gzip damage has its own error: bad magic (first and second byte,
  and at a later member), unsupported method, reserved flags, over-long name, header CRC-16,
  invalid DEFLATE data, CRC-32 and ISIZE; after an error the reader refuses further input.
- **OA-ZIP-010** A gzip stream cut short (a file a crash left after a sync flush) is read up to the
  cut: `finish` reports it truncated, with the stage (header, data, trailer), the bytes recovered
  from the unfinished member and how many of them lie before its last sync flush; all of it was
  delivered; the cut is never reported as success and never an error that loses the good data;
  a cut after a flush recovers exactly the flushed data, at every cut point.
- **OA-ZIP-011** Never panics: every byte flip, deletion, insertion and truncation of valid
  streams, and seeded noise in several chunkings, give output and errors for the streaming
  inflater and the gzip reader; the streaming inflater gives the same output and error for every
  chunking of the same input.

## M1 — SWIM input adapters

### XML (oa-xml)

- **OA-XML-001** Accepts the XML declaration with version 1.0 and encoding UTF-8 (any case) or
  none, and an optional standalone pseudo-attribute and UTF-8 byte order mark; rejects any other
  version, any other encoding, and a malformed declaration, each with its own error.
- **OA-XML-002** Accepts elements, attributes in either quote style, namespace declarations,
  comments, CDATA sections and whitespace, and produces a flat struct-of-arrays document in which
  every name, value and text is a span into the input; whitespace-only text between markup is
  not stored. `Parser::new` returns a `Result` (OA-XML-027).
- **OA-XML-003** Accepts character references (decimal and hexadecimal, with any number of digits
  including leading zeros: the body is scanned to its `;`, bounded only by the input) and the
  five predefined entities, and flags the attribute values and text that contain them. A value
  above U+10FFFF is `BadChar` however it is written.
- **OA-XML-004** Rejects any DOCTYPE, wherever it appears, with a dedicated error, so that
  entity-expansion (billion laughs) and external-entity (XXE) documents are refused.
- **OA-XML-005** Rejects processing instructions other than the XML declaration.
- **OA-XML-006** Rejects undefined entities and malformed or out-of-range references, with the
  offset of the reference.
- **OA-XML-007** Rejects input that is not valid UTF-8, with the offset of the first bad byte.
- **OA-XML-008** Rejects characters outside the XML 1.0 `Char` production, whether raw (in text,
  attribute values, comments, CDATA and names) or written as a character reference; U+FFFE
  and U+FFFF are `BadChar` everywhere.
- **OA-XML-009** Rejects mismatched end tags, unclosed elements, input that ends inside markup,
  and malformed tags, names and attributes. Every truncation of a valid message is reported
  at the end offset: `UnexpectedEof` when the input stops inside markup (a tag, attribute, end
  tag, comment, CDATA section, reference, the XML declaration or the opening of one),
  `Unclosed` when it stops between complete markup with elements open, `NoRoot` before any
  root, `Utf8` inside a multi-byte character.
- **OA-XML-010** Rejects duplicate attributes on one element, by qualified name and by expanded
  name (namespace URI and local name, Namespaces in XML 1.0 section 6.3: `a:x` and `b:x`
  bound to one URI are duplicates, wherever on the element the declarations sit). The cost
  per element is O(a^2) in its attributes, `a` at most `max_attrs_per_elem` (64 by default).
- **OA-XML-011** Rejects text, elements and other markup before or after the root element;
  whitespace and comments are allowed there.
- **OA-XML-012** Rejects an undeclared namespace prefix on an element or attribute, honouring
  declaration scope; `xml` is predefined.
- **OA-XML-013** Resolves an element's or attribute's prefix to its namespace URI span through
  the in-scope declarations, with a free function, including default namespaces, `xmlns=""` and
  unprefixed attributes. Each element carries the head of a chain of its in-scope
  declarations, so a call follows at most `max_ns_bindings` links (OA-XML-022).
- **OA-XML-014** Decodes a span's references into a caller-supplied buffer and fails, without
  writing past it, when the buffer is too small. All or nothing: on any error (too small, bad
  reference, span out of range) the buffer is unchanged. It validates and measures in a first
  pass and writes in a second, because restoring would need a saved copy of the buffer, which
  cannot be had without allocating.
- **OA-XML-015** Compares a span to a string, decoding references on the fly without allocating,
  and returns the error `decode_into` would return for the same span: it does not stop at the
  first mismatch, so the rest of the span is validated either way.
- **OA-XML-016** Enforces every limit (input bytes, depth, elements, attributes, attributes per
  element, name length, text nodes, namespace bindings in scope): a message at the limit parses and one unit over is refused
  with an error naming the limit and the byte offset.
- **OA-XML-017** Never panics: every truncation, every single-byte replacement, deletion and
  duplication of valid messages, and arbitrary byte strings, give a document or an error.
- **OA-XML-018** Every error carries the byte offset of the problem and a readable message.
- **OA-XML-019** A parser is reusable: tables keep their construction-time capacity across
  parses (no allocation after initialisation), and no state leaks from one message to the next,
  including after a failed parse.
- **OA-XML-020** Every real SWIM message in the local sample (`OPENATCA_SCDS`) parses; the test
  reports file count, maximum depth, maximum element count and throughput, and skips when the
  variable is unset.
- **OA-XML-021** The element table is a consistent tree: parents, first children and next
  siblings agree, and a walk from the root visits every element once.
- **OA-XML-022** The namespace bindings in scope at any point (the sum of the declarations over
  the open elements) are limited by `max_ns_bindings`, default 64, and the binding table has
  exactly that capacity, so every prefix lookup, while parsing and in `resolve_elem_ns` and
  `resolve_attr_ns`, is at most that many comparisons. The real corpus (19,025 messages)
  peaks at 15 bindings in scope; 64 is 4x that, rounded up to a power of two. In the worst
  case the default admits (64 live, the prefix found last) parsing costs under 10x the
  time per byte of the same message without the prefix.
- **OA-XML-023** Names follow the XML 1.0 fifth-edition `NameStartChar` and `NameChar`
  productions exactly, decoding UTF-8 code points, with an ASCII fast path; a colon only
  separates one prefix from a local name, and neither part may be empty.
- **OA-XML-024** Rejects the reserved namespace bindings: `xmlns:xml` bound to anything but the
  XML namespace, any other prefix or the default namespace bound to the XML namespace, and
  any binding of `http://www.w3.org/2000/xmlns/`.
- **OA-XML-025** Normalises on decode, not in the spans: `decode_into` and `span_eq_str` take a
  `SpanKind`. Text: `\r\n` and a lone `\r` become `\n` (XML 1.0 section 2.11). Attribute
  value: a literal tab, `\n`, `\r` or `\r\n` each become one space (section 3.3.3, CDATA
  type); character references are never normalised. A value that needs decoding for either
  reason is flagged; the fast path allocates nothing. CDATA sections are not flagged and
  keep their line ends raw (a recorded deviation).
- **OA-XML-026** A span that does not lie inside the input is `SpanOutOfRange` in every public
  function that takes a span, never an empty value.
- **OA-XML-027** `Parser::new` returns a `Result`. Every limit has a hard ceiling
  (`Limits::CEILING`: input bytes `u32::MAX`; elements, attributes and text nodes 16,777,216;
  depth and name length 65,536; attributes per element and namespace bindings 1,024), and
  input bytes, depth, elements and name length must be at least 1; a violation is
  `LimitsInvalid` naming the limit. Tables are reserved with `try_reserve_exact`, so a limit
  too large to allocate is `LimitsInvalid` too, never an abort.

### SCDS feed frames (oa-feed)

Format: `docs/FEED_FRAMES.md`. The bridge itself (`adapters/scds-bridge`, Java) is outside code
under RULESET F6 and is verified by its live run, not by the cargo tests.

- **OA-FEED-001** A frame of any kind round-trips every field exactly (kind, flags, seq, receive
  time, JMS timestamp, payload, raw bytes, stream offset), including empty and 300-byte payloads
  and extreme integer values.
- **OA-FEED-002** The reader decodes the golden vector produced by the bridge's Java encoder,
  and the Rust encoder produces the same bytes.
- **OA-FEED-003** Frames are identical however the stream is chunked: every two-way split point
  at the start and end of a mixed stream, and chunk sizes from 1 byte up.
- **OA-FEED-004** A CRC mismatch is reported as a CRC error at the frame's byte offset; the
  reader resynchronises to the next good frame and counts the skipped bytes exactly, at every
  chunk split.
- **OA-FEED-005** Bytes that are not a magic are reported as a bad-magic error at their offset
  and skipped exactly up to the next frame.
- **OA-FEED-006** A length above 4 MiB is refused as an oversize error as soon as the header is
  present, without waiting for the payload, and the reader resynchronises.
- **OA-FEED-007** An unknown kind (including 0) is refused with its own error and the frame is
  skipped.
- **OA-FEED-008** A stream that ends inside a frame gives a truncated-tail error with the bytes
  present; leftover bytes during resynchronisation are counted as skipped and not reported
  twice.
- **OA-FEED-009** Never panics: every byte flip, truncation, insertion and deletion of a valid
  stream, and 200 000 bytes of seeded noise in several chunkings, give frames and errors; every
  input byte is either in a delivered frame or counted as skipped.
- **OA-FEED-010** The reader runs in a fixed caller-supplied buffer of one maximum frame, refuses
  a smaller one, accepts a frame with a 4 MiB payload, takes only what fits on a push and never
  stalls; a payload above 4 MiB cannot be encoded.
- **OA-FEED-011** Health counts frames, messages, payload bytes, minimum and maximum payload size,
  the payload-size histogram, redelivered messages and rejects.
- **OA-FEED-012** Health reports message rates over the last complete second and the last 60
  seconds, from fixed ring buffers, with injected times; a stale ring reads as empty.
- **OA-FEED-013** Health counts sequence gaps (and the frames missing), duplicates and
  regressions.
- **OA-FEED-014** Health puts latency (receive time minus JMS timestamp) in a fixed-bin
  histogram, counts negative values as clock skew apart from the histogram, counts messages with
  no timestamp, and reports p50 and p95 (bin upper edge, ms) and the exact maximum.
- **OA-FEED-015** STATUS frames set the connected or disconnected state and count reconnect
  attempts with their announced delay.
- **OA-FEED-016** Time since the last frame is computed from the injected clock and never
  negative.
- **OA-FEED-017** The hot structs (frame view, rate slot, health record) keep their budgeted
  sizes with no hidden padding (RULESET A7).
- **OA-FEED-018** Replaying recorded files gives an identical health record and summary every
  time, equal to reading the same bytes as one stream, from frame fields only (never the clock).
- **OA-FEED-019** Recording rotates at the size limit on frame boundaries, names files
  `<feed>-<UTC start>-<NNNN>.oaf` (raw; `.oaf.gz` when compressed, OA-FEED-026) with a
  zero-padded counter from 0001 within one second, so name order is creation order; counts
  existing recordings toward the total cap, stops once with a warning at the cap and never
  deletes a file.
- **OA-FEED-020** UTC file names and timestamps are correct calendar times, including leap days
  and times before 1970.
- **OA-FEED-021** Every frame error carries and prints the byte offset of the problem.
- **OA-FEED-022** The command line accepts the documented forms and refuses missing or malformed
  arguments, exclusive options and unknown flags.
- **OA-FEED-023** The `oa-feed` binary in `--stdin` mode reads frames, prints a health line,
  records the input byte for byte (`--record-raw`), and `oa-feed replay` of the recording prints the same figures
  on every run.
- **OA-FEED-024** The latency histogram has 10 ms bins to 100 ms, 100 ms bins to 1 s, 1 s bins to
  60 s, 10 s bins to 600 s and one overflow bin; each bin's upper edge is the next bin's start,
  and a p50, p95 or maximum in the overflow bin prints as `>=600000`.
- **OA-FEED-025** Latencies of 7 to 10.5 s (as measured on the live TBFM feed) fall in distinct
  1 s bins, and p50, p95 and the maximum come out as numbers in that range; a printed quantile never exceeds the maximum.
- **OA-FEED-026** `--record` writes `<feed>-<UTC start>.oaf.gz` (gzip, level 6) and
  `--record-raw` keeps the plain `.oaf`; the two are exclusive; the recorded frames are the input
  frames exactly.
- **OA-FEED-027** A compressed recording is sync-flushed once a frame's receive time is a second
  past the last flush, by an idle flush when the feed is quiet, and on rotation and exit (each
  file is finished with its trailer): a file read while recording, or after a crash, holds every
  frame up to the last flush.
- **OA-FEED-028** Rotation and the total cap count the compressed bytes on disk, including
  existing `.oaf` and `.oaf.gz` files, rotate on frame boundaries, stop once at the cap and never
  delete a file.
- **OA-FEED-029** Replay chooses gzip or raw from the file's first bytes, not its name, even when
  the source returns one byte per read.
- **OA-FEED-030** Replaying a compressed recording gives a health record, summary and byte count
  identical to replaying the same frames uncompressed; a file cut by a crash replays every frame
  up to the cut and is reported truncated with the bytes recovered; damaged gzip data is
  reported and the frames before it still replay.
- **OA-FEED-031** The `oa-feed` binary in `--stdin` mode records a compressed file that
  `gzip -t` accepts and that decodes to the input, and `oa-feed replay` of it prints exactly what
  replay of the raw recording prints.
- **OA-FEED-032** The real SCDS recordings under `OPENATCA_SCDS_REC` (`*.oaf`) converted to
  `.oaf.gz` replay to identical summaries; the test reports the compressed sizes and ratios and
  skips when the variable is unset.

### Compression (oa-deflate)

- **OA-DEF-001** Compresses to a raw RFC 1951 DEFLATE stream that the oa-zip inflater decodes to
  exactly the input, at levels 1 to 9 (others are clamped), for empty input, one byte, all-same
  bytes, random bytes and data at 32 KiB, 64 KiB and buffer-slide boundaries (each +-1).
- **OA-DEF-002** Uses maximum-length (258) matches and the maximum distance (32768); data that
  repeats a 32 KiB block compresses to about one copy, and a repeat just outside the window is
  still decoded correctly.
- **OA-DEF-003** The gzip writer emits an RFC 1952 member (no timestamp, no name) whose CRC-32
  and ISIZE are correct, writes the header even for empty input, and is reusable for further
  members with identical output.
- **OA-DEF-004** The system `gzip -t` and `gzip -dc` accept our output, including concatenated
  members, for all levels and for chunked writes.
- **OA-DEF-005** The oa-zip inflater decodes the system `gzip` output (levels 1, 6, 9) of the
  same inputs; the test skips when `gzip` is missing.
- **OA-DEF-006** Huffman codes are complete prefix codes of at most 15 bits (7 for the
  code-length code) for any frequencies, including those whose unlimited tree is deeper, and for
  zero or one used symbol.
- **OA-DEF-007** Incompressible input falls back to stored blocks, so output exceeds input by
  framing only (about 5 bytes per 64 KiB).
- **OA-DEF-008** Sync flush ends on a byte boundary with an empty stored block; the output cut
  there, closed with an empty final block, decodes (also in system `gzip`) to exactly the data
  written before the flush. A flush with no new input emits nothing.
- **OA-DEF-009** Every feed directory of the real SWIM sample (`OPENATCA_SCDS`, `*.msg`
  concatenated) round-trips at levels 1, 6 and 9; the test reports ratio and MB/s per level
  against `gzip -6` and `xz -6`, requires level 6 to be within 10% of `gzip -6` (release builds),
  reports level 6 speed against a 30 MB/s target with a warning below it, and skips when the variable is unset.
- **OA-DEF-010** Never panics and always round-trips for chunk sizes from 1 byte upward
  (1, 7, 258, 4097, ...), irregular chunking and interspersed flushes, empty writes, and finish
  or flush with nothing written.
- **OA-DEF-011** Output is a pure function of input, level and write boundaries (no clock, no
  randomness); a compressor is reusable after `finish` or `reset` with identical output, and
  allocates only in `new`.
- **OA-DEF-012** An error returned by the output closure is returned unchanged by `write`,
  `flush` and `finish`; the compressor works again after `reset`.

### SFDPS flight messages (oa-fixm)

- **OA-FIXM-001** Decodes a FIXM 3.0 + FAA NAS 3.0 `MessageCollection` of flight messages into one
  row per `message`, in document order, with source, centre, message time (Unix ms), GUFI,
  callsign, computer id, flight plan id, departure and arrival points, status, flight type,
  assigned and reported altitude, position and position time, ground speed, track velocity,
  aircraft type, route text, operator, beacon code, controlling and handoff units, runway
  times and sequence numbers; verified on hand-written messages of source types TH, HZ, FH, AH,
  OH, DH, CL, BA and LH.
- **OA-FIXM-002** Parses timestamps as strict ISO-8601 UTC (`Z` only, real calendar dates, no
  leap second, 1970 to 9999) to Unix milliseconds, GUFIs as strict UUIDs and decimals without
  rounding surprises; anything else is refused.
- **OA-FIXM-003** A field that is present but malformed is a counted field error: the row is kept,
  the field is absent and its bit is set in the row's error word, nothing is defaulted.
- **OA-FIXM-004** Altitudes are range-checked (-2,000 to 100,000 ft) and converted from their
  `uom`; an unknown or missing `uom` is a field error.
- **OA-FIXM-005** Positions are range-checked (latitude +-90, longitude +-180) and read as
  EPSG:4326 latitude then longitude only; any other reference system is a field error.
- **OA-FIXM-006** Ground speed and track velocity are converted from their `uom` to 0.1 knot and
  range-checked; the ground track is derived from the velocity components by a free function.
- **OA-FIXM-007** A message with a missing or invalid GUFI, a missing or invalid timestamp, a
  type other than `FlightMessageType`, or a second `flight` element is refused whole, counted
  by reason, and appends no row; neighbouring messages are unaffected.
- **OA-FIXM-008** `Events` has a stated row capacity and string-table capacity; a document that
  does not fit is refused with the rows and string table exactly as before, never partly
  appended (IF-R1).
- **OA-FIXM-009** Elements and attributes outside the structure seen in real data are counted per
  path in a bounded table (overflow is counted too); they are never errors, and only the
  topmost unknown element of a subtree is counted.
- **OA-FIXM-010** A document whose root is not a `MessageCollection` in the FAA NAS 3.0 namespace,
  one over the element limit, or an input that is not the parsed bytes, is refused with an
  error.
- **OA-FIXM-011** A single-valued field given twice in one message is a field error, not a silent
  first-wins.
- **OA-FIXM-012** The decoder never panics: every deletion, duplication, truncation and
  replacement of a byte of valid fixtures, and random multi-byte damage, give rows or an error,
  with every present column inside its model range.
- **OA-FIXM-013** A decoder is reusable: tables keep their construction-time capacity, no state
  leaks from one document to the next, including after a failed decode.
- **OA-FIXM-014** Every real SFDPS message in the local sample (`OPENATCA_SCDS`) decodes; the test
  reports messages, rows, rejections with reasons, field errors by field, top unknown paths,
  distinct GUFIs and callsigns, rows with a position and throughput (at least 20,000
  messages/s in a release build), and skips when the variable is unset.
- **OA-FIXM-015** Route text is kept in the bounded string table, entity-decoded; a route longer
  than the limit, or empty, is a field error and takes no table space.
- **OA-FIXM-016** Field names, bits and display tables agree, and short codes read back as text.
- **OA-FIXM-017** A document with several messages gives one row per message, in order, and
  counts each message.

### TBFM metering publication (oa-tbfm)

Built on the survey in `docs/TBFM_SURVEY.md` (12,760 real messages, 20 centres).

- **OA-TBFM-001** Times are parsed from strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SS`, an optional
  fraction of one to three digits, `Z`) to exact Unix milliseconds, including leap days, the
  century rules and the year range 1970 to 9999.
- **OA-TBFM-002** A timestamp that is not exactly that form is refused with its own error:
  wrong shape (offsets, lower case, spaces, date only, compact form, trailing bytes), a
  non-digit, a fraction of more than three digits, or a value out of range (month, day of
  month, hour, minute, second `:60`, year before 1970).
- **OA-TBFM-003** The output is a caller-owned struct-of-arrays with a stated capacity, one
  `Vec` per column sharing an index, reserved once; rows round-trip through the columns, a push
  at capacity writes nothing and says so, and the staged row keeps its budgeted size
  (RULESET A3, A7, C2).
- **OA-TBFM-004** A `flt` section becomes one row with the flight identity (`aid`, `apt`,
  `dap`, `gufi`, `cid`, `tmaId`), the envelope and message fields, and the flight-plan fields
  (status, type, engine, beacon code, speed, altitudes, runways, coordination fix and times);
  the repeats of `aid`, `apt` and `dap` inside `flt` must agree with the `air` attributes or
  fill an attribute `air` lacks, and a disagreement is refused. The route and crossing text
  fields are known and not stored.
- **OA-TBFM-005** An `mrp` section becomes one row with its own `mrpType` and the meter route
  names: TRACON, meter fix, delay and scheduling fixes, the four arcs, gate, runway,
  configuration, category and scenario.
- **OA-TBFM-006** Each `eta` and `sta` section becomes one row, and each time is filed under its
  place (delay fix, scheduling fix, meter fix, the four arcs, runway) in the `eta_ms` or
  `sta_ms` column for that place; several `eta` or `mrp` sections in one `air` are several
  rows.
- **OA-TBFM-007** The `sch` flags (`man`, `rfz`, `sfz`, `sus`) are stored as two bits each:
  absent, `NO`, `YES`; any other word is refused.
- **OA-TBFM-008** Every message is classified: `air` (flight data), adaptation, control, other,
  empty, and the `mis` heartbeat and sync markers; non-flight kinds produce no rows and are
  counted, and the centre and envelope time are reported for all of them.
- **OA-TBFM-009** A message is accepted whole or not at all: a refused field, a wrong root or
  namespace, an XML error or a capacity overrun leaves the caller's rows exactly as they were,
  and a refused field carries the byte offset of its element or attribute value.
- **OA-TBFM-010** Every field has its own refusal: a missing required attribute (`envSrce`,
  `envTime`, `msgTime`, `msgId`, `tmaId`, `aid`, `airType`), a name longer than its column, a
  character outside printable ASCII, a malformed number, transponder code, centre or time, a
  word outside the vocabulary, a field twice in one element, a value element with children or
  with text in several nodes; each is reported with the field and the reason. An element sent
  empty is not an error: it is recorded in the row's `cleared` mask.
- **OA-TBFM-011** Reaching the row capacity is the reject reason `capacity`, never a
  reallocation or a partial message, and rows already held count against it.
- **OA-TBFM-012** Unknown elements (a whole subtree counts once), unknown attributes and
  elements outside the TBFM namespace are counted and do not affect the rows or the decision to
  accept; namespace declarations and `targetNamespace` are not counted; known elements that
  are deliberately not stored are counted separately; statistics add both up over accepted
  messages and count rejects by reason and by field.
- **OA-TBFM-013** The decoder never panics: every truncation, deletion, insertion and
  replacement of each byte of a message with every section kind, and seeded random
  corruption, give either rows or an error, and a refused message leaves every column the same
  length as before.
- **OA-TBFM-014** Character references and CDATA are decoded before a value is validated, an
  attribute value is normalised as XML requires, and a decoded value longer than the 64-byte
  working buffer is refused as too long.
- **OA-TBFM-015** After construction decoding does not grow any column or any parser table,
  across accepted messages, refused messages and capacity overruns.
- **OA-TBFM-016** Every message of the real TBFM sample (`OPENATCA_SCDS`, directories named
  `tbfm*`) decodes with no reject and no unknown path; the test reports messages, rows and
  rejects by reason, field errors by field, distinct flights, GUFIs, airports and meter fixes,
  and throughput, which must be at least 20,000 messages per second (parse and decode, one
  thread, release builds); it skips when the variable is unset.
- **OA-TBFM-017** Against an airspace snapshot of one ARTCC (`OPENATCA_SNAPSHOT`) the real rows'
  meter fixes and airports are resolved to model points, and the distinct identifiers and rows
  that resolve are reported for all centres and for the model's own centre; the test skips
  when either variable is unset.
- **OA-TBFM-018** The cross-check resolves a meter fix to a model fix, navaid or terminal
  waypoint and an airport to a model airport by its identifier or alternate identifier (never
  to a runway threshold), counts distinct identifiers and rows per role and per scope (all
  centres, the model's own centre) and lists the identifiers that do not resolve.

## M1 — Controller display

### Display list, projection, font, rasterizer (oa-display)

- **OA-DSP-001** The display list is a flat struct-of-arrays list of line, polyline (a span of a
  shared vertex array), filled rectangle, circle outline and text primitives. Every primitive
  carries a palette index and a layer, coordinates are `i32` in 1/16 pixel, and a colour index,
  layer, text size or circle radius out of range is refused.
- **OA-DSP-002** Capacity is fixed at creation. A push to a full table returns an error naming the
  table and its capacity and marks the list faulted; a faulted list is never drawn.
- **OA-DSP-003** Clearing a list for the next frame keeps every capacity, so building frames does
  not allocate in the list.
- **OA-DSP-004** The projection puts the centre at the middle of the screen with north up and
  east right, and projecting then unprojecting a screen position, or the reverse, returns it to
  within 1/16 pixel (and the position to within 1e-7 degree plus that quantum).
- **OA-DSP-005** The distance from the centre drawn on screen is within 0.15 % of the WGS-84
  geodesic (oa-geo) out to 400 NM at centres from the equator to 65 N, and bearings are true to
  0.3 degree.
- **OA-DSP-006** The projection refuses a centre off the globe, a range outside 0.05 to 20000 NM,
  a zero-size screen, the absent-position sentinel and positions off the globe, and saturates a
  far projected point instead of overflowing; ring radii are true ground distances.
- **OA-DSP-007** The stroke font has every character of A to Z, 0 to 9 and ` .,/-+:()*#` with the
  up and down arrows, sorted for lookup, with every stroke on the 5 by 8 grid; lowercase folds to
  capitals and an unknown character draws a crossed box.
- **OA-DSP-008** The easily confused characters 0 and O, 1 and I, 5 and S, 8 and B, and 2 and Z
  differ in at least 4 cells of the glyph grid, other close neighbours in at least 3, and no two
  glyphs are identical.
- **OA-DSP-009** The rasterizer clips every primitive to the buffer exactly: a clipped line, circle
  and rectangle equal the part inside the buffer of the unclipped drawing; lines entirely outside,
  crossing each edge and on each corner behave correctly; nothing is written outside the visible
  pixels, however extreme the coordinates; and a line is the same whichever end comes first.
- **OA-DSP-010** The rasterizer is integer only and deterministic: the frame depends only on the
  list and the frame geometry, not on what the buffer held before or on the stride.
- **OA-DSP-011** A deterministic sweep of random primitives, including `i32` extremes, every size
  and alignment, odd strings and many buffer geometries, never panics, writes only palette colours
  to visible pixels, writes nothing else, and gives the same hashes on a second run.
- **OA-DSP-012** Small fixed scenes (a line fan with rectangle, circle and polyline; every glyph
  class at several sizes and alignments; layers with clipping; a hand-built airspace map) render
  to pinned SHA-256 hashes.
- **OA-DSP-013** The frame hash is the SHA-256 of the visible pixels in row order, independent of
  stride, and changes with any pixel; an unusable frame geometry or short buffer is refused.
- **OA-DSP-014** A faulted list, or a list whose columns, indexes, spans or text were edited into
  an inconsistent state, is refused before any pixel is written.
- **OA-DSP-015** Text is anchored left, centre or right at the top of its capitals, and its
  extent is the glyph grid scaled by the size.
- **OA-DSP-016** Every foreground palette colour has at least 4.5:1 contrast on the background,
  and no two data colours come within 55 (of 255) of each other in plain RGB or as seen with
  protanopia, deuteranopia or tritanopia.
- **OA-DSP-017** The airspace map draws ARTCC HIGH and LOW boundaries (closed, in distinct
  colours and layers), airway legs, fixes, navaids and airports as distinct symbols, labels below
  configurable range thresholds, range rings at their true radius with labels, and a legend naming
  the facility, cycles, range, ring spacing and both strata.
- **OA-DSP-018** A scene that overflows any table fails with an error naming it, leaves the list
  faulted and is never drawn, and the capacities computed from the model hold at every range and
  centre including the poles and the antimeridian.
- **OA-DSP-019** The same model and view give the same list and the same bytes however many
  frames come between, and a frame never changes the list's capacity.
- **OA-DSP-020** The PPM writer emits P6 with RGB order, no row padding, and an error, not a
  panic, for a short buffer.
- **OA-DSP-021** A real ZAU snapshot (`OPENATCA_SNAPSHOT`) renders, every frame has the same hash,
  and the time per frame at 1920 by 1080 is reported; the test skips when the variable is unset.
- **OA-DSP-022** `oa-scope render` draws a snapshot to a PPM and reports the frame's SHA-256, and
  a malformed command line is a usage error naming the problem.
- **OA-DSP-023** Points and legs the map cannot place are counted and said on the screen in the
  alert colour, never dropped silently.
