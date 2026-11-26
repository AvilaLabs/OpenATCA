# ASDI XML feed packet header (preview)

`oa-asdi` implements the fixed packet header and sequence observations documented in the FAA
*Aircraft Situation Display To Industry: Functional Description and Interface Control Document for
the XML Version*, version 1.8, 15 April 2011. The type-1 heartbeat zero-size rule is supported by
the FAA's public *TFMS-to-ASDI Interface Control Document* (CSC/TFMM-08/0473, March 2010 Release 3
Final, Revision 1), §3.2.1.3.2.2.10, page 3-11. The v1.8 source copy is pinned by SHA-256 in
`validation/interface-conformance-v1.json`; the 2010 document is linked there to its public FAA/CDM
PDF. This is a partial profile, not an ASDI feed decoder.

## Implemented

`parse_header(&[u8])` accepts exactly 32 bytes. Bytes 0–15 are a 14-digit `yyyymmddhhmmss` timestamp,
NUL terminator and blank pad. The remaining four network-order 32-bit fields are data type, sequence
number, compressed size, and decompressed size. The decoder validates timestamp digits/padding,
Gregorian date and time-of-day ranges, packet type 1 or 2, and sequence range 0 through 100000.
Timestamp components are exposed without timezone or epoch conversion.

`inspect_packet` applies caller-supplied compressed and decompressed byte caps, each limited by a
provisional 1 MiB hard ceiling matching `oa-xml`'s current input default. This ceiling is an
implementation bound, not a size promised by the ICD. It verifies the exact declared
compressed-payload length. Type 1 is accepted only with zero sizes and no body, as stated in the
2010 TFMS-to-ASDI ICD. Type 2 is
reported as `XmlCompressedNotDecoded`; the crate has not verified its compressed bytes or XML
content. Sequence observations use `100000 -> 1` as the ordinary wrap. A mismatch is a discontinuity,
not proof of loss or its cause. A later zero is reported as `ZeroObserved`, not asserted to be a
restart.

## Not implemented

This crate does not implement TCP/IP, vendor registration, credentials, socket buffering or
reconnection. It does not decompress the zlib payload, validate the ASDI preamble/XSD, decode ASDI
message types, or expose controller or flight-data semantics. The ICD references XSD files and
sample messages from the FAA ASDI site; their exact current editions are not pinned here. Generic XML
well-formedness tests cannot substitute for this missing schema validation.

No performance or live interoperability claim follows from this header-only profile. Tests use
synthetic byte fixtures derived from the public document.
