# SFDPS message survey

Structure-only survey of the FAA SWIM SFDPS sample (1,916 files, one 60 s capture, 2026-11-14,
`OPENATCA_SCDS/sfdps-*`), made with `crates/oa-fixm/examples/survey.rs` over `oa-xml`. It decided
the column set of `oa-fixm`. It holds counts and element and attribute names only; no flight
values, identifiers or positions from the data appear here, and none are in the repository.
Fixtures in `crates/oa-fixm/tests` are hand-written and use made-up callsigns and identifiers.

## Corpus

| Root element | Files | Content |
|---|---:|---|
| `MessageCollection` (namespace `http://www.faa.aero/nas/3.0`) | 1,901 | 14,851 `message` elements, every one `xsi:type="ns5:FlightMessageType"` |
| `FDPSMsg` (namespace `us:gov:dot:faa:atm:enroute:entities:flightdata`) | 14 | status messages: `properties/prop*`, `center`, `msgTimes`, `details`, `source`, `statusType`, `time`; no flight data |
| `AIXMBasicMessage` (AIXM 5.1 message namespace) | 1 | one special-use airspace timeslice |

"One message per file" holds for 1,589 of the 1,901 collection files. The
rest carry 2 to 100 messages (17 files carry exactly 100, which looks like a batch cap). The
decoder therefore handles many `message` elements per document and gives one row per message.
`FDPSMsg` and `AIXMBasicMessage` documents are refused as `UnsupportedRoot` (15 files, counted).

Inside a message the structure is `message/flight` (`xsi:type="ns5:NasFlightType"`), always one,
carrying the attributes `centre`, `source`, `system` (always `SLC` in the sample), `timestamp`
(`YYYY-MM-DDThh:mm:ss.fffZ`) and, in 1.8% of messages, `flightType`. All element names in
`message` are unqualified; the prefixes `ns2` to `ns5` occur only inside `xsi:type` values.

## The `source` attribute

`source` is on `flight`, not on `message`. Counts over the 14,851 flight messages. "Structure"
lists what the payload of that source carries beyond the always-present identification, plan
identifier, status, GUFI and sequence numbers. **No source code could be confirmed from FAA
documentation: no FAA SFDPS documentation was available to this survey, and the meaning
column is inferred from payload structure only. Treat every meaning as unconfirmed.**

| Source | Messages | Structure seen | Meaning |
|---|---:|---|---|
| TH | 13,268 | `enRoute/position` (position, speed, velocity, altitude), `controllingUnit`, `assignedAltitude`, sometimes `handoff/receivingUnit` | unconfirmed; payload is a track/position update |
| OH | 338 | `handoff` with `transferringUnit`, `receivingUnit`, sometimes `acceptingUnit` | unconfirmed; payload is a handoff/ownership change |
| HX | 265 | `agreed/route` with `expandedRoute` | unconfirmed; payload is an expanded route |
| HZ | 241 | `enRoute/position` without time or altitude, `assignedAltitude` | unconfirmed; a position report with less detail than TH |
| HP | 233 | `coordination` (coordination fix and time) | unconfirmed; payload is coordination data |
| AH | 169 | `agreed/route`, `aircraftDescription`, `coordination`, mostly `assignedAltitude`, `beaconCodeAssignment`; status mostly `ACTIVE`, 15 `PROPOSED` | unconfirmed; payload is a full flight plan, probably an amendment |
| FH | 89 | like AH, plus `requestedAltitude` | unconfirmed; payload is a full flight plan |
| LH | 71 | `interimAltitude` | unconfirmed; payload is an interim altitude |
| CL | 53 | identification and status only; statuses `ACTIVE`, `CANCELLED`, `COMPLETED`, `DROPPED` | unconfirmed; a status change, probably cancellation |
| DH | 27 | `aircraftDescription`, `coordination`, departure times | unconfirmed; payload is a departure |
| HU | 22 | route, aircraft, `assignedAltitude`, `coordination`, `beaconCodeAssignment`, `operator` | unconfirmed; a full flight plan |
| HT | 18 | `enRoute/pointout` | unconfirmed; payload is a point-out |
| RH | 17 | identification, status `ACTIVE` or `DROPPED` | unconfirmed; probably a removal |
| HF | 15 | `enRoute/cleared` with `clearanceHeading` | unconfirmed; payload is a clearance |
| HV | 14 | identification, status `COMPLETED` | unconfirmed; probably a completion |
| NP | 3 | `beaconCodeAssignment` | unconfirmed; payload is a beacon code |
| PT | 3 | `enRoute/pointout` | unconfirmed; payload is a point-out |
| BA | 2 | `beaconCodeAssignment` | unconfirmed; payload is a beacon code |
| NL | 2 | identification only | unconfirmed |
| IH | 1 | `flightIdentificationPrevious` | unconfirmed; payload is an identification change |

Source codes are two letters in every sample
message. A source not in this table would still decode: the source is stored as stated and
nothing depends on its value.

## Centres

Twenty ARTCC identifiers appear in `centre`, 42 to 1,997 messages each: ZAB, ZAU, ZBW, ZDC, ZDV,
ZFW, ZHU, ZID, ZJX, ZKC, ZLA, ZLC, ZMA, ZME, ZMP, ZNY, ZOA, ZOB, ZSE, ZTL. No ZAN, ZHN or ZSU
messages were present in this capture.

## Units seen (`uom` attributes)

| Element | `uom` values (occurrences) |
|---|---|
| `enRoute/position/altitude`, `targetAltitude` | `FEET` (13,255 each) |
| `enRoute/position/actualSpeed/surveillance` | `KNOTS` (13,509) |
| `enRoute/position/trackVelocity/x`, `y` | `KNOTS` (13,244 each) |
| `assignedAltitude/simple` | `FEET` (12,694) |
| `assignedAltitude/vfrPlus` | `FEET` (802) |
| `assignedAltitude/block/above`, `below` | `FEET` (20 each) |
| `assignedAltitude/altFixAlt/pre`, `post` | `FEET` (12 each) |
| `requestedAltitude/simple`, `vfrPlus` | `FEET` (52, 7) |
| `interimAltitude` | `FEET` (71) |
| `requestedAirspeed/nasAirspeed` | `KNOTS` (275), `MACH` (5) |
| `coordinationFix/distance`, `expandedRoute/.../distance` | `NAUTICAL_MILES` |
| `.../radial` | `DEGREES` |
| `aircraftDescription/accuracy/cmsFieldType` | `NAUTICAL_MILES` (771) |

Positions are `<pos>lat lon</pos>` under `location srsName="urn:ogc:def:crs:EPSG::4326"` (latitude
first, decimal degrees, 2 to 6 decimals). Times are ISO-8601 UTC with a trailing `Z`: message
timestamps with milliseconds, all other times without.

## Decisions for the decoder

Columns are decoded when the field is operationally meaningful and present in more than 1% of
messages (148 messages), or is identity or a sequence anchor present in nearly all.

| Column | Path | Present | Why |
|---|---|---:|---|
| source, centre | `flight@source`, `@centre` | 100% | message kind and originating facility |
| message time | `flight@timestamp` | 100% | ordering, latency (IF-R4); always rejected without |
| GUFI | `gufi` | 100% | flight identity; rejected without; always a UUID in the sample |
| callsign, computer id | `flightIdentification` | 100% | identity |
| flight plan id | `flightPlan@identifier` | 99.97% | plan identity |
| departure, arrival | `departure@departurePoint`, `arrival@arrivalPoint` | 99.9% | 3 to 12 characters (airports and named points) |
| status | `flightStatus@fdpsFlightStatus` | 99.97% | five values: PROPOSED, ACTIVE, COMPLETED, CANCELLED, DROPPED |
| flight type | `flight@flightType` | 1.8% | SCHEDULED, NON_SCHEDULED, GENERAL |
| assigned altitude | `assignedAltitude/simple`, `vfrPlus`; `vfr` as a flag | 92% | clearance |
| reported altitude | `enRoute/position/altitude` | 89% | surveillance |
| position, position time | `position/position/location/pos`, `@positionTime` | 91%, 89% | track |
| ground speed | `actualSpeed/surveillance` | 91% | |
| velocity x, y | `trackVelocity` (east, north, knots) | 89% | track is derived by a free function |
| coast | `position@coastIndicator` | 0.1% | below 1% but tells a position is extrapolated |
| aircraft type | `aircraftDescription/aircraftType/icaoModelIdentifier` | 2.1% | |
| route text | `agreed/route@nasRouteText` | 1.9% | string table |
| operator | `operator/operatingOrganization/organization@name` | 74% | free text up to 61 characters; mostly three letters |
| beacon code | `beaconCodeAssignment/currentBeaconCode` | 1.4% | |
| controlling unit | `controllingUnit` | 83% | sector ownership |
| handoff units | `handoff/receivingUnit`, `transferringUnit` | 19%, 2.3% | |
| departure actual, arrival estimated | `runwayTime/actual`, `runwayTime/estimated` | 89% each | |
| message sequence, plan sequence | `nameValue MSG_SEQ_NO`, `FLIGHT_PLAN_SEQ_NO` | 100% | gap and duplicate detection (IF-R6) |

Left known and not decoded (below 1%, or not row-shaped): `aircraftDescription` capabilities and
accuracy, `expandedRoute` points, `estimatedElapsedTime`, `coordination`, alternates,
`requestedAltitude` (0.4%), `requestedAirspeed` (1.9%, kept for a later release), `interimAltitude`
(0.5%), `assignedAltitude/block` and `altFixAlt` (0.1% each), `pointout`, `cleared`, `originator`,
`routeToRevisedDestination`, `siteSpecificPlanId`, `targetAltitude`, `targetPosition` (duplicates of
position), the `ADSB_*` and other `nameValue` entries (the ADS-B ones are in 54% of messages and
look like a second surveillance source; a candidate for a later column set).

## Anomalies worth knowing

* `targetAltitude` carries `invalid="INVALID"` once in the capture; it is not decoded.
* All 241 HZ positions lack `positionTime` and have an empty `actualSpeed`, and 254 of the
  13,509 positions (those 241 and 13 TH) have no altitude, so a position row does not imply
  speed or altitude.
* A unit identifier may be `???`: 14 `controllingUnit` and 37 `receivingUnit` elements. It seems to
  mean an unknown unit; it is kept as written.
* Operator names are free text: three of 14,851 exceed 24 characters and are field errors.
* Entity-bearing values did not occur in the capture; the decoder handles them anyway.
* A `nameValue` named `4TH_ADAPTED_FIELD` appears six times; it is known and ignored.

## Element paths

Every distinct element path under `message` (flight messages only), with the number of messages
containing it, its total occurrences, and the attribute local names seen on it (the survey lists attribute local
names, so `xsi:type` shows as `type` and `xsi:nil` as `nil`; the only unprefixed `type` is on
`cmsFieldType`). This is also the content of the decoder's known-structure table
(`crates/oa-fixm/src/known.rs`).

| Path (from message) | Messages with it | Occurrences | Attributes (local names) |
|---|---:|---:|---|
| `(message)` | 14851 | 14851 | `nil`, `type`, `xsi` |
| `flight` | 14851 | 14851 | `centre`, `flightType`, `source`, `system`, `timestamp`, `type` |
| `flight/agreed` | 545 | 545 | - |
| `flight/agreed/route` | 545 | 545 | `atcIntendedRoute`, `initialFlightRules`, `localIntendedRoute`, `nasRouteText`, `type` |
| `flight/agreed/route/adaptedArrivalDepartureRoute` | 8 | 8 | `nasRouteAlphanumeric`, `nasRouteIdentifier` |
| `flight/agreed/route/adaptedDepartureRoute` | 42 | 42 | `nasRouteAlphanumeric`, `nasRouteIdentifier` |
| `flight/agreed/route/estimatedElapsedTime` | 190 | 772 | `elapsedTime` |
| `flight/agreed/route/estimatedElapsedTime/location` | 190 | 772 | - |
| `flight/agreed/route/estimatedElapsedTime/location/longitude` | 3 | 18 | - |
| `flight/agreed/route/estimatedElapsedTime/location/point` | 13 | 98 | `fix`, `type` |
| `flight/agreed/route/estimatedElapsedTime/location/point/location` | 2 | 15 | `srsName` |
| `flight/agreed/route/estimatedElapsedTime/location/point/location/pos` | 2 | 15 | - |
| `flight/agreed/route/estimatedElapsedTime/location/region` | 185 | 656 | `airspaceType` |
| `flight/agreed/route/expandedRoute` | 265 | 265 | `type` |
| `flight/agreed/route/expandedRoute/routePoint` | 265 | 1397 | `estimatedTime` |
| `flight/agreed/route/expandedRoute/routePoint/point` | 265 | 1397 | `fix`, `type` |
| `flight/agreed/route/expandedRoute/routePoint/point/distance` | 87 | 88 | `uom` |
| `flight/agreed/route/expandedRoute/routePoint/point/location` | 5 | 5 | `srsName` |
| `flight/agreed/route/expandedRoute/routePoint/point/location/pos` | 5 | 5 | - |
| `flight/agreed/route/expandedRoute/routePoint/point/radial` | 87 | 88 | `uom` |
| `flight/agreed/route/nasadaptedArrivalRoute` | 39 | 39 | `nasRouteAlphanumeric`, `nasRouteIdentifier` |
| `flight/agreed/route/nasadaptedArrivalRoute/nasFavNumber` | 1 | 1 | - |
| `flight/aircraftDescription` | 307 | 307 | `aircraftAddress`, `aircraftPerformance`, `aircraftQuantity`, `equipmentQualifier`, `registration`, `tfmsSpecialAircraftQualifier`, `type`, `wakeTurbulence` |
| `flight/aircraftDescription/accuracy` | 259 | 259 | - |
| `flight/aircraftDescription/accuracy/cmsFieldType` | 259 | 771 | `phase`, `type`, `uom` |
| `flight/aircraftDescription/aircraftType` | 307 | 307 | - |
| `flight/aircraftDescription/aircraftType/icaoModelIdentifier` | 307 | 307 | - |
| `flight/aircraftDescription/capabilities` | 280 | 280 | `standardCapabilities` |
| `flight/aircraftDescription/capabilities/communication` | 259 | 259 | `otherCommunicationCapabilities`, `otherDataLinkCapabilities`, `selectiveCallingCode` |
| `flight/aircraftDescription/capabilities/communication/communicationCode` | 257 | 257 | - |
| `flight/aircraftDescription/capabilities/communication/dataLinkCode` | 118 | 118 | - |
| `flight/aircraftDescription/capabilities/navigation` | 273 | 273 | `otherNavigationCapabilities` |
| `flight/aircraftDescription/capabilities/navigation/navigationCode` | 273 | 273 | - |
| `flight/aircraftDescription/capabilities/navigation/performanceBasedCode` | 259 | 259 | - |
| `flight/aircraftDescription/capabilities/surveillance` | 280 | 280 | `otherSurveillanceCapabilities` |
| `flight/aircraftDescription/capabilities/surveillance/surveillanceCode` | 280 | 280 | - |
| `flight/arrival` | 14832 | 14832 | `arrivalPoint`, `type` |
| `flight/arrival/arrivalAerodrome` | 3 | 3 | `type` |
| `flight/arrival/arrivalAerodrome/point` | 3 | 3 | `type` |
| `flight/arrival/arrivalAerodrome/point/location` | 3 | 3 | `srsName` |
| `flight/arrival/arrivalAerodrome/point/location/pos` | 3 | 3 | - |
| `flight/arrival/arrivalAerodromeAlternate` | 114 | 114 | `code`, `type` |
| `flight/arrival/runwayPositionAndTime` | 13224 | 13224 | - |
| `flight/arrival/runwayPositionAndTime/runwayTime` | 13224 | 13224 | - |
| `flight/arrival/runwayPositionAndTime/runwayTime/actual` | 27 | 27 | `time` |
| `flight/arrival/runwayPositionAndTime/runwayTime/estimated` | 13197 | 13197 | `time` |
| `flight/assignedAltitude` | 13719 | 13719 | - |
| `flight/assignedAltitude/altFixAlt` | 12 | 12 | - |
| `flight/assignedAltitude/altFixAlt/point` | 12 | 12 | `fix`, `type` |
| `flight/assignedAltitude/altFixAlt/post` | 12 | 12 | `uom` |
| `flight/assignedAltitude/altFixAlt/pre` | 12 | 12 | `uom` |
| `flight/assignedAltitude/block` | 20 | 20 | - |
| `flight/assignedAltitude/block/above` | 20 | 20 | `uom` |
| `flight/assignedAltitude/block/below` | 20 | 20 | `uom` |
| `flight/assignedAltitude/simple` | 12694 | 12694 | `uom` |
| `flight/assignedAltitude/vfr` | 191 | 191 | - |
| `flight/assignedAltitude/vfrPlus` | 802 | 802 | `uom` |
| `flight/controllingUnit` | 12276 | 12276 | `sectorIdentifier`, `type`, `unitIdentifier` |
| `flight/coordination` | 540 | 540 | `coordinationTime`, `coordinationTimeHandling` |
| `flight/coordination/coordinationFix` | 513 | 513 | `fix`, `type` |
| `flight/coordination/coordinationFix/distance` | 375 | 375 | `uom` |
| `flight/coordination/coordinationFix/location` | 4 | 4 | `srsName` |
| `flight/coordination/coordinationFix/location/pos` | 4 | 4 | - |
| `flight/coordination/coordinationFix/radial` | 375 | 375 | `uom` |
| `flight/departure` | 14832 | 14832 | `departurePoint`, `type` |
| `flight/departure/runwayPositionAndTime` | 13309 | 13309 | - |
| `flight/departure/runwayPositionAndTime/runwayTime` | 13309 | 13309 | - |
| `flight/departure/runwayPositionAndTime/runwayTime/actual` | 13205 | 13205 | `time` |
| `flight/departure/runwayPositionAndTime/runwayTime/estimated` | 104 | 104 | `time` |
| `flight/departure/takeoffAlternateAerodrome` | 3 | 3 | `code`, `type` |
| `flight/enRoute` | 14109 | 14109 | `type` |
| `flight/enRoute/alternateAerodrome` | 10 | 25 | `code`, `type` |
| `flight/enRoute/beaconCodeAssignment` | 231 | 231 | - |
| `flight/enRoute/beaconCodeAssignment/currentBeaconCode` | 201 | 201 | - |
| `flight/enRoute/beaconCodeAssignment/reassignedBeaconCode` | 3 | 3 | - |
| `flight/enRoute/boundaryCrossings` | 2807 | 2807 | `type` |
| `flight/enRoute/boundaryCrossings/handoff` | 2807 | 2807 | `event`, `type` |
| `flight/enRoute/boundaryCrossings/handoff/acceptingUnit` | 148 | 148 | `sectorIdentifier`, `type`, `unitIdentifier` |
| `flight/enRoute/boundaryCrossings/handoff/receivingUnit` | 2807 | 2807 | `sectorIdentifier`, `type`, `unitIdentifier` |
| `flight/enRoute/boundaryCrossings/handoff/transferringUnit` | 338 | 338 | `sectorIdentifier`, `type`, `unitIdentifier` |
| `flight/enRoute/cleared` | 8 | 8 | `clearanceHeading`, `type` |
| `flight/enRoute/pointout` | 21 | 21 | - |
| `flight/enRoute/pointout/originatingUnit` | 21 | 21 | `sectorIdentifier`, `type`, `unitIdentifier` |
| `flight/enRoute/pointout/receivingUnit` | 21 | 22 | `sectorIdentifier`, `type`, `unitIdentifier` |
| `flight/enRoute/position` | 13509 | 13509 | `coastIndicator`, `positionTime`, `reportSource`, `targetPositionTime`, `type` |
| `flight/enRoute/position/actualSpeed` | 13509 | 13509 | - |
| `flight/enRoute/position/actualSpeed/surveillance` | 13509 | 13509 | `uom` |
| `flight/enRoute/position/altitude` | 13255 | 13255 | `uom` |
| `flight/enRoute/position/position` | 13509 | 13509 | `type` |
| `flight/enRoute/position/position/location` | 13509 | 13509 | `srsName` |
| `flight/enRoute/position/position/location/pos` | 13509 | 13509 | - |
| `flight/enRoute/position/targetAltitude` | 13255 | 13255 | `invalid`, `uom` |
| `flight/enRoute/position/targetPosition` | 13255 | 13255 | `srsName` |
| `flight/enRoute/position/targetPosition/pos` | 13255 | 13255 | - |
| `flight/enRoute/position/trackVelocity` | 13244 | 13244 | - |
| `flight/enRoute/position/trackVelocity/x` | 13244 | 13244 | `uom` |
| `flight/enRoute/position/trackVelocity/y` | 13244 | 13244 | `uom` |
| `flight/flightIdentification` | 14851 | 14851 | `aircraftIdentification`, `computerId`, `siteSpecificPlanId`, `type` |
| `flight/flightIdentificationPrevious` | 1 | 1 | `aircraftIdentification`, `computerId`, `siteSpecificPlanId` |
| `flight/flightPlan` | 14846 | 14846 | `flightPlanRemarks`, `identifier` |
| `flight/flightStatus` | 14846 | 14846 | `fdpsFlightStatus`, `type` |
| `flight/gufi` | 14851 | 14851 | `codeSpace` |
| `flight/interimAltitude` | 71 | 71 | `nil`, `uom` |
| `flight/operator` | 11014 | 11014 | - |
| `flight/operator/operatingOrganization` | 11014 | 11014 | - |
| `flight/operator/operatingOrganization/organization` | 11014 | 11014 | `name` |
| `flight/originator` | 61 | 61 | - |
| `flight/originator/aftnAddress` | 61 | 61 | - |
| `flight/requestedAirspeed` | 280 | 280 | - |
| `flight/requestedAirspeed/nasAirspeed` | 280 | 280 | `uom` |
| `flight/requestedAltitude` | 59 | 59 | - |
| `flight/requestedAltitude/simple` | 52 | 52 | `uom` |
| `flight/requestedAltitude/vfrPlus` | 7 | 7 | `uom` |
| `flight/routeToRevisedDestination` | 2 | 2 | - |
| `flight/routeToRevisedDestination/route` | 2 | 2 | `routeText`, `type` |
| `flight/specialHandling` | 1 | 1 | - |
| `flight/supplementalData` | 14851 | 14851 | `type` |
| `flight/supplementalData/additionalFlightInformation` | 14851 | 14851 | - |
| `flight/supplementalData/additionalFlightInformation/nameValue` | 14851 | 119197 | `name`, `value` |
