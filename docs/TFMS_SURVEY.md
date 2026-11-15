# TFMS tfmDataService publication: survey

Status: written 2026-11-14 from one recording of the live SWIM SCDS TFMS feed. It describes
structure only; it quotes no flight, airport, fix, facility or time value (value patterns are
described in words). The decoder (`crates/oa-tfms`) and `REQUIREMENTS.md` (OA-TFMS-001..022) are
built on it.

## Method and what is not known

* Sample: 4,237 messages (`*.msg`, one XML document each) in one recording directory,
  40,491,617 bytes. Document size is 1,290 B to 212,689 B, mean 9,557 B. The `sourceTimeStamp`
  values of the inner messages span about six days, so the recording holds messages that were
  stamped well before it was taken. The file order is the arrival order.
* Every statement about structure and counts below was measured over all 4,237 documents.
* Every statement about what a field **means** is an inference from its name, its value
  pattern and how it co-occurs with other fields. No FAA TFMS document was read for this
  survey and no network was available, so **no meaning in this file is confirmed**; each one is
  marked "unconfirmed" and says what supports it. The FAA TFMS and SWIM TFMS-publication
  specifications are the place to settle them (open questions at the end).
* A "T" in the appendix tables marks an element that has text content and no children.

## The feed in one page

* Every message is one XML document with the root `tfmDataService` in the namespace
  `urn:us:gov:dot:faa:atm:tfm:tfmdataservice`. All are UTF-8 with an XML declaration. The
  prefix of the root is not fixed: `ds:` (1,888 documents) or `ns5:` (2,349). Prefixes of every
  other namespace vary the same way and a document may also use the default namespace, so
  elements must be matched by namespace URI and local name, never by prefix.
* The root has one or more output elements: `fltdOutput` (1,801 documents, flight data),
  `fiOutput` (2,434, flow information) and `tfmsStatusOutput` (2, feed status). In the sample
  no document mixes them; the decoder still accepts a mix.
* `fltdOutput` holds `fltdMessage` elements, each one flight message. `fiOutput` holds
  `fiMessage` elements, each one flow-information message. A document holds one or more
  messages (9,678 inner messages in 4,237 documents).
* Each `fltdMessage` carries the flight identity **as attributes**: `acid`, `airline`, `arrArpt`,
  `cdmPart`, `depArpt`, `fdTrigger`, `flightRef`, `major`, `msgType`, `sensitivity`,
  `sourceFacility`, `sourceTimeStamp`, and one body element named after the message type
  (`msgType` is the body element's name). The body repeats the identity in a
  `qualifiedAircraftId` (unconfirmed: the attributes are a header copy; they agree in the whole
  sample, and the decoder refuses a document where they do not).
* Each `fiMessage` has the attributes `sensitivity`, `visDomain`, `destinationCodes`,
  `sourceFacility`, `sourceTimeStamp`, `msgType`, and one body.
* The feed carries events, not state: a track update, a flight-plan amendment, a departure
  report and so on. A flight is known by the flight reference number `flightRef` (a decimal
  integer) and, for most messages, by its `gufi` (below).
* The documents declare FIXM 3.0/3.1 and FAA NAS namespaces (`http://www.fixm.aero/...`,
  `http://www.faa.aero/nas/3.1`) in 2,349 documents; **no element or attribute of the sample is
  in any of them**. All content is in the TFMS namespaces:

| Namespace (URN suffix after `urn:us:gov:dot:faa:atm:tfm:`) | Role (unconfirmed) | Documents declaring it |
|---|---|---:|
| `tfmdataservice` | root and outputs | 6,038 declarations |
| `tfmdatacoreelements` | identity, positions, altitudes | 4,237 |
| `flightdata` | flight message bodies | 4,150 |
| `flightdatacommonmessages` | shared flight message parts | 4,150 |
| `flowinformation` | `fiMessage` | 2,436 |
| `ficommonmessages` | restriction, advisory, FCA, configuration bodies | 2,436 |
| `ficommonmessages2` | TMI flight data | 2,436 |
| `ficommondatatypes`, `rapttimeline` | declared, used rarely or not at all | 2,436 |
| `tfmrequestreplytypes` | declared, not used | 2,349 |

### Message kinds

| Output | `msgType` | Messages | Notes |
|---|---|---:|---|
| flight | `trackInformation` | 3,991 | radar track: position, altitude, speed, next position |
| flight | `FlightSectors` | 1,597 | sector traversal list for a flight |
| flight | `FlightTimes` | 750 | ETA/ETD update |
| flight | `FlightModify` | 373 | airline data, times, status |
| flight | `flightPlanAmendmentInformation` | 172 | amendment of a filed plan |
| flight | `FlightScheduleActivate` | 140 | activation of a scheduled flight |
| flight | `flightPlanInformation` | 44 | filed plan |
| flight | `departureInformation` | 34 | departure report |
| flight | `oceanicReport` | 31 | oceanic position report |
| flight | `flightPlanCancellation` | 30 | cancellation |
| flight | `FlightRoute` | 29 | route update |
| flight | `boundaryCrossingUpdate` | 27 | centre boundary crossing |
| flight | `arrivalInformation` | 14 | arrival report |
| flight | `FlightCreate` | 10 | flight created |
| flow | `TMI_FLIGHT_LIST` | 2,347 | list of flights with a traffic management initiative |
| flow | `RSTR` | 63 | restriction (miles in trail, minutes in trail, stop, ...) |
| flow | `GADV` | 13 | general advisory (free text) |
| flow | `FXA` | 10 | flow evaluation / constrained area definition |
| flow | `APTC` | 1 | airport configuration |
| status | `status` elements | 478 | feed liveness per service and facility (2 documents) |

The flight trigger attribute `fdTrigger` has 18 values in the sample (a track message trigger,
create, modify, activate, amendment, departure-time-out, ...); the decoder keeps it as a
vocabulary code and refuses a value outside the list (OA-TFMS-011).

### What links this feed to SFDPS (measured)

* The TFMS `gufi` is a short code (10 characters in the sample, letters and digits) and is
  carried by 2,834 of 4,775 distinct flights. The SFDPS `gufi` is a `urn:uuid` UUID. **The two
  `gufi` elements are not the same kind of value and never compare equal.**
* The TFMS `gufi` does equal the SFDPS `flightPlan@identifier` (the plan identifier): 1,327
  flights are found both through that equality and through aircraft identification plus
  departure plus arrival, and in all 1,327 both routes name the same SFDPS flight.
* About 41 % of the TFMS flights have no `gufi` at all (which message kinds lack it was not
  broken down); for these the fallback key is the three attributes `acid`, `depArpt`, `arrArpt`,
  and 325 of them lack a departure or an arrival, so have no key either.
* Both feeds are partial samples of the same minutes; flights the SFDPS recording does not
  contain cannot match. The linkage function reports its numbers as found (OA-TFMS-020/021).

### Where programs live (measured)

* Ground delay program, ground stop, reroute and airspace flow program messages exist in the
  sample only as general advisories (`GADV`), identified by **words in the advisory title**
  (the title, `advisoryTitle`, carries the advisory number, the facility and the program name
  or its abbreviation; a cancellation carries `CNX` or `CANCELLATION`). There is no coded
  program type element. The decoder types a `GADV` row by these words (whole-word match) and
  flags cancellation.
* Restrictions (`RSTR`) carry a coded `restrictionCategory` (`MIT`, `MINIT`, `STOP`, `APREQ`,
  `TBM` in the sample) and a scope `restrictionType` (`ARRIVAL`, `DEPARTURE`, `ENROUTE`).
* `FXA` carries a coded `feaFcaType` (`FEA`, `FCA`).

## The column set and why

Flight rows hold what the flight messages carry in more than 1 % of the messages of their
kind, plus identity for every message:

* identity: message kind, trigger, `acid`, `flightRef`, `gufi`, `igtd`, airline, major carrier,
  departure and arrival airport, source facility and source time, computer id (facility and
  number), aircraft category and user category;
* position: latitude and longitude (DMS in the track and oceanic messages, decimal in
  next-position and next-event elements) to 1e-7 degree, time at position, altitude (feet,
  with the trailing letter of the track altitude kept), assigned and requested altitude,
  speed, true airspeed;
* times: ETA/ETD with their type, ARR/DEP fix with time, coordination fix/time/type, boundary
  crossing, time of departure and arrival with an estimated flag, airline out/off/on/in,
  runway and gate departure/arrival, flight creation, original departure/arrival, and the
  next position;
* aircraft and status: aircraft type (`aircraftModel`), engine class, equipment qualifier,
  special aircraft qualifier, flight status, diversion indicator, RVSM flags (tri-state),
  `cdmPart`;
* route text: the route of flight (the legacy format wins over the element text) with the
  departure and arrival procedure names, remarks, in the bounded text table;
* TMI status (TMI flight list entries): flight status, update type and time, FCA identifier in
  the text table, and the number of `fxaFlight` elements.

Not stored, counted as known (the appendix shows each): traversal lists
(`flightTraversalData2`, `fltTraversals`), ground speed outputs of the route messages, the
`fxaFlight` subtrees, and every TFM status tag that appears in fewer than 1 % of the messages
(`ARTD`, `BETA`, `BETD`, `CDM_MBR`, `GDC`, `PGTA`, ... appear in 1 to 2 of 2,347 list
messages and are listed in the appendix with their counts).

Flow rows hold, for `RSTR`, `GADV`, `FXA`, `APTC`: kind, program type, scope, element (the
restricted NAS element, the advisory's facility list, the FCA name or the airport), start and
end, event time, source time, rates (`mitValue`, `arrRate`, `depRate`, `stratAar`), identifiers
(restriction id, reference id, advisory number, action), origin, the text, the title or
qualifier, airports and remarks in the text table.

## Field notes (all unconfirmed meanings)

* Times are strict `YYYY-MM-DDTHH:MM:SSZ` with an optional fraction; no offset other than
  `Z` appears. `igtd` is read as the initial gate departure time (name and position).
* Latitude/longitude come as degrees/minutes/seconds attributes with a direction word, or as
  decimal attributes. A latitude without a longitude is refused.
* The altitude `simpleAltitude` is in hundreds of feet with an optional trailing letter in
  track messages (read as a status letter; kept as a code); the decoder stores feet.
* `speed` in track messages is knots; `filedTrueAirSpeed` is knots. The sample has whole numbers
  only, but the decoder does not assume it: it stores a speed in thousandths with its unit read
  from the form (a whole number or `ddd.d` is knots, `d.dd` and `Mddd` are Mach, `Nddd`-style is
  knots), keeps any other digit-and-dot shape with unit unknown and counts it, and refuses the
  message only for a byte that cannot be part of a speed.
* The three `rvsmData` attributes are booleans (`true` / `false`); stored with a third state for
  absent.
* The empty-leaf convention: an empty element such as an unset runway configuration is the
  absent value, not zero (the sample has them).

## Open questions

1. What is the meaning and code list of each `fdTrigger` value? (Names only in the sample.)
2. Is the trailing letter of a track altitude a status or a unit marker?
3. The `sensitivity`, `visDomain` and `destinationCodes` attributes: audience restrictions? They
   are carried by every message and not stored.
4. Why do about 41 % of the TFMS flights have no `gufi`? Does a flight acquire one later, and is
   the plan identifier assigned by the host computer (the SFDPS match suggests so)?
5. Is the sector list (`FlightSectors`) the full path or a delta? It is 1,597 messages, the
   second largest kind, and the decoder keeps only the message's identity and the ignored count.
6. Do advisories that start a ground delay program or a ground stop always say so in the title?
   Is there a coded type that this recording happens not to contain?
7. The 478 status elements: is `state` a finite code list?
8. Are the `fic2` status flags (`SPURIOUS_FLT`, `CANCELLED_BUT_FLEW`, ...) worth storing? They
   occur in 2 of 2,347 list messages in this recording.

## Appendix: per-message element tables

For each message type: every element path below the message element, in how many messages it
appears (`Messages`), how many times it occurs (`Occurrences`) and which attributes it carries
with their counts. Prefixes are only labels for this table (`fd` flight data, `fcm` flight data
common messages, `core` core elements, `fi` flow information, `fic`/`fic2` the two flow common
message namespaces, `svc` the service namespace). "(subtree: N distinct child paths)" marks an
element the decoder skips as one unit.


### `trackInformation` (3,991 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:trackInformation` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:ncsmRouteData` | 142 | 142 |  |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 74 | 74 | `arrTime` 74, `fixName` 74 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:departureFixAndTime` | 86 | 86 | `arrTime` 86, `fixName` 86 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:diversionIndicator` T | 142 | 142 |  |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:dp` | 48 | 48 | `routeName` 48, `routeType` 48 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:dpTransitionFix` T | 48 | 48 |  |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:eta` | 142 | 142 | `etaType` 142, `timeValue` 142 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:etd` | 142 | 142 | `etdType` 142, `timeValue` 142 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 142 | 142 |  |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:nextPosition` | 142 | 142 | `latitudeDecimal` 142, `longitudeDecimal` 142 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:remarksKeywords` T | 5 | 5 |  |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:routeOfFlight` T | 142 | 142 |  |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:rvsmData` | 142 | 142 | `currentCompliance` 142, `equipped` 142, `futureCompliance` 142 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:star` | 87 | 87 | `routeName` 87, `routeType` 87 |
| `fd:trackInformation/fcm:ncsmRouteData/fcm:starTransitionFix` T | 87 | 87 |  |
| `fd:trackInformation/fcm:ncsmTrackData` | 3,849 | 3,849 |  |
| `fd:trackInformation/fcm:ncsmTrackData/fcm:arrivalFixAndTime` | 1,874 | 1,874 | `arrTime` 1,874, `fixName` 1,874 |
| `fd:trackInformation/fcm:ncsmTrackData/fcm:departureFixAndTime` | 2,069 | 2,069 | `arrTime` 2,069, `fixName` 2,069 |
| `fd:trackInformation/fcm:ncsmTrackData/fcm:eta` | 3,346 | 3,346 | `etaType` 3,346, `timeValue` 3,346 |
| `fd:trackInformation/fcm:ncsmTrackData/fcm:nextEvent` | 3,344 | 3,344 | `latitudeDecimal` 3,344, `longitudeDecimal` 3,344 |
| `fd:trackInformation/fcm:ncsmTrackData/fcm:rvsmData` | 3,849 | 3,849 | `currentCompliance` 3,849, `equipped` 3,849, `futureCompliance` 3,849 |
| `fd:trackInformation/fcm:position` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:position/core:latitude` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:position/core:latitude/core:latitudeDMS` | 3,991 | 3,991 | `degrees` 3,991, `direction` 3,991, `minutes` 3,991, `seconds` 3,719 |
| `fd:trackInformation/fcm:position/core:longitude` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:position/core:longitude/core:longitudeDMS` | 3,991 | 3,991 | `degrees` 3,991, `direction` 3,991, `minutes` 3,991, `seconds` 3,719 |
| `fd:trackInformation/fcm:qualifiedAircraftId` | 3,991 | 3,991 | `aircraftCategory` 3,991, `userCategory` 3,991 |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:aircraftId` T | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:arrivalPoint` | 3,488 | 3,488 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 3,487 | 3,487 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:fix` (subtree: 5 distinct child paths) | 1 | 1 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:computerId` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:departurePoint` | 3,488 | 3,488 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 3,486 | 3,486 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:departurePoint/core:fix` (subtree: 1 distinct child paths) | 2 | 2 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:gufi` T | 3,464 | 3,464 |  |
| `fd:trackInformation/fcm:qualifiedAircraftId/core:igtd` T | 3,468 | 3,468 |  |
| `fd:trackInformation/fcm:reportedAltitude` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:reportedAltitude/core:assignedAltitude` | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:reportedAltitude/core:assignedAltitude/core:simpleAltitude` T | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:speed` T | 3,991 | 3,991 |  |
| `fd:trackInformation/fcm:timeAtPosition` T | 3,991 | 3,991 |  |

### `flightPlanInformation` (44 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:flightPlanInformation` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:altitude` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:altitude/core:assignedAltitude` | 5 | 5 |  |
| `fd:flightPlanInformation/fcm:altitude/core:assignedAltitude/core:simpleAltitude` T | 5 | 5 |  |
| `fd:flightPlanInformation/fcm:altitude/core:requestedAltitude` | 39 | 39 |  |
| `fd:flightPlanInformation/fcm:altitude/core:requestedAltitude/core:simpleAltitude` T | 39 | 39 |  |
| `fd:flightPlanInformation/fcm:coordinationPoint` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:coordinationPoint/core:fixRadialDistance` T | 1 | 1 | `distance` 1, `radial` 1 |
| `fd:flightPlanInformation/fcm:coordinationPoint/core:namedFix` T | 43 | 43 |  |
| `fd:flightPlanInformation/fcm:coordinationTime` T | 44 | 44 | `type` 44 |
| `fd:flightPlanInformation/fcm:flightAircraftSpecs` T | 44 | 44 | `equipmentQualifier` 44, `numberOfAircraft` 1, `specialAircraftQualifier` 1 |
| `fd:flightPlanInformation/fcm:ncsmRouteData` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 19 | 19 | `arrTime` 19, `fixName` 19 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:departureFixAndTime` | 29 | 29 | `arrTime` 29, `fixName` 29 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:diversionIndicator` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:dp` | 23 | 23 | `routeName` 23, `routeType` 23 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:dpTransitionFix` T | 23 | 23 |  |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:eta` | 44 | 44 | `etaType` 44, `timeValue` 44 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:etd` | 44 | 44 | `etdType` 44, `timeValue` 44 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:nextPosition` | 44 | 44 | `latitudeDecimal` 44, `longitudeDecimal` 44 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:remarksKeywords` T | 2 | 2 |  |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:routeOfFlight` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:rvsmData` | 44 | 44 | `currentCompliance` 44, `equipped` 44, `futureCompliance` 44 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:star` | 24 | 24 | `routeName` 24, `routeType` 24 |
| `fd:flightPlanInformation/fcm:ncsmRouteData/fcm:starTransitionFix` T | 24 | 24 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId` | 44 | 44 | `aircraftCategory` 44, `userCategory` 44 |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:aircraftId` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:arrivalPoint` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:computerId` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 43 | 43 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:departurePoint` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:gufi` T | 42 | 42 |  |
| `fd:flightPlanInformation/fcm:qualifiedAircraftId/core:igtd` T | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:routeOfFlight` | 44 | 44 | `legacyFormat` 44 |
| `fd:flightPlanInformation/fcm:speed` | 44 | 44 |  |
| `fd:flightPlanInformation/fcm:speed/core:filedTrueAirSpeed` T | 41 | 41 |  |
| `fd:flightPlanInformation/fcm:speed/core:mach` T | 3 | 3 |  |

### `flightPlanAmendmentInformation` (172 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:flightPlanAmendmentInformation` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newAltitude` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newAltitude/core:assignedAltitude` | 160 | 160 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newAltitude/core:assignedAltitude/core:simpleAltitude` T | 160 | 160 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newAltitude/core:requestedAltitude` | 12 | 12 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newAltitude/core:requestedAltitude/core:simpleAltitude` T | 12 | 12 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newCoordinationPoint` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newCoordinationPoint/core:fixRadialDistance` T | 108 | 108 | `distance` 108, `radial` 108 |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newCoordinationPoint/core:latLong` (subtree: 4 distinct child paths) | 9 | 9 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newCoordinationPoint/core:namedFix` T | 55 | 55 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newCoordinationTime` T | 172 | 172 | `type` 172 |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newFlightAircraftSpecs` T | 172 | 172 | `equipmentQualifier` 172, `specialAircraftQualifier` 14 |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newRouteOfFlight` | 172 | 172 | `legacyFormat` 172 |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newSpeed` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newSpeed/core:filedTrueAirSpeed` T | 167 | 167 |  |
| `fd:flightPlanAmendmentInformation/fcm:amendmentData/fcm:newSpeed/core:mach` T | 5 | 5 |  |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData` | 10 | 10 |  |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 5 | 5 | `arrTime` 5, `fixName` 5 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:departureFixAndTime` | 4 | 4 | `arrTime` 4, `fixName` 4 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:diversionIndicator` T | 10 | 10 |  |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:dp` | 5 | 5 | `routeName` 5, `routeType` 5 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:dpTransitionFix` T | 5 | 5 |  |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:eta` | 10 | 10 | `etaType` 10, `timeValue` 10 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:etd` | 10 | 10 | `etdType` 10, `timeValue` 10 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 10 | 10 |  |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:nextPosition` | 10 | 10 | `latitudeDecimal` 10, `longitudeDecimal` 10 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:routeOfFlight` T | 10 | 10 |  |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:rvsmData` | 10 | 10 | `currentCompliance` 10, `equipped` 10, `futureCompliance` 10 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:star` | 6 | 6 | `routeName` 6, `routeType` 6 |
| `fd:flightPlanAmendmentInformation/fcm:ncsmRouteData/fcm:starTransitionFix` T | 6 | 6 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId` | 172 | 172 | `aircraftCategory` 172, `userCategory` 172 |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:aircraftId` T | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:arrivalPoint` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:computerId` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:departurePoint` | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:gufi` T | 172 | 172 |  |
| `fd:flightPlanAmendmentInformation/fcm:qualifiedAircraftId/core:igtd` T | 171 | 171 |  |

### `departureInformation` (34 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:departureInformation` | 34 | 34 |  |
| `fd:departureInformation/fcm:flightAircraftSpecs` T | 34 | 34 | `equipmentQualifier` 34, `specialAircraftQualifier` 1 |
| `fd:departureInformation/fcm:ncsmFlightTimeData` | 34 | 34 |  |
| `fd:departureInformation/fcm:ncsmFlightTimeData/fcm:eta` | 34 | 34 | `etaType` 34, `timeValue` 34 |
| `fd:departureInformation/fcm:ncsmFlightTimeData/fcm:etd` | 34 | 34 | `etdType` 34, `timeValue` 34 |
| `fd:departureInformation/fcm:ncsmFlightTimeData/fcm:rvsmData` | 34 | 34 | `currentCompliance` 34, `equipped` 34, `futureCompliance` 34 |
| `fd:departureInformation/fcm:qualifiedAircraftId` | 34 | 34 | `aircraftCategory` 34, `userCategory` 34 |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:aircraftId` T | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:arrivalPoint` | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:computerId` | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:departurePoint` | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:gufi` T | 34 | 34 |  |
| `fd:departureInformation/fcm:qualifiedAircraftId/core:igtd` T | 34 | 34 |  |
| `fd:departureInformation/fcm:timeOfArrival` T | 3 | 3 | `estimated` 3 |
| `fd:departureInformation/fcm:timeOfDeparture` T | 34 | 34 | `estimated` 34 |

### `arrivalInformation` (14 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:arrivalInformation` | 14 | 14 |  |
| `fd:arrivalInformation/fcm:ncsmFlightTimeData` | 14 | 14 |  |
| `fd:arrivalInformation/fcm:ncsmFlightTimeData/fcm:eta` | 14 | 14 | `etaType` 14, `timeValue` 14 |
| `fd:arrivalInformation/fcm:ncsmFlightTimeData/fcm:etd` | 14 | 14 | `etdType` 14, `timeValue` 14 |
| `fd:arrivalInformation/fcm:ncsmFlightTimeData/fcm:rvsmData` | 14 | 14 | `currentCompliance` 14, `equipped` 14, `futureCompliance` 14 |
| `fd:arrivalInformation/fcm:qualifiedAircraftId` | 14 | 14 | `aircraftCategory` 14, `userCategory` 14 |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:aircraftId` T | 14 | 14 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:arrivalPoint` | 14 | 14 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 13 | 13 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:arrivalPoint/core:fix` (subtree: 5 distinct child paths) | 1 | 1 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:computerId` | 14 | 14 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 14 | 14 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:departurePoint` | 14 | 14 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 14 | 14 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:gufi` T | 13 | 13 |  |
| `fd:arrivalInformation/fcm:qualifiedAircraftId/core:igtd` T | 14 | 14 |  |
| `fd:arrivalInformation/fcm:timeOfArrival` T | 14 | 14 | `estimated` 14 |

### `flightPlanCancellation` (30 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:flightPlanCancellation` | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId` | 30 | 30 | `aircraftCategory` 30, `userCategory` 30 |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:aircraftId` T | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:arrivalPoint` | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:computerId` | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 3 | 3 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:departurePoint` | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 30 | 30 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:gufi` T | 12 | 12 |  |
| `fd:flightPlanCancellation/fcm:qualifiedAircraftId/core:igtd` T | 30 | 30 |  |

### `boundaryCrossingUpdate` (27 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:boundaryCrossingUpdate` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:boundaryPosition` | 27 | 27 | `boundaryCrossingTime` 27 |
| `fd:boundaryCrossingUpdate/fcm:boundaryPosition/core:fixRadialDistance` T | 25 | 25 | `distance` 25, `radial` 25 |
| `fd:boundaryCrossingUpdate/fcm:boundaryPosition/core:latLong` (subtree: 4 distinct child paths) | 2 | 2 |  |
| `fd:boundaryCrossingUpdate/fcm:flightAircraftSpecs` T | 27 | 27 | `equipmentQualifier` 27, `specialAircraftQualifier` 2 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData` | 3 | 3 |  |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 2 | 2 | `arrTime` 2, `fixName` 2 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:departureFixAndTime` | 2 | 2 | `arrTime` 2, `fixName` 2 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:diversionIndicator` T | 3 | 3 |  |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:dp` | 1 | 1 | `routeName` 1, `routeType` 1 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:dpTransitionFix` T | 1 | 1 |  |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:eta` | 3 | 3 | `etaType` 3, `timeValue` 3 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:etd` | 3 | 3 | `etdType` 3, `timeValue` 3 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 3 | 3 |  |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:nextPosition` | 3 | 3 | `latitudeDecimal` 3, `longitudeDecimal` 3 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:routeOfFlight` T | 3 | 3 |  |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:rvsmData` | 3 | 3 | `currentCompliance` 3, `equipped` 3, `futureCompliance` 3 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:star` | 3 | 3 | `routeName` 3, `routeType` 3 |
| `fd:boundaryCrossingUpdate/fcm:ncsmRouteData/fcm:starTransitionFix` T | 3 | 3 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId` | 27 | 27 | `aircraftCategory` 27, `userCategory` 27 |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:aircraftId` T | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:arrivalPoint` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:computerId` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:departurePoint` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:gufi` T | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:qualifiedAircraftId/core:igtd` T | 26 | 26 |  |
| `fd:boundaryCrossingUpdate/fcm:reportedAltitude` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:reportedAltitude/core:assignedAltitude` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:reportedAltitude/core:assignedAltitude/core:simpleAltitude` T | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:routeOfFlight` | 27 | 27 | `legacyFormat` 27 |
| `fd:boundaryCrossingUpdate/fcm:speed` | 27 | 27 |  |
| `fd:boundaryCrossingUpdate/fcm:speed/core:filedTrueAirSpeed` T | 26 | 26 |  |
| `fd:boundaryCrossingUpdate/fcm:speed/core:mach` T | 1 | 1 |  |

### `oceanicReport` (31 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:oceanicReport` | 31 | 31 |  |
| `fd:oceanicReport/fcm:ncsmRouteData` | 2 | 2 |  |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 1 | 1 | `arrTime` 1, `fixName` 1 |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:diversionIndicator` T | 2 | 2 |  |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:eta` | 2 | 2 | `etaType` 2, `timeValue` 2 |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:etd` | 2 | 2 | `etdType` 2, `timeValue` 2 |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 2 | 2 |  |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:nextPosition` | 2 | 2 | `latitudeDecimal` 2, `longitudeDecimal` 2 |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:routeOfFlight` T | 2 | 2 |  |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:rvsmData` | 2 | 2 | `currentCompliance` 2, `equipped` 2, `futureCompliance` 2 |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:star` | 1 | 1 | `routeName` 1, `routeType` 1 |
| `fd:oceanicReport/fcm:ncsmRouteData/fcm:starTransitionFix` T | 1 | 1 |  |
| `fd:oceanicReport/fcm:ncsmTrackData` | 29 | 29 |  |
| `fd:oceanicReport/fcm:ncsmTrackData/fcm:arrivalFixAndTime` | 19 | 19 | `arrTime` 19, `fixName` 19 |
| `fd:oceanicReport/fcm:ncsmTrackData/fcm:departureFixAndTime` | 7 | 7 | `arrTime` 7, `fixName` 7 |
| `fd:oceanicReport/fcm:ncsmTrackData/fcm:eta` | 28 | 28 | `etaType` 28, `timeValue` 28 |
| `fd:oceanicReport/fcm:ncsmTrackData/fcm:nextEvent` | 28 | 28 | `latitudeDecimal` 28, `longitudeDecimal` 28 |
| `fd:oceanicReport/fcm:ncsmTrackData/fcm:rvsmData` | 29 | 29 | `currentCompliance` 29, `equipped` 29, `futureCompliance` 29 |
| `fd:oceanicReport/fcm:plannedPositionData` (subtree: 7 distinct child paths) | 31 | 60 | `planNumber` 60 |
| `fd:oceanicReport/fcm:qualifiedAircraftId` | 31 | 31 | `aircraftCategory` 31, `userCategory` 31 |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:aircraftId` T | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:arrivalPoint` | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:computerId` | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:departurePoint` | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 31 | 31 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:gufi` T | 19 | 19 |  |
| `fd:oceanicReport/fcm:qualifiedAircraftId/core:igtd` T | 29 | 29 |  |
| `fd:oceanicReport/fcm:reportedPositionData` | 31 | 31 |  |
| `fd:oceanicReport/fcm:reportedPositionData/core:altitude` T | 31 | 31 |  |
| `fd:oceanicReport/fcm:reportedPositionData/core:position` | 31 | 31 |  |
| `fd:oceanicReport/fcm:reportedPositionData/core:position/core:latitude` | 31 | 31 |  |
| `fd:oceanicReport/fcm:reportedPositionData/core:position/core:latitude/core:latitudeDMS` | 31 | 31 | `degrees` 31, `direction` 31, `minutes` 31 |
| `fd:oceanicReport/fcm:reportedPositionData/core:position/core:longitude` | 31 | 31 |  |
| `fd:oceanicReport/fcm:reportedPositionData/core:position/core:longitude/core:longitudeDMS` | 31 | 31 | `degrees` 31, `direction` 31, `minutes` 31 |
| `fd:oceanicReport/fcm:reportedPositionData/core:time` T | 31 | 31 |  |
| `fd:oceanicReport/fcm:speed` T | 31 | 31 |  |

### `FlightCreate` (10 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:ncsmFlightCreate` | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:airlineData` | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:arrivalFixAndTime` | 4 | 4 | `arrTime` 4, `fixName` 4 |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:diversionIndicator` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:eta` | 10 | 10 | `etaType` 10, `timeValue` 10 |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:etd` | 10 | 10 | `etdType` 10, `timeValue` 10 |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:flightStatusAndSpec` | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:flightStatusAndSpec/fcm:aircraftModel` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:flightStatusAndSpec/fcm:aircraftSpecification` T | 10 | 10 | `aircraftEngineClass` 10, `equipmentQualifier` 1, `numberOfAircraft` 10, `specialAircraftQualifier` 7 |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:flightStatusAndSpec/fcm:flightStatus` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:flightTimeData` | 10 | 10 | `airlineInTime` 10, `airlineOffTime` 10, `airlineOnTime` 10, `airlineOutTime` 10, `flightCreation` 10, `originalArrival` 10, `originalDeparture` 10 |
| `fd:ncsmFlightCreate/fcm:airlineData/fcm:rvsmData` | 10 | 10 | `currentCompliance` 10, `equipped` 10, `futureCompliance` 10 |
| `fd:ncsmFlightCreate/fcm:diversionCancelData` (subtree: 2 distinct child paths) | 2 | 2 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId` | 10 | 10 | `aircraftCategory` 10, `userCategory` 10 |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:aircraftId` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:arrivalPoint` | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:computerId` | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:departurePoint` | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 10 | 10 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:gufi` T | 1 | 1 |  |
| `fd:ncsmFlightCreate/fcm:qualifiedAircraftId/core:igtd` T | 10 | 10 |  |

### `FlightModify` (373 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:ncsmFlightModify` | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:airlineData` | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:arrivalFixAndTime` | 280 | 280 | `arrTime` 280, `fixName` 280 |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:diversionIndicator` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:eta` | 373 | 373 | `etaType` 373, `timeValue` 373 |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:etd` | 373 | 373 | `etdType` 373, `timeValue` 373 |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:flightStatusAndSpec` | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:flightStatusAndSpec/fcm:aircraftModel` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:flightStatusAndSpec/fcm:aircraftSpecification` T | 373 | 373 | `aircraftEngineClass` 373, `equipmentQualifier` 200, `numberOfAircraft` 373, `specialAircraftQualifier` 25 |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:flightStatusAndSpec/fcm:flightStatus` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:flightTimeData` | 373 | 373 | `airlineInTime` 365, `airlineOffTime` 359, `airlineOnTime` 355, `airlineOutTime` 365, `flightCreation` 373, `gateArrival` 7, `gateDeparture` 94, `originalArrival` 373, `originalDeparture` 373, `runwayArrival` 16, `runwayDeparture` 61 |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:remarksKeywords` T | 7 | 7 |  |
| `fd:ncsmFlightModify/fcm:airlineData/fcm:rvsmData` | 373 | 373 | `currentCompliance` 373, `equipped` 373, `futureCompliance` 373 |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId` | 373 | 373 | `aircraftCategory` 373, `userCategory` 373 |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:aircraftId` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:arrivalPoint` | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:computerId` | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 186 | 186 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:departurePoint` | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 373 | 373 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:gufi` T | 180 | 180 |  |
| `fd:ncsmFlightModify/fcm:qualifiedAircraftId/core:igtd` T | 373 | 373 |  |

### `FlightTimes` (750 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:ncsmFlightTimes` | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:arrivalFixAndTime` | 436 | 436 | `arrTime` 436, `fixName` 436 |
| `fd:ncsmFlightTimes/fcm:departureFixAndTime` | 480 | 480 | `arrTime` 480, `fixName` 480 |
| `fd:ncsmFlightTimes/fcm:eta` | 750 | 750 | `etaType` 750, `timeValue` 750 |
| `fd:ncsmFlightTimes/fcm:etd` | 750 | 750 | `etdType` 750, `timeValue` 750 |
| `fd:ncsmFlightTimes/fcm:flightStatusAndSpec` | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:flightStatusAndSpec/fcm:aircraftModel` T | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:flightStatusAndSpec/fcm:aircraftSpecification` T | 750 | 750 | `aircraftEngineClass` 750, `equipmentQualifier` 570, `numberOfAircraft` 750, `specialAircraftQualifier` 67 |
| `fd:ncsmFlightTimes/fcm:flightStatusAndSpec/fcm:flightStatus` T | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId` | 750 | 750 | `aircraftCategory` 750, `userCategory` 750 |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:aircraftId` T | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:arrivalPoint` | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 749 | 749 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:arrivalPoint/core:fix` (subtree: 1 distinct child paths) | 1 | 1 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:computerId` | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 556 | 556 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:departurePoint` | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 750 | 750 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:gufi` T | 540 | 540 |  |
| `fd:ncsmFlightTimes/fcm:qualifiedAircraftId/core:igtd` T | 749 | 749 |  |
| `fd:ncsmFlightTimes/fcm:rvsmData` | 750 | 750 | `currentCompliance` 750, `equipped` 750, `futureCompliance` 750 |

### `FlightRoute` (29 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:ncsmFlightRoute` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:altitude` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:altitude/core:assignedAltitude` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:altitude/core:assignedAltitude/core:simpleAltitude` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:flightStatusAndSpec` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:flightStatusAndSpec/fcm:aircraftModel` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:flightStatusAndSpec/fcm:aircraftSpecification` T | 29 | 29 | `aircraftEngineClass` 29, `equipmentQualifier` 21, `numberOfAircraft` 29, `specialAircraftQualifier` 8 |
| `fd:ncsmFlightRoute/fcm:flightStatusAndSpec/fcm:flightStatus` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 18 | 18 | `arrTime` 18, `fixName` 18 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:departureFixAndTime` | 18 | 18 | `arrTime` 18, `fixName` 18 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:diversionIndicator` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:dp` | 13 | 13 | `routeName` 13, `routeType` 13 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:dpTransitionFix` T | 13 | 13 |  |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:eta` | 29 | 29 | `etaType` 29, `timeValue` 29 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:etd` | 29 | 29 | `etdType` 29, `timeValue` 29 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:nextPosition` | 29 | 29 | `latitudeDecimal` 29, `longitudeDecimal` 29 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:routeOfFlight` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:rvsmData` | 29 | 29 | `currentCompliance` 29, `equipped` 29, `futureCompliance` 29 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:star` | 21 | 21 | `routeName` 21, `routeType` 21 |
| `fd:ncsmFlightRoute/fcm:ncsmRouteData/fcm:starTransitionFix` T | 21 | 21 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId` | 29 | 29 | `aircraftCategory` 29, `userCategory` 29 |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:aircraftId` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:arrivalPoint` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:computerId` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 20 | 20 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:departurePoint` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:gufi` T | 20 | 20 |  |
| `fd:ncsmFlightRoute/fcm:qualifiedAircraftId/core:igtd` T | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:speed` | 29 | 29 |  |
| `fd:ncsmFlightRoute/fcm:speed/core:filedTrueAirSpeed` T | 8 | 8 |  |
| `fd:ncsmFlightRoute/fcm:speed/core:groundSpeed` T | 21 | 21 |  |

### `FlightSectors` (1,597 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:ncsmFlightSectors` | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId` | 1,597 | 1,597 | `aircraftCategory` 1,597, `userCategory` 1,597 |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:aircraftId` T | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:arrivalPoint` | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:computerId` | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:computerId/core:idNumber` T | 391 | 391 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:departurePoint` | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 1,597 | 1,597 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:gufi` T | 374 | 374 |  |
| `fd:ncsmFlightSectors/fcm:qualifiedAircraftId/core:igtd` T | 1,591 | 1,591 |  |

### `FlightScheduleActivate` (140 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fd:ncsmFlightScheduleActivate` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:altitude` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:altitude/core:assignedAltitude` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:altitude/core:assignedAltitude/core:simpleAltitude` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:flightStatusAndSpec` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:flightStatusAndSpec/fcm:aircraftModel` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:flightStatusAndSpec/fcm:aircraftSpecification` T | 140 | 140 | `aircraftEngineClass` 140, `numberOfAircraft` 140 |
| `fd:ncsmFlightScheduleActivate/fcm:flightStatusAndSpec/fcm:flightStatus` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:arrivalFixAndTime` | 87 | 87 | `arrTime` 87, `fixName` 87 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:departureFixAndTime` | 85 | 85 | `arrTime` 85, `fixName` 85 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:diversionIndicator` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:dp` | 62 | 62 | `routeName` 62, `routeType` 62 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:dpTransitionFix` T | 62 | 62 |  |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:eta` | 140 | 140 | `etaType` 140, `timeValue` 140 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:etd` | 140 | 140 | `etdType` 140, `timeValue` 140 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:flightTraversalData2` (subtree: 5 distinct child paths) | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:nextPosition` | 140 | 140 | `latitudeDecimal` 140, `longitudeDecimal` 140 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:routeOfFlight` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:rvsmData` | 140 | 140 | `currentCompliance` 140, `equipped` 140, `futureCompliance` 140 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:star` | 91 | 91 | `routeName` 91, `routeType` 91 |
| `fd:ncsmFlightScheduleActivate/fcm:ncsmRouteData/fcm:starTransitionFix` T | 91 | 91 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId` | 140 | 140 | `aircraftCategory` 140, `userCategory` 140 |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:aircraftId` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:arrivalPoint` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:arrivalPoint/core:airport` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:computerId` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:computerId/core:facilityIdentifier` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:departurePoint` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:departurePoint/core:airport` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:qualifiedAircraftId/core:igtd` T | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:speed` | 140 | 140 |  |
| `fd:ncsmFlightScheduleActivate/fcm:speed/core:filedTrueAirSpeed` T | 140 | 140 |  |

### `TMI_FLIGHT_LIST` (2,347 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fi:tmiFlightDataList` | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData` | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:ARTD` T | 1 | 1 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:BETA` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:BETD` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:CANCELLED_BUT_FLEW` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:CDM_MBR` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:CTA_COMPLIANCE` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:CTD_COMPLIANCE` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:DVSTATUS` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:ETE_COMPLIANCE` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:GDC` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:IGTA` T | 1 | 1 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:MAJOR` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:OETA` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:OETD` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:PETE` T | 1 | 1 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:PGTA` T | 1 | 1 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:PGTD` T | 1 | 1 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:SPURIOUS_FLT` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:aircraftCategory` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:engineClass` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight` | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:aircraftId` T | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:arrivalPoint` | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:arrivalPoint/core:airport` T | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:departurePoint` | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:departurePoint/core:airport` T | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:gufi` T | 1,996 | 1,996 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flight/core:igtd` T | 2,339 | 2,339 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:flightReference` T | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:fltTraversals` (subtree: 5 distinct child paths) | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:rvsmData` | 2 | 2 | `currentCompliance` 2, `equipped` 2, `futureCompliance` 2 |
| `fi:tmiFlightDataList/fi:flightData/fic2:status` T | 2,347 | 2,347 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:tmiFlightInfoList` | 2,345 | 2,345 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:tmiFlightInfoList/fic2:fxaFlightData` | 2,321 | 2,321 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:tmiFlightInfoList/fic2:fxaFlightData/fic2:fxaFlight` (subtree: 16 distinct child paths) | 2,321 | 2,352 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:tmiFlightInfoList/fic2:tmi` | 2,345 | 2,345 | `lastUpdateTime` 2,345, `updateType` 2,345 |
| `fi:tmiFlightDataList/fi:flightData/fic2:tmiFlightInfoList/fic2:tmi/fic2:fcaId` T | 2,345 | 2,345 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:userCategory` T | 2 | 2 |  |
| `fi:tmiFlightDataList/fi:flightData/fic2:weightClass` T | 2 | 2 |  |

### `RSTR` (63 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fi:restrictionMessage` | 63 | 63 |  |
| `fi:restrictionMessage/fic:action` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:aircraftType` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:airports` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:altitude` T | 1 | 1 |  |
| `fi:restrictionMessage/fic:approvalTime` T | 33 | 33 |  |
| `fi:restrictionMessage/fic:entryTime` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:eventTime` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:exclusions` T | 2 | 2 |  |
| `fi:restrictionMessage/fic:facility` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:impactedElements` T | 6 | 6 |  |
| `fi:restrictionMessage/fic:mitNumber` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:mitValue` T | 56 | 56 |  |
| `fi:restrictionMessage/fic:passback` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:providerStatus` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:qualifier` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:reasonText` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:referenceRestrictionEndTime` T | 27 | 27 |  |
| `fi:restrictionMessage/fic:referenceRestrictionId` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:remarks` T | 19 | 19 |  |
| `fi:restrictionMessage/fic:restrictedNasElements` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:restrictionCategory` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:restrictionId` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:restrictionType` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:speed` T | 2 | 2 |  |
| `fi:restrictionMessage/fic:startTime` T | 63 | 63 |  |
| `fi:restrictionMessage/fic:stopTime` T | 63 | 63 |  |

### `GADV` (13 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fi:generalAdvisory` | 13 | 13 |  |
| `fi:generalAdvisory/fic:advisoryNumber` T | 13 | 13 |  |
| `fi:generalAdvisory/fic:advisoryText` T | 13 | 13 |  |
| `fi:generalAdvisory/fic:advisoryTitle` T | 13 | 13 |  |
| `fi:generalAdvisory/fic:dateSent` T | 13 | 13 |  |
| `fi:generalAdvisory/fic:effectivePeriod` | 11 | 11 |  |
| `fi:generalAdvisory/fic:effectivePeriod/fidt:endTime` T | 11 | 11 |  |
| `fi:generalAdvisory/fic:effectivePeriod/fidt:startTime` T | 11 | 11 |  |
| `fi:generalAdvisory/fic:facilities` T | 7 | 7 |  |
| `fi:generalAdvisory/fic:origin` T | 13 | 13 |  |

### `FXA` (10 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fi:feaFca` | 10 | 10 |  |
| `fi:feaFca/fic:colorId` T | 10 | 10 |  |
| `fi:feaFca/fic:createSiteId` T | 10 | 10 |  |
| `fi:feaFca/fic:createWorkstationId` T | 10 | 10 |  |
| `fi:feaFca/fic:endTime` T | 10 | 10 |  |
| `fi:feaFca/fic:extendedFeaFlag` T | 10 | 10 |  |
| `fi:feaFca/fic:fcaDisplayPreferences` (subtree: 10 distinct child paths) | 10 | 10 |  |
| `fi:feaFca/fic:fcaDomain` T | 10 | 10 |  |
| `fi:feaFca/fic:fcaId` T | 10 | 10 |  |
| `fi:feaFca/fic:fcaName` T | 10 | 10 |  |
| `fi:feaFca/fic:fcaReason` T | 10 | 10 |  |
| `fi:feaFca/fic:feaFcaType` T | 10 | 10 |  |
| `fi:feaFca/fic:fsmEligibleFlag` T | 10 | 10 |  |
| `fi:feaFca/fic:lastUpdate` T | 10 | 10 |  |
| `fi:feaFca/fic:lastUpdateSiteId` T | 10 | 10 |  |
| `fi:feaFca/fic:lastUpdateWorkstationId` T | 10 | 10 |  |
| `fi:feaFca/fic:line` (subtree: 9 distinct child paths) | 7 | 7 |  |
| `fi:feaFca/fic:lookAheadTime` T | 10 | 10 |  |
| `fi:feaFca/fic:nas` | 2 | 2 |  |
| `fi:feaFca/fic:nas/fic:airportName` T | 2 | 2 |  |
| `fi:feaFca/fic:polygon` (subtree: 9 distinct child paths) | 1 | 1 |  |
| `fi:feaFca/fic:primaryFilter` (subtree: 22 distinct child paths) | 10 | 10 |  |
| `fi:feaFca/fic:secondaryFilter` (subtree: 21 distinct child paths) | 2 | 7 |  |
| `fi:feaFca/fic:startTime` T | 10 | 10 |  |
| `fi:feaFca/fic:tmiStatus` T | 10 | 10 |  |

### `APTC` (1 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `fi:airportConfigMessage` | 1 | 1 |  |
| `fi:airportConfigMessage/fic:aarAdjust` | 1 | 1 |  |
| `fi:airportConfigMessage/fic:adrEnteredOnPanel` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:airport` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:airportInFile` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:arrRate` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:arrRunwayConf` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:arrUserSpecified` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:depRate` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:depRunwayConf` | 1 | 1 |  |
| `fi:airportConfigMessage/fic:depUserSpecified` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:dynArrEnteredOnPanel` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:enteringFacility` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:entryTime` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:eventTime` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:facility` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:remarks` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:remarksGroupColor` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:stratAar` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:updateTime` T | 1 | 1 |  |
| `fi:airportConfigMessage/fic:weather` T | 1 | 1 |  |

### `tfmsStatusOutput/status` (478 messages)

| Path (below the message element) | Messages | Occurrences | Attributes (count) |
|---|---:|---:|---|
| `svc:businessFunc` T | 478 | 478 |  |
| `svc:direction` T | 478 | 478 |  |
| `svc:facility` T | 478 | 478 |  |
| `svc:numberMsgs` T | 478 | 478 |  |
| `svc:service` T | 478 | 478 |  |
| `svc:state` T | 478 | 478 |  |
| `svc:time` T | 352 | 352 |  |
