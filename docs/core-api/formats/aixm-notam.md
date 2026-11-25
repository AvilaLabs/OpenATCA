# AIXM NOTAM public API proposal

**Tier:** Preview, narrowly scoped to the FAA FNS profile.

**Proposed public surface:** `NotamDecoder::new(limits, DecodePolicy)`, `decode_document(doc, source_bytes, &mut Notams) -> Result<Outcome, DecodeError>`, plus a caller-owned statistics object and explicit field/error accessors. Public docs must call this “FAA FNS AIXM 5.1 profile decoder,” not an AIXM 5.1 implementation generally. AIXM namespace/profile version and FAA event extension are explicit policy metadata, not inferred from prefixes.

**Source-backed contract:** decodes FAA SWIM FNS `AIXMBasicMessage` AIXM 5.1 with FAA event extension into flat NOTAM rows. XML timestamps accepted by current scalar routines include ISO UTC and a compact format; values are represented as Unix milliseconds. Geometry is E7 integer coordinates; altitude references distinguish AGL/FL/MSL and preserve coded units/flags. No local timezone assumption should be introduced.

**Bounds/refusals:** default max 50,000 XML elements, 8,192 bytes text/schedule, 4,096 vertices per polygon; caller-owned row/text/polygon/vertex capacities also bound decoding. One document is all-or-nothing for capacity/root failure; malformed individual fields are marked absent with error bits and counts, unknown XML paths are counted in bounded stats, and no bad polygon is partially retained. Whole-document refusal types and field-level errors are distinct. Preserve XML parser positions if available; current `DocError`/reject model does not promise precise byte/line location for every semantic field error, so this is a public API gap to resolve.

**Encode/publish gap:** no AIXM/FNS encoder or publisher exists. Do not promise NOTAM serialization, message signing, SWIM transport, or general AIXM round trips. The test support does not establish a redistributable corpus; vectors must be synthetic/licensed. Stable evidence is not established.
