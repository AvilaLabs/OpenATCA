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

### TFMS data service (oa-tfms)

Built on the survey in `docs/TFMS_SURVEY.md` (4,237 real documents, flight data, flow
information and feed status). Decision: the decoder follows the oa-fixm and oa-tbfm pattern; flow
messages are a separate row type; the TFMS `gufi` is linked to the SFDPS plan identifier, not to
the SFDPS `gufi` (they are different kinds of value, survey).

- **OA-TFMS-001** Times are parsed from strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SS`, an optional
  fraction of one to nine digits, `Z`) into Unix milliseconds (`i64`); any other shape, digit,
  fraction or range is refused with its own error.
- **OA-TFMS-002** Scalar fields have strict parsers: names (bounded, upper-case letters, digits
  and a few separators), unsigned integers, altitudes (hundreds of feet with an optional letter),
  decimal and degrees-minutes-seconds angles to 1e-7 degree, booleans, vocabularies and whole
  words; each refuses anything else.
- **OA-TFMS-003** Rows are caller-owned structs of arrays with a stated capacity and exact size
  guards; every value has an explicit absent form (`NO_TIME`, `NO_NUM`, `NO_COORD`, `NO_STR`, a
  zero code or a zero name).
- **OA-TFMS-004** Every flight message becomes one row whose identity (message kind, trigger,
  aircraft identification, flight reference, GUFI, departure and arrival, source facility and
  time, computer id, categories) comes from the message attributes and the qualified aircraft
  identifier, which must agree; an unknown message type is a row of kind `other`.
- **OA-TFMS-005** Positions (degrees-minutes-seconds and decimal), time at position, altitudes
  in feet, speeds and the next position are stored as fixed point in 1e-7 degree, feet and knots.
- **OA-TFMS-006** Estimated, actual, scheduled, proposed and metered times with their types,
  fixes with times, coordination, boundary crossing, departure and arrival, the airline's
  out/off/on/in, runway and gate times and the original times are stored as Unix ms.
- **OA-TFMS-007** Route text is kept in the bounded string table, entity-decoded, with the legacy
  format preferred over the element text in either order, and with remarks and the departure and
  arrival procedure names.
- **OA-TFMS-008** A traffic management initiative flight list becomes one flight row per entry,
  with the entry's status, update type and time, FCA identifier and the number of `fxaFlight`
  elements; the message's own time and facility go to every entry.
- **OA-TFMS-009** Restrictions, advisories, FCA/FEA definitions and airport configurations become
  flow rows with program type, scope, element, start, end, rates, identifiers and text; the
  program of an advisory is read from whole words of its title and a cancellation is flagged.
- **OA-TFMS-010** The identity attributes of a message and the identity in its body must agree
  (refused as inconsistent otherwise); the aircraft identification, flight reference, source
  time and message type are required.
- **OA-TFMS-011** Every field has its own refusal naming the field and the kind (missing, too
  long, bad character, format, time, vocabulary, range, duplicate, inconsistent, not a leaf,
  text), with the offset of the value.
- **OA-TFMS-012** A document is accepted whole or not at all: a refused field, a capacity
  overrun or a text-table overrun in any message leaves the caller's rows and text table exactly
  as they were before the document.
- **OA-TFMS-013** A document that is not well-formed, whose root is not `tfmDataService` in the
  service namespace, or whose content is refused is rejected with a reason; status documents,
  empty documents and mixed documents are classified.
- **OA-TFMS-014** Capacity of flight rows, flow rows and text bytes is stated and never exceeded;
  reaching it is a reject reason, never a growth; after construction no column or table grows.
- **OA-TFMS-015** Unknown elements (a whole subtree counts once), unknown attributes and unknown
  namespaces are counted, not errors, and elements are matched by namespace and local name
  whatever the prefix; known elements that are deliberately not stored are counted separately.
- **OA-TFMS-016** The decoder never panics: every truncation, byte replacement, deletion and
  insertion of flight and flow documents is decoded or refused, and the columns stay
  consistent.
- **OA-TFMS-017** Every message of the real TFMS sample (`OPENATCA_SCDS`, directories named
  `tfms*`) decodes with no reject and no unknown path; the test reports messages, rows by
  type, rejects by reason, field errors by field, distinct flights and throughput, which must be
  at least 20,000 messages per second (parse and decode, one thread, release builds); it skips
  when the variable is unset.
- **OA-TFMS-018** Against the real SFDPS sample, TFMS flights are linked by GUFI and the overlap
  is reported as found; no TFMS GUFI equals an SFDPS GUFI and the two matching routes never
  name different flights; the test skips when the variable is unset.
- **OA-TFMS-019** Counters over many documents (accepted, messages, rows by kind, rejects by
  reason, field errors by field, unknown and ignored paths) are folded by a free function.
- **OA-TFMS-020** The linkage matches a TFMS flight to the SFDPS flights by its GUFI (equal to
  the SFDPS flight plan identifier) and, when it has no GUFI, by aircraft identification plus
  departure plus arrival, and cross-checks flights found both ways.
- **OA-TFMS-021** The linkage report counts, as found, the flights with and without a GUFI,
  UUID-shaped GUFIs, keys that name more than one SFDPS flight and empty inputs.
- **OA-TFMS-022** The default capacities and the vocabulary tables agree with the codes.

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
  up and down arrows and `<` `>`, sorted for lookup, with every stroke on the 5 by 8 grid; lowercase folds to
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
  alert colour, never dropped silently. A leg that ends at a published border crossing is
  expected data, not an unplaced one (see OA-DSP-027).
- **OA-DSP-024** Lines and polylines are 1 or 2 pixels wide (another width is refused). A 2 pixel
  line is the Bresenham line plus a copy one pixel along its minor axis, deterministic, the same
  whichever end is first, clipped like any line, and pinned by a golden hash.
- **OA-DSP-025** The map hides detail by default and says so. Every layer (own boundary, neighbour
  boundaries, high airways J and Q, low airways V and T, VOR-class navaids, airports, fixes,
  labels, rings) can be switched off. Every range threshold keys on the display range (the zoom,
  nautical miles from the centre to the nearest edge of the map area), never on a symbol's
  distance from the centre, and each is configurable. Defaults: fixes and labels of fixes, minor
  navaids and other airports at a range of 40 NM or less; terminal waypoints 20 NM or less;
  runway thresholds and localizers 15 NM or less; airports without a coded instrument procedure
  and NDB, DME and TACAN 40 NM or less; VOR, VORTAC and VOR-DME and airports with a coded
  instrument procedure always, with their labels out to 100 NM. A `HIDDEN:` legend line names
  everything left out by a layer switch or a threshold at the current range.
- **OA-DSP-026** The own facility's boundary is drawn 2 pixels wide in its stratum colour;
  neighbouring facilities' boundaries are 1 pixel wide in a dimmer colour under it.
- **OA-DSP-027** The alert colour is used only for conditions that need attention, never for
  expected data. Legs ending at border crossings (unresolved role BORDER_CROSSING) are reported in
  the normal text colour as a count; the alert colour appears only when points, legs or edges are
  genuinely unplaced.
- **OA-DSP-028** The legend sits in a fixed strip at the top of the screen that the projection
  excludes from its map area, so the legend never hides map data and the range circle fits below
  it; legend rows stay inside the strip and do not overlap.
- **OA-DSP-029** Each range ring is labelled where it crosses the 45 degree north-east diagonal
  from the centre, and no two ring labels overlap at any ring spacing or screen size.
- **OA-DSP-030** Positions outside a latitude and longitude box around the view are rejected
  before projection, and the box never changes the picture: the list is identical with and
  without it, including near the antimeridian.
- **OA-DSP-031** The frame hash is computed only on request (`--hash`, or in tests), not on every
  frame; the command line takes `--hide` and the range thresholds, and a bad layer name is a usage
  error.
- **OA-DSP-032** Labels are placed deterministically with collision culling. Candidates are sorted
  by priority (VOR-class navaids, airports with instrument procedures, other airports, fixes and
  minor navaids, terminal waypoints), then ident, then position, then table row. Each takes the
  first of four fixed offsets (north-east, south-east, north-west, south-west of its symbol) that
  overlaps no placed label, ring label, the legend strip or the screen edge; otherwise it is
  dropped and counted, and the legend shows `LABELS DROPPED: N` in the normal colour. Placement
  uses a fixed-capacity grid of cells with a bounded list each, allocated once; a full cell drops
  the label and counts it. No two drawn labels overlap, a label is dropped only where every offset
  is taken by a label of equal or higher priority, and the result does not depend on point order.

### Replay and flight state (oa-replay)

Milestone M1, "record and replay; bit-exact replay harness". Recorded feeds go in, one
deterministic flight state comes out.

- **OA-RPL-001** The flight table is a struct-of-arrays of fixed capacity (default 32,768
  flights) allocated once at construction; bad settings (capacity 0 or above 2^24, a non-positive
  age limit or sweep spacing) are errors. Its key indexes are sized to at least twice the capacity,
  so the load never exceeds 1/2.
- **OA-RPL-002** An SFDPS row, applied with the replay time as an argument, fills the flight's
  identity (callsign, computer id, departure, arrival, aircraft type, flight-plan id), last
  position (latitude, longitude, altitude, ground speed, track velocity), position time,
  last-update time, last source and controlling unit, and counts the update.
- **OA-RPL-003** A position older than the flight's last position time never overwrites it: the
  row's position fields are dropped and the row is counted as out of order (per flight and in
  total), while its other fields still apply. An equal or newer position applies. A row without
  a position is never out of order.
- **OA-RPL-004** Identity fields fill when empty and confirm when equal; a different non-empty
  callsign, departure, arrival or aircraft type is a counted field conflict and the held value
  is kept. The computer id is not identity (each centre assigns its own) and takes the latest.
- **OA-RPL-005** A TBFM row links to the flight whose flight-plan identifier equals the row's
  `gufi` attribute (the TBFM `gufi` is the SFDPS `flightPlan@identifier`, not the SFDPS GUFI). A
  `gufi` matching no flight creates a TBFM-first flight, which takes its real GUFI when SFDPS
  reports the same flight plan; linked, created and promoted rows are counted.
- **OA-RPL-006** A TBFM row with no `gufi` links through the `(centre, tmaId)` pair, held in a
  second fixed-size index; a row that matches nothing is counted as unlinked and applies
  nothing. The same `tmaId` from another centre is another key.
- **OA-RPL-007** TBFM rows set the metering columns: meter fix, ETA at the meter fix, STA (the
  scheduling-fix time where the feed has no meter-fix time), runway and the frozen flag (`sfz`).
  A TBFM row older than the flight's last TBFM message time is refused whole and counted as out
  of order.
- **OA-RPL-008** A TBFM `DEL` change type, on the `air` or on the `mrp` section, clears the
  metering columns and the frozen flag. It does not delete the flight, and metering can start
  again.
- **OA-RPL-009** A flight is evicted only when its last update is more than the age limit
  (default 30 minutes, configurable) older than the replay time, at a sweep that runs on a fixed
  cadence of replay time (multiples of the sweep spacing, default one minute). Exactly the limit
  is not older than it. Nothing is evicted between sweeps.
- **OA-RPL-010** A sweep visits slots in slot order and returns the evicted slots to a free stack,
  so slot reuse is deterministic; eviction removes the flight from every index.
- **OA-RPL-011** A full table refuses a new flight (from either feed) and counts it; it never
  evicts early to make room. Existing flights keep updating, and the refused flight is admitted
  once a sweep has freed a slot (INTERFACES IF-R1).
- **OA-RPL-012** The state hash is SHA-256 over a canonical serialisation iterating live flights
  in GUFI order, so it does not depend on insertion history: shuffled application of commuting
  updates gives the same hash.
- **OA-RPL-013** The state hash does not depend on slot assignment or eviction history, and
  changes when any column changes; an empty table's hash does not depend on capacity.
- **OA-RPL-014** The open-addressing index (linear probing, backward-shift deletion) agrees with
  a model under random inserts and removes, including with a degenerate hash that forces long
  probe runs; it refuses past load 1/2; its hash functions are fixed, unkeyed, deterministic
  functions of the key (not adversarial-robust, and bounded by the index length per operation).
- **OA-RPL-015** The flight table allocates nothing after construction: every column and index
  keeps its address and capacity through applying, refusing, sweeping and evicting.
- **OA-RPL-016** The pipeline takes replay time from each frame's `recv_unix_ns` and records a
  checkpoint (replay time, frames, rows applied, flights live, state hash) at the first frame at
  or after each multiple of the checkpoint spacing (default 10 s).
- **OA-RPL-017** Every parse, decode and apply failure is counted under its own kind (XML,
  unsupported root, bad GUFI, bad timestamp, malformed field, TBFM field, and the rest); REJECT
  and STATUS frames are counted and not decoded. Nothing is silently skipped.
- **OA-RPL-018** Replay time never goes backwards; a frame earlier than one already seen is
  counted as a time regression.
- **OA-RPL-019** Replaying the same recordings twice, or with the input read in any chunking
  (1 byte to 16 MiB), or with the streams named in another order, gives identical checkpoints,
  counters and final state hash.
- **OA-RPL-020** A `.oaf.gz` recording replays identically to its raw `.oaf` (chosen by the
  file's first bytes); a gzip file cut short is counted and its frames up to the cut replay.
- **OA-RPL-021** Recordings are read as streams and merged with a fixed-capacity min-heap, one
  head frame per stream, ordered by `(recv_unix_ns, feed kind, seq, stream index, byte offset)`: a
  total order, equal to a global sort of the same frames whenever each stream is in receive order.
  Memory is bounded by the number of streams times a fixed per-stream buffer, not by the input
  length (peak resident memory on the real recordings is reported by the binary's `timing` line).
- **OA-RPL-022** Corrupted recordings (every single-byte flip, truncations, random garbage and
  damage) and corrupted XML payloads never panic; a damaged frame leaves a count, and a cut inside
  a frame is a counted truncation.
- **OA-RPL-023** The `oa-replay` command line gives each file the feed kind of the flag that
  names it, sets the checkpoint spacing, age limit, sweep spacing and capacity, and rejects
  missing, zero, non-numeric and unknown arguments.
- **OA-RPL-024** The `oa-replay` binary prints the checkpoints and a final summary (frames and
  rows per feed, TBFM linking, table totals, errors by kind, flights live and the final state
  hash); checkpoints are printed as they are recorded; a progress line (replay time, frames per
  second, replay-time to wall-time ratio, resident memory) goes to stderr every N minutes of
  replay time (default 10); every stdout line except the `timing` line is identical from run to
  run, whatever the flag order or read size; exit status is 2 for usage errors and 1 for an unreadable file.
- **OA-RPL-025** Replaying the real SCDS recordings under `OPENATCA_REC` (SFDPS and TBFM) twice
  and with another read size gives identical checkpoints and the same final state hash, with no
  frame, gzip or clock error and TBFM rows linked to SFDPS flights (skipped if unset). The final
  hash equals that of the earlier all-in-memory replay.
- **OA-RPL-026** The canonical serialisation of a fixed table state has a pinned golden hash, so
  a change of the serialisation or of what the state holds is a deliberate, visible edit.

### FNS NOTAM messages (oa-notam)

Built on the survey in `docs/FNS_SURVEY.md` (112 real messages, AIXM 5.1 `AIXMBasicMessage` with
the FAA event extension).

- **OA-NOTAM-001** Decodes an AIXM 5.1 basic message with one `event:Event` into one row, in a
  caller-owned struct-of-arrays: event identifier (UUID), NOTAM series, number, year, type (new,
  replace, cancel), the referenced NOTAM (series, number, year, all three
  or none), location designator, affected FIR, ICAO location, cross-reference id, issue time and
  cancellation time; verified on hand-written messages.
- **OA-NOTAM-002** Times are strict: ISO-8601 UTC (`Z` only, real calendar dates, no leap second,
  1970 to 9999, a fraction of at most three digits) and the compact `YYYYMMDDhhmm` form read to
  exact Unix milliseconds; every other form is refused.
- **OA-NOTAM-003** The output has a stated capacity for rows, text bytes, polygons and vertices,
  reserved once; rows round-trip through the columns, a push at capacity writes nothing and says
  so, and the staged row keeps its budgeted size (RULESET A3, A7, C2).
- **OA-NOTAM-004** The features of a message (airport, runway, runway direction, runway element,
  taxiway, taxiway element, navaid, unit, obstacle area, vertical structure, radar system,
  primary surveillance radar) are recorded as a kind mask and the most specific kind, with the
  airport designator and ICAO indicator, the runway (or runway direction) designator, the navaid
  type and the unit designator; member order does not matter.
- **OA-NOTAM-005** The classification codes are read: Q code (`selectionCode`), scenario,
  classification (INTL, DOM, MIL) and the traffic, purpose and scope letters as bit sets; short
  codes are trimmed of surrounding white space; a letter outside the vocabulary or repeated is a
  field error.
- **OA-NOTAM-006** The start and end of the effective period come from both the `validTime` and
  the compact `effectiveStart` and `effectiveEnd`; either alone stands, and two that disagree
  are a field error on that field. An end written `PERM` or ending in `EST` sets an explicit
  permanent or estimated flag.
- **OA-NOTAM-007** The NOTAM text and the schedule are kept in a bounded string table,
  entity-decoded with white space kept exactly; a text longer than the limit, empty, or in
  several text nodes is a field error and takes no table space.
- **OA-NOTAM-008** The altitude floor and ceiling are in feet with a reference (MSL, AGL, flight
  level): `lowerLimit` and `upperLimit` (`SFC`, `GND`, `UNL`, `FLnnn`, feet and metres above
  ground or mean sea level) win over the `minimumFL` and `maximumFL` pair; an unknown form, a
  value outside -2,000 to 100,000 ft, or a floor above the ceiling in the same reference is a
  field error.
- **OA-NOTAM-009** The circle is the centre of `coordinates` (`DDMM[NS]DDDMM[EW]`, whole minutes,
  range checked) in 1e-7 degree and the `radius` (1 to 999 nautical miles) in metres; a centre
  or radius alone is kept but is not a circle.
- **OA-NOTAM-010** Polygons (`posList` of runway and taxiway element surfaces) are spans of a
  shared vertex array of 1e-7 degree coordinates, closing vertex not repeated, each with its
  feature kind; a coordinate out of range or not a number, an open ring, fewer than four points,
  an odd number of values, a reference system other than EPSG:4326 as a URN, or a ring over the
  vertex limit is a field error on the row, none of the message's polygons is kept, and nothing is
  dropped silently.
- **OA-NOTAM-011** Every field has its own refusal: a malformed value is a counted field error,
  the row is kept, the field is absent and its bit is set in the row's error word; a
  single-valued field given twice, or an empty value element, is an error rather than first-wins.
- **OA-NOTAM-012** A document with no event, several events, no or an invalid event identifier, no
  or several NOTAM elements, a wrong root or namespace, or that is not well-formed XML is refused
  whole, appends nothing, is counted by reason and does not affect its neighbours.
- **OA-NOTAM-013** Reaching a row, text, polygon or vertex capacity is a refusal naming the
  table, with the rows, string table, polygons and vertices exactly as before (IF-R1); a
  document over the element limit is refused likewise.
- **OA-NOTAM-014** Elements and attributes outside the structure seen in real data, including a
  known name in another namespace and feature kinds not in the survey, are counted per path in a
  bounded table (overflow counted too) and never errors; only the topmost unknown element of a
  subtree is counted and nothing inside it is read.
- **OA-NOTAM-015** The decoder never panics: every truncation, deletion, duplication and
  replacement of each byte of messages with every feature kind and field, and seeded random
  damage, give a row or an error, with every present column inside its model range and the
  tables unchanged after a refusal.
- **OA-NOTAM-016** After construction decoding does not grow any column or table, across accepted
  and refused documents; no state leaks from one document to the next or between decoders.
- **OA-NOTAM-017** Every real FNS message of the local sample (`OPENATCA_SCDS`, directories named
  `fns*`) decodes with no rejection, no field error and no unknown path; the test reports
  messages, rows, rejections by reason, field errors by field, feature kinds, rows with geometry
  and throughput, which must be at least 20,000 messages per second (parse and decode, one
  thread, release builds); it skips when the variable is unset.
- **OA-NOTAM-018** Against an airspace snapshot of one ARTCC (`OPENATCA_SNAPSHOT`) the real rows'
  locations, airports, runways and navaids are resolved to model entries, and the polygons are
  tested against the model's ARTCC boundary; the distinct identifiers and rows that resolve are
  reported for all rows and for the model's own FIR; the test skips when either variable is
  unset.
- **OA-NOTAM-019** Field names, bits and display tables agree.
- **OA-NOTAM-020** The cross-check resolves a location to a model airport (identifier or
  alternate), fix, navaid, terminal waypoint or ARTCC; an airport to a model airport; a runway
  when the airport resolves and every end named is a threshold of that airport; and an ILS when
  the airport has a localizer; it counts distinct identifiers and rows per role and scope and
  lists those that do not resolve.
- **OA-NOTAM-021** The cross-check reports which polygons touch the model's own ARTCC boundary
  (high and low) by geodesic containment of vertices and boundary samples, and how many circle
  centres are inside it; polygons oa-geo refuses and boundaries oa-geo refuses are counted, not
  errors.

- **OA-RPL-027** A frame whose receive time goes backwards inside one stream is not reordered
  (that would need unbounded lookahead): it is emitted in file order and the pipeline counts a time
  regression.
- **OA-RPL-028** Rotated recordings of one feed chain in name order (`--sfdps-dir`, `--tbfm-dir`,
  `--tfms-dir`, `--fns-dir`, or several files in one stream, raw or gzip, any mix) and replay
  identically to the unsplit recording, with the same checkpoints and final state hash.
- **OA-RPL-029** TFMS and FNS frames are counted per feed in a "not yet applied" bucket until their
  decoders exist; they change no flight state and raise no error.

## M5 — Terrain and obstacles

### GeoTIFF elevation (oa-tiff)

- **OA-TIFF-001** Reads classic TIFF 6.0 in little and big endian; a bad byte-order mark or magic
  number is refused, BigTIFF (magic 43) is refused with a typed error, and truncated headers are
  errors.
- **OA-TIFF-002** Reads strips (any rows per strip, including a short last strip) and tiles
  (including tiles that overhang the right and bottom edges), uncompressed, giving the samples
  that were written.
- **OA-TIFF-003** Decompresses Deflate (tags 8 and 32946) through the zlib wrapper, checking the
  header, refusing a preset dictionary and verifying the Adler-32 and the exact decompressed size.
- **OA-TIFF-004** Decodes TIFF 6.0 LZW (MSB-first, early change, clear codes, table full) to
  exactly the size the segment needs, and refuses bad codes, streams that end early and streams
  that run long.
- **OA-TIFF-005** Applies predictor 2 (horizontal differencing, 16-bit integers) and predictor 3
  (floating point byte planes), in both byte orders, with every compression and in both layouts,
  restoring float bit patterns exactly; a predictor that does not fit the sample type is refused.
- **OA-TIFF-006** Reads one sample per pixel as 32-bit float or 16-bit signed integer; other
  layouts, other sample counts, planar data and other compressions are refused with a typed error.
- **OA-TIFF-007** Places samples by the GeoTIFF raster-type convention: for `PixelIsArea` the
  tiepoint is the outer corner of the pixel and samples are half a pixel inside it, for
  `PixelIsPoint` the tiepoint is the sample; a tiepoint away from the raster origin is honoured.
- **OA-TIFF-008** Accepts only geographic rasters in NAD83 (4269) or WGS 84 (4326) with axis-aligned
  pixels, one tiepoint and a positive finite pixel scale inside the valid latitude and longitude
  range; missing or inconsistent georeferencing and `ModelTransformation` are refused.
- **OA-TIFF-009** Turns the `GDAL_NODATA` value and any NaN into NaN samples and counts them; an
  unparsable nodata text is refused.
- **OA-TIFF-010** Enforces stated limits (file, dimension, pixel count, segment, tag value count,
  IFD entries, ASCII length) and never panics: every truncation, byte inversion and seeded random
  input gives a grid or an error; a repeated tag is refused.
- **OA-TIFF-011** `grid_elevation_at` interpolates bilinearly, is exact at samples, reproduces a
  plane exactly, reports points beyond the outermost samples as outside, and reports a cell with
  any nodata corner as no data rather than averaging the rest.
- **OA-TIFF-012** `grid_max_in_radius` returns the highest valid sample within a great-circle
  radius with its position, counts valid and nodata samples, reports whether the circle's edge lies
  inside the data, and refuses a bad centre or a radius that is not positive, finite and within the
  limit.
- **OA-TIFF-013** `grid_max_in_polygon` does the same inside a polygon (even-odd in the
  latitude/longitude plane, concave polygons included), and refuses polygons with fewer than three
  or too many vertices, non-finite vertices or a span over 180 degrees of longitude.
- **OA-TIFF-014** A tile set answers elevation, radius and polygon queries across tile edges for
  both overlapping tiles and abutting tiles whose lattices line up, agrees with the same query on
  the undivided grid, does not invent a value across tiles that do not line up, and prefers a tile
  with data over one with nodata.
- **OA-TIFF-015** Tile sets over the tile limit are refused, empty sets answer outside, and
  polygons over the vertex limit are refused.
- **OA-TIFF-016** On the real USGS 3DEP 1 arc-second tiles (`OPENATCA_DATA`) every tile is 3612 by
  3612, pixel-is-area, its nominal degree begins on an integer degree at a pixel corner, shared
  borders with the east and north neighbours coincide in position, agree in value to within a metre
  for all but a fraction of a percent of samples, and match far better than the same overlap shifted
  by one pixel; skipped when the variable is unset.
- **OA-TIFF-017** For every NASR airport inside the loaded tiles the 3DEP elevation at the airport
  reference point is compared with the surveyed field elevation; the test reports count, median,
  95th percentile and the ten worst per airport kind, and the same comparison with the sampling point
  moved by half and whole pixels; it requires a median under 15 ft and a 95th percentile under 60 ft
  for land airports; skipped when the data is absent.
- **OA-TIFF-018** For every DOF obstacle inside the loaded tiles `AMSL - AGL` is compared with the
  3DEP elevation at its position, verified and unverified separately; the test reports median,
  95th percentile and the ten worst, and requires a median under 25 ft for verified obstacles;
  skipped when the data is absent.
- **OA-TIFF-019** On two adjacent real tiles a tile set answers elevation points on both sides of
  the shared edge to within a metre of the owning tile, and a radius across the seam returns the
  highest sample of both tiles; skipped when the data is absent.

### Digital Obstacle File (oa-dof)

- **OA-DOF-001** Parses every field of the FAA DOF line format (OAS code and number, verification,
  country, state, city, latitude and longitude with hemisphere, type, quantity, AGL and AMSL feet
  including negative AMSL, lighting, horizontal and vertical accuracy, marking, FAA study number,
  action, Julian date) into struct-of-arrays with the currency date of the file.
- **OA-DOF-002** Positions are exact in 1e-7 degrees for seconds to 0.01, signed by hemisphere;
  degrees, minutes, seconds and hemisphere are range checked, the poles and the antimeridian are
  accepted and anything beyond them is refused.
- **OA-DOF-003** Accuracy codes decode per the README tables to feet and metres (horizontal 1 to 8,
  vertical A to H; unknown, blank and invalid codes decode to none), and every lighting and marking
  letter of the README has a name.
- **OA-DOF-004** Each field that fails validation, including separator columns, rejects the line
  with a typed error naming the field and the 1-based line; short and over-long lines are rejected
  with their length; Julian dates are checked against the calendar including leap years.
- **OA-DOF-005** Counts physical lines and rejected lines (the kept list is capped, the total is
  not), accepts CRLF and LF and a missing final newline, skips nothing silently, and refuses files
  with a missing or malformed currency line or header as a whole.
- **OA-DOF-006** The bounding-box query returns the obstacles inside a latitude/longitude box with
  inclusive edges and supports boxes that cross the antimeridian.
- **OA-DOF-007** The parser never panics: every truncation, byte replacement and seeded random
  input gives a file or an error, and the accepted columns stay the same length with
  lines = accepted + rejected + header.
- **OA-DOF-008** The real `DOF.DAT` (`OPENATCA_DATA`) parses with no rejected line, the line and
  obstacle counts agree, the per-state and per-country files are subsets whose total falls short
  by under 100 obstacles, and positions are inside the world; skipped when the data is absent.

## M1 — ADS-B history input

### JSON (oa-json)

- **OA-JSON-001** Parses exactly RFC 8259: every JSONTestSuite `y_` case is accepted and every
  `n_` case refused (gated on `OPENATCA_DATA/raw/jsontestsuite`, 95 and 188 cases), and a
  hand-written set of valid and invalid texts (leading zeros, `+`, `NaN`, trailing commas,
  single quotes, comments, vertical tab as whitespace, unterminated values) agrees.
- **OA-JSON-002** Every bound is a constant in `Limits` and exceeding it is a typed error with the
  byte offset: input bytes, nesting depth, string bytes (counted on the raw text, escapes
  included) and node count (values plus object keys); a text exactly at a bound is accepted.
- **OA-JSON-003** Numbers are kept as their source text; `as_f64` returns `None` when the value is
  not finite, and `as_i64` returns `None` for a fraction, an exponent or a value outside `i64`
  (`i64::MIN` and `i64::MAX` are accepted).
- **OA-JSON-004** Strings decode the eight short escapes and `\u` including surrogate pairs; a lone
  or mis-ordered surrogate, a bad escape and an unescaped control character are refused with
  their own errors.
- **OA-JSON-005** Invalid UTF-8 anywhere in the input, a byte-order mark, and any non-whitespace
  after the top-level value are refused, the last with the offset of the first extra byte.
- **OA-JSON-006** Duplicate object keys are accepted and all kept in document order; `get`
  returns the last.
- **OA-JSON-007** Nothing recurses: a million nested arrays parse (with the depth limit raised)
  and are walked without overflowing the stack, and the default depth limit refuses them.
- **OA-JSON-008** For each of the 35 JSONTestSuite `i_` cases the outcome is the documented choice
  in the test table (out-of-range numbers accepted as text with `as_f64` finite or not; lone
  surrogates, invalid UTF-8, a byte-order mark and 500-deep nesting refused); a case added to
  the suite without a documented choice fails the test.
- **OA-JSON-009** The document is a flat arena in document order; `elements`, `members`, `get`,
  `skip` and the kind accessors give the right nodes for nested arrays and objects, and queries
  of the wrong kind or an out-of-range index return empty or `None`.
- **OA-JSON-010** No input panics: every prefix of a sample and 20 000 seeded random byte
  mutations give a document or an error.

### Tar archives (oa-tar)

- **OA-TAR-001** Reads ustar entries with prefix names, directories, empty files and data padded
  to 512-byte blocks, and finds the end-of-archive marker.
- **OA-TAR-002** A sequence of readers is one stream: splitting the archive at every byte offset
  into two parts, and into parts of one byte and zero bytes, gives the same entries.
- **OA-TAR-003** If the stream ends early, at any byte offset, the reader returns a typed
  `Truncated` (offset, stage, entry, declared size, data present) after the complete entries,
  never an error and never a panic; a stream ending between entries without the end marker, or
  inside the marker, is also `Truncated`; the reader stays finished.
- **OA-TAR-004** GNU long names (type `L`), PAX `path` overrides and ignored keys, skipped PAX
  global headers, and GNU base-256 sizes are read.
- **OA-TAR-005** A size over the entry limit, a name over the name limit, a long-name or PAX record
  over the metadata limit, a bad checksum, a non-octal size, a non-UTF-8 name, a negative
  base-256 size and data after a lone zero block are each refused with a typed error carrying
  the offset, and the reader then refuses further calls.
- **OA-TAR-006** Entry data the caller did not read is skipped; link, device and directory entries
  carry no data whatever the size field says.
- **OA-TAR-007** A reader error is reported with the stream offset and an interrupted read is
  retried.
- **OA-TAR-008** Seeded random byte damage and garbage of several lengths never panic.

### ADS-B history (oa-adsb)

- **OA-ADSB-001** A trace row with every column and an aircraft object decodes to the right time
  (base plus offset, to the millisecond), position (1e-7 degree), barometric and geometric
  altitude, ground speed (0.1 kt), track (0.1 degree), vertical rate, source type, callsign,
  squawk, category, `nic`, `nac_p`, `nac_v`, `sil`, `sil_type`, `version`, `gva` and `sda`;
  a row without an object has those fields absent.
- **OA-ADSB-002** Each flag bit (stale, new leg, rate is geometric, altitude is geometric) is read
  as readsb writes it; a geometric altitude is reported as geometric and not as barometric;
  `"ground"` sets the ground flag; null altitude, speed, track and rate are absent.
- **OA-ADSB-003** All 14 readsb source types are recognised by name in readsb's order; rows from
  files before 2022 (7 to 9 columns) decode with the source absent; extra columns are ignored.
- **OA-ADSB-004** Aircraft-object fields are range checked (bit-field widths) and trailing spaces
  are removed from callsigns; unknown keys are ignored; duplicate keys keep the last; a malformed
  known field refuses the row as `object`.
- **OA-ADSB-005** Each value rule (position range, altitude, speed, track, flags, vertical rate,
  source, row shape, time) refuses the row alone and counts it under its own reason; the other
  rows of the trace are kept.
- **OA-ADSB-006** The box (edges inclusive) and the time window (`[from, to)`) are applied while
  decoding; rows outside are counted separately and not delivered; the ZAU box is
  38.5 to 46.5 N, 83.5 to 93.5 W.
- **OA-ADSB-007** A structural error (not an object, missing or malformed `icao`, `timestamp` or
  `trace`, an out-of-range timestamp, a JSON error) refuses the whole trace and leaves the output
  and the counts untouched; a `~` address sets the non-ICAO bit.
- **OA-ADSB-008** A trace over the row limit is refused whole; a row over the column limit and an
  object over the member limit are refused alone.
- **OA-ADSB-009** No trace input panics: every prefix of a sample and 5 000 seeded random byte
  mutations give reports or an error, and every kept report is in range.
- **OA-ADSB-010** The OAF1 payload (`docs/ADSB_PAYLOAD.md`) round-trips and has the documented
  layout: header, 64-byte little-endian records, reserved bytes zero.
- **OA-ADSB-011** A payload with a wrong length, magic, record length, reserved field, count or a
  record value out of range is refused and appends nothing; the encoder refuses an empty or
  out-of-range row range and more than the per-frame maximum.
- **OA-ADSB-012** The summary counts distinct aircraft, reports with `nac_p`, and those with
  `nac_p` of 8 or more, and finds position jumps (implied speed over 700 kt and at least
  0.5 NM apart) between consecutive reports of one aircraft in time order, exempting a report
  that starts a new leg and counting closer pairs separately as noise; input order does not
  matter.
- **OA-ADSB-013** Scanning a day archive decodes gzip and plain trace files, skips other entries,
  counts and names a corrupt gzip and a JSON error without stopping, delivers one batch per
  trace with reports, and applies the time window.
- **OA-ADSB-014** An archive split into parts gives the same result as the whole; the first part
  alone ends in a typed truncation after delivering every complete trace before the cut; a sink
  error stops the scan.
- **OA-ADSB-015** `oa-adsb-clip` writes OAF1 MESSAGE frames sorted by time whose receive time is
  the first report's time, honours `--box`, `--from` and `--to` (Unix seconds or ISO UTC),
  reports truncation and exits 3 unless `--allow-truncated`, writes only frames to standard
  output with `--out -`, and exits 2 on bad arguments and 1 on an unreadable part.
- **OA-ADSB-016** On the real first part of an adsb.lol day archive (`OPENATCA_DATA/raw/adsblol`;
  the first 8 MiB, or all of it with `OPENATCA_ADSB_FULL`) no trace is refused and no row is
  rejected, reports are kept in the ZAU box, and under 0.1 % of consecutive pairs are position
  jumps; the test prints the counts and skips when the data is absent.

## M5 — Minimum vectoring and IFR altitudes (oa-mva)

- **OA-MVA-001** Loads one FAA AIXM 5.1 chart message (MVA FUS3, MVA FUS5 or the center MIA
  "TAV") into struct-of-arrays sectors: facility and set taken from the message name, sector id,
  minimum altitude in feet MSL, and vertices stored as exact 1e-7 degrees.
- **OA-MVA-002** A sector holds its exterior ring first and its holes after it; rings are ranges
  of one shared vertex table (closing vertex kept as given) and a hole flag is a bitset.
- **OA-MVA-003** Anything outside the documented accepted structure is refused with a typed
  error naming the element (an upper limit, a curve segment, a second patch or time slice) or
  attribute, and a refused message leaves the model unchanged.
- **OA-MVA-004** A value outside what is accepted is refused naming the field: a unit other
  than FT, a limit reference other than MSL, a coordinate reference other than CRS84, non-planar
  interpolation, a time period that ends, an interpretation other than BASELINE, a sector name
  that is not 1 to 8 letters and digits, an altitude outside 0 to 18,000 ft, a longitude or
  latitude out of range, an odd or short coordinate list, three-dimensional coordinates, and a
  ring that does not close.
- **OA-MVA-005** Every count is bounded by a named constant: file bytes, sectors per chart,
  rings per sector, vertices per ring, sector extent, charts and vertices in the model; a
  duplicate sector in a chart and a duplicate chart are refused.
- **OA-MVA-006** Decimal degrees are converted to 1e-7 degrees with integer arithmetic only;
  more than 7 decimals, signs other than a leading minus, exponents and empty parts are refused.
- **OA-MVA-007** `min_altitude_at` returns the highest minimum altitude of the sectors of the
  requested set that contain the point, with how many contain it, and `None` where no sector
  does; sets are never mixed.
- **OA-MVA-008** Point containment is exact (128-bit integer arithmetic): a point on a sector's
  edge or vertex is inside it, a point strictly inside a hole is outside it, a point on a hole's
  edge is inside it, and a point one unit (1e-7 degree) off an edge is outside.
- **OA-MVA-009** `sectors_within_radius` lists every sector of the set whose polygon (edge or
  interior, holes included) comes within the radius, not only those with a vertex within it,
  using a distance metric that never exceeds the true WGS-84 distance.
- **OA-MVA-010** Queries refuse a latitude or longitude that is out of range or not finite and a
  radius that is not positive, finite and at most 200 nm; an empty model answers `None`.
- **OA-MVA-011** Ring integrity is counted and never repaired: open rings, proper crossings,
  touching edge pairs, zero-width spikes, repeated vertices, zero-area rings, clockwise
  exteriors, and holes outside their exterior.
- **OA-MVA-012** Coverage of a chart is rasterised and reports gaps (uncovered pockets inside
  the union of its sectors) and overlaps (cells claimed by two sectors) with areas, with sectors
  that share an edge neither overlapping nor leaving a gap.
- **OA-MVA-013** Sector labels are read from `pdftotext -raw` text and compared with the XML:
  matched, mismatched, missing from the PDF and extra on the PDF are reported separately.
- **OA-MVA-014** Terrain maxima take only the samples whose centres lie inside the sector
  polygon, count no-data samples separately, and the margin is the minimum altitude less the
  higher of the terrain and obstacle maxima.
- **OA-MVA-015** On the real FAA files (`OPENATCA_DATA/raw/mva`; the test prints the counts and
  skips when the data is absent) all 35 messages load with no refusal: 423 sectors, 604 rings,
  altitudes 2,000 to 4,300 ft, and queries at known places answer.
- **OA-MVA-016** On the real files every ring closes, no polygon crosses itself, and there are
  no spikes, zero-area rings or holes outside their exterior.
- **OA-MVA-017** On the real files no gap between the sectors of a chart is 10 raster cells
  (about 0.1 nm2) or larger, and no overlap is 2 nm2 or larger.
- **OA-MVA-018** On the real files every sector altitude that appears on its PDF plot equals the
  XML value, except the two charts recorded as findings (CVG FUS3 and TOL FUS5); the test skips
  when `pdftotext` is absent.
- **OA-MVA-019** On the real Rockford FUS3 chart (every chart with `OPENATCA_MVA_FULL`) the
  3DEP terrain and DOF obstacle maxima are found for every sector and every margin is positive;
  the test skips when the terrain or DOF files are absent.
- **OA-MVA-020** `oa-mva-check` prints the load, ring, coverage, PDF and clearance sections,
  exits 0 when every file loads, 1 when a file is refused (naming it), and 2 on bad arguments.
