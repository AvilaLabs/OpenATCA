# XML, JSON and CSV

## `oa-xml`

`Parser::new(Limits) -> Result<Parser, XmlError>` validates limits and preallocates parser
tables; `parse(&mut self, input) -> Result<Doc<'_>, XmlError>` parses one complete byte message.
Document spans and indexes refer into the borrowed input; text/attribute/entity decoding is
explicit through `decode_into`, `decode_text_run`, `span_eq_str`, namespace helpers and span
accessors. The parser accepts XML 1.0 fifth-edition UTF-8 subset: declaration, elements,
attributes, namespaces, comments, CDATA, character references and five predefined entities. It
rejects DTD/DOCTYPE, external entity expansion, other processing instructions, undefined entities,
invalid UTF-8/XML characters, duplicate attributes, mismatched/unclosed tags, undeclared prefixes,
and content outside root. Input is one-pass, nonrecursive and bounded. Defaults/ceilings are in
`Limits`; defaults are tuned to message counts noted in source. Failures are typed `XmlError` with
limit/error class and byte offset. Whitespace-only inter-markup text is omitted; text split by
comments/CDATA may require `decode_text_run`; normalization happens during explicit decoding.
These choices must be preserved in the compatibility contract.

## `oa-json`

`parse(input, &Limits) -> Result<Doc, JsonError>` parses a complete UTF-8 RFC 8259 text into a
flat, bounded arena. `Doc` exposes node count/kind/length, strings, raw number text, optional
`as_f64`/`as_i64`/`as_bool`, child/member iterators, subtree skip and `get`. Number conversion can
return `None` if nonfinite or out of range; integer conversion rejects decimal/exponent syntax.
Duplicate keys are retained in document order and `get` returns the last; callers needing
duplicate rejection must inspect `members`. Limits: 32 MiB input, depth 64, 1 MiB raw string,
4,194,304 nodes by default; exact failures carry offset and kind (UTF-8, depth, node/string,
grammar, escape, trailing data, etc.). Parsing rejects BOM, comments, trailing commas, non-JSON
numeric constants, bad escapes/surrogates and invalid UTF-8.

## `oa-csv`

`Reader::new(source, BufRead)` or `with_limits(source, BufRead, Limits)` requires a header;
`header()` returns names and column lookup, and `next_record()` streams `Option<Record>`.
`Record::field(index)`, `get(header,name)` and `fields()` expose unquoted UTF-8 fields. The first
record is the header; duplicate/empty header names, field-count mismatch, malformed quoting, bare
CR and invalid UTF-8 are errors. Quoted values, doubled quotes, CRLF/LF and embedded quoted
newlines work; a leading UTF-8 BOM is skipped. Errors identify source and one-based physical line.
Defaults: record 1 MiB raw, field 64 KiB unquoted, 512 columns, 5,000,000 data records. Errors
include explicit I/O, missing/duplicate header, missing column, malformed quoting/line ending,
UTF-8, count mismatch and named limit exceeded. This is a streaming reader, not a CSV writer.

**Tier/stability proposal.** L0 bounded text codecs, with each supported grammar/decision recorded
separately. Existing JSONTestSuite and adversarial XML/CSV tests are useful evidence but do not
establish all §8 conditions. For each, publish edition/conformance tables and clean-room vectors;
run the required fuzz duration; review mutations; establish no-panic and architecture results;
write runnable Rust/Python examples and publish vectors. XML's chosen subset is not full XML
validation and should be named accordingly; JSON duplicate-key policy is caller-significant; CSV
line counting and BOM policy are observable compatibility choices. All parsers are byte/input
driven with explicit limits; none reads a clock, consults seals, opens paths, or decides that parsed
content is trusted.
