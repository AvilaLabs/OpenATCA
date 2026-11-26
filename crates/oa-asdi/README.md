# oa-asdi

This preview crate implements a pure, allocation-free subset of the FAA ASDI XML interface
document, version 1.8 (2011-04-15): its 32-byte packet header, documented packet types and
sequence-number observations. The type-1 zero-size heartbeat rule is cross-checked against the FAA
TFMS-to-ASDI ICD (CSC/TFMM-08/0473, March 2010 Release 3 Final, Revision 1). See
[`docs/core-api/formats/asdi.md`](../../docs/core-api/formats/asdi.md) for the exact scope.

`inspect_packet` requires explicit compressed and decompressed size caps and returns an XML packet
as `XmlCompressedNotDecoded`. Each cap is limited to a provisional 1 MiB implementation ceiling
matching the current `oa-xml` input default; it is not an ICD size guarantee. This release does not implement sockets, client registration,
credential handling, zlib decoding, XSD validation, XML message semantics or flight data. Synthetic
tests are source-derived; they do not establish interoperability with a live feed.
