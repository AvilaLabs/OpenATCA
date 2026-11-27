# `oa-fixm`

This crate decodes the documented FIXM 3.0 core and FAA NAS 3.0 SFDPS profile and provides a
bounded encoder for its implemented event subset. It does not publish or transport messages. See
the [FIXM/SFDPS API proposal](https://github.com/AvilaLabs/OpenATCA/blob/main/docs/core-api/formats/fixm-sfdps.md)
for the profile boundary and current package dependency gap.

**Tier: Preview for decode and bounded encoding.** The proposed external package name in Cargo metadata is not reserved
or published.
