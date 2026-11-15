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
- **OA-ASP-064** A special-use boundary keeps its source time code and level: `time_code` is
  continuous, part-time (blank in the source) or unknown, and `level` is high, low, both or
  unknown; a boundary from NASR or a controlled-airspace volume, and any other source letter,
  is unknown, never a default. The columns are in the snapshot (format version 2); a version 1
  snapshot is refused with a typed version error.


### Traceability (oa-trace)

- **OA-TRC-001** Lists every requirement without a test or proof and every test or proof tag naming an
  unknown requirement, and exits non-zero if either list is non-empty. A Kani harness (`// REQ:` above
  `#[kani::proof]`, with an optional `#[kani::unwind(n)]`) counts as a proof and the report marks it as one,
  apart from tests.
- **OA-TRC-100** A `// REQ:` tag above `#[kani::proof]` (or an unwind bound and then the proof, in either
  order) is accepted as a proof and listed as one; a tag above an unwind bound with no proof is malformed.

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
- **OA-ZIP-061** A repeated entry name is refused with `DuplicateName` (the first repeat in
  directory order), exactly and at any length; the check costs time proportional to the total
  name bytes, not to the square of the entry count: ten thousand 4,096-byte names sharing a
  4,093-byte prefix parse in seconds.


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
- **OA-XML-028** The pieces of an element's text that a comment or CDATA section splits are several
  text nodes of one owner, and `decode_text_run` joins them as XML 1.0 defines the element's
  character data (`12<!--x-->34` and `<![CDATA[12]]>34` are `1234`), decoding each as text and
  bounded by the output buffer (`OutputTooSmall` past it). Whitespace-only text is still layout
  and not stored, except next to text or a CDATA section of the same element (`a<!--c--> <!--d-->b`),
  where it is data.

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
- **OA-FEED-033** Flush timing and recording file names follow the local clock, not frame
  stamps: a frame stamped far in the future does not stop later flushes or name the file, and frames
  whose stamp is over an hour from local time are counted and reported.
- **OA-FEED-034** The sequence-gap total saturates instead of overflowing when sequence numbers
  jump by more than the counter can hold.
- **OA-FEED-050** A frame whose reserved header bytes (offsets 6 and 7) are not zero is refused
  with its own error kind (`Reserved`) even when its CRC matches, counted by `oa-feed` health and by
  `oa-replay`, and the reader resynchronises after it; `docs/FEED_FRAMES.md` states the rule, and
  every frame the encoder or the bridge writes has zero there (the real recordings replay without
  one).
- **OA-FEED-051** A recording that fails while writing (a write, an idle flush or the final
  finish) is a persistent failed state of the recorder: the failure is kept with its first error,
  every later frame is counted as lost rather than silently dropped, the final summary prints the
  failure and the lost count, and `oa-feed` exits non-zero when a requested recording failed
  (feed consumption may continue, and the log says so). Tested with a writer that fails on demand.
- **OA-FEED-100** (proof) For every sequence number and every reachable health counter, `observe_seq` never
  panics or overflows; a forward jump adds exactly one gap event and the saturated number of missing frames,
  and any other frame changes neither.

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
- **OA-TBFM-009** A flight (see OA-TBFM-019) is accepted whole or not at all; a wrong root or
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
- **OA-TBFM-019** A flight is decoded all or nothing, not the message: an `air` with a refused field
  is rolled back (rows and counters) and counted with its field and reason (`rejected`,
  `first_reject`, per field and per reason), its sibling flights are kept, and a `tma` with bad
  attributes goes the same way; a bad envelope, a wrong root, an XML error and a capacity overrun
  still refuse the whole message. Statistics and the replay pipeline count the refused flights.
- **OA-TBFM-020** `spd` carries its unit with the value (`spd_unit`: none, knots, Mach, unknown), read
  from the value's form because the feed has no marker: `ddd.d` is knots, `d.dd` is Mach (all
  jets in the recorded sample), any other form is stored with the unknown unit and counted
  (`unknown_units`); units are never mixed in a column without it.
- **OA-TBFM-021** A schedule flag with a word other than `YES` or `NO` keeps the flight, stores the flag
  as unknown and is counted (`unknown_words`); an unknown change type still refuses the flight.
  `msgId` (ten digits) is stored as `u64`.
- **OA-TBFM-022** Values are trimmed of outer whitespace; the site-adapted labels (`gat`, `cfg`, `scn`,
  `cat`) may hold interior spaces (printable ASCII), identifiers and fixes may not. The TBFM schema
  is not in the repository, so that labels allow spaces is a decision, not a confirmed rule.
- **OA-TBFM-023** Text split by a comment or CDATA section is joined before validation, and the 64-byte
  working bound applies to the total.

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
- **OA-TFMS-012** A message is accepted whole or not at all (OA-TFMS-023); a capacity
  overrun or a text-table overrun leaves the caller's rows and text table exactly
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
- **OA-TFMS-023** A refused field refuses its message, not the document: the message (or TMI flight list
  entry) is rolled back with the text table and counters, counted with its field and reason, and
  its siblings are kept; only an unreadable document or a capacity overrun is refused whole.
- **OA-TFMS-024** A word outside a vocabulary (trigger, status, categories, time types, program, scope,
  true/false flags) keeps the message with the column marked unknown (`UNKNOWN_WORD`,
  `flag::UNKNOWN`) and counted; the direction of a position still refuses the message.
- **OA-TFMS-025** Ten-digit identifiers (`flightRef`, restriction id, reference id, number) are `u64`; eleven
  digits are refused. The replay tables key on them and hash ordinary values as before.
- **OA-TFMS-026** Text split by a comment or CDATA section is joined, for values and for the text table,
  with the size bounds applied to the total.
- **OA-TFMS-027** A speed is stored in thousandths with its unit (`speed_unit`, `tas_unit`: none, knots, Mach,
  unknown): a whole number, `ddd.d` and `Nddddd` are knots, `d.dd` and `Mddd` are Mach; any other shape of
  digits, `.`, `M` and `N` is kept with unit unknown (the value when it reads as a number) and counted
  (`unknown_units`), and the message is kept; a byte that is none of those refuses the message as before.
- **OA-TFMS-028** Whether a message carried a flight reference is its own flag (`flag::FLIGHT_REF`), so a reference
  of 0 ("0000000000") is told from none; the linkage counts a row without one in `tfms_rows_without_ref` and
  treats it as a flight of its own, never as part of the flight whose reference is 0.

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
- **OA-RPL-009** A flight is evicted only when its last update is more than its age limit
  (default 30 minutes, configurable, for a flight with no status seen; see OA-RPL-067 for the others)
  older than the replay time, at a sweep that runs on a fixed
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
- **OA-RPL-018** A frame earlier than one already seen, by no more than the clock's step tolerance
  (10 minutes), does not move replay time back and is counted as a time regression; a larger
  difference follows OA-RPL-061 to OA-RPL-063.
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
- **OA-RPL-029** TFMS and FNS payloads are decoded (OA-RPL-041 to OA-RPL-059); a payload of the wrong
  kind of document is counted under its typed error, and nothing is left in a "not applied" bucket
  (the `not_applied` count stays in the summary line and is always 0).

### ADS-B in the replay (oa-replay)

ADS-B reports (`docs/ADSB_PAYLOAD.md`) merge with the SWIM recordings in one time-ordered stream
(input path of the M2 benchmark, `docs/M2_DESIGN.md`).

- **OA-RPL-030** The ADS-B aircraft table keeps, per address (the 24-bit address, with the
  non-ICAO bit making a separate key), the latest report (time, latitude, longitude, barometric
  altitude, ground speed, track, flags, source type) and counts; callsign, squawk, category and the
  quality fields (NIC, NACp, NACv, SIL, SIL type, version, GVA, SDA) are carried forward from
  reports that have them.
- **OA-RPL-031** The ADS-B table is a struct-of-arrays of fixed capacity (65,536 aircraft) allocated
  once; a new address when it is full is refused and counted (per table and per feed), known
  aircraft keep updating, and no column moves or grows. Aircraft not heard for a while are freed
  (OA-RPL-065).
- **OA-RPL-032** An ADS-B report older than the aircraft's held report is refused whole and counted
  (per aircraft, in total and per feed); an equal time applies.
- **OA-RPL-033** The ADS-B table is part of the replay state hash (`replay_hash`, used by every
  checkpoint and the final hash) in address order, so the hash does not depend on the order
  aircraft were first seen, and changes with any column or count.
- **OA-RPL-034** Until an ADS-B frame has been fed the replay hash equals the flight-table hash, so
  the checkpoints and final hash of every replay without ADS-B are unchanged (pinned to the value the
  code gave before ADS-B existed).
- **OA-RPL-035** An ADS-B payload whose header or length `oa_adsb::decode_payload_lenient` refuses
  (length, magic, layout, too many records) is counted under its own error kind and applies none of
  its reports; no corruption of a payload panics.
- **OA-RPL-036** ADS-B is a feed kind of its own that sorts after TFMS and FNS in the merge key, so
  at an equal receive time SWIM frames come first and ADS-B last, whatever order the streams were
  named in; the merged order is still the global sort of the total key (OA-RPL-021).
- **OA-RPL-037** A mixed SFDPS, TBFM and ADS-B replay of hand-made recordings gives the same
  checkpoints, counters and final hash in every stream order and read size, with the right frame,
  row, aircraft and error counts.
- **OA-RPL-038** `--adsb <file>` and `--adsb-dir <dir>` give a file or a directory of rotated
  recordings the ADS-B feed kind; the binary prints the ADS-B feed line, an `adsb` table line and
  the errors, identical run to run and across read sizes and rotation.
- **OA-RPL-039** Replaying a real ADS-B clip written by `oa-adsb-clip` (`OPENATCA_ADSB_CLIP`) twice
  and with another read size gives identical checkpoints and final hash, with no error and no refused
  aircraft (skipped if unset).
- **OA-RPL-040** An `oa-replay` merge of a one-second-frame ADS-B clip and a SWIM feed gives the
  frames in receive-time order, interleaved at about one-second granularity.

### TFMS and FNS in the replay (oa-replay)

TFMS flight and flow messages (`oa-tfms`) and FNS NOTAMs (`oa-notam`) are applied to tables of their
own beside the flight table. Both are bounded, struct-of-arrays, and left out of the state hash until a
frame of their feed has been fed (as the ADS-B table is).

- **OA-RPL-041** The TFMS flight table keeps, per flight reference, the latest value of each estimate and
  time the messages carried (estimated and actual departure and arrival, original, runway, gate, fix and
  coordination times, estimate flags and types, status), leaving a value alone when a later row lacks it;
  identity (acid, departure, arrival, identifier) fills an empty column and a disagreement is counted.
- **OA-RPL-042** A TFMS flight row older (by `sourceTimeStamp`) than the flight's latest is refused whole
  and counted; an equal time applies.
- **OA-RPL-043** A TFMS flight links to the SFDPS flight whose flight-plan identifier equals its
  identifier, or, with no identifier, to the single SFDPS flight with its acid, departure and arrival;
  an ambiguous key and an identifier that matches nothing leave it unlinked; the link is the SFDPS GUFI,
  retried while unlinked (the identifier every row, the acid key on the 1st, 2nd, 4th, 8th ... row) and counted by method.
- **OA-RPL-044** TFMS programme rows (restrictions, advisories, FCA/FEA, airport configurations) keep one
  row per programme key with the latest row, count rows by program type, refuse an older row, and leave
  the table at the first sweep after the programme's end time.
- **OA-RPL-045** A TMI flight list entry sets the flight's TMI update type, flight-list entry count, first
  FCA identifier, TMI update time and status.
- **OA-RPL-046** The TFMS flight table has a fixed capacity: a new flight with no free slot is refused and
  counted, a held flight still updates, a flight silent for six hours of replay time is evicted at a
  sweep and its slot is reused.
- **OA-RPL-047** A TFMS payload that is not well-formed XML, not a TFMS document, has a refused field or
  is counted under its own typed error and applies nothing; the next good payload applies. A payload
  with more rows than the row tables hold applies the rows that fit (OA-RPL-073).
- **OA-RPL-048** A TFMS flight row with no flight reference is not applied and is counted.
- **OA-RPL-049** FNS new, replacement and cancellation NOTAMs are keyed by series, number and year (a
  local-format NOTAM, which has no series, by its location in the series' place): a
  replacement removes the NOTAM it names and is stored, a cancellation removes the NOTAM it names and is
  not stored, and a replacement or cancellation naming nothing held is counted.
- **OA-RPL-050** A NOTAM re-issued under a held identity replaces it whole, unless it was issued earlier
  than the held one, which is refused whole and counted.
- **OA-RPL-051** A NOTAM leaves the table at the first sweep after its end or cancelled time in replay
  time; one with no end time leaves after thirty days without an update; a reader can ask how many are in
  force at a time.
- **OA-RPL-052** A NOTAM keeps its circle, altitude limits and references, affected feature, polygon and
  vertex counts and the bounding box of its polygon vertices.
- **OA-RPL-053** An FNS payload that is not well-formed XML or is refused by the decoder, and a row with
  no usable identity, are counted under typed errors and applied to nothing; a malformed
  field is counted and treated as absent.
- **OA-RPL-054** The NOTAM table has a fixed capacity and refuses and counts a new NOTAM when full.
- **OA-RPL-055** Until a TFMS or an FNS frame has been fed the replay hash is the one it was before they
  were applied, so SFDPS and TBFM replays (with or without ADS-B) hash unchanged.
- **OA-RPL-056** Once fed, the TFMS flight, TFMS programme and FNS tables are in the replay hash, each
  table's change moves it, and the same frames give the same hash.
- **OA-RPL-057** A mixed SFDPS, TFMS, FNS replay from hand-made recordings gives the same checkpoints,
  errors and final hash whatever the order the files are named in and however they are chunked.
- **OA-RPL-058** TFMS and FNS STATUS and REJECT frames are counted as other frames and apply nothing.
- **OA-RPL-059** Replaying the real 2026-11-14 SCDS TFMS and FNS samples (`OPENATCA_DATA`, `.msg` files
  given synthetic one-second receive times by the test) twice gives identical checkpoints, errors and
  final hash (skipped if absent).
- **OA-RPL-060** The TFMS, programme and NOTAM tables allocate nothing after construction: every buffer
  keeps its address and capacity through applying, sweeping and reusing slots.

### Clock, ages and oversized documents (oa-replay)

- **OA-RPL-061** One frame whose receive time is more than 2 minutes ahead of the replay clock and of its
  feed's last frame does not move the clock, is processed at the old time and is counted as held, and nothing is evicted
  because of it.
- **OA-RPL-062** A forward step of the receive times confirmed by three consecutive frames (each no
  earlier than the one before, all more than 2 minutes ahead) moves the clock to the third frame and
  shifts the last-update times in the flight, TFMS, programme and NOTAM tables by the same amount, so
  no flight is evicted by the step and eviction goes on from the new time.
- **OA-RPL-063** The same for a step back: three consecutive frames more than 2 minutes behind the
  clock move it back (the earlier ones are counted as time regressions as before), the tables' times
  shift with it, and time is not frozen.
- **OA-RPL-064** A replay whose frames are never more than 2 minutes from the clock (small
  regressions included) hashes exactly as it did before the step rules, so the existing hashes hold.
- **OA-RPL-065** The ADS-B table frees an aircraft whose held report is more than 30 minutes older than
  the replay clock, at a sweep each minute of replay time, and reuses the slot; a long run or a flood
  of addresses therefore fills the table only while it lasts, and the hash does not depend on slot
  assignment.
- **OA-RPL-066** An ADS-B report whose time is more than 2 minutes after the replay clock is refused
  and counted before it takes a slot or becomes the held report, so it cannot make the real reports of
  that aircraft look out of order.
- **OA-RPL-067** A flight stays as long as its latest SFDPS flight status allows without a message:
  active 2 hours, proposed 4 hours, completed, cancelled or dropped 10 minutes, no status seen 30
  minutes (each configurable), judged at the sweep.
- **OA-RPL-068** An SFDPS document with more messages than the event table has rows applies the first
  messages that fit; the others are counted, one each, as row-capacity errors.
- **OA-RPL-069** An SFDPS document whose route text does not fit the string table applies the messages
  before the first that does not fit; the others are counted, one each, as string-capacity errors.
- **OA-RPL-070** A TBFM document with more rows than the row table applies the rows that fit; the
  others are counted, one each, as capacity errors.
- **OA-RPL-071** Wild frames that are scattered, or that disagree in direction or order, never confirm
  a step and each is counted as held; confirmed forward and backward steps are counted separately.
- **OA-RPL-072** ADS-B evictions and refusals for time are counted, appear in the replay hash from the
  first of either (under a new tag; a replay with neither hashes as before), and the same history
  gives the same hash.
- **OA-RPL-073** A TFMS document with more flight or programme rows than the row tables applies the
  rows that fit; the others are counted, one each, as capacity errors.
- **OA-RPL-074** The three state ages are settings that must be positive and have command-line flags
  (`--active-age-secs`, `--proposed-age-secs`, `--terminal-age-secs`).
- **OA-RPL-075** A flight's age limit follows its latest status: a message with no status leaves it, a
  new status replaces it.
- **OA-RPL-076** Ten-digit TFMS flight references and restriction identifiers that differ by a multiple of 2^32
  are different flights and programmes; the state hash writes an identifier below `u32::MAX` as four
  bytes as before and a larger one as an escape plus eight bytes.
- **OA-RPL-077** An ADS-B record whose position or flags are out of range is dropped and counted under
  the bad-record error kind, one for each; the other records of its payload are applied, and a record
  with only an unknown source, a bad measurement or a set reserved byte is kept.
- **OA-RPL-078** A frame more than 2 minutes ahead of the clock and of its own feed's last frame is
  held, not accepted: 9.9 minutes ahead, repeated among honest frames, never moves the clock and the
  honest frames are never counted as regressions.
- **OA-RPL-079** Each feed's frames are compared with that feed's own last frame: feeds whose receive
  times differ by a constant under the tolerance count no regressions against each other, a frame
  before its own feed's last is still a regression, and a feed steadily more than the tolerance behind
  the others is held and never steps the clock.
- **OA-RPL-080** A confirmed step marks every flight, programme and NOTAM row as stale until it is
  updated; the marked rows are counted (`stale_marked`) and the rows still stale can be counted at any
  time (`pipeline_stale_count`); eviction ages are as before.
- **OA-RPL-081** A frame time equal to `i64::MIN`, the clock's "no frame yet" value, is held and counted,
  never taken as the clock or as a candidate: five such frames neither step the clock nor let a later
  frame a day ahead be taken as a first frame, and before the first frame it is held as well.
- **OA-RPL-100** (proof) `clock_observe` never panics or overflows for any clock state, feed index (any
  `usize`) and frame time (any `i64`, including the extremes) from any reachable candidate state; a step is
  returned only on the frame that completes `CLOCK_CONFIRM_FRAMES` consecutive candidates of that frame's
  feed, with the saturated difference as its delta, every candidate count is then cleared, no step is returned
  from an empty candidate state, and a frame never adds candidates to another feed.
- **OA-RPL-101** (proof) From any state the pipeline can hold (every feed with an accepted frame has it at or
  before the clock), one frame that does not complete a confirmation moves a running clock forward by at most
  `CLOCK_TOLERANCE_NS` (2 minutes), and the state stays consistent with the clock that results.

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

- **OA-TIFF-020** The `*_conservative` radius queries count a sample when any quad it is a corner
  of touches the circle (the closed square of one full step around its centre, a superset of the
  `PixelIsArea` pixel and valid for `PixelIsPoint`), so no interpolation between samples can exceed
  the result; a high sample clipped by the circle edge is found, a query smaller than one cell
  returns the corners of the cell it lies in, and a sample two steps out is not counted.
- **OA-TIFF-021** The conservative radius is a WGS 84 geodesic distance bounded below with the
  smallest meridian and prime-vertical radii over the latitude band the radius can reach, so a
  sample within the true distance always counts and over-inclusion is bounded (documented, tested
  against Vincenty's inverse formula at 0.5, 20, 41 and 60 degrees); a circle reaching a pole or
  crossing the antimeridian is refused with a typed error.
- **OA-TIFF-022** The conservative polygon queries count a sample when its footprint touches the
  polygon (edge contact, containment either way, concave polygons, polygons smaller than a cell).
- **OA-TIFF-023** The conservative queries across a tile set (abutting and overlapping tiles)
  return what the undivided grid returns; for overlapping tiles a valid sample beats nodata.
- **OA-TIFF-024** A conservative result is never a bare number: it is complete or incomplete, and
  an incomplete one carries the count of nodata samples, the count of missing samples (the area
  leaves the loaded data, including past the outermost samples), the number of whole-degree squares
  holding them, and the highest valid sample as a lower bound only.
- **OA-TIFF-025** Conservative queries refuse an empty tile set, tiles that do not share one
  lattice, too many tiles, too large a region, a bad radius, a bad centre and bad polygons.
- **OA-TIFF-026** On random small grids the conservative radius maximum is never below a
  brute-force reference (bilinear and nearest-sample surfaces sampled at 1/16 of a step, inside the
  circle by WGS 84 distance) and never above the best sample within the documented over-inclusion
  margin.
- **OA-TIFF-027** The same property for polygons against a brute-force reference.
- **OA-TIFF-028** On a 3 by 3 block of real 3DEP tiles around Chicago (`OPENATCA_DATA`), for 500
  random points and radii of 1, 3 and 5 nm, every conservative answer is complete and not below
  the centre-only maximum except for the sphere-versus-ellipsoid radius difference (under 1% of
  queries); the test reports how often and by how much (in feet) it exceeds it and the query time;
  skipped when the data is absent.
- **OA-TIFF-029** Before the elevation array is sized from the declared dimensions, every strip or
  tile must lie inside the file and be able to hold what the declaration needs (at least its
  uncompressed size, or its size over 1032 when compressed, the most DEFLATE can reach); a file of
  a few hundred bytes declaring 8192 by 8192 is refused with a typed error and nothing allocated.
- **OA-TIFF-030** The Orientation tag is honoured by refusal: absent or 1 (top-left) is read, any
  other value is refused with a typed `Unsupported` error rather than placing terrain wrongly.
- **OA-TIFF-080** A point at exactly +180 or -180 degrees longitude (the same meridian) gets the
  same elevation answer from a tile that holds either alias.

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
- **OA-TAR-058** One `next_entry` call consumes at most `max_meta_records` long-name, long-link
  and PAX header records (zero-length ones included) and `max_meta_total_bytes` of their
  content; a chain longer than either is refused with a typed `TooManyMeta` error carrying the
  offset and the reader then refuses further calls, so a caller never waits on an unbounded run
  of extension headers. The default ceilings are 64 records and 1 MiB.

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
  or null known field is left absent and counted (OA-ADSB-025), the row is kept.
- **OA-ADSB-005** Only the rules the position's meaning needs (position range, flags, row shape,
  time) refuse a row, alone, counted under their own reason; the other rows of the trace are kept.
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
  layout: header, 80-byte little-endian records, reserved byte zero.
- **OA-ADSB-011** A payload with a wrong length, magic, record length, reserved field, count or a
  record value out of range is refused and appends nothing; the encoder refuses an empty or
  out-of-range row range and more than the per-frame maximum.
- **OA-ADSB-012** The summary counts distinct aircraft, reports with `nac_p`, and those with
  `nac_p` of 8 or more, and finds position jumps (a pair the speed rule of OA-ADSB-028 calls
  impossible and at least 0.5 NM apart) between consecutive reports of one aircraft in time order,
  exempting a report
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
- **OA-ADSB-017** `oa-adsb-clip` starts a new frame when the next report is at least
  `--frame-ms` (default 1000) after the frame's first report, stamps each frame with its first
  report's time, and writes every report exactly once and in time order whatever the span.
- **OA-ADSB-018** `--frame-ms` outside 1 to 60000, or not a whole number, is refused with exit 2
  and the usage line; 1 and 60000 are accepted.
- **OA-ADSB-019** The frame splitter ends a frame at the span (a report exactly one span later
  starts the next) or at 60 000 reports, clamps the span to 1 to 60000 ms, and its ranges are
  contiguous, non-empty, in order and cover every report once.
- **OA-ADSB-020** A clip frames reports with any grouping, including the earlier one of up to
  60 000 reports per frame, and `decode_payload` gives the same reports in the same order.
- **OA-ADSB-021** The `ADS2` payload carries the measurements (wind speed and direction, true and
  indicated airspeed, Mach, magnetic and true heading, outer and total air temperature, roll, track
  rate) in 80-byte records at the documented offsets, round-trips them, and refuses a record whose
  direction, heading or roll is out of range.
- **OA-ADSB-022** An `ADS1` payload as the first clips wrote it still decodes, with the measurements
  absent and its own rules (64-byte records, zero bytes after offset 57, up to 60 000 records).
- **OA-ADSB-023** The measurements are read from the aircraft object (and indicated airspeed and roll
  from the row's own columns, the object overriding), are per observation and never carried to a row
  without them.
- **OA-ADSB-024** A malformed or out-of-range measurement is left absent and counted once per row in
  `Stats::bad_measurements`; the row is still kept; 360 degrees wraps to 0.
- **OA-ADSB-025** A row keeps its position when a field other than time, position, shape or flags is
  unknown, malformed, out of range or null: an altitude, speed, track, vertical rate, an object
  field (callsign, squawk, category, NIC, NACp, NACv, SIL, GVA, SDA, version, a non-object or
  oversized object) is left absent, an unknown source type becomes `unknown` and an unknown
  `sil_type` name becomes `unknown`, flag bits above the four readsb defines are cleared. Each
  occurrence is counted per field and per value (`Stats::unknown_fields`, `Stats::unknown_values`
  for the first 32 distinct pairs, the rest in `unknown_overflow`); counts add across traces. Flags
  that are null, not an integer or above 255 still refuse the row, because they say whether the
  position is current and continuous.
- **OA-ADSB-026** Rows of a trace whose address is 0x000000 or 0xFFFFFF (misconfigured
  transponders, shared by unrelated aircraft) are counted in `Stats::reserved_address` and not
  output; the same digits with the non-ICAO mark are an ordinary identifier.
- **OA-ADSB-027** `decode_payload_lenient` keeps the readable records of a payload and counts the
  rest (position or flags out of range dropped; unknown source kept as `unknown`; bad measurement
  kept absent; reserved byte ignored) and a wrong header or length still refuses the whole payload;
  `decode_payload` stays strict and gives the same reports on a clean payload. The `ADS1` and
  `ADS2` layouts are unchanged.
- **OA-ADSB-028** `jump_verdict` decides whether a displacement is possible from the distance, the
  time gap, the emitter category and the reported ground speeds: not over the category's hard cap
  (1,000 kt ordinary and reserved codes, 2,000 kt high-performance, unmanned and no-information,
  4,088 kt space), consistent with a reported ground speed (at most 1.2 times the larger plus 40 kt plus 2 kt
  per second of gap), and under the class ceiling when no ground speed is given. A 900 kt aircraft
  reporting 900 kt is plausible; a 450 kt aircraft that jumps 2,782 kt is not; a displacement
  within the position allowance is always plausible.
- **OA-ADSB-029** The emitter category the jump rule uses comes from `category_observe`: an ordinary
  class is taken at once, a cap-raising one (`A0`, `A6`, `B0`, `B6`, `B7`) only after three
  consecutive printed categories agree, and a reserved code is ignored and never raises the cap above
  the ordinary one. The summary applies it, and a report that is itself a jump does not count.

- **OA-ADSB-150** The hard cap of an emitter category is 2,000 kt for `A0`, `A6`, `B0`, `B6` and no category
  (never below a configured ordinary cap), 4,088 kt for `B7`, and the ordinary cap for every other code, reserved
  codes included; the no-speed ceiling is 600 kt for `A1` and `A7`, 450 kt for `B1` to `B4`, 800 kt for `A2` to
  `A5` and `B6`, 200 kt for `C1` to `C3`, 4,088 kt for `B7` and 2,000 kt for every other code.
- **OA-ADSB-151** A pair beyond the position allowance whose time gap is not a positive number (NaN, zero,
  negative) or whose distance is not a number is a jump of the cap kind; within the allowance it is plausible
  whatever the gap; the allowance widens the cap and the ceiling alike.
- **OA-ADSB-152** With a ground speed on either or both reports, a displacement is plausible up to 1.2 times the
  larger speed plus 40 kt plus 2 kt per second of the gap and not beyond it, whatever the class ceiling; the hard
  cap still applies first, and with no ground speed the class ceiling applies.
- **OA-ADSB-153** `is_jump` is true exactly when `jump_verdict` is not `Plausible`.
- **OA-ADSB-154** Every field of an `ADS2` record (geometric altitude, vertical rate, category, NIC, NACp, NACv,
  SIL, SIL type, version, GVA, SDA and the rest) is encoded at its documented offset and decoded back unchanged.
- **OA-ADSB-155** Every field of an `ADS1` record that the layout has decodes unchanged, the measurements of
  the later layout are absent, and a non-zero reserved byte is counted and ignored.
- **OA-ADSB-156** A payload of exactly the most records of its layout is accepted and one more is refused; the
  length must be exactly header plus count records; `encode_payload` writes `to - from` records and refuses
  an empty, reversed or past-the-end range and one over the most; `frame_ranges` starts a frame at the first
  report that is a full span after the frame's first, or after the most rows, and clamps the span to 1 to 60000 ms.
- **OA-ADSB-157** A record with an unknown source, a bad measurement and a set reserved byte at once is kept,
  and each of the three counts shows it; any two of them show as two.
- **OA-ADSB-158** Unknown values are counted per field and per value, cut at 24 bytes on a character boundary,
  kept for 32 distinct pairs with the rest in the overflow count; adding two `Stats` sums every count and merges
  their value tables (a pair both have is summed, one the table cannot hold goes to the overflow count).
- **OA-ADSB-159** The archive scan counts trace and other entries (a cut entry is not a trace entry), keeps
  at most 16 refusal messages while counting every refusal, refuses an entry over the trace size limit unread,
  and applies the gzip ratio limit only above 1 MiB of output.
- **OA-ADSB-160** `parse_iso` reads `YYYY-MM-DDTHH:MM:SSZ` to the exact Unix second and refuses an out-of-range
  month, day, hour, minute or second (a leap second 60 is the next minute) and any other shape.
- **OA-ADSB-161** The clip command line reads every flag (`--out`, `--box`, `--from`, `--to`, `--frame-ms`,
  `--allow-truncated`, `--help`, `-h`, parts), with times as Unix seconds or ISO, and refuses a missing value, an
  unknown option, a bad box, a frame span outside 1 to 60000 ms, a missing output or no parts.

- **OA-ADSB-100** (proof) The record decoder, for every `ADS2` record (80 octets) and every `ADS1` record
  (64 octets), never panics or overflows; a record it keeps has a position inside +-90 and +-180 degrees,
  a source type that is absent, known or `unknown`, and anomaly bits among the three that exist.
- **OA-ADSB-101** (proof) For every byte string of up to 176 octets the payload header check never panics, and
  a header it accepts has a length of exactly header plus count records; for a payload with a valid header and 4
  records of either layout (the record decoder and the column append taken as their stated contracts),
  `decode_payload_lenient` and `decode_payload` never panic or overflow, kept plus dropped equals the
  records, each anomaly count is at most the kept records, and the strict decoder reports only a record
  below the count.
## Weather aloft (oa-grib2)

GRIB Edition 2 decoder for NOAA RAP and HRRR winds and temperatures aloft, written from the WMO
Manual on Codes (FM 92 GRIB) and NCEP's public GRIB2 documentation. Pure core; not yet wired into
`oa-replay`.

- **OA-GRIB-001** Reads sections 0 to 8 of GRIB Edition 2: several messages back to back in one
  file, several fields in one message (sections 4 to 7 repeated, section 3 optionally omitted so
  the previous grid holds), and the local-use section is skipped.
- **OA-GRIB-002** Every input is bounded and inconsistent input is refused with a typed error:
  message length (`MAX_MESSAGE_BYTES`), grid points (`MAX_GRID_POINTS`, `MAX_MESSAGE_POINTS`),
  fields per message (`MAX_FIELDS_PER_MESSAGE`), section lengths, section order, end marker
  `7777`, truncation, a point count that disagrees with the grid, and group lengths that do not
  add up. Damage to the message framing stops the file walk and is reported without losing the
  messages before it.
- **OA-GRIB-003** Grid template 3.30 (Lambert conformal) is read on a spherical earth. The
  projection maps latitude and longitude to fractional grid indexes and back (round trip under
  1e-6 of a cell, both hemispheres, tangent and secant cones), the first grid point maps to
  index (0, 0), and grid length at LaD is carried to the map plane.
- **OA-GRIB-004** Grid template 3.0 (regular latitude/longitude) is read, with the stated far
  corner checked against Ni, Nj and the increments; longitude wraps on a global grid.
- **OA-GRIB-005** Anything else is refused with its own typed error: scanning modes other than
  0x00 and 0x40, earth shape codes other than the spherical 0, 1, 6 and 8, and grid templates
  other than 3.0 and 3.30.
- **OA-GRIB-006** Product templates 4.0, 4.1, 4.8 and 4.11 give parameter (named for TMP, RH,
  UGRD, VGRD, VVEL, HGT and PRES; the raw discipline, category and number are always kept), fixed
  surface type and scaled value (isobaric in Pa, height above ground in m), reference time and
  forecast lead; other templates are refused.
- **OA-GRIB-007** Data representation template 5.0 (simple packing) decodes exactly
  `(R + X * 2^E) / 10^D` for every bit width from 0 to 32, and a field of zero bits per value is
  constant.
- **OA-GRIB-008** Template 5.2 (complex packing) decodes groups of any width and length from
  the group tables (NG entries in each table, the last length taken from the template), and
  honours missing-value management 1 and 2 for single points and for whole groups.
- **OA-GRIB-009** Template 5.3 (complex packing with spatial differencing) decodes orders 1 and 2
  with descriptors of any width and a negative minimum; with missing-value management the
  differencing runs over the non-missing points only.
- **OA-GRIB-010** Section 6 indicators 0 (bitmap follows), 254 (previous bitmap) and 255 (none)
  are honoured; the number of set bits must equal the number of data values; missing points are
  NaN and counted.
- **OA-GRIB-011** Data representation templates other than 5.0, 5.2, 5.3 and 5.40 (5.41 PNG,
  and the unassigned numbers) are refused with `UnsupportedPacking(template)`; the message is
  recorded as skipped and the next message of the file is still decoded.
- **OA-GRIB-012** Bilinear interpolation at a latitude and longitude reproduces a bilinear
  field exactly, returns nothing outside the grid or when any of the four surrounding points is
  missing, and wraps across the seam of a global grid.
- **OA-GRIB-013** Earth-relative wind at a point: when the resolution and component flags say
  u and v follow the grid axes, they are rotated by the Lambert convergence angle
  `n (lon - LoV)`; the rotation is checked against true east and north found independently by
  projecting small steps, and against the sign convention on both sides of the orientation
  longitude. A grid whose components are already east and north is left alone.
- **OA-GRIB-014** The `.idx` inventory parser reads `msg:offset:d=YYYYMMDDHH:VAR:level:forecast:`
  lines including `N.M` message numbers for several fields in one message, refuses malformed or
  non-increasing lines and over-long input, finds rows by variable and level, and gives the
  inclusive byte range of a message.
- **OA-GRIB-015** No input makes the decoder panic: every truncation of a message and thousands
  of randomly mutated messages produce a value or a typed error.
- **OA-GRIB-016** Every RAP and HRRR file under `OPENATCA_DATA/raw` has its inventory read
  without unpacking (`inspect_stream`): grid first point equals the stated one, parameter and
  level names and order agree with the file's `.idx`; the test reports which packing templates
  the files use and skips when the data is absent.
- **OA-GRIB-017** On every decodable real field: TMP lies in 180 to 330 K, wind speed is below
  120 m/s, there are no missing points, and geopotential height rises as pressure falls at every
  grid point; skips when the data is absent.
- **OA-GRIB-018** RAP and HRRR 250 mb winds of the same analysis time, sampled at 1,000 points
  inside the HRRR domain, agree as earth-relative vectors with median below 4 m/s and 95th
  percentile below 12 m/s, and agree better than the same comparison of unrotated grid-relative
  components; skips when the data is absent.
- **OA-GRIB-019** Public ECMWF ecCodes test files (`OPENATCA_DATA/raw/grib2-testdata`) decode
  to physical fields: 5.0 temperature, 5.3 order 1 with a bitmap, 5.3 order 2 with a bitmap, 5.3
  order 2 with missing-value management, with exact missing counts and smooth neighbours;
  refusals (rotated grid) are typed; skips when the data is absent.
- **OA-GRIB-020** `inspect_stream` reads sections 1 to 6 of every field, including fields
  whose packing is not supported, and names the packing template number.
- **OA-GRIB-021** Template 5.40 (JPEG 2000), decoded by `oa-j2k`, gives exactly the packed
  integers of the independent oracle: every field of every `OPENATCA_DATA/raw/rap` file (2,516
  fields, 382,399,292 points) is compared sample by sample, before the `R`, `E`, `D` scaling, with
  the same field re-encoded as simple packing after decoding with OpenJPEG outside this
  repository (`derived/grib2-repack/rap`, whose scaling parameters must also be identical); no
  mismatch is allowed, and the test reports the decode time per field; skips when the data is
  absent.
- **OA-GRIB-022** The public NCEP GEFS file (`raw/grib2-testdata/gep10.t12z.pgrb2af78.grib2`, 43
  messages, 51 fields, all template 5.40) decodes with no field skipped, and its fields are
  physical: temperature 180 to 330 K, relative humidity 0 to 100, wind below 120 m/s, geopotential
  height rising as pressure falls at every point; skips when the data is absent.
- **OA-GRIB-023** Template 5.40 decodes a hand-built message: the codestream's samples are the
  packed integers and are scaled by `(R + X * 2^E) / 10^D`; a bitmap spreads the values over the
  grid; a field of 0 bits per value is constant and has no codestream.
- **OA-GRIB-024** Template 5.40 refuses what does not hold together: a compression type other than
  lossless, a sample depth or point count that differs from the section, a section that is not 23
  octets, and a damaged codestream (typed `Jpeg2000(error)`); the next message of a file is
  still decoded.
- **OA-GRIB-025** The value count of section 5 is checked against the grid's point count (or the
  bitmap's set bits) before anything is unpacked or allocated, for templates 5.0, 5.2, 5.3 and
  5.40, and the unpackers refuse a count beyond `MAX_GRID_POINTS` when called directly; a message
  claiming 4,294,967,295 values with zero bits each, or one group of that length, is refused with
  a typed error and nothing is sized from the claim.
- **OA-GRIB-050** A decoded value that is not finite after scaling (R, E and D can push a packed
  integer past the f32 range) is refused with a typed error for every packing; only a point the
  field marks missing comes out as NaN. `oa-wind` refuses an infinite cell when building an hour.
- **OA-GRIB-051** A complex-packed field's scratch memory (group tables, packed integers and the
  decoded values) is checked against a budget derived from the point count and the data
  section's size before anything is allocated: one length-one group per point with zero-bit
  tables and no data is refused, the limits are small enough to test with modelled values, and the
  real RAP, HRRR and GEFS fields pass.

## JPEG 2000 decoder (oa-j2k)

JPEG 2000 Part 1 codestream decoder for exactly the profile of NOAA's GRIB2 template 5.40, written
from ITU-T T.800 (08/2002) (`raw/standards/itu-t800`) with no outside code: a raw codestream with
one tile, one component, LRCP, the reversible 5-3 wavelet, no quantization and code-block style 0.
Everything else is refused with a typed error. Pure core. The real-data check against another
decoder is OA-GRIB-021.

- **OA-J2K-001** The MQ arithmetic decoder (T.800 Annex C) reproduces the 256-bit test sequence of
  the identical coder in JBIG2 (T.88 H.2) from its published encoding, and decodes what an
  independently written MQ encoder produces over all 19 contexts, with the initial context states
  of Table D.7. Past the end of a segment it is fed 1-bits and never fails.
- **OA-J2K-002** Tag trees (B.10.2) decode a hand-written bit string, and round-trip random trees
  from 1 x 1 to 9 x 9 leaves queried in random order at rising thresholds. Decoding is iterative
  with a bounded depth.
- **OA-J2K-003** Packet headers are read with the bit stuffing of B.10.1: after 0xFF the next byte
  carries seven bits and its top bit must be 0, and a header ending on 0xFF is followed by one
  stuffed byte.
- **OA-J2K-004** The inverse reversible 5-3 transform (Annex F) is integer exact: known vectors
  (including the floor of a negative sum), a single sample of either parity, every length 1 to 40
  at both parities, and 2-D reconstruction with odd sizes and odd image origins over up to 4
  levels, horizontal then vertical as F.3.2 requires.
- **OA-J2K-005** The EBCOT block decoder (Annex D) reproduces what an independently written block
  encoder codes, for all four sub-band orientations (zero-coding contexts of Table D.1), blocks of
  1 x 1 to 64 x 64 and sparse, dense and signed data; more coding passes than bit-planes allow are
  refused.
- **OA-J2K-006** Whole codestreams round-trip exactly: 1 x 1 to 451 x 33 images of 1 to 16 bits
  with 0 to 6 levels, code blocks of 4 x 4 to 1024 x 4, signed samples, odd image offsets,
  one to eleven quality layers, constant and extreme images.
- **OA-J2K-007** Two frozen byte-for-byte codestreams decode to their known samples, and the test
  encoder still produces the first of them.
- **OA-J2K-008** Everything outside the profile is refused with its own typed error: a missing SOC,
  the markers POC, PPM, PPT and RGN (informative TLM, PLM, PLT, CRG and COM are skipped), other
  progression orders, a multiple component transform, code-block styles other than 0, the 9-7
  wavelet, precincts, SOP and EPH markers, quantization other than none, subsampling, several
  components, tiles or tile-parts, bit depth above 16, a missing EOC or bytes after it, and a
  Psot beyond the stream.
- **OA-J2K-009** Packet damage is refused: a packet running past the data, bytes after the last
  packet, and exponents that leave a band no magnitude bit-planes.
- **OA-J2K-010** No input makes the decoder panic: every truncation of a codestream, four thousand
  randomly mutated ones and garbage after valid headers produce an image or a typed error (release
  builds check overflow as well, RULESET E3).
- **OA-J2K-011** Bounds are enforced before allocation: image sides (`MAX_DIM`), samples
  (`MAX_SAMPLES`), code blocks (`MAX_CODE_BLOCKS`), levels, layers and coding passes.
- **OA-J2K-012** COC and QCC override COD and QCD for component 0 in either order; a COC, QCC or
  second COD that names another component or repeats is refused.
- **OA-J2K-013** Tier-1 work (samples visited over all coding passes of all included blocks) is
  counted from the packet headers and refused with `WorkBudget` before any block is decoded when
  it exceeds `work_budget`: at most `3 * (bits + 8)` visits per sample and at most `WORK_FLOOR`
  plus `WORK_PER_BYTE` per byte of tile body, which every real RAP, HRRR and GEFS field passes with
  margin.
- **OA-J2K-050** Magnitude bit-planes are bounded by the SIZ precision at header parse (each
  quantization exponent is at most the sample bits plus the sub-band gain: 0 for LL, 1 for HL and
  LH, 2 for HH), and the inverse lifting uses checked arithmetic, so no coefficient can overflow
  32 bits: an inconsistent stream or an overflowing line is refused with a typed error.

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
- **OA-MVA-021** A message named `<FAC>_<SUB>_MVA[_<year>][_v<n>]` (`SAV_LHW_MVA_2022`) loads as
  a sub-area chart of facility FAC with set kind `Sub` and the sub-area kept per sector and per
  chart; the same facility and sub-area twice is a duplicate, another sub-area is not.
- **OA-MVA-022** A message name that is none of the accepted forms (`HLN_MVA_MULTI`,
  `ZLC_GTF_MVA_FUS5`, a malformed edition label, ...) is refused with an error naming it, and
  the model is left unchanged.
- **OA-MVA-023** Sub-area charts are not part of a FUS3, FUS5 or TAV query; `min_altitude_with_sub`
  returns the higher of the set's answer and the facility's own sub-area charts at the point.
- **OA-MVA-024** On the real `raw/mva-zjx` files (skipped when absent) all 20 messages load,
  including `SAV_LHW_MVA` as a sub-area chart with 17 sectors.
- **OA-MVA-074** The meridian at 180 degrees east and west is one place: containment, sector
  distance and radius queries give the same answer for a point at +180 and at -180, and a sector
  on either side of the seam is found from either alias (a sector whose own extent crosses the
  seam is refused at load by the extent limit).

## M2 — Weather inputs

### Aviation Weather Center data (oa-awc)

Built on the captured files in `raw/awc` (18 hourly METAR and PIREP files, snapshots of TAF, SIGMET,
US SIGMET and G-AIRMET); fields and null patterns are recorded in that directory's `SOURCE.md`.

- **OA-AWC-001** Timestamps are strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`, optional fraction of one
  to three digits, years 2000 to 2099); other forms, impossible dates and `:60` are refused.
  Station and aircraft identifiers are 3 to 4 (aircraft type 2 to 4) upper-case letters or digits.
- **OA-AWC-002** A METAR row carries station, observation time, position, altimeter (tenths of hPa),
  temperature and dew point (tenths of a degree), wind (`VRB` coded), gust, visibility in hundredths
  of a mile with the "or more" flag for `"10+"` (strings and numbers both read), flight category and
  the raw text; absent and null fields give the `NONE` sentinels.
- **OA-AWC-003** The altimeter group is read from `rawOb` (`A3014` as inHg times 100, `Q1021` as hPa,
  nothing after `RMK`) and compared with `altim`; compared, disagreeing (over 0.4 hPa), missing and
  no-group records are counted, and a disagreeing row keeps its fields but has its altimeter withheld.
- **OA-AWC-004** Duplicate METARs (same station, observation time and type, as in overlapping hourly
  files) are removed by `finalize_metar`, keeping the greatest receipt time then text; duplicates and
  keys whose copies differ are counted, and the table is identical whatever the order of the files.
- **OA-AWC-005** `altimeter_at(table, station, t, max_age)` returns the latest trusted altimeter at or
  before `t` with its age, never a later one, skipping withheld altimeters, and refuses with a typed
  error an unknown station, no observation yet, no trusted altimeter, or an age over `max_age`.
- **OA-AWC-006** `indicated_altitude_ft` converts pressure altitude and a setting in hPa to indicated
  altitude with the ISA relation (identity at 1013.25 hPa; about 270 ft at 1023.1 hPa), and returns
  nothing for a setting or altitude outside its bounds.
- **OA-AWC-007** A METAR record that is not an object, lacks a required field, has a bad identifier,
  time, type or code, or a value out of its bound is refused alone, appends nothing and is counted by
  reason; the other records of the file are kept.
- **OA-AWC-008** A file that is not valid JSON, not an array, or holds more records than the limit is
  refused whole with a typed error and leaves the table and counts unchanged; a file that would
  exceed the table row limit is refused before anything is appended.
- **OA-AWC-009** A PIREP row carries time, position, flight level (hundreds of feet; none for during
  descent or climb), aircraft type, temperature, wind, and two icing and two turbulence layers with
  coded intensity, type and frequency; a reversed base and top, an unknown code or a flight level over
  600 is refused and counted; duplicates are removed and counted.
- **OA-AWC-010** International SIGMET records give a polygon (one ring, or several for `AREAS`) in
  degrees, base and top in feet with absent ends open, and validity times; a ring of under three
  points (other than the one-coordinate `AREA`, OA-AWC-018), an unknown geometry or a base above
  the top is refused and counted.
- **OA-AWC-011** US SIGMET/AIRMET records take the base as the smallest `altitudeLow*` and the top as
  the largest `altitudeHi*` given, and the creation time as the issue time.
- **OA-AWC-012** G-AIRMET records read text coordinates and hundreds-of-feet altitude text into feet,
  `SFC` as 0, a base of `FZL` as a flag, the freezing-level band and level, validity from `validTime`
  to `expireTime`, and refuse a bad altitude text or a base above the top.
- **OA-AWC-013** Duplicate hazard areas across snapshots are removed and counted independently of file
  order; `active_hazards_at` selects the rows valid at a time; `hazard_covers` tests a point and an
  optional altitude against the rings (open bounds, antimeridian longitudes, and a line covers nothing).
- **OA-AWC-014** TAF records give a header (station, issue, validity, position) and periods (change
  group, probability, wind, gust, visibility, lowest broken or overcast ceiling); a period with an
  end before its start refuses the whole forecast; duplicate issuances are removed and counted.
- **OA-AWC-015** No input panics: every prefix of a sample for each product and 400 seeded random byte
  mutations of each give tables or typed errors.
- **OA-AWC-016** On the real captured files (`OPENATCA_DATA/raw/awc`; skipped with a note when absent)
  every file parses, every METAR, PIREP and TAF record is accepted, every hazard record is accepted
  or refused for `reversed-band` only (any other refusal fails the test, and each refused record is
  printed), a point never disagrees with its `rawSigmet` position, `altim` and `rawOb` never disagree, de-duplication
  counts balance, and at least 90 % of the stations in the ZAU box have a trusted altimeter in at
  least 95 % of the hours; the test prints the counts.
- **OA-AWC-017** `oa-awc-summary [dir]` scans a directory tree, parses each recognised file by its
  name, reports records, refusals by reason, duplicates, the altimeter cross-check and station
  availability, lists files refused whole and skipped names, and exits 1 if any file was refused whole.
- **OA-AWC-018** An international SIGMET `AREA` with exactly one coordinate is accepted as a point
  hazard (`GEOM_POINT`, one vertex) that covers nothing; an `AREA` of two coordinates, and a
  one-coordinate area from any other product, is refused as `bad-geometry` (the AWC API schema
  documents no other meaning); `describe_hazard_refusals` lists each refused record of a file.
- **OA-AWC-019** A point's coordinate is compared with the position `rawSigmet` states after
  `OBS AT` or `FCST AT` (minutes or whole-degree form, optional time); checked and disagreeing
  points are counted, and a disagreement is never a refusal.
- **OA-AWC-020** `point_hazards_near` returns the point hazards within a caller-chosen great-circle
  distance of a position (and, optionally, whose altitude band contains an altitude), across the
  antimeridian; it returns nothing for a negative or non-finite distance and never returns areas.

## M2 — Benchmark truth (oa-truth)

### ADS-B reference trajectories (oa-truth)

- **OA-TRUTH-001** Reports are selected on NACp >= 8 (and at most 11), NIC >= 7, SIL 3 and, where
  the version is present, ADS-B version >= 2; a row failing one of these, carrying readsb's stale
  flag, or with a source other than ADS-B (MLAT, TIS-B, ADS-R, ...) is removed and counted under
  one named reason, and rows in always equal rows kept plus rows removed.
- **OA-TRUTH-002** Quality fields are carried forward within an aircraft for at most 60 s; a row
  with no NACp, NIC or SIL value that young is not truth and is counted as such.
- **OA-TRUTH-003** A report that `oa_adsb::jump_verdict` calls impossible from the last kept one
  (hard cap of the emitter category, reported ground speed, time gap, plus a 200 m allowance) is
  removed and counted as a jump and does not affect the track; five jumps in a row start a new leg
  instead of rejecting the rest of the flight.
- **OA-TRUTH-004** An aircraft splits into legs at readsb's new-leg flag (also when the flagged
  row is itself removed) and at a gap of more than 120 s between kept reports, not at exactly
  120 s; leg start, end, report count and address are readable.
- **OA-TRUTH-005** On a straight track with known noise (5 m per axis, 10 minutes) the
  smoothed position is under 4 m RMS from the true path and the velocity under 1.5 m/s RMS, the
  velocity points the right way, and the reported 95 % radius is above the real error and under 50 m.
- **OA-TRUTH-006** On a constant-rate turn (8 km radius, 200 m/s, 5 m noise) the smoothed
  position is under 6 m RMS and 20 m worst, and the velocity within 5 m/s.
- **OA-TRUTH-007** A leg 400 km long stays within 4 m RMS of the true path: the local frame adds
  no position error, and velocity is true east and north.
- **OA-TRUTH-008** Barometric altitude is not smoothed: exact at a report time, the linear
  interpolation between the bracketing reports otherwise, absent if either lacks it; vertical
  rate is the reported rate interpolated, else the altitude difference over a bracket of 2 s or more.
- **OA-TRUTH-009** There is no extrapolation: no truth before a leg's first or after its last
  report or between legs (no bracket), none inside a leg unless a real report lies within 10 s on
  both sides (gap; exactly 10 s is allowed), none for an unknown address, and an address differing
  only in the non-ICAO bit is a different aircraft.
- **OA-TRUTH-010** Truth whose 95 % horizontal radius exceeds the gate (50 m) is refused as too
  uncertain; the same track with precise reports, or a looser gate, is answered.
- **OA-TRUTH-011** Leave-one-out hides every fifth report of each leg, predicts it from the rest
  under the bracket rule and publishes N, mean, RMS, median, p95, p99 and max of the horizontal,
  cross-track, along-track and altitude residuals, the share of predictions under the gate and the
  share of hidden reports inside their predicted 95 % radius; on known noise the residuals have
  the expected size and the radius is honest; the measurement can be switched off.
- **OA-TRUTH-012** The same input gives the same truth and statistics bit for bit, in any input
  order, and one changed report changes the SHA-256 digest of the model.
- **OA-TRUTH-013** Aircraft are keyed by the full address word, so non-ICAO addresses stay
  separate from ICAO ones with the same 24 bits.
- **OA-TRUTH-014** Columns of different lengths and out-of-range configuration values are refused
  with a typed error, out-of-range coordinates are removed and counted, empty input gives an empty
  truth, and the gates are the design's values (NACp 8, NIC 7, SIL 3, version 2, 60 s, 120 s,
  10 s, 50 m).
- **OA-TRUTH-015** On the real ZAU ADS-B clip (skipped when `OPENATCA_ADSB_CLIP` is unset) the
  accounting balances, the model's 95 % radius covers 85 to 99.5 % of hidden reports, the
  leave-one-out horizontal p95 is under the 50 m gate, nothing is answered before an aircraft's
  first report, and two builds are identical.
- **OA-TRUTH-016** `oa-truth-check <clip.oaf>` reads OAF1 ADS-B frames and prints the legs, rows
  kept and removed by reason, the leave-one-out table, the share of query times under the
  uncertainty gate, the build time and the model digest (equal to the library's); `--accel-q`,
  `--sigma-scale` and `--no-loo` are honoured; usage errors and unreadable input exit 2.
- **OA-TRUTH-017** An aircraft flying 900 kt in a straight line and reporting 900 kt gives one
  continuous leg with no report removed as a jump, at 1 s and at 3 s between reports.
- **OA-TRUTH-018** A position jump of 2,782 kt in a 450 kt track is removed and counted, whether or
  not the reports carry a ground speed.
- **OA-TRUTH-019** Reports of the reserved addresses 0x000000 and 0xFFFFFF are removed as invalid
  and give no aircraft or leg; the same digits with the non-ICAO mark are kept.
- **OA-TRUTH-021** The quality values carried forward within an aircraft come only from rows that
  are in range, not stale and from an ADS-B source: a row from another source does not vouch for the
  ADS-B rows after it.
- **OA-TRUTH-022** Rows of one aircraft at one instant do not make the truth depend on input order: those
  further apart than the 50 m truth radius contradict each other and are all removed (counted as
  duplicate times), closer ones are tried in the order of their content, so the same reports in any
  input order give the same truth and statistics.
- **OA-TRUTH-020** The emitter category that sets the jump cap is taken only from reports that pass
  the quality rules and are kept, and a cap-raising one must persist (`oa_adsb::category_observe`):
  one report printing B7 or a reserved code does not let a 2,782 kt jump into truth, and a category
  printed by every report does raise the cap after three.
## M2 — Benchmark harness (oa-bench)

Built on `docs/M2_DESIGN.md` sections 2 to 6. Truth comes through a plain query and leg list (`Truth`),
so `oa-truth` is wired in later without changing the metric code; until then the binary runs the
synthetic world only. Tests use hand-made fixtures; the real-sample test reads
`OPENATCA_DATA/scds/{sfdps,tbfm,tfms}-20261114T*` and skips when absent.

- **OA-BENCH-001** Every statistic reports N, the signed mean, the RMS, and the median, 95th and 99th
  percentile and maximum of the absolute values, by the nearest-rank method (rank `ceil(p*n/100)`, 1-based,
  no interpolation); the result does not depend on the order of the values, non-finite values are skipped,
  and an empty set gives N = 0.
- **OA-BENCH-002** Every paired-sample metric is split by flight phase (climb, cruise, descent, unknown
  from the truth vertical rate, 500 fpm), altitude band (breaks at 10,000, 18,000, 29,000 and 41,000 ft),
  coast flag and association method; each split partitions the samples.
- **OA-BENCH-003** Truth is consumed as `Truth { query(addr, t_ms) -> Result<TruthSample, NoTruth>, legs }`
  with `TruthSample` (lat, lon, baro altitude, east and north velocity, vertical rate, 95% horizontal
  uncertainty, leg) and `NoTruth` (unknown aircraft, no bracket, too uncertain, gap); a time with no truth
  is counted in the exclusions table by reason and never filled in, and a sample whose uncertainty exceeds
  the gate (50 m) is excluded as too uncertain.
- **OA-BENCH-004** SFDPS extraction reads ERAM track samples (flight key, aircraft identification,
  position time, receive time, position, reported altitude, track velocity, coast flag), the aircraft
  address and the address hint, handoff events with their event kind and both units, and expanded-route
  points (fix, time, coordinates where given) into struct-of-arrays tables, using the `oa-fixm` decoder.
- **OA-BENCH-005** TBFM extraction keeps ETA at the meter fix and runway and STA at the scheduling fix and
  runway with the meter fix, flight plan identifier and issue time; TFMS extraction keeps estimated
  arrival and departure times with the flight plan identifier, airports and issue time.
- **OA-BENCH-006** Every table has a stated capacity; a row past it is dropped and counted by table, and a
  document the parser or decoder refuses is counted by feed and not ingested.
- **OA-BENCH-007** Association takes `aircraftAddress` first, the address hint second and the callsign with
  the gate last; ambiguous flights (two aircraft pass the gate) are not associated and are counted; every
  flight has one outcome, the outcomes are counted, and the samples of unassociated flights are excluded
  by reason.
- **OA-BENCH-008** The callsign check compares the ERAM position with the truth: a flight with at least
  80% of the shared times within 3 NM horizontally and 1,000 ft vertically is associated as method
  callsign; one that fails the check is still associated, under its own method, so it never leaves the
  results; the address needs no such check; the method is stored per pair and every metric can be split
  by it.
- **OA-BENCH-009** Cross-track error (T1, positive to the right of the truth track) recovers an injected
  bias and noise; along-track error (T2) is reported raw and after fitting one constant time offset by
  least squares (`along = tau * speed`), the offset is published, and the corrected error recovers the
  injected noise.
- **OA-BENCH-010** The fitted offset drifts when a time bin (or, after trimming 5% either side, an
  aircraft group) of at least 30 samples differs from the overall offset by more than 1 s; a drifting
  offset withholds the corrected along-track numbers with the reason while the offset and the raw numbers
  stay published; a large steady offset is not drift.
- **OA-BENCH-011** Speed error (T3), heading error (T3, truth speed above 30 kt) and altitude error (T4)
  recover injected biases.
- **OA-BENCH-012** Publication interval (T5, consecutive position times of one flight, repeats and
  intervals over 600 s counted apart) and staleness (T6, receive time minus position time, withheld with
  the reason when the recording has no receive time) are measured from the stream.
- **OA-BENCH-013** Presence (T7: share of truth flight time in the zone with an ERAM position within
  30 s, legs with no associated flight counting as absent) and initiation delay (T8: first ERAM position
  minus zone entry or takeoff, signed) match the construction.
- **OA-BENCH-014** Association errors (T9: the ERAM position closer to another aircraft's truth than to its
  own for 3 or more consecutive tested samples) find the deliberate track swap, list it with both
  aircraft, and report none where there is none.
- **OA-BENCH-015** Meter-fix ETA error (F1) and scheduled-time error (F2) against the truth crossing of
  the fix (closest approach within 2 NM, not at the end of a truth leg) are reported by look-ahead in
  5-minute bins to 60 minutes; a fix never approached and an estimate issued after the crossing are
  excluded and counted.
- **OA-BENCH-016** TFMS arrival-estimate error against the truth landing (last truth point on the ground
  within 1 NM of a destination threshold) and departure-estimate error against the truth takeoff are
  reported by look-ahead.
- **OA-BENCH-017** Route conformance (R1) reports the truth distance from ERAM's expanded route in force at
  each sample, the share of flight time beyond 2 NM, and lists each flight with time beyond it (bounded,
  the rest counted).
- **OA-BENCH-018** Handoff timing (H1) compares the initiation and acceptance event times with the truth
  crossing of the boundary between the transferring and receiving unit, signed, by event.
- **OA-BENCH-019** Samples and estimates that cannot be scored are counted in an exclusions table keyed by
  scope and reason (no truth by kind, not associated by outcome, no meter fix, never within 2 NM, no
  route, and others); nothing is dropped silently.
- **OA-BENCH-020** The report carries the input file hashes, the code version, the replay state hash, the
  extraction counts, the scope statement (published track, not internal), the statistics definitions, every
  metric with N and its withheld reason if any, and the exclusions table, in plain text and CSV.
- **OA-BENCH-021** The same inputs give byte-identical text and CSV reports.
- **OA-BENCH-022** A directory of raw `.msg` files (name order, no receive time, no replay hash) or of
  `.oaf` recordings (receive times, replay pipeline state hash, identical on repeat) loads; the input hash
  of one file is its SHA-256 and of several is the SHA-256 of the `name:sha256` lines.
- **OA-BENCH-023** The `oa-bench` binary refuses `--adsb` without `--out` and a non-positive truth gate
  override (exit 2), runs the synthetic world with `--synthetic --out <dir>` writing `report.txt` and `report.csv`
  with every self-test check passing and identical bytes on a repeat, and prints extraction counts with
  `--extract-only`.
- **OA-BENCH-024** On the real 2026-11-14 sample (skipped when absent) extraction yields the expected
  counts: 13,268 ERAM track samples, 2,807 handoff events, 265 expanded routes with 1,397 points, 165
  flights with `aircraftAddress`, no capacity drops, and no refused TBFM or TFMS document.
- **OA-BENCH-025** The synthetic self-test (closed-form truth; injected bias, noise, time offset,
  decimation, receive delay, coasting, dropout, start delay, estimate errors, handoff times, an off-route
  stretch and a track swap) is recovered by every metric within its stated tolerance (26 checks, the last two on the callsign-only aircraft with a 20 km error that stays in the headline).
- **OA-BENCH-026** A truth built from a plain lookup table and the same legs gives the same paired samples
  and metrics as the closed-form truth, so the interface needs nothing but the query and the legs.
- **OA-BENCH-027** The address hint associates a flight that has no aircraftAddress, is stored as its own
  method, and a hint with no truth aircraft is not retried by callsign.
- **OA-BENCH-028** `oa-truth` query answers map onto the benchmark's truth interface reason for reason:
  unknown aircraft, outside legs, no nearby report and too uncertain each map to the same-named
  benchmark reason with the same meaning; a sample keeps its position, velocity,
  uncertainty and leg, with altitude and vertical rate rounded to whole units.
- **OA-BENCH-029** Truth legs are `oa-truth`'s legs in index order with their first and last whole
  millisecond; the leg callsign is the most frequent callsign among the decoded reports of the address
  inside the leg's span, and `None` when no report carries one.
- **OA-BENCH-030** The truth gates come from `oa-truth`'s configuration (design: 10 s bracket, 50 m); the
  bracket and uncertainty overrides change it, and the report's truth section carries the gates, the
  statistics, the removals by reason, the leave-one-out table and the truth digest.
- **OA-BENCH-031** An OAF1 ADS-B clip is read through `oa-feed` and `oa-adsb` into reports, counting
  frames that did not decode, and its SHA-256 is the input hash.
- **OA-BENCH-032** The airspace lookups come from the M0 model: fixes and navaids by identifier (not
  airports), airports by ICAO and FAA identifier with elevation and thresholds, the model's ARTCC as the
  zone, and the containing ARTCC as the unit.
- **OA-BENCH-033** `oa-bench --adsb <clip.oaf>` runs end to end: on a hand-made clip and SFDPS fixture
  whose track is 500 m left of the truth, T1 recovers -500 m (cross-track, positive right) within 2 m with
  every sample paired; the report holds the truth section, the clip hash and the gate values, a repeat gives
  identical bytes, and an override of the gates is recorded in the header with its reason.
- **OA-BENCH-034** On the real ADS-B clip and the SCDS sample (skipped when `OPENATCA_DATA` or
  `OPENATCA_ADSB_CLIP` is unset), whose times do not overlap, the run completes with no paired samples and
  an exclusions table.
- **OA-BENCH-035** Association never depends on the error being measured. A callsign flight is kept when
  truth overlaps it in time (at least 5 shared times) and lies within 50 NM of its positions for 80% of
  them; a flight that fails the 3 NM / 1,000 ft check is associated all the same under its own method
  (callsign, failed the check), counted in the headline rows, split out by the method split, and listed
  as a case. Two or more callsign candidates are ambiguous.
- **OA-BENCH-036** An address or address-hint association is checked without using ERAM's accuracy:
  callsigns that contradict (both known, none of the truth legs agrees) or ERAM positions beyond the 50 NM
  region for more than 20% of the shared times make it implausible; with too few shared times it is
  trusted and counted as unchecked. An implausible flight is not in the primary rows and its cross-track,
  raw along-track and altitude errors are published in separate flagged tables, each flight listed with
  its reason; an address with a believable error is trusted as before.
- **OA-BENCH-037** The statistics of an empty set have n = 0 and NaN in every other field, never 0.0; the
  text report prints such a row as "no data" and the CSV as `NA`; scalars of an empty set (presence, share
  of time beyond the route limit, largest per-flight share, offset deviations with nothing tested) are NaN
  and print the same way; no report contains the text `NaN`.
- **OA-BENCH-038** Raw along-track error is the primary along-track result and the offset-corrected one
  is secondary, and says that the fitted offset cannot be told apart from real ERAM lag. The offset is
  published with its sample standard error, its between-aircraft standard error and a 95% half-width
  (1.96 times the larger); the drift test keeps its 5% trim and also publishes the untrimmed largest
  group deviation and the five worst aircraft groups.
- **OA-BENCH-039** A tracker's output columns are scored by the same code path as ERAM's samples: the
  tracker entry point is the legacy-table adapter followed by the one benchmark function, gives a
  report identical to calling the two directly, and yields the same metrics, in the same order, with the
  same offset scalars and notes as an ERAM run; a tracker that lags the truth shows the lag in the raw
  along-track error.
- **OA-BENCH-040** A tracker's output scored in address ranges gives the report of the same rows scored
  in one piece, byte for byte in the text and the CSV, whatever order the flights are in the table and
  whatever order the ranges are added in: the case lists (association, T9) are sorted by severity, truth
  address and time before they are cut to 200, and every sum behind a statistic (means, RMS, the along-track
  offset fit) is exactly rounded, so the order of the values cannot change a bit. T5 is summed from whole
  milliseconds in both paths.
- **OA-BENCH-041** Scoring in address ranges refuses ranges that overlap, ranges that together leave any
  key from 0 to 0xFFFFFFFF uncovered (the radar-only tracks keyed from 0x40000000 included), and a chunk with
  a flight keyed or associated outside its range; no flight or truth leg is counted twice or dropped.
- **OA-BENCH-042** The headline tables T1, T2-raw and T4 carry the number of flagged flights and of their paired
  samples that they leave out (`flagged_flights_excluded`, `flagged_points_excluded`) and a last row, "all
  associated including flagged", over the headline and the flagged samples together; the other rows are
  unchanged, and the run in ranges gives the same.
- **OA-BENCH-043** A chunk scored in address ranges is accepted only when its plan-data results (F1 to F3, R1,
  H1) equal exactly what a table with no data gives, NaN included; a result that differs only in a NaN is
  refused, not read as empty.
- **OA-BENCH-044** The association of a run in address ranges is the real one, one entry per flight with the
  chunks' associations in range order, and its counts equal those of the run in one piece; it is never an
  empty stand-in.
- **OA-BENCH-045** Reading an ADS-B clip drops only a record that cannot be read (position or flags out of
  range), counts it apart from refused frames, and keeps the other records of its frame.

## Winds aloft (oa-wind)

Wind and temperature at any (latitude, longitude, pressure altitude, time) from NOAA RAP analyses decoded
by `oa-grib2`: the trajectory predictor's weather input. Pure core, file shell in `archive`, and the
`oa-wind-check` validation tool. Nothing is extrapolated.

- **OA-WIND-001** Pressure altitude becomes pressure by the ICAO standard atmosphere (exact, since
  pressure altitude is defined on the ISA): 1013.25 hPa at 0 ft, 506.0 at FL180, 300.9 at FL300, 226.32 at
  the tropopause, continuous across it, and the inverse round-trips. Non-finite input and altitudes outside
  -5000 m to 20 000 m are refused.
- **OA-WIND-002** Horizontal interpolation is bilinear in the grid's own coordinates; a field linear in
  the grid indexes is reproduced exactly at any position inside the grid.
- **OA-WIND-003** Vertical interpolation is linear in log-pressure between the bracketing isobaric levels;
  a field linear in ln p is reproduced exactly between levels, and the aircraft's pressure altitude is
  converted through the ISA pressure.
- **OA-WIND-004** U and V stored grid-relative (the grid's component flag) are rotated to east and north
  by the convergence angle at the query longitude; a uniform earth-relative wind stored grid-relative is
  recovered, is wrong without the rotation, and a grid that is already earth-relative is not rotated.
- **OA-WIND-005** Time is linear between the two bracketing analysis hours; the answer reports the hours
  used and the weight; on an hour the weight is 0 (or 1 against the previous hour for the last).
- **OA-WIND-006** A query is refused with a typed `Refusal` outside the grid, above or below the level
  range (the limits themselves are answered), outside the time span by even one second, outside the ISA
  range, for non-finite input, and with no hours; never extrapolated.
- **OA-WIND-007** A missing value at a grid corner or level that the query uses is refused as
  `MissingValue`, never filled; a query that does not touch it is answered.
- **OA-WIND-008** An hour is built only from complete, consistent fields: a level lacking U, V, T or
  height, a repeated field, differing grids, point counts or valid times, a fractional level, an unusable
  time unit, fewer than two levels and more than 64 levels are refused with typed errors; other
  parameters and non-isobaric levels are ignored; the arrays are level-major in ascending pressure.
- **OA-WIND-009** Bytes that are not a GRIB2 file with the needed fields are refused, and an hour is
  loaded by decoding only the isobaric UGRD, VGRD, TMP and HGT messages (the rest are only inspected).
- **OA-WIND-010** The `.idx` byte ranges of exactly the isobaric UGRD, VGRD, TMP and HGT messages are
  selected (layer labels such as `500-1000 mb` and other variables excluded) and adjacent ranges joined.
- **OA-WIND-011** The archive refuses two files with one valid time and too many files, sorts by time,
  refuses queries before reading any file where it can, reports an unreadable file as a typed error, and
  holds at most three hours.
- **OA-WIND-012** The validation statistics (median, 95th percentile, mean, maximum), the conversion of a
  reported "from" direction and speed in knots to east and north, the vector comparison with a reference
  field pair (a rotation-correct model agrees, an unrotated one does not) and the observation comparison
  (refused queries counted by reason, not compared) behave as stated.
- **OA-WIND-013** On the real RAP t12z hour (skipped when absent) the hour has 37 levels on the 451 x 337
  grid, 90.0 MB of arrays, equals the hour built by `load_hour` from the whole file, its stored values
  equal `oa-grib2`'s decoded values exactly, and the query at grid points agrees with `oa-grib2`'s
  `wind_at` and bilinear sampling to rounding.
- **OA-WIND-014** On the real data (skipped when absent) RAP agrees with the HRRR 250 hPa wind at t12z with
  a median vector difference under 2 m/s (and is worse without the rotation), and a half-hourly sweep over
  all hours never holds more than three and refuses a time after the last hour.
- **OA-WIND-015** ADS-B observations for the RAP comparison are the reports that have a barometric
  altitude (not on the ground) and a readsb wind (speed and direction) or outer air temperature, at most
  one per aircraft per gap, with position, time, roll and track rate carried over in SI-ready units.
- **OA-WIND-016** The ADS-B comparison gives, for RAP minus readsb, the count, median and 95th percentile
  vector difference, the speed bias and the u and v bias, and the temperature difference against `oat`,
  for all observations, by altitude band (4) and by turning state (straight, turning, unknown); a refused
  query is counted by reason and left out, and a failed file load stops it.
- **OA-WIND-017** `oa-wind-check adsb` refuses a bad `--gap-s` (exit 2), skips with a note and exit 0
  when there are no RAP files, does not scan an archive whose day does not overlap the RAP hours, and on
  the real data (skipped when absent) exits 0 with either the comparison or the no-overlap statement.

- **OA-WIND-050** No answer carries a non-finite number: `build_hour` refuses an infinite cell (a
  NaN is a missing value and stays), and the sampler refuses an infinite corner value, and any
  point or sample that would not be finite, instead of returning it.
- **OA-WIND-051** `bracket` refuses valid times that are not strictly increasing (a duplicate would
  give a NaN weight), as well as an empty list and a time outside the span.
## M3 — ASTERIX codecs (oa-asterix)

Encoder and decoder for EUROCONTROL ASTERIX category 021 (ADS-B target reports, Ed 2.7), 048 (monoradar
target reports, Ed 1.32) and 062 (system track data, Ed 1.21), written from the specifications (Part 1
Ed 3.1 for the general structure). A generic core (data block, FSPEC, item kinds, driven by one constant
UAP table per category) and a typed layer for the items the tracker needs; every other item round-trips as
raw bytes. The decisions where the specification leaves room are listed in the `wire` module docs.

- **OA-AST-001** A data block is CAT, a two-octet LEN that counts the header and every record (at most
  65 535), and records that tile the block exactly; several blocks in one buffer decode one after another,
  and a stream whose last block is cut short is refused.
- **OA-AST-002** The FSPEC is read and written bit 8 first with the LSB as FX: FRN 1 to 7 in the first
  octet, 8 to 14 in the second, and so on; the encoder writes the shortest FSPEC and sets FX only between
  octets.
- **OA-AST-003** Every item kind (fixed, extended, repetitive, explicit-length SP and RE, compound with
  extended and repetitive subitems) is walked from its constant table entry, and the untyped items of a
  record come back as exactly the bytes that were put in.
- **OA-AST-004** A block that is shorter than its header, has a LEN below three or past the input, holds no
  record, or is of a category other than 021, 048 and 062 is refused with a typed error naming the
  category and byte offset; an error in the second record names record index 1 and a stream-absolute
  offset.
- **OA-AST-005** An FSPEC that is empty, ends with an all-zero octet that is not the only one, runs past
  the block or past the longest FSPEC the UAP can address, sets the bit of a slot the UAP leaves unused
  (CAT062 FRN 2 and 29 to 33, CAT021 FRN 43 to 47), or omits the data source identifier is refused.
- **OA-AST-006** An item whose FX chain does not end inside the record or within 32 octets, a repetitive
  item with a factor of zero or that runs past the block, an explicit-length field shorter than two octets
  or past the block, a compound item with no subitem, with a subitem the category does not define (naming
  the subitem), with FX on its last defined primary octet or (in an item decoded into fields) with a trailing all-zero
  primary octet, and a fixed item or subitem cut short are refused with the item number and byte offset.
- **OA-AST-007** A bit that the specification fixes to zero (a spare bit) is refused in every fixed and
  extended item and compound subitem whose table entry lists it, at the extension the specification puts it
  in (bits that are assigned in another extension of the same item are accepted); reserved enumeration
  codes are accepted.
- **OA-AST-008** A CAT021 record built by hand from the specification (data source, target report
  descriptor, high-resolution position, target address, quality indicators with an extension, Mode 3/A,
  flight level) decodes to the expected values and encodes to the expected bytes, the bytes derived from
  the LSBs in the test.
- **OA-AST-009** The same for a CAT048 record (time of day, descriptor, polar position, Mode 3/A, flight
  level, plot characteristics, aircraft address and identification, track number, Cartesian position, track
  status).
- **OA-AST-010** The same for a CAT062 record (time, WGS-84 and Cartesian position, velocity, Mode 3/A,
  aircraft derived data with address, identification and selected altitude, track number, track status,
  system track update ages, measured flight level, geometric altitude, rate of climb).
- **OA-AST-011** `encode(decode(bytes))` equals the bytes for every block the decoder accepts, and
  `decode(encode(record))` equals the record, for blocks with every typed item and every untyped item.
- **OA-AST-012** Untyped items, including SP and RE and compound, repetitive and extended items, are
  written back in FRN order to the same bytes whatever order they are given in.
- **OA-AST-013** The unit LSBs are named constants that cite the specification section; positions convert
  between raw counts and 1e-7 degree with at most half an LSB of error and the count survives the round trip
  through e-7 degree exactly; times convert between 1/128 s counts and milliseconds.
- **OA-AST-014** The encoder refuses a value that does not fit its element, a character outside the
  six-bit set, a latitude beyond 90 degrees, an item given twice, a raw item that is not exactly one
  well-formed item or whose slot does not exist, an empty block, too many records and a block over 65 535
  octets, with the record, item and kind.
- **OA-AST-015** Truncating every block of the corpus at every byte never panics: with LEN left alone every
  cut is refused, and with LEN made to match every cut is refused except those on a record boundary, which
  are valid blocks.
- **OA-AST-016** 4000 random mutations of the corpus (and 4000 arbitrary byte strings) never panic, and
  every mutation that is accepted re-encodes to exactly the bytes it was decoded from.
- **OA-AST-017** The bounds are named constants (block length 65 535, records per block, extension chain 32,
  repetitions 255, primary subfield 8) and hold at the edge: a block of exactly 65 535 octets and 255
  repetitions are accepted, one more is not.
- **OA-AST-018** Six-bit characters code A to Z, space and 0 to 9 and nothing else (six octets of zero in an
  identification are the one exception, see OA-AST-026); a latitude of exactly
  +-90 degrees is accepted and one count more is refused.
- **OA-AST-019** A CAT021 record built from an `oa-adsb` row (position, address, NACp, NIC, SIL, MOPS version,
  Mode 3/A, callsign, altitudes, ground vector, vertical rate) round-trips, with every quantised field within
  half its LSB: position 9 mm (1e-7 degree after conversion), flight level 12.5 ft, geometric height
  3.125 ft, speed 0.11 kt, track 0.0028 degree, vertical rate 3.1 ft/min.
- **OA-AST-020** On real adsb.lol rows (skipped when `OPENATCA_DATA` is absent) the same round trip holds
  for tens of thousands of rows within the same bounds.
- **OA-AST-021** A refusal renders as text naming the category, the record, the data item (and subitem) and
  the byte, and leaves out the parts that do not apply.

## M3 — Sensor simulation (oa-sensorsim)

Simulated secondary radar plots and injected ADS-B spoofs, generated from `oa-truth` (docs/M3_DESIGN.md
sections 1, 2 and 4). Error values are only those of docs/BENCHMARKS.md 3.1 "Simulated sensor inputs"; the
radar biases, split geometry and earth-radius factor have no sourced value and are parameters with no default.

- **OA-SIM-001** The generator is xoshiro256** seeded by SplitMix64, written from the published algorithms;
  both reproduce their published output sequences, streams for different (seed, aircraft, scan) words differ
  and repeat exactly, and uniform and normal draws have the right moments.
- **OA-SIM-002** Each named profile (`MSSR_EUROCONTROL_1997`, `MODE_S_ATC302`, `SSR_CD2_ATC302`) carries the
  tabulated values (range and azimuth sigma, quantum, altitude resolution, scan periods 4.8 s and 12 s,
  detection, Mode A, Mode C, false and split rates) and the simulated range and azimuth errors have the
  profile's sigma within 4% over more than 10 000 plots, with a mean consistent with zero.
- **OA-SIM-003** Range and azimuth bias are added exactly (with the random error set to zero, measured minus
  true equals the bias to 1e-9).
- **OA-SIM-004** A plot is made at the instant the beam azimuth equals the target azimuth: exactly the scan
  start plus the azimuth fraction of the period for a stationary target (to the nanosecond), and, for a moving
  target, at a time where the beam azimuth equals the azimuth of the truth position at that time (checked
  through the public truth query), inside the revolution, and not where a frozen-target solve puts it.
- **OA-SIM-005** Detection, false-plot and split-plot rates match the profile within 4.5 binomial standard
  deviations; false plots are uniform in area over the coverage disc and in azimuth, timed by the beam; split
  plots repeat the plot time and differ by the stated range and azimuth offsets.
- **OA-SIM-006** A target beyond the coverage range or below the 4/3-earth line of sight (effective-radius
  factor a site parameter) gets no plot and is counted by cause; the line-of-sight test matches the radio
  horizon formula 4.12 (sqrt h1 + sqrt h2) km.
- **OA-SIM-007** Altitude is quantised to 100 ft (Mode C) or 25 ft (Mode S) by the profile, and range to the
  1/8 NM grid for SSR CD-2.
- **OA-SIM-008** CAT048 output decodes with `oa-asterix` to the same plots within the element quantisation
  (range 1/512 NM, azimuth half of 360/2^16 degree, time 1/256 s plus the millisecond floor), with Mode A,
  flight level, address, SIM flag and target type, one block per scan.
- **OA-SIM-009** The same seed, truth and site give identical plots and identical CAT048 bytes; a different
  seed differs; an aircraft's plots do not depend on which other aircraft are in the truth.
- **OA-SIM-010** Each aircraft gets its own Mode A code (never 0000, 7500, 7600 or 7700); Mode S profiles put
  the aircraft address on the plot and non-Mode S profiles do not.
- **OA-SIM-011** An invalid site or profile (latitude, period, azimuth, coverage past the CAT048 range
  element, earth factor, non-finite bias, probability above one) is refused with a typed error, and a plot
  range past the CAT048 element is refused at encoding.
- **OA-SIM-012** The ghost transform adds an aircraft with a new address on a straight track at the stated
  speed and track, labels exactly those rows, and leaves every original row intact and in order.
- **OA-SIM-013** The drift transform moves the victim's reports in the window by the stated rate times the
  elapsed time along the stated bearing, labels exactly those rows, and changes nothing else.
- **OA-SIM-014** The replay transform copies the victim's reports of a window shifted later in time, under
  the victim's or another address, with the original positions, and labels them.
- **OA-SIM-015** The duplicate-address transform re-sends a donor aircraft's reports under the victim's
  address and labels them; the victim's and donor's own rows are unchanged.
- **OA-SIM-016** Every spoof transform refuses an address already present (ghost), an address past its
  width, a non-positive interval, shift or window, a victim or donor without reports and equal victim and
  donor, with a typed error.
- **OA-SIM-017** Transforms compose, and a detector's flags score against the labels as a confusion matrix
  (true and false positives and negatives, and per attack kind) whose cells sum to the row count.
- **OA-SIM-018** On a real ZAU clip (skipped when `OPENATCA_SENSORSIM_CLIP` is absent) a terminal and an
  en-route radar produce plots whose CAT048 output decodes, with a detection rate within one percentage point
  of the profile's.
- **OA-SIM-019** The `oa-sensorsim` binary writes the plots (CSV and CAT048) of an OAF1 clip, gives the same
  bytes for the same arguments, and refuses to run without the bias, coverage and split parameters.
- **OA-SIM-020** Reading a clip drops only a record that cannot be read (position or flags out of range) and
  counts it; the other records of its frame are kept.
- **OA-AST-022** The test-side pcap reader (`tests/common/pcap.rs`) returns the UDP payload of every packet of a
  libpcap file (little or big endian, microsecond or nanosecond magic) over Ethernet II and IPv4 without
  fragmentation, trims Ethernet padding by the IP length, and refuses with a typed error a bad magic or
  version, a link type other than Ethernet, a truncated file or packet, a packet cut by the snaplen, a captured
  length over 262 144, an Ethernet type other than IPv4, a bad IP header or length, a fragment, a protocol
  other than UDP and a bad UDP length.
- **OA-AST-023** On real captures from another organisation's system (skipped when `OPENATCA_DATA` is absent):
  every block of CAT021, CAT048 and CAT062 in the eight sample files decodes (none refused except the
  pre-standard file of OA-AST-028), re-encodes to the same bytes, and the blocks of categories 001, 002, 034
  and 065 are counted by category with their framing checked; the counts per file are pinned.
- **OA-AST-024** On the same files our decoded values agree with the upstream decoder's expected output: block
  category and length, record bytes, every item's bytes and identity, and the CAT048 and CAT062 field values
  (position, time, Mode 3/A, flight level, address, identification, track number, plot characteristics,
  track status, velocity, ages, altitudes), with zero differences; the container times of the GPS file
  equal the upstream time stamps and the CAT001/002 frames match its block list.
- **OA-AST-025** A compound item kept as raw bytes may have a primary subfield whose last octet sets no bit
  (real trackers write one in I062/390) and goes back as it came; a compound item with no subitem is still
  refused; in a compound item decoded into fields the trailing zero octet is refused, naming its byte.
- **OA-AST-026** An identification (I021/170, I048/240, I062/380 subitem 2) of six zero octets, which real
  radars send for "none", is kept raw and round-trips with the typed field empty; any other code outside the
  six-bit set is still refused.
- **OA-AST-027** The Reserved Expansion Field and the Special Purpose Field of CAT021, CAT048 and CAT062 are
  kept as raw bytes and round-trip, and a length past the record is refused.
- **OA-AST-028** A CAT062 record written in a pre-standard layout (the Ed 0.17 UAP, the 100 datagrams of the
  sample `asterix.pcap`) is refused by the Ed 1.21 decoder and not misread; on the real file all 100 are
  refused and all 100 tile their blocks exactly under the Ed 0.17 layout with a constant data source and a
  time of track that never runs backwards.
- **OA-AST-078** Each field of the CAT021 quality indicators (I021/090: NICBARO, SIL, NACp in the first
  extension; SILS, SDA, GVA in the second; PIC in the third) is read from its own bits, so that a value
  in one field never shows in its neighbour, and an extension that is absent gives no value.
- **OA-AST-079** Each flag and field of the CAT021 target report descriptor extension (I021/040: GBS, SIM,
  TST, CL) is read from its own bits, an extension that is absent gives no value, and the primary subfield
  gives ATP, ARC, RC and RAB at their own bits.
- **OA-AST-080** A descriptor and a quality item with every extension present decode from the wire to
  the values worked out by hand from the bit layout and encode back to the same bytes.
- **OA-AST-081** Every unit constant (the LSBs of CAT021, CAT048 and CAT062 and the bit counts of the
  positions) has its exact value.
- **OA-AST-082** A refusal in a block header (short input, LEN below the header, LEN past the input,
  category not carried, no record) names its byte offset, counted from the start of the block plus the
  offset the caller gave for it.
- **OA-AST-083** A refusal in the FSPEC of a record that is not the first (empty, cut short, not the
  shortest, too long, a slot the category leaves unused, no data source identifier) names the record and
  the byte offset of the FSPEC octet or of the record.
- **OA-AST-084** A refusal inside an item of a record that is not the first (fixed, extended, repetitive,
  explicit-length and compound items, spare bits) names the record, the item, the subitem of a compound
  item, and the offset where the item or the offending octet starts.
- **OA-AST-085** A refusal by the typed layer in a record that is not the first (a value out of range, a
  compound primary subfield that is not the shortest, which is named before a failure of the same
  item) names the record, the item and the offset.
- **OA-AST-086** In a stream of blocks, a refusal in the second block reports the offset from the start
  of the whole stream.
- **OA-AST-087** In the aircraft derived data of a CAT062 record (I062/380) an unparsed subitem that
  repeats one the record already gives typed, or that stands for the address or the selected altitude, is
  refused as a duplicate; the same subitem number with no typed counterpart is written; subitem
  number zero is refused, and so is a record with no subitem.
- **OA-AST-088** A position count converts to 1e-7 degree rounded to nearest with ties away from zero,
  for both signs and for each width, and the conversion back rounds the same way.
- **OA-AST-089** A compound primary subfield is refused as not the shortest only when an all-zero octet
  ends it after the first octet, naming that octet; a single zero octet at the start is left to the empty
  compound refusal, and an item that is not compound is never refused.
- **OA-AST-090** A value is packed into an unsigned element of a given width exactly when it fits: the
  largest value of the width is accepted, the next is refused, and a 32-bit element takes every value.
- **OA-AST-100** (proof) `rd` and `take` never panic, overflow or read out of bounds for any start and
  length over inputs of up to 6 octets, and advance the position only on success by exactly the octets read;
  `sext` keeps the low bits and stays in range, for every value and width of 1 to 32 bits.
- **OA-AST-101** (proof) The FSPEC parser returns, for every input, either a non-empty list of FRNs with a
  position inside the input, or an error positioned inside the input, without panic or overflow. CAT048:
  every input (the FSPEC is read or refused within the 4 octets the UAP can address, so 5 octets cover all).
  CAT021 and CAT062: inputs of up to 3 octets.
- **OA-AST-102** (proof) Walking any item of the CAT021, CAT048 and CAT062 UAPs (every slot, SP and RE
  included) over any bytes of up to 24 octets never panics, overflows or reads out of bounds; a walk that
  succeeds ends after its start and inside the input, and an error is positioned inside the input.
- **OA-AST-103** (proof) `parse_block` (the framing stage of `decode_block`) over any byte string of up to 3 octets, which holds the refusals
  of a short header, a LEN below three or past the input, no record and an unsupported category,
  never panics or overflows, and a block it accepts occupies more than zero and at most the input's octets.
  Whole blocks of 4 octets and more are beyond what the solver finishes (see `docs/VERIFICATION.md`).
- **OA-AST-104** (proof) Every six octets that decode as an identification encode back to the same six octets;
  a 24-bit time of day survives 1/128 s to milliseconds and back exactly, and neither conversion overflows for
  any `u32`.
## M3 — Surveillance tracking (oa-track)

### ADS-B-only tracker and its scoring (oa-track, oa-bench adapter)

- **OA-TRK-001** A measurement (time of applicability, sensor id, position with covariance, optional
  velocity with covariance, optional barometric or geometric altitude with quantisation, optional vertical
  rate, address and identity; one 72-byte record) is validated before use: a position out of range, a
  covariance that is not finite and positive definite, a bad velocity, an altitude or rate out of range, and
  a missing address are refused with a typed reason and counted; no track is made from them. Bad
  configurations (zero capacity, M above N, a non-positive time, an unknown filter mode) are refused.
- **OA-TRK-002** On straight flight with noise taken from NACp 9, the tracker's position error is below
  7 m RMS at 1 Hz (raw reports: 17 m) and its speed and heading errors are below 1 m/s and 0.2 degree; at
  17 s input the one-second output stays under 25 m. The horizontal state lives in a local east-north plane
  (`oa_geo::aeqd`, moved from `oa-truth` so both share it) re-centred on every update.
- **OA-TRK-003** Position noise is the NACp 95 % bound over 2.448 per axis; with those values the mean
  normalised innovation squared of a position is 2.0 (within 1.7 to 2.4), and a report that claims three
  times the true accuracy pushes it above 3.
- **OA-TRK-004** Constant-rate turns at 1.5, 3 (standard rate) and 6 degrees per second at 1 s input, and
  3 degrees per second at 5 s input, are followed (position under 15 m RMS, heading under 3 degrees, scored
  on the one-second output); the interacting pair beats a single straight-model filter with the same
  process noise by 34 to 99 % in position and by 4 to 50 % in speed, and beats the best of three single
  filters (process noise 0.05 to 5 m^2/s^3) by 0 to 99 % in position (the test prints the table).
- **OA-TRK-005** On straight flight the pair costs nothing against one straight filter, and the model
  probabilities follow the aircraft: the turn probability is above 0.8 in the middle of a turn and below
  0.1 on a straight leg, and the turn rate estimate is within 0.5 degree per second of a 3 degree per
  second turn.
- **OA-TRK-006** An aircraft accelerating at 1 m/s^2 is tracked to under 10 m RMS position and 2 m/s RMS
  speed.
- **OA-TRK-007** The vertical filter (level and climb/descent models, quantisation as noise of variance
  step squared over 12) tracks a profile of level, 2,000 fpm climb, level and 1,500 fpm descent with 25 ft
  quantised altitudes to under 12 ft RMS and 120 fpm RMS, and its model probabilities exceed 0.9 in the
  middle of each phase.
- **OA-TRK-008** A track 600 km from the origin at 64 degrees north is as accurate as one at the origin
  (position under 8 m RMS, speed under 1 m/s), because the frame is re-centred on every update and the
  velocity rotated by the change of the frame's axes.
- **OA-TRK-009** A measurement that arrives out of order inside the window is applied by re-filtering from
  stored state: with every fifth neighbour pair swapped, each swap is re-filtered and replayed exactly
  once and the output after the last swap is bit-identical to the in-order run.
- **OA-TRK-010** A measurement older than the clock by more than the window, one inside the window but
  older than every stored state of its track, and an exact repeat (same time and sensor) are each counted
  and dropped under their own counter; one inside the stored states is applied.
- **OA-TRK-011** A track is confirmed by 2 reports within 3 expected intervals, one expected interval after
  the first (the initiation delay is logged); a lone report expires, a second report after the window
  restarts the track, and a second report implying more than 700 kt restarts it (each counted).
- **OA-TRK-012** A silent track is flagged coasting from the first tick more than 3 expected intervals
  after its last update (104 s for a last update at 100 s) and dropped, logged and its slot freed at the
  first tick more than 60 s after it (no tick after 160 s).
- **OA-TRK-013** The fixed-rate output is a row per confirmed track at every whole second, one second
  apart; update rows carry the reports' own times.
- **OA-TRK-014** The same measurements give the same output bytes (the output digest and every column are
  equal across runs and across a different table capacity).
- **OA-TRK-015** Capacities are bounded and overflow is counted: reports of aircraft beyond the track
  capacity are refused and counted one by one, output rows past the table capacity are counted, and track
  log entries past the log capacity are counted.
- **OA-TRK-016** Two aircraft crossing 120 m apart keep their own addresses and each stays within 50 m
  of its own truth: identity is the address, never the kinematics.
- **OA-TRK-017** After the tracker is built no buffer it owns changes address or capacity through a run with
  confirmations, late measurements, drops, restarts and overflow of the output, the log and the track
  table (a counting allocator needs `unsafe`, which RULESET A0 forbids here, so the check is on the
  buffers).
- **OA-TRK-018** The ADS-B adapter derives the position covariance from NACp (bound over 2.448, per axis;
  NACp 8 to 11 checked against the DO-260B bounds) and the velocity covariance from NACv, with velocity
  from ground speed and track and 25 ft altitude quantisation.
- **OA-TRK-019** NACp, NIC, SIL, version and NACv are carried forward at most 60 s; a row with none in
  that age is removed like truth's `no_quality_in_age`; a velocity without NACv is used as NACv 1 and
  counted; a row with only a geometric altitude passes it flagged geometric.
- **OA-TRK-020** The adapter's gates are `oa_truth::quality_check`, the code truth selection uses: on a mixed
  batch the removals by reason (stale flag, source, no quality, NACp, NIC, SIL, version) equal
  `oa-truth`'s.
- **OA-TRK-021** A tracker's output rows are scored by `oa-bench`'s unchanged metric functions through
  `tracker_input`: a 100 m error in a known direction is recovered from T1 and T2, 2 m/s in speed from T3,
  50 ft from T4, and a copy of truth scores zero.
- **OA-TRK-022** Track sample columns of different lengths are refused with a typed error.
- **OA-TRK-023** The scoring split is exact: per aircraft, reports in (time, input index) order numbered
  from 0, even to set A (the tracker) and odd to set B (truth); the sets are disjoint, together every
  report, and independent of input order except between same-time reports.
- **OA-TRK-024** The quality fields are carried forward over the whole aircraft sequence before the split
  (the same 60 s rule and code) and written into both sets, so splitting does not starve the tracker of
  NACp.
- **OA-TRK-025** The scoring run's configuration comes from the measured interval distribution of set A:
  the expected interval is the median, the initiation interval the 90th percentile, the drop time truth's
  leg gap (120 s).
- **OA-TRK-026** Straight and turning are separated as in ESASSP Vol 1 4.2.14: transversal acceleration
  (speed times heading rate over a 6 s span of truth) above 1.5 m/s^2 is a turn; heading wraps at north.
- **OA-TRK-027** On a synthetic clip with known truth (a straight, a turning and an accelerating aircraft),
  the split, tracker, truth from the other half and the unchanged metrics run end to end: every track is
  confirmed 4 s after its first report, the position error is under 25 m RMS on both the one-second output
  and the report-time rows, both classes are populated and T5 of the output is one second.
- **OA-TRK-028** `oa-track-run --adsb <clip.oaf> --out <dir>` writes `report.txt`, `report.csv`,
  `tracks.csv` and `esassp.csv`, each scored number labelled as on thinned input and not an ESASSP exit
  result, byte-identical output digest across runs; usage errors exit 2 and an unreadable clip exits 1.
- **OA-TRK-029** On the real ZAU hour (skipped when `OPENATCA_ADSB_CLIP` is unset) the tracker runs with no
  capacity refusals or lost rows, is reproducible, confirms more than 80 % of its tracks and scores under
  60 m RMS at report times against the other half's truth.
- **OA-TRK-040** Re-filtering keeps one anchor filter state and a ring of the last 12 measurements per
  track and rebuilds the state at a late measurement by applying the ring's measurements to the anchor; on
  three aircraft whose reports arrive scrambled by up to 14 positions, with wild positions and exact and
  different-sensor repeats, every output bit and every counter equals those recorded from the
  implementation that stored a full filter state in each ring slot (recorded again at OA-TRK-077 and
  OA-TRK-078, which changed five duplicate counts).
- **OA-TRK-041** A tracker filled to `MAX_TRACKS` aircraft with more reports each than the ring holds
  refuses the next aircraft under its own counter, re-filters a late report of a track in the full table,
  and reallocates no buffer.
- **OA-TRK-042** Tracker memory is linear in the track capacity and under 2.2 kB per track (4,096 tracks:
  8.8 MB; 38,000 tracks: 81.7 MB; it was 8.9 kB and 339 MB with a filter state in every ring slot).
- **OA-TRK-043** The chunked benchmark's millisecond histogram gives the statistics of the values it was
  made from: count, median, p95, p99 and maximum exactly, mean and RMS to rounding (bit for bit for whole
  seconds), and adding two histograms gives the histogram of the union.
- **OA-TRK-044** Scoring a tracker's output in address ranges (one aircraft per range, a few, or all)
  gives exactly the metrics, exclusions, pair count, association counts and ESASSP-style split of scoring
  it in one piece with `run_benchmark`; the ranges tile the address space.
- **OA-TRK-045** A run whose output is written to column files as it is produced has the digest of the
  same rows held in memory, the same counters and the same rows and scores when read back in ranges, and
  removes its files when dropped.
- **OA-TRK-046** The latency stages are stamped from an injected clock: with a fixed-step clock every
  stage difference, percentile and the capacity overflow count are exact, a clock that goes backwards is
  counted, and the tracker's output is the output without a clock.
- **OA-TRK-047** `oa-track-run --no-score` runs the tracker core and the latency replay without truth or
  scoring, writes `core.txt` with throughput with and without the one-second output, per-stage latency,
  and resident memory per phase, and the replay's output digest equals the scoring run's.

### Radar fusion with registration (oa-track: radar, assoc, registration, fusion; study binary oa-track-fusion)

- **OA-TRK-030** A radar plot (slant range, azimuth, Mode C altitude) becomes a position in the track's local
  plane by the exact inverse of the WGS-84 east-north-up forward model the simulator uses (spherical start
  for the elevation, Newton on the target height), earth curvature and height included; the result inverts
  the forward model to within a millimetre and agrees with `oa-sensorsim`'s forward model.
- **OA-TRK-031** The plot covariance is first order in range, azimuth and height (along-range from range and
  height error, cross-range from azimuth times the ground range) with the range and altitude quantisation
  variance added, and matches a Monte Carlo of the conversion.
- **OA-TRK-032** Height comes from Mode C, else from the track the plot is tested against, else the plot is
  refused; Mode C altitude carries its quantisation noise.
- **OA-TRK-033** A registration bias is subtracted from the raw plot and its variance added to the plot's.
- **OA-TRK-034** An invalid site or plot (non-finite, out of range, zero period) is refused with a named
  error and never converted.
- **OA-TRK-035** The assignment of plots to tracks within a gate is solved exactly (components of the gate
  graph by union-find, a padded square problem per component, Hungarian method with potentials) and equals
  an exhaustive search on random instances; nearest neighbour is worse where two plots want one track;
  components above 160 fall back to greedy and are counted.
- **OA-TRK-036** A pair at or above the gate is never assigned; a plot whose assigned track has a competitor
  within the ambiguity margin is marked ambiguous.
- **OA-TRK-037** The per-radar range and azimuth bias estimate converges to the true bias with an uncertainty
  that is honest (the error is within 3 sigma over repeated runs).
- **OA-TRK-038** A residual beyond 4 sigma is rejected and the estimate is clamped to its limits.
- **OA-TRK-039** A reference with a larger position uncertainty moves the estimate less.
- **OA-TRK-048** Radar-only initiation needs 3 plots in 4 scans of one radar, consistent in position,
  speed, acceleration and altitude; one or two plots never make a track.
- **OA-TRK-049** Two isolated false plots never make a track.
- **OA-TRK-050** A Mode A code alone never assigns a plot, on any track.
- **OA-TRK-051** A plot carrying a Mode S address goes to the track keyed by that address (created if need
  be) and to no other, whatever the kinematics say.
- **OA-TRK-052** No decision reads the simulator's truth label: scrambling every label leaves the tracker's
  output byte-identical.
- **OA-TRK-053** Two tracks crossing at one altitude are held (the plot is not applied) while an assignment
  is ambiguous; identity is never swapped on kinematics alone.
- **OA-TRK-054** Registration learns a radar bias from ADS-B tracks of NACp 9 or better (at least 3 updates,
  newest at most 5 s old, predicted position sigma at most 15 m, altitude known, range at least 5 km),
  applies it to radar-only plots and publishes per-radar estimates over time. Invalid configuration and
  input are refused.
- **OA-TRK-055** Synthetic truth is exact by construction: a smooth analytic path (straight legs, turns,
  climbs) written as one-second knots, interpolated by `oa_truth::truth_at` within a centimetre of the
  analytic path.
- **OA-TRK-056** Synthetic paths are ordered and bounded (addresses, legs, times); invalid input is refused.
- **OA-TRK-057** The base world has 44 aircraft: 20 straight, 12 turning (1.5, 3 and 6 deg/s, both
  directions), 12 climbing or descending at 1,500 fpm, with no same-altitude approach inside 5 NM.
- **OA-TRK-058** Every pair geometry (crossing and parallel at 3 NM, crossing and same-track at 1,000 ft, and
  the stress geometries) has its stated closest approach.
- **OA-TRK-059** Every world builds into an exact truth that `oa-bench` accepts.
- **OA-TRK-060** Scoring a copy of the truth through `oa-bench` metrics gives no error beyond the 1e-7 degree
  representation; the same copy shifted 100 m scores 100 m; straight and turning rows are both populated.
- **OA-TRK-061** Identity accounting from simulator labels counts swaps (a label change within a track),
  wrong plots, false plots applied, false tracks (false plots at least as many as true) and duplicate tracks
  on hand-made logs.
- **OA-TRK-062** The simulated ADS-B has the NACp 10 error (a 2-D RMS of 5.8 m), 25 ft altitude steps,
  millisecond stamps, only equipped aircraft, and is the same for the same seed.
- **OA-TRK-063** A run is reproducible (output digest, assignments, counters). Radar-only, on a small world
  with the MSSR profile: every aircraft is tracked, no swap, no false track, no duplicate, and the position
  error of the track is below the raw plots' error against the same exact truth.
- **OA-TRK-064** Under a 150 m and 0.15 degree bias the registration estimate is within 20 m and 0.03 degree
  of the bias, radar-only aircraft score more than 40 % better with registration than without, and
  ADS-B-equipped aircraft are not made worse.
- **OA-TRK-065** `oa-track-fusion --out <dir>` writes `report.txt` and `tables.csv` stating the truth, its own
  error (zero), the baselines on identical plots and each table's ESASSP bars; usage errors exit 2.
- **OA-TRK-066** An output row without an altitude has NaN in the altitude, altitude variance and vertical
  rate columns (not 0 ft) and the flag clear; the scorer takes the altitude through
  `scored_altitude_ft`, which gives the absent value for a clear flag or a non-finite column; the
  output digest hashes absent columns as zero, so it equals the earlier digest of the same run.
- **OA-TRK-067** The tracker's plausibility check on the second report of a tentative track is the speed
  rule `oa_adsb::jump_verdict` (category, ground speeds, hard cap), not a fixed 700 kt: a 950 kt
  aircraft reporting 950 kt is not restarted, a 300 kt one that moves as far is, and one report
  printing a fast category does not excuse a 2,782 kt displacement. The category is the track's
  trusted one under `oa_adsb::category_observe`. The output digest of the ZAU clip is unchanged.
- **OA-TRK-068** A measurement more than 5 minutes ahead of the tracker clock does not move the clock
  or touch any track until three in a row, none earlier than the one before, agree (the others are
  counted as held); a confirmed step, and `tracker_advance` over a long gap, emit ticks only while a
  track could still be live and jump the tick grid over the rest in one sweep, counting the skipped
  ticks.
- **OA-TRK-069** A time of applicability or an advance beyond about 146 years either side of the epoch
  is refused and counted under its own reason, and a tick must be between 1 ms and a quarter of the
  range, so rounding to a tick and stepping from tick to tick cannot overflow or stop advancing.
- **OA-TRK-070** The assignment solver refuses a gate that is not a finite positive number, with a typed
  error and every plot left unassigned (a NaN or infinite penalty makes the padded problem meaningless);
  the same solver then works on a valid gate.
- **OA-TRK-071** The assignment (union-find, component grouping, Hungarian method, greedy fallback, the
  pair lookup and the ambiguity pass) runs in buffers allocated at start: their addresses and capacities
  are the same after solves of every shape, and an input with more tracks, plots or pairs than the buffers
  hold is refused with a typed error (counted by the fusion layer), never grown into. Fusion's own
  association buffers are likewise unchanged across a run with holds and forced resolutions.
- **OA-TRK-072** A plot with a Mode S address is applied to the track of that address only inside a wide
  gate (chi-square, 3 degrees of freedom, tail about 8e-11); a gross mismatch is counted and not applied. A
  track takes at most one plot per radar and bucket: of several Mode S plots the nearest, and no anonymous
  plot once a Mode S plot has taken the track.
- **OA-TRK-073** A flooded bucket costs a bounded amount: plots of one radar beyond 16,384 in a bucket,
  pairs beyond the pair limit and exact gate tests beyond the test limit are counted and not processed;
  the ambiguity and pair-lookup passes are one pass over the pairs, so 20,000 plots over 20,000 tracks are
  associated in under 3 s in a debug build (the quadratic scans took over 3 s of that alone and grow with
  the square), and a call that finds no input ready does not scan the queue.
- **OA-TRK-074** Radar registration uses a reference aircraft only after it has made several residuals and
  at least four distinct references are current, and only when its mean residual agrees with the median of
  the others'; a few references that misreport their position (two of twelve, 250 m) do not move the
  estimate, which stays within 10 m and 0.015 degree of the truth, and two references alone teach nothing.
  Through the fusion layer, two of eight ADS-B aircraft displaced 200 m leave the estimate within 20 m.
- **OA-TRK-075** A step change of a radar's bias (a realignment, 50 m and 0.05 degree to 200 m and 0.2
  degree) is followed within a few scans of the references: a CUSUM over the residuals that passed the
  agreement test restarts the estimate at their mean, counted and published with each estimate; an honest
  steady radar over 9,600 residuals is never restarted.
- **OA-TRK-076** Two aircraft flying together 90 m apart, every plot of which is ambiguous, are not starved:
  a track whose last two plots were held takes the next on the assignment's evidence (counted), where the
  plain hold applied none of the 74; both tracks are confirmed and no third track is made.
- **OA-TRK-077** A report is a duplicate when any report in the track's ring has its time and sensor, not
  only the newest or the one just before it: a repeat of one sensor's report after another sensor's report
  at the same instant is counted and not applied again, in order and out of order.
- **OA-TRK-078** A late report applied by re-filtering keeps the track's run of rejected reports, so the
  rejection that would have re-initialised the filter still does.
- **OA-TRK-079** A spill refuses a directory that exists and has anything in it, creates its column files
  new, and on drop removes only those files (by name) and then the directory if it is empty; a file put
  into the directory meanwhile survives.
- **OA-TRK-080** Reading a spilled run checks every column file's size against the row count and refuses a
  truncated or extended column of any of the 15, with a typed error naming the column and the bytes found
  and expected.
- **OA-TRK-081** A plot in the overhead cone (within about three degrees of the vertical, where the
  height barely depends on the elevation) is converted, by bisection on the elevation, to within 5 mm of
  the point it was made from; a plot whose altitude is higher than its range reaches by up to 500 m (the
  range error of a radar) converts to the point under the vertical, and only a larger discrepancy is
  refused, with its own error.
- **OA-TRK-082** Time passes for the tracks when every sensor is quiet: advancing the fusion layer to a
  time with nothing pending still runs the tracker to the end of the last completed bucket, so a track
  with no reports gets its fixed-rate rows, is marked coasting and is dropped at the drop time.
- **OA-TRK-083** The antipode of a frame's origin is not projected onto the origin: it is placed half the
  sphere's circumference away, so a report of a position on the far side of the earth is a jump the
  tracker refuses, not a coherent update.
- **OA-TRK-084** The fusion layer's bookkeeping does not grow with the run: the per-address registration
  table is cut back to the tracks the tracker holds whenever it reaches the tracker's capacity (an
  address the tracker refused is not recorded), and the radar-only track records stop at a limit, counted,
  and can be drained by the caller.
- **OA-TRK-085** Every live track up to the configured capacity takes part in association: the
  association's limit is not below the tracker's capacity, and a tracker configured for more than the
  limit is refused when the fusion layer is made.
- **OA-TRK-086** The digest of a spilled run equals the digest of the same rows in memory when rows have
  no altitude: the altitude columns of such a row hash as zero in both.
- **OA-TRK-087** A report the tracker did not take (a repeat, or one refused for its covariance) does not
  count toward a track's eligibility as a registration reference.
- **OA-TRK-088** A bucket already processed is not processed again: inputs for it that arrive after the
  advance past it are counted late and dropped, so batching the calls does not change what is
  associated.
