# FIXM US Extension 3.0 subset evidence

This file records the limited NAS schema facts used by the OA-FIXM encoder. It is not a copy of the schema and does not claim full FAA SFDPS validation.

## Sources

- Official NAS 3.0 generated schema documentation: <https://fixm.aero/releases/US_Ext-3.0/doc/diagrams/Nas.html>
- Retrieved page SHA-256: `2486d467a070c7ef3e7789185e7ab5cd2f21d1395e328d2e4bc87e33502fc6e7`.
- Official FIXM Core 3.0.1 schema archive: <https://www.fixm.aero/releases/FIXM-3.0.1/FIXM_Core_v3_0_1_Schemas.zip>
- The exact 132,340-byte Core archive is checked in at [FIXM_Core_v3_0_1_Schemas.zip](schemas/FIXM_Core_v3_0_1_Schemas.zip), SHA-256 `fe53afa19ef16f9323c83f74f7626550b66f8bf511b418ba007810da3ca05e48`.

The NAS documentation identifies NAS local elements as unqualified. Its `MessageCollectionType` has one-or-more messages, and `NasFlightType` extends Core `FlightType` with NAS extension fields. The checked-in Core archive supplies the qualified element declarations and types for the Core fields emitted by this profile.

## Direct NAS type facts used

- `NasAltitudeType` is a choice. The implementation represents `simple`, `vfrPlus`, `vfr`, `block`, and `altFixAlt`; other parsed choices remain unsupported until their model representation is added.
- `BlockAltitudeType` has `above` and `below` children. The emitted children are unqualified and carry `ff:AltitudeType` values.
- `AltFixAltAltitudeType` has ordered `point`, `post`, and `pre` children. `point` is `fb:SignificantPointType`; `post` and `pre` are `ff:AltitudeType` values.
- `NasHandoffType/acceptingUnit` is an unqualified element with type `fb:AtcUnitReferenceType`.
- `NasHandoffEventType` values are `INITIATION`, `ACCEPTANCE`, `RETRACTION`, `TAKE_CONTROL`, `UPDATE`, and `FAILURE`.

These facts ground this encoder's finite element/profile checks. The NAS source is generated documentation, not a retrieved NAS XSD archive; no generic schema validator is implemented.
