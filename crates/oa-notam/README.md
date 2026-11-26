# `oa-notam`

This crate decodes the documented FAA FNS AIXM 5.1 profile and provides a bounded encoder for
the event fields, feature time slices, retained designators/airport indicators, and polygon
geometry represented by its model. It preserves feature order and per-object associations during
round trips. A concrete end is retained even when the NOTAM effective end is `PERM`; when no
concrete end is retained, the encoder emits GML `indeterminatePosition="unknown"` without
inventing a timestamp. The pinned GML `TimePositionType` defines `unknown`, `after`, `before` and
`now` as distinct values; this profile refuses unsupported non-unknown indeterminate values.
Optional source properties absent from the model are omitted; XML byte identity is not promised.
It does not publish NOTAMs, provide SWIM transport, or implement general AIXM.

`encode_row` writes canonical UTF-8 XML to a caller buffer, is bounded by
`MAX_ENCODED_BYTES`, and leaves the destination unchanged on refusal. It is intended for model
round-trip and generated synthetic messages; it is not evidence of FNS acceptance or full XSD
conformance. The emitted profile structure is tested by parsing the generated message back into
the same row, feature and geometry model. The default constructor reserves sixteen feature
records per row as a memory-sizing budget, not an AIXM occurrence limit; callers can specify a
larger total feature capacity. FAA event-extension declarations are profile-specific and are not
included in the official core AIXM schema archive.

The official AIXM 5.1 XSD archive and retrieval receipt are pinned in `testdata/schema/`:

- `aixm_5_1_xsd.zip`, from <https://aixm.aero/sites/default/files/imce/AIXM51/aixm-5-1-20100201-xsd.zip>, SHA-256 `e14090244c66c38bd459b954b896faeeb7f7828f135164cf0546ae1c1f69c221`.
- `aixm_5_1_1_xsd.zip`, from <https://aixm.aero/sites/default/files/imce/library/aixm_5_1_1_xsd.zip>, SHA-256 `6794fd3ac9b9b4f23f9040cd6fcc7d61cf8415c9cca6a6ec13c6979e225c2b0a`.

The emitted namespaces use AIXM 5.1, matching the decoder. The 5.1.1 archive is retained as
version context only; its namespace differs. The AIXM 5.1 page also identifies an unavailable
GML profile schema, so these pinned archives are not used to claim full schema validation. The
GML `TimePositionType` and `TimeIndeterminateValueType` declarations are in
`xsd/ISO_19136_Schemas/temporal.xsd` inside the pinned 5.1 archive.
See the [NOTAM API proposal](https://github.com/AvilaLabs/OpenATCA/blob/main/docs/core-api/formats/aixm-notam.md).

**Tier: Preview.** The proposed external package name in Cargo metadata is not reserved or
published.
