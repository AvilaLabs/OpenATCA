# FNS (NOTAM) publication: survey

Status: written 2026-11-14 from a recorded sample of the SWIM SCDS FNS feed. It describes
structure only; it quotes no NOTAM number, location, text, coordinate or identifier. The decoder
(`crates/oa-notam`) and `REQUIREMENTS.md` (OA-NOTAM-001..021) are built on it.

## Method and what is not known

* Sample: 112 messages (`*.msg`), one XML document each, 2.5 KB to 35.9 KB (median 4.2 KB),
  37 to 183 elements per message, element depth up to 12. Every count below was measured over
  all 112 messages.
* Every statement about what a field **means** is an inference from its name, its value pattern
  and how it co-occurs with other fields. No FAA FNS, AIXM 5.1 or event-extension document was
  read (there is no network access in this environment), so **no meaning in this file is
  confirmed**; each is marked "unconfirmed" and says what supports it. The FAA FNS interface
  specification and the AIXM 5.1 model are the place to settle them (open questions at the end).
* Value patterns are written with `A` for a letter and `9` for a digit where a value would
  identify a NOTAM. "Distinct" counts are the number of different values in the sample.

## The feed in one page

* Every message is an `message:AIXMBasicMessage` (AIXM 5.1 basic message) with prefixes for the
  `message`, `gml`, `aixm`, `event`, `fnse`, `html`, `xlink` and `xsi` namespaces declared on the
  root. The decoder matches on the namespace URI alias, not the author's prefix text.
* The root carries `gml:id`, `xsi:schemaLocation`, an optional `gml:boundedBy` (37 of 112) and
  one `message:hasMember` per member: 249 in all. Members per message: 1 (23 messages), 2 (73),
  4 (4), 5 (8), 6 (4).
* Exactly one member per message is an `event:Event` (the NOTAM event itself). The other members
  are the features the NOTAM concerns (an airport, a runway, a navaid ...). The member-to-event
  link is an `xlink:href` on `event:theEvent` inside each feature's extension (unconfirmed: it
  looks like a reference to the Event's `gml:identifier`); the decoder does not follow it.
* The event is `event:Event/event:timeSlice/event:EventTimeSlice`. Its NOTAM content is under
  `event:textNOTAM/event:NOTAM`; FAA-specific fields are under
  `event:extension/fnse:EventExtension`.
* `aixm:sequenceNumber`, `aixm:correctionNumber` and `aixm:interpretation` are on every time
  slice. `interpretation` takes the words `BASELINE` (on the Event, 112), `SNAPSHOT` (135) and
  `TEMPDELTA` (19) (unconfirmed meanings, from the words).

## Feature types (members other than the Event)

| Member element | Elements | Messages |
|---|---:|---:|
| `event:Event` | 112 | 112 |
| `aixm:AirportHeliport` | 74 | 74 |
| `aixm:Unit` | 15 | 15 |
| `aixm:RunwayDirection` | 10 | 9 |
| `aixm:Runway` | 9 | 9 |
| `aixm:RunwayElement` | 9 | 9 |
| `aixm:TaxiwayElement` | 7 | 4 |
| `aixm:Taxiway` | 4 | 4 |
| `aixm:Navaid` | 2 | 2 |
| `aixm:ObstacleArea` | 2 | 2 |
| `aixm:PrimarySurveillanceRadar` | 2 | 1 |
| `aixm:VerticalStructure` | 2 | 2 |
| `aixm:RadarSystem` | 1 | 1 |

No `aixm:Airspace`, `aixm:AirTrafficControlService` or other airspace-class member appears.
The decoder reads twelve feature kinds (those above plus the event) and counts any other member
as an unknown path.

## NOTAM fields (paths below the Event time slice)

Unconfirmed meanings, from names and value shapes:

* `series` / `number` / `year`: the NOTAM identifier (a letter, a serial, a year). `series` is
  absent in 47 of 112 messages (65 present). `number` and `year` are always present. `year`
  is always 4 digits in the sample.
* `type`: `N` (94), `C` (13) or `R` (3), two messages with none. Unconfirmed: new, cancel,
  replace. `C` and `R` are the only rows that carry `referredSeries/Number/Year` (6 of the 16).
* `location`: a short location code (always present). `affectedFIR`: a centre code (109 of 112).
  `fnse:icaoLocation`: the ICAO form of the location (110).
* `issued`, `effectiveStart`, `effectiveEnd`: ISO-8601 instants; `gml:beginPosition` and
  `gml:endPosition` repeat the validity. Unconfirmed: `effectiveEnd` may carry a permanent or
  estimated marker; none was seen in this sample.
* `schedule`: free text on 2 of 112 messages (unconfirmed: day/time schedule).
* `text`: the NOTAM text (always present, up to 341 bytes in this sample).
  `translation` appears twice or once per message: a formatted (`html:div`, 95) or simple
  (63) text, each with a `type` (`OTHER:ICAO` on the 95 formatted texts, `LOCAL_FORMAT` on the 63 simple ones;
  unconfirmed).
* `selectionCode` (79 of 112, a short code, sometimes with trailing white space) and
  `scenario` (always present). Unconfirmed: the Q-line selection code and the FAA scenario.
* `traffic`, `purpose`, `scope` (31 each): short letter codes (unconfirmed: the Q-line
  traffic, purpose and scope sets).
* `classification`: `INTL` (61), `DOM` (47), `MIL` (4).
* `coordinates` (72), `radius` (86), `minimumFL`/`maximumFL` (93 each),
  `lowerLimit`/`upperLimit` (9 each): the geometry and vertical limits below.
* Extension: `accountId`, `airportname` (107), `canceled` (29), `lastUpdated`, `originID` (37),
  `qline` (57), `snowtamCountryCode` (2), `xoveraccountID` and `xovernotamID` (52 each).
  Unconfirmed: `xovernotamID` links the same NOTAM in the other of the two formats (ICAO and
  local), because it is present on 52 messages and a message with it has a counterpart; the
  decoder keeps `canceled` and drops the rest of the extension values.

## Geometry

* Circles: `coordinates` (a latitude-longitude text, one point) with `radius`, a whole number
  of 1 to 3 digits (unit unconfirmed; the decoder reads it as nautical miles and stores metres).
  No `gml:Polygon` appears under the Event.
* Vertical limits: `minimumFL`/`maximumFL` are 3-digit flight levels (93 messages);
  `lowerLimit`/`upperLimit` (9 messages) are words or numbers with a reference: `SFC`, `GND`,
  `FL` plus 3 digits, `UNL`, feet `AGL` and metres `AMSL` forms. Unconfirmed which takes precedence; the decoder
  prefers `lowerLimit`/`upperLimit` and falls back to the FL pair.
* Polygons: the only polygons are under feature members, `aixm:ElevatedSurface` inside
  `RunwayElement` and `TaxiwayElement` (12 surfaces), each `gml:Polygon` with one exterior ring
  as a `gml:posList` in the EPSG:4326 URN (latitude first). No interior rings occur. The
  closing vertex repeats the first.
* Points: `aixm:ARP` on airports and `aixm:Unit` positions are `ElevatedPoint` (89); the
  decoder does not extract them.
* No `gml:Circle` / `gml:CircleByCenterPoint` and no airspace reference appears; there is no
  Airspace feature in this sample.

## Not present in this sample

`PERM` / `EST` forms of `effectiveEnd`, a `referred*` set on a non-cancel/replace row beyond
the 6 seen, `schedule` beyond 2 messages, polygons outside runway and taxiway surfaces,
interior rings and airspace features. The decoder implements the strict forms it can validate and counts everything
else as unknown; none of these is exercised by real data.

## Decoder decisions

* A message with zero or several `event:Event` members is rejected; so is one without an AIXM
  root, without an event identifier, or without a `NOTAM` element.
* One row per Event. Unknown paths and attributes are counted (with a bounded table), not
  errors. A bad coordinate or other bad field is an error bit on the row.
* The referenced NOTAM comes only from `referredSeries/Number/Year`; it is not parsed from text.
* Short codes are trimmed of white space (the sample has a trailing space on one).

## Element path tables

Counts are elements and the number of messages that contain at least one. `TS` and the other
abbreviations are defined in each heading.

#### Root and members

| Path | Elements | Messages |
|---|---:|---:|
| `(root)` | 112 | 112 |
| `/gml:boundedBy` | 37 | 37 |
| `/message:hasMember` | 249 | 112 |

#### `event:Event` (paths relative to the Event; `TS` = `event:timeSlice/event:EventTimeSlice`, `NOTAM` = `event:textNOTAM/event:NOTAM`, `EXT` = `event:extension/fnse:EventExtension`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 112 | 112 |
| `event:timeSlice` | 112 | 112 |
| `TS` | 112 | 112 |
| `TS/aixm:correctionNumber` | 112 | 112 |
| `TS/aixm:interpretation` | 112 | 112 |
| `TS/aixm:sequenceNumber` | 112 | 112 |
| `TS/event:extension` | 112 | 112 |
| `TS/EXT` | 112 | 112 |
| `TS/EXT/fnse:accountId` | 112 | 112 |
| `TS/EXT/fnse:airportname` | 107 | 107 |
| `TS/EXT/fnse:canceled` | 29 | 29 |
| `TS/EXT/fnse:classification` | 112 | 112 |
| `TS/EXT/fnse:icaoLocation` | 110 | 110 |
| `TS/EXT/fnse:lastUpdated` | 112 | 112 |
| `TS/EXT/fnse:originID` | 37 | 37 |
| `TS/EXT/fnse:qline` | 57 | 57 |
| `TS/EXT/fnse:snowtamCountryCode` | 2 | 2 |
| `TS/EXT/fnse:xoveraccountID` | 52 | 52 |
| `TS/EXT/fnse:xovernotamID` | 52 | 52 |
| `TS/event:scenario` | 112 | 112 |
| `TS/event:textNOTAM` | 112 | 112 |
| `TS/NOTAM` | 112 | 112 |
| `TS/NOTAM/event:affectedFIR` | 109 | 109 |
| `TS/NOTAM/event:coordinates` | 72 | 72 |
| `TS/NOTAM/event:effectiveEnd` | 112 | 112 |
| `TS/NOTAM/event:effectiveStart` | 112 | 112 |
| `TS/NOTAM/event:issued` | 112 | 112 |
| `TS/NOTAM/event:location` | 112 | 112 |
| `TS/NOTAM/event:lowerLimit` | 9 | 9 |
| `TS/NOTAM/event:maximumFL` | 93 | 93 |
| `TS/NOTAM/event:minimumFL` | 93 | 93 |
| `TS/NOTAM/event:number` | 112 | 112 |
| `TS/NOTAM/event:purpose` | 31 | 31 |
| `TS/NOTAM/event:radius` | 86 | 86 |
| `TS/NOTAM/event:referredNumber` | 6 | 6 |
| `TS/NOTAM/event:referredSeries` | 6 | 6 |
| `TS/NOTAM/event:referredYear` | 6 | 6 |
| `TS/NOTAM/event:schedule` | 2 | 2 |
| `TS/NOTAM/event:scope` | 31 | 31 |
| `TS/NOTAM/event:selectionCode` | 79 | 79 |
| `TS/NOTAM/event:series` | 65 | 65 |
| `TS/NOTAM/event:text` | 112 | 112 |
| `TS/NOTAM/event:traffic` | 31 | 31 |
| `TS/NOTAM/event:translation` | 158 | 112 |
| `TS/NOTAM/event:translation/event:NOTAMTranslation` | 158 | 112 |
| `TS/NOTAM/event:translation/event:NOTAMTranslation/event:formattedText` | 95 | 95 |
| `TS/NOTAM/event:translation/event:NOTAMTranslation/event:formattedText/html:div` | 95 | 95 |
| `TS/NOTAM/event:translation/event:NOTAMTranslation/event:simpleText` | 63 | 63 |
| `TS/NOTAM/event:translation/event:NOTAMTranslation/event:type` | 158 | 112 |
| `TS/NOTAM/event:type` | 110 | 110 |
| `TS/NOTAM/event:upperLimit` | 9 | 9 |
| `TS/NOTAM/event:year` | 112 | 112 |
| `TS/gml:validTime` | 112 | 112 |
| `TS/gml:validTime/gml:TimePeriod` | 112 | 112 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 112 | 112 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 112 | 112 |
| `gml:boundedBy` | 37 | 37 |
| `gml:identifier` | 112 | 112 |

#### `aixm:AirportHeliport` (relative to the member; `TS` = `aixm:timeSlice/aixm:AirportHeliportTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 74 | 74 |
| `timeSlice` | 74 | 74 |
| `TS` | 74 | 74 |
| `TS/aixm:ARP` | 71 | 71 |
| `TS/aixm:ARP/aixm:ElevatedPoint` | 71 | 71 |
| `TS/aixm:ARP/aixm:ElevatedPoint/gml:pos` | 71 | 71 |
| `TS/aixm:designator` | 74 | 74 |
| `TS/aixm:extension` | 74 | 74 |
| `TS/aixm:extension/event:AirportHeliportExtension` | 74 | 74 |
| `TS/aixm:extension/event:AirportHeliportExtension/event:theEvent` | 74 | 74 |
| `TS/aixm:interpretation` | 74 | 74 |
| `TS/aixm:locationIndicatorICAO` | 72 | 72 |
| `TS/aixm:name` | 74 | 74 |
| `TS/gml:validTime` | 74 | 74 |
| `TS/gml:validTime/gml:TimeInstant` | 74 | 74 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 74 | 74 |
| `gml:boundedBy` | 16 | 16 |
| `gml:identifier` | 74 | 74 |

#### `aixm:Navaid` (relative to the member; `TS` = `aixm:timeSlice/aixm:NavaidTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 2 | 2 |
| `timeSlice` | 4 | 2 |
| `TS` | 4 | 2 |
| `TS/aixm:availability` | 2 | 2 |
| `TS/aixm:availability/aixm:NavaidOperationalStatus` | 2 | 2 |
| `TS/aixm:availability/aixm:NavaidOperationalStatus/aixm:operationalStatus` | 2 | 2 |
| `TS/aixm:correctionNumber` | 2 | 2 |
| `TS/aixm:extension` | 2 | 2 |
| `TS/aixm:extension/event:NavaidExtension` | 2 | 2 |
| `TS/aixm:extension/event:NavaidExtension/event:theEvent` | 2 | 2 |
| `TS/aixm:interpretation` | 4 | 2 |
| `TS/aixm:runwayDirection` | 2 | 2 |
| `TS/aixm:sequenceNumber` | 2 | 2 |
| `TS/aixm:servedAirport` | 2 | 2 |
| `TS/aixm:type` | 2 | 2 |
| `TS/gml:validTime` | 4 | 2 |
| `TS/gml:validTime/gml:TimeInstant` | 2 | 2 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 2 | 2 |
| `gml:boundedBy` | 2 | 2 |
| `gml:identifier` | 2 | 2 |

#### `aixm:ObstacleArea` (relative to the member; `TS` = `aixm:timeSlice/aixm:ObstacleAreaTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 2 | 2 |
| `timeSlice` | 2 | 2 |
| `TS` | 2 | 2 |
| `TS/aixm:correctionNumber` | 2 | 2 |
| `TS/aixm:extension` | 2 | 2 |
| `TS/aixm:extension/event:ObstacleAreaExtension` | 2 | 2 |
| `TS/aixm:extension/event:ObstacleAreaExtension/event:theEvent` | 2 | 2 |
| `TS/aixm:interpretation` | 2 | 2 |
| `TS/aixm:obstacle` | 2 | 2 |
| `TS/aixm:reference_ownerAirport` | 2 | 2 |
| `TS/aixm:sequenceNumber` | 2 | 2 |
| `TS/gml:validTime` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 2 | 2 |
| `gml:boundedBy` | 2 | 2 |
| `gml:identifier` | 2 | 2 |

#### `aixm:PrimarySurveillanceRadar` (relative to the member; `TS` = `aixm:timeSlice/aixm:PrimarySurveillanceRadarTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 2 | 1 |
| `timeSlice` | 4 | 1 |
| `TS` | 4 | 1 |
| `TS/aixm:annotation` | 2 | 1 |
| `TS/aixm:annotation/aixm:Note` | 2 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:propertyName` | 2 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:purpose` | 2 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote` | 2 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote/aixm:LinguisticNote` | 2 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote/aixm:LinguisticNote/aixm:note` | 2 | 1 |
| `TS/aixm:correctionNumber` | 2 | 1 |
| `TS/aixm:extension` | 2 | 1 |
| `TS/aixm:extension/event:PrimarySurveillanceRadarExtension` | 2 | 1 |
| `TS/aixm:extension/event:PrimarySurveillanceRadarExtension/event:theEvent` | 2 | 1 |
| `TS/aixm:interpretation` | 4 | 1 |
| `TS/aixm:sequenceNumber` | 2 | 1 |
| `TS/aixm:type` | 2 | 1 |
| `TS/gml:validTime` | 4 | 1 |
| `TS/gml:validTime/gml:TimeInstant` | 2 | 1 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 2 | 1 |
| `TS/gml:validTime/gml:TimePeriod` | 2 | 1 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 2 | 1 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 2 | 1 |
| `gml:boundedBy` | 2 | 1 |
| `gml:identifier` | 2 | 1 |

#### `aixm:RadarSystem` (relative to the member; `TS` = `aixm:timeSlice/aixm:RadarSystemTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 1 | 1 |
| `timeSlice` | 1 | 1 |
| `TS` | 1 | 1 |
| `TS/aixm:airportHeliport` | 1 | 1 |
| `TS/aixm:extension` | 1 | 1 |
| `TS/aixm:extension/event:RadarSystemExtension` | 1 | 1 |
| `TS/aixm:extension/event:RadarSystemExtension/event:theEvent` | 1 | 1 |
| `TS/aixm:interpretation` | 1 | 1 |
| `TS/aixm:location` | 1 | 1 |
| `TS/aixm:location/aixm:ElevatedPoint` | 1 | 1 |
| `TS/aixm:location/aixm:ElevatedPoint/gml:pos` | 1 | 1 |
| `TS/aixm:radarEquipment` | 2 | 1 |
| `TS/aixm:radarEquipment/aixm:RadarComponent` | 2 | 1 |
| `TS/aixm:radarEquipment/aixm:RadarComponent/aixm:theRadarEquipment` | 2 | 1 |
| `TS/gml:validTime` | 1 | 1 |
| `TS/gml:validTime/gml:TimeInstant` | 1 | 1 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 1 | 1 |
| `gml:boundedBy` | 1 | 1 |
| `gml:identifier` | 1 | 1 |

#### `aixm:Runway` (relative to the member; `TS` = `aixm:timeSlice/aixm:RunwayTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 9 | 9 |
| `timeSlice` | 9 | 9 |
| `TS` | 9 | 9 |
| `TS/aixm:associatedAirportHeliport` | 4 | 4 |
| `TS/aixm:designator` | 8 | 8 |
| `TS/aixm:extension` | 9 | 9 |
| `TS/aixm:extension/event:RunwayExtension` | 9 | 9 |
| `TS/aixm:extension/event:RunwayExtension/event:theEvent` | 9 | 9 |
| `TS/aixm:interpretation` | 9 | 9 |
| `TS/gml:validTime` | 9 | 9 |
| `TS/gml:validTime/gml:TimeInstant` | 9 | 9 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 9 | 9 |
| `gml:boundedBy` | 9 | 9 |
| `gml:identifier` | 9 | 9 |

#### `aixm:RunwayDirection` (relative to the member; `TS` = `aixm:timeSlice/aixm:RunwayDirectionTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 10 | 9 |
| `timeSlice` | 17 | 9 |
| `TS` | 17 | 9 |
| `TS/aixm:availability` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:operationalStatus` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage` | 4 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage/aixm:ManoeuvringAreaUsage` | 4 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage/aixm:ManoeuvringAreaUsage/aixm:operation` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage/aixm:ManoeuvringAreaUsage/aixm:selection` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage/aixm:ManoeuvringAreaUsage/aixm:selection/aixm:ConditionCombination` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage/aixm:ManoeuvringAreaUsage/aixm:selection/aixm:ConditionCombination/aixm:logicalOperator` | 2 | 1 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:usage/aixm:ManoeuvringAreaUsage/aixm:type` | 4 | 1 |
| `TS/aixm:correctionNumber` | 7 | 6 |
| `TS/aixm:designator` | 10 | 9 |
| `TS/aixm:extension` | 17 | 9 |
| `TS/aixm:extension/event:RunwayDirectionExtension` | 12 | 9 |
| `TS/aixm:extension/event:RunwayDirectionExtension/event:theEvent` | 12 | 9 |
| `TS/aixm:extension/fns:RunwayDirectionExtension` | 5 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant` | 20 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination` | 20 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:extension` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:extension/fns:RunwaySectionContaminationExtension` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:extension/fns:RunwaySectionContaminationExtension/fns:runwayConditionCode` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:layer` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:layer/aixm:SurfaceContaminationLayer` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:layer/aixm:SurfaceContaminationLayer/aixm:type` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:observationTime` | 5 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:proportion` | 15 | 5 |
| `TS/aixm:extension/fns:RunwayDirectionExtension/fns:areaContaminant/aixm:RunwaySectionContamination/aixm:section` | 15 | 5 |
| `TS/aixm:interpretation` | 17 | 9 |
| `TS/aixm:sequenceNumber` | 7 | 6 |
| `TS/aixm:usedRunway` | 11 | 9 |
| `TS/gml:validTime` | 17 | 9 |
| `TS/gml:validTime/gml:TimeInstant` | 10 | 9 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 10 | 9 |
| `TS/gml:validTime/gml:TimePeriod` | 7 | 6 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 7 | 6 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 7 | 6 |
| `gml:boundedBy` | 10 | 9 |
| `gml:identifier` | 10 | 9 |

#### `aixm:RunwayElement` (relative to the member; `TS` = `aixm:timeSlice/aixm:RunwayElementTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 9 | 9 |
| `timeSlice` | 9 | 9 |
| `TS` | 9 | 9 |
| `TS/aixm:associatedRunway` | 9 | 9 |
| `TS/aixm:extension` | 9 | 9 |
| `TS/aixm:extension/event:RunwayElementExtension` | 9 | 9 |
| `TS/aixm:extension/event:RunwayElementExtension/event:theEvent` | 9 | 9 |
| `TS/aixm:extent` | 9 | 9 |
| `TS/aixm:extent/aixm:ElevatedSurface` | 9 | 9 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches` | 9 | 9 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch` | 9 | 9 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch/gml:exterior` | 9 | 9 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch/gml:exterior/gml:LinearRing` | 9 | 9 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch/gml:exterior/gml:LinearRing/gml:posList` | 9 | 9 |
| `TS/aixm:interpretation` | 9 | 9 |
| `TS/gml:validTime` | 9 | 9 |
| `TS/gml:validTime/gml:TimeInstant` | 9 | 9 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 9 | 9 |
| `gml:boundedBy` | 9 | 9 |
| `gml:identifier` | 9 | 9 |

#### `aixm:Taxiway` (relative to the member; `TS` = `aixm:timeSlice/aixm:TaxiwayTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 4 | 4 |
| `timeSlice` | 4 | 4 |
| `TS` | 4 | 4 |
| `TS/aixm:annotation` | 4 | 4 |
| `TS/aixm:annotation/aixm:Note` | 4 | 4 |
| `TS/aixm:annotation/aixm:Note/aixm:propertyName` | 4 | 4 |
| `TS/aixm:annotation/aixm:Note/aixm:purpose` | 4 | 4 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote` | 4 | 4 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote/aixm:LinguisticNote` | 4 | 4 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote/aixm:LinguisticNote/aixm:note` | 4 | 4 |
| `TS/aixm:associatedAirportHeliport` | 4 | 4 |
| `TS/aixm:extension` | 4 | 4 |
| `TS/aixm:extension/event:TaxiwayExtension` | 4 | 4 |
| `TS/aixm:extension/event:TaxiwayExtension/event:theEvent` | 4 | 4 |
| `TS/aixm:interpretation` | 4 | 4 |
| `TS/gml:validTime` | 4 | 4 |
| `TS/gml:validTime/gml:TimeInstant` | 4 | 4 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 4 | 4 |
| `gml:boundedBy` | 4 | 4 |
| `gml:identifier` | 4 | 4 |

#### `aixm:TaxiwayElement` (relative to the member; `TS` = `aixm:timeSlice/aixm:TaxiwayElementTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 7 | 4 |
| `timeSlice` | 11 | 4 |
| `TS` | 11 | 4 |
| `TS/aixm:annotation` | 1 | 1 |
| `TS/aixm:annotation/aixm:Note` | 1 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:purpose` | 1 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote` | 1 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote/aixm:LinguisticNote` | 1 | 1 |
| `TS/aixm:annotation/aixm:Note/aixm:translatedNote/aixm:LinguisticNote/aixm:note` | 1 | 1 |
| `TS/aixm:associatedTaxiway` | 7 | 4 |
| `TS/aixm:availability` | 4 | 4 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability` | 4 | 4 |
| `TS/aixm:availability/aixm:ManoeuvringAreaAvailability/aixm:operationalStatus` | 4 | 4 |
| `TS/aixm:correctionNumber` | 4 | 4 |
| `TS/aixm:extension` | 7 | 4 |
| `TS/aixm:extension/event:TaxiwayElementExtension` | 7 | 4 |
| `TS/aixm:extension/event:TaxiwayElementExtension/event:theEvent` | 7 | 4 |
| `TS/aixm:extent` | 3 | 2 |
| `TS/aixm:extent/aixm:ElevatedSurface` | 3 | 2 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches` | 3 | 2 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch` | 3 | 2 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch/gml:exterior` | 3 | 2 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch/gml:exterior/gml:LinearRing` | 3 | 2 |
| `TS/aixm:extent/aixm:ElevatedSurface/gml:patches/gml:PolygonPatch/gml:exterior/gml:LinearRing/gml:posList` | 3 | 2 |
| `TS/aixm:interpretation` | 11 | 4 |
| `TS/aixm:sequenceNumber` | 4 | 4 |
| `TS/gml:validTime` | 11 | 4 |
| `TS/gml:validTime/gml:TimeInstant` | 7 | 4 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 7 | 4 |
| `TS/gml:validTime/gml:TimePeriod` | 4 | 4 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 4 | 4 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 4 | 4 |
| `gml:boundedBy` | 7 | 4 |
| `gml:identifier` | 7 | 4 |

#### `aixm:Unit` (relative to the member; `TS` = `aixm:timeSlice/aixm:UnitTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 15 | 15 |
| `timeSlice` | 15 | 15 |
| `TS` | 15 | 15 |
| `TS/aixm:designator` | 15 | 15 |
| `TS/aixm:interpretation` | 15 | 15 |
| `TS/aixm:name` | 15 | 15 |
| `TS/aixm:position` | 15 | 15 |
| `TS/aixm:position/aixm:ElevatedPoint` | 15 | 15 |
| `TS/aixm:position/aixm:ElevatedPoint/gml:pos` | 15 | 15 |
| `TS/aixm:type` | 15 | 15 |
| `TS/gml:validTime` | 15 | 15 |
| `TS/gml:validTime/gml:TimeInstant` | 15 | 15 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 15 | 15 |
| `gml:identifier` | 15 | 15 |

#### `aixm:VerticalStructure` (relative to the member; `TS` = `aixm:timeSlice/aixm:VerticalStructureTimeSlice`)

| Path | Elements | Messages |
|---|---:|---:|
| `` | 2 | 2 |
| `timeSlice` | 4 | 2 |
| `TS` | 4 | 2 |
| `TS/aixm:correctionNumber` | 2 | 2 |
| `TS/aixm:extension` | 2 | 2 |
| `TS/aixm:extension/event:VerticalStructureExtension` | 2 | 2 |
| `TS/aixm:extension/event:VerticalStructureExtension/event:theEvent` | 2 | 2 |
| `TS/aixm:interpretation` | 4 | 2 |
| `TS/aixm:lighted` | 2 | 2 |
| `TS/aixm:lightingAvailability` | 2 | 2 |
| `TS/aixm:lightingAvailability/aixm:VerticalStructureLightingStatus` | 2 | 2 |
| `TS/aixm:lightingAvailability/aixm:VerticalStructureLightingStatus/aixm:status` | 2 | 2 |
| `TS/aixm:name` | 2 | 2 |
| `TS/aixm:part` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart/aixm:designator` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart/aixm:horizontalProjection_location` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart/aixm:horizontalProjection_location/aixm:ElevatedPoint` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart/aixm:horizontalProjection_location/aixm:ElevatedPoint/aixm:elevation` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart/aixm:horizontalProjection_location/aixm:ElevatedPoint/gml:pos` | 2 | 2 |
| `TS/aixm:part/aixm:VerticalStructurePart/aixm:verticalExtent` | 2 | 2 |
| `TS/aixm:sequenceNumber` | 2 | 2 |
| `TS/aixm:type` | 2 | 2 |
| `TS/gml:validTime` | 4 | 2 |
| `TS/gml:validTime/gml:TimeInstant` | 2 | 2 |
| `TS/gml:validTime/gml:TimeInstant/gml:timePosition` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod/gml:beginPosition` | 2 | 2 |
| `TS/gml:validTime/gml:TimePeriod/gml:endPosition` | 2 | 2 |
| `gml:boundedBy` | 2 | 2 |
| `gml:identifier` | 2 | 2 |

## Attributes

Element, attribute, count over all messages.

| Element | Attribute | Count |
|---|---|---:|
| `` | `gml:id` | 112 |
| `` | `xsi:schemaLocation` | 112 |
| `aixm:AirportHeliport` | `gml:id` | 74 |
| `aixm:AirportHeliportTimeSlice` | `gml:id` | 74 |
| `aixm:ConditionCombination` | `gml:id` | 2 |
| `aixm:ElevatedPoint` | `gml:id` | 89 |
| `aixm:ElevatedPoint` | `srsDimension` | 89 |
| `aixm:ElevatedPoint` | `srsName` | 89 |
| `aixm:ElevatedSurface` | `gml:id` | 12 |
| `aixm:ElevatedSurface` | `srsDimension` | 12 |
| `aixm:ElevatedSurface` | `srsName` | 12 |
| `aixm:LinguisticNote` | `gml:id` | 7 |
| `aixm:ManoeuvringAreaAvailability` | `gml:id` | 6 |
| `aixm:ManoeuvringAreaUsage` | `gml:id` | 4 |
| `aixm:Navaid` | `gml:id` | 2 |
| `aixm:NavaidOperationalStatus` | `gml:id` | 2 |
| `aixm:NavaidTimeSlice` | `gml:id` | 4 |
| `aixm:Note` | `gml:id` | 7 |
| `aixm:ObstacleArea` | `gml:id` | 2 |
| `aixm:ObstacleAreaTimeSlice` | `gml:id` | 2 |
| `aixm:PrimarySurveillanceRadar` | `gml:id` | 2 |
| `aixm:PrimarySurveillanceRadarTimeSlice` | `gml:id` | 4 |
| `aixm:RadarComponent` | `gml:id` | 2 |
| `aixm:RadarSystem` | `gml:id` | 1 |
| `aixm:RadarSystemTimeSlice` | `gml:id` | 1 |
| `aixm:Runway` | `gml:id` | 9 |
| `aixm:RunwayDirection` | `gml:id` | 10 |
| `aixm:RunwayDirectionTimeSlice` | `gml:id` | 17 |
| `aixm:RunwayElement` | `gml:id` | 9 |
| `aixm:RunwayElementTimeSlice` | `gml:id` | 9 |
| `aixm:RunwaySectionContamination` | `gml:id` | 20 |
| `aixm:RunwayTimeSlice` | `gml:id` | 9 |
| `aixm:SurfaceContaminationLayer` | `gml:id` | 15 |
| `aixm:Taxiway` | `gml:id` | 4 |
| `aixm:TaxiwayElement` | `gml:id` | 7 |
| `aixm:TaxiwayElementTimeSlice` | `gml:id` | 11 |
| `aixm:TaxiwayTimeSlice` | `gml:id` | 4 |
| `aixm:Unit` | `gml:id` | 15 |
| `aixm:UnitTimeSlice` | `gml:id` | 15 |
| `aixm:VerticalStructure` | `gml:id` | 2 |
| `aixm:VerticalStructureLightingStatus` | `gml:id` | 2 |
| `aixm:VerticalStructurePart` | `gml:id` | 2 |
| `aixm:VerticalStructureTimeSlice` | `gml:id` | 4 |
| `aixm:airportHeliport` | `xlink:href` | 1 |
| `aixm:associatedAirportHeliport` | `xlink:href` | 8 |
| `aixm:associatedRunway` | `xlink:href` | 9 |
| `aixm:associatedTaxiway` | `xlink:href` | 7 |
| `aixm:elevation` | `uom` | 2 |
| `aixm:obstacle` | `xlink:href` | 2 |
| `aixm:reference_ownerAirport` | `xlink:href` | 2 |
| `aixm:runwayDirection` | `xlink:href` | 2 |
| `aixm:servedAirport` | `xlink:href` | 2 |
| `aixm:theRadarEquipment` | `xlink:href` | 2 |
| `aixm:usedRunway` | `xlink:href` | 11 |
| `aixm:verticalExtent` | `uom` | 2 |
| `event:AirportHeliportExtension` | `gml:id` | 74 |
| `event:Event` | `gml:id` | 112 |
| `event:EventTimeSlice` | `gml:id` | 112 |
| `event:NOTAM` | `gml:id` | 112 |
| `event:NOTAMTranslation` | `gml:id` | 158 |
| `event:NavaidExtension` | `gml:id` | 2 |
| `event:ObstacleAreaExtension` | `gml:id` | 2 |
| `event:PrimarySurveillanceRadarExtension` | `gml:id` | 2 |
| `event:RadarSystemExtension` | `gml:id` | 1 |
| `event:RunwayDirectionExtension` | `gml:id` | 12 |
| `event:RunwayElementExtension` | `gml:id` | 9 |
| `event:RunwayExtension` | `gml:id` | 9 |
| `event:TaxiwayElementExtension` | `gml:id` | 7 |
| `event:TaxiwayExtension` | `gml:id` | 4 |
| `event:VerticalStructureExtension` | `gml:id` | 2 |
| `event:theEvent` | `xlink:href` | 124 |
| `fns:RunwayDirectionExtension` | `gml:id` | 5 |
| `fns:RunwaySectionContaminationExtension` | `gml:id` | 15 |
| `fnse:EventExtension` | `gml:id` | 112 |
| `gml:TimeInstant` | `gml:id` | 135 |
| `gml:TimePeriod` | `gml:id` | 131 |
| `gml:boundedBy` | `xsi:nil` | 138 |
| `gml:identifier` | `codeSpace` | 249 |

## Open questions

1. Meaning of `type` values, the `effectiveEnd` permanent and estimated forms, and `year`
   width. Next step: FAA FNS interface specification.
2. Whether `lowerLimit`/`upperLimit` or `minimumFL`/`maximumFL` takes precedence when both
   appear, and the unit of `radius`. Next step: the same specification and AIXM 5.1 UOM rules.
3. Whether `xovernotamID` pairs the ICAO and local format of one NOTAM.
4. How the member features link to the Event (`theEvent` href) and whether a message can ever
   carry several Events.
5. Whether an Airspace feature or a non-surface polygon appears in a larger sample.
