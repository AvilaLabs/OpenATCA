# TBFM metering publication: survey

Status: written 2026-11-14 from one 62-second sample of the live SWIM SCDS TBFM feed. It
describes structure only; it quotes no flight, airport, fix or arc value. The decoder
(`crates/oa-tbfm`) and `REQUIREMENTS.md` (OA-TBFM-001..018) are built on it.

## Method and what is not known

* Sample: 12,760 messages (`*.msg`) in one recording directory, 6,962,990 bytes. Message size is
  193 B to 132,077 B, mean 546 B. The `envTime` values run over 62 s (14:11:42 to 14:12:44
  UTC), about 200 messages a second across the 20 sources. The file order is the arrival order
  and is not the `envTime` order.
* Every statement about structure and counts below was measured over all 12,760 messages.
* Every statement about what a field **means** is an inference from its name, its value
  pattern and how it co-occurs with other fields. No FAA TBFM document was read for this
  survey (no network access was used), so **no meaning in this file is confirmed**; each one
  is marked "unconfirmed" and says what supports it. The FAA TBFM and SWIM TBFM-publication
  specifications are the place to settle them (open questions at the end).
* Value patterns are written with `A` for a letter and `9` for a digit, so a pattern never
  reveals a value. A "distinct" count is the number of different values in the sample.

## The feed in one page

* Every message is one XML document in the namespace
  `urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0` (declared as the default
  namespace; the root also repeats it in a `targetNamespace` attribute). All are UTF-8 with the
  XML declaration, no DTD, no CDATA, no references, no comments, no prefixes, no whitespace
  between elements.
* Two root elements: `env` (12,561 messages, the publication) and `mis` (199 messages, feed
  liveness: `hb` 198 times, `sync` once).
* `env` has attributes `envSrce` (the sending TBFM instance), `envTime` and `targetNamespace`,
  and exactly one `tma` child (12,561 of 12,561). `tma` has `msgTime` and `msgId` and exactly
  one body child: `air` (12,384), `con` (149), `adp` (15) or `oth` (13).
* `air` (the only body that is about a flight) has at most one `air` per `tma` and holds one
  or more section elements; in the sample a message carries sections of one kind only:
  `flt`, `mrp`, `eta`, `sta` or `sch`.
* The feed carries changes, not state: `airType` is `NEW`, `AMD` or `DEL`, an amendment carries
  only the sections and fields that changed, and a section that is present but whose children
  are empty appears to clear them (below).

### Sources (`envSrce`, `misSrce`)

The value has the shape `TMA.<centre>.FAA.GOV`. The 20 values are the 20 contiguous-US ARTCCs
(ZAB, ZAU, ZBW, ZDC, ZDV, ZFW, ZHU, ZID, ZJX, ZKC, ZLA, ZLC, ZMA, ZME, ZMP, ZNY, ZOA, ZOB, ZSE,
ZTL), one TBFM instance per centre. Messages per source in the sample: ZMA 1317, ZJX 1269, ZDV
877, ZBW 845, ZTL 839, ZLA 780, ZNY 771, ZDC 717, ZLC 689, ZAB 630, ZAU 556, ZME 500, ZFW 484,
ZID 430, ZHU 418, ZOB 376, ZKC 335, ZMP 332, ZOA 275, ZSE 121 (`env` messages; the heartbeats are not in these counts). Each source sends
a heartbeat every 6 s (`misTime` at :42, :48, :54 ...); 7 to 11 per source in the sample. The
first letter of `tmaId` is the same for every message of one source (one letter per centre,
not always the centre's own).

### Message kinds

| Kind | Messages | Notes |
|---|---:|---|
| `env` / `tma` / `air` | 12,384 | flight data, split below |
| `env` / `tma` / `con` | 149 | control: rates, configurations, spacing scenarios |
| `env` / `tma` / `adp` | 15 | adaptation: airports, gates, meter references, scenarios, TRACONs |
| `env` / `tma` / `oth` | 13 | other: interface status (`int`) and metering status (`tmg`) |
| `mis` / `hb` | 198 | heartbeat, no content |
| `mis` / `sync` | 1 | sync marker, no content |

`air` messages by `airType` and the section they carry:

| airType | section | messages |
|---|---|---:|
| AMD | `flt` | 6,333 |
| AMD | `eta` | 2,814 (+11 with two `eta`, +1 with three) |
| AMD | `sta` | 2,471 |
| NEW | `flt` | 224 |
| NEW | `mrp` | 135 (+3 with two, +1 with three, +1 with four) |
| NEW | `sta` | 78 |
| AMD | `mrp` | 77 (+2 with two, +1 with three) |
| NEW | `eta` | 77 |
| NEW | `sch` | 77 |
| AMD | `sch` | 17 |
| DEL | `flt` | 61 |

The `mrp` section has its own `mrpType`, which differs from the enclosing `airType` in 30 of
232 sections (NEW inside AMD 22, DEL inside AMD 8): an amendment of the flight can create or
delete one meter route. `msgId` and `msgTime` repeat on 2 to 5 consecutive messages from one
source (5,247 ids appear twice, 105 three times, 76 five times, 1 four times); in all 5,764
repeats `msgTime` and `envTime` are also equal, so a repeat looks like one TBFM update fanned
out into one SCDS message per section (unconfirmed). `msgId` has 8 digits in 11,970 messages
and 1 to 3 digits in the 591 adaptation, control and other messages.

## Every element path

Columns: the path (`air` stands for `tma/air`; `env` and `mis` are the roots), the number of
elements in the sample, the number of messages that contain it, for leaf elements the number
with text and the number that are empty (`<x/>`), the attributes (`name xcount`), and for
leaves the value pattern. Free text (`a10`, `tcr`) is not shaped.

| Path | Elements | Messages | With text | Empty | Attributes | Value pattern |
|---|---:|---:|---:|---:|---|---|
| `env` | 12561 | 12561 |  |  | `envSrce` x12561, `envTime` x12561, `targetNamespace` x12561 |  |
| `tma` | 12561 | 12561 |  |  | `msgId` x12561, `msgTime` x12561 |  |
| `tma/adp` | 15 | 15 |  |  |  |  |
| `tma/adp/apns` | 3 | 3 |  |  |  |  |
| `tma/adp/apns/apn` | 132 | 3 |  |  |  |  |
| `tma/adp/apns/apn/cfns` | 4 | 2 |  |  |  |  |
| `tma/adp/apns/apn/cfns/cfn` | 21 | 2 |  |  |  |  |
| `tma/adp/apns/apn/cfns/cfn/nam` | 21 | 2 | 21 | 0 |  | `99`, `99A` (+9 more); 17 distinct, max 12 |
| `tma/adp/apns/apn/cfns/cfn/rwys` | 21 | 2 |  |  |  |  |
| `tma/adp/apns/apn/cfns/cfn/rwys/rwy` | 30 | 2 | 30 | 0 |  | `99`, `99A` (+1 more); 12 distinct, max 3 |
| `tma/adp/apns/apn/nam` | 132 | 3 | 132 | 0 |  | `AAA`, `AAAA`; 132 distinct, max 4 |
| `tma/adp/apns/apn/rwns` | 4 | 2 |  |  |  |  |
| `tma/adp/apns/apn/rwns/rwn` | 18 | 2 |  |  |  |  |
| `tma/adp/apns/apn/rwns/rwn/nam` | 18 | 2 | 18 | 0 |  | `99`, `99A` (+2 more); 14 distinct, max 3 |
| `tma/adp/gans` | 3 | 3 |  |  |  |  |
| `tma/adp/gans/gan` | 33 | 3 |  |  |  |  |
| `tma/adp/gans/gan/nam` | 33 | 3 | 33 | 0 |  | `AAAAA_AAAA`, `AAAAA` (+8 more); 33 distinct, max 12 |
| `tma/adp/mrns` | 3 | 3 |  |  |  |  |
| `tma/adp/mrns/mrn` | 70 | 3 |  |  |  |  |
| `tma/adp/mrns/mrn/ahi` | 19 | 2 | 19 | 0 |  | `99999`; 4 distinct, max 5 |
| `tma/adp/mrns/mrn/alo` | 19 | 2 | 19 | 0 |  | `99999`, `9`; 3 distinct, max 5 |
| `tma/adp/mrns/mrn/gat` | 70 | 3 | 70 | 0 |  | `AAAAA_AAAA`, `AAAAA` (+8 more); 33 distinct, max 12 |
| `tma/adp/mrns/mrn/lan` | 51 | 3 | 51 | 0 |  | `999.9`, `99.9` (+1 more); 36 distinct, max 5 |
| `tma/adp/mrns/mrn/lat` | 70 | 3 | 70 | 0 |  | `999999A`; 42 distinct, max 7 |
| `tma/adp/mrns/mrn/lon` | 70 | 3 | 70 | 0 |  | `999999A`, `99999A`; 43 distinct, max 7 |
| `tma/adp/mrns/mrn/mfx` | 19 | 2 | 19 | 0 |  | `AAAAA`, `AAA`; 6 distinct, max 5 |
| `tma/adp/mrns/mrn/mrt` | 70 | 3 | 70 | 0 |  | `AAA`; 4 distinct, max 3 |
| `tma/adp/mrns/mrn/nam` | 70 | 3 | 70 | 0 |  | `AAAAA`, `AAA_AAA` (+9 more); 70 distinct, max 9 |
| `tma/adp/mrns/mrn/oma` | 2 | 1 | 2 | 0 |  | `AAAAA9`, `AAAAA`; 2 distinct, max 6 |
| `tma/adp/mrns/mrn/rad` | 51 | 3 | 51 | 0 |  | `999.9`, `99.9` (+2 more); 39 distinct, max 6 |
| `tma/adp/mrns/mrn/ran` | 51 | 3 | 51 | 0 |  | `999.9`, `99.9`; 37 distinct, max 5 |
| `tma/adp/scns` | 3 | 3 |  |  |  |  |
| `tma/adp/scns/scn` | 518 | 3 |  |  |  |  |
| `tma/adp/scns/scn/nam` | 518 | 3 | 518 | 0 |  | `A99A_AAA`, `AAAA_AAA` (+25 more); 518 distinct, max 18 |
| `tma/adp/tra` | 12 | 12 | 12 | 0 |  | `A99`, `AAA`; 3 distinct, max 3 |
| `tma/adp/trn` | 3 | 3 |  |  |  |  |
| `tma/adp/trn/nam` | 3 | 3 | 3 | 0 |  | `A99`, `AAA`; 3 distinct, max 3 |
| `tma/air` | 12384 | 12384 |  |  | `aid` x12384, `airType` x12384, `apt` x12384, `cid` x6929, `dap` x12378, `gufi` x12284, `tmaId` x12384 |  |
| `tma/air/eta` | 2916 | 2903 |  |  |  |  |
| `tma/air/eta/eta_dfx` | 2606 | 2593 | 2605 | 1 |  | `9999-99-99A99:99:99A`; 2306 distinct, max 20 |
| `tma/air/eta/eta_mfx` | 2606 | 2593 | 2605 | 1 |  | `9999-99-99A99:99:99A`; 2315 distinct, max 20 |
| `tma/air/eta/eta_o3a` | 119 | 119 | 42 | 77 |  | `9999-99-99A99:99:99A`; 42 distinct, max 20 |
| `tma/air/eta/eta_o4a` | 77 | 77 | 0 | 77 |  | ; 0 distinct, max 0 |
| `tma/air/eta/eta_oma` | 434 | 434 | 365 | 69 |  | `9999-99-99A99:99:99A`; 352 distinct, max 20 |
| `tma/air/eta/eta_ooa` | 239 | 239 | 166 | 73 |  | `9999-99-99A99:99:99A`; 166 distinct, max 20 |
| `tma/air/eta/eta_rwy` | 655 | 655 | 655 | 0 |  | `9999-99-99A99:99:99A`; 626 distinct, max 20 |
| `tma/air/eta/eta_sfx` | 2606 | 2593 | 2605 | 1 |  | `9999-99-99A99:99:99A`; 2306 distinct, max 20 |
| `tma/air/eta/mfx` | 2916 | 2903 | 2916 | 0 |  | `AAAAA`, `AAA` (+19 more); 475 distinct, max 7 |
| `tma/air/flt` | 6618 | 6618 |  |  |  |  |
| `tma/air/flt/a10` | 429 | 429 | 429 | 0 |  | free text (not shaped here) |
| `tma/air/flt/acs` | 439 | 439 | 439 | 0 |  | `AAAAAAAA`, `AAAAAAA` (+2 more); 7 distinct, max 8 |
| `tma/air/flt/aid` | 6618 | 6618 | 6618 | 0 |  | `AAA9999`, `AAA999` (+21 more); 1470 distinct, max 7 |
| `tma/air/flt/apt` | 6618 | 6618 | 6618 | 0 |  | `AAA`, `AAAA` (+2 more); 381 distinct, max 4 |
| `tma/air/flt/ara` | 366 | 366 | 366 | 0 |  | `99999`, `9999` (+2 more); 44 distinct, max 5 |
| `tma/air/flt/bcn` | 406 | 406 | 214 | 192 |  | `9999`; 124 distinct, max 4 |
| `tma/air/flt/cfx` | 397 | 397 | 397 | 0 |  | `AAA999999`, `AAAA` (+5 more); 200 distinct, max 12 |
| `tma/air/flt/ctm` | 497 | 497 | 497 | 0 |  | `9999-99-99A99:99:99A`; 126 distinct, max 20 |
| `tma/air/flt/dap` | 6613 | 6613 | 6613 | 0 |  | `AAA`, `AAAA` (+4 more); 456 distinct, max 4 |
| `tma/air/flt/drw` | 226 | 226 | 6 | 220 |  | `AAA_99`, `AAA_99A` (+1 more); 5 distinct, max 7 |
| `tma/air/flt/eng` | 224 | 224 | 224 | 0 |  | `AAA`, `AAAAAA` (+1 more); 3 distinct, max 10 |
| `tma/air/flt/est` | 225 | 225 | 225 | 0 |  | `AAA`; 1 distinct, max 3 |
| `tma/air/flt/etd` | 244 | 244 | 244 | 0 |  | `9999-99-99A99:99:99A`; 110 distinct, max 20 |
| `tma/air/flt/etm` | 224 | 224 | 0 | 224 |  | ; 0 distinct, max 0 |
| `tma/air/flt/fps` | 363 | 363 | 363 | 0 |  | `AAAAAAAA`, `AAAAAA` (+1 more); 4 distinct, max 9 |
| `tma/air/flt/ina` | 403 | 403 | 107 | 296 |  | `99999`, `9999`; 21 distinct, max 5 |
| `tma/air/flt/spd` | 233 | 233 | 228 | 5 |  | `999.9`, `9.99`; 105 distinct, max 5 |
| `tma/air/flt/std` | 229 | 229 | 4 | 225 |  | `9999-99-99A99:99:99A`; 4 distinct, max 20 |
| `tma/air/flt/tcr` | 221 | 221 | 221 | 0 |  | free text (not shaped here) |
| `tma/air/flt/tds` | 453 | 453 | 262 | 191 |  | `AAA`, `A99`; 38 distinct, max 3 |
| `tma/air/flt/trw` | 225 | 225 | 1 | 224 |  | `99`; 1 distinct, max 2 |
| `tma/air/flt/typ` | 246 | 246 | 246 | 0 |  | `A99A/A`, `A999/A` (+12 more); 76 distinct, max 8 |
| `tma/air/mrp` | 232 | 220 |  |  | `mrpType` x232 |  |
| `tma/air/mrp/cat` | 170 | 161 | 170 | 0 |  | `AAAAAAAAA_AA_AAAAAAAA`, `AAAAAAAAAA` (+2 more); 4 distinct, max 21 |
| `tma/air/mrp/cfg` | 79 | 79 | 79 | 0 |  | `AAAAA_99A_99A_AAA`, `9A` (+12 more); 16 distinct, max 20 |
| `tma/air/mrp/dfx` | 170 | 161 | 170 | 0 |  | `AAA`, `AAAAA` (+14 more); 104 distinct, max 7 |
| `tma/air/mrp/gat` | 170 | 161 | 170 | 0 |  | `AAA`, `AAAAA` (+19 more); 73 distinct, max 14 |
| `tma/air/mrp/mfx` | 232 | 220 | 232 | 0 |  | `AAAAA`, `AAA` (+11 more); 115 distinct, max 7 |
| `tma/air/mrp/o3a` | 170 | 161 | 0 | 170 |  | ; 0 distinct, max 0 |
| `tma/air/mrp/o4a` | 170 | 161 | 0 | 170 |  | ; 0 distinct, max 0 |
| `tma/air/mrp/oma` | 170 | 161 | 20 | 150 |  | `AAAAA9`, `AAA/AAA` (+8 more); 15 distinct, max 10 |
| `tma/air/mrp/ooa` | 170 | 161 | 9 | 161 |  | `AAAAA9`, `AAA/AAA` (+4 more); 6 distinct, max 9 |
| `tma/air/mrp/rwy` | 85 | 85 | 85 | 0 |  | `99A`, `9A` (+1 more); 18 distinct, max 3 |
| `tma/air/mrp/scn` | 171 | 162 | 171 | 0 |  | `AAA_AAA`, `AAAA_AAA` (+35 more); 124 distinct, max 17 |
| `tma/air/mrp/sfx` | 170 | 161 | 170 | 0 |  | `AAA`, `AAAAA` (+14 more); 104 distinct, max 7 |
| `tma/air/mrp/tra` | 232 | 220 | 232 | 0 |  | `AAA`, `A99`; 16 distinct, max 3 |
| `tma/air/sch` | 94 | 94 |  |  |  |  |
| `tma/air/sch/man` | 77 | 77 | 77 | 0 |  | `AA`; 1 distinct, max 2 |
| `tma/air/sch/mfx` | 94 | 94 | 94 | 0 |  | `AAAAA`, `AAAA` (+3 more); 40 distinct, max 7 |
| `tma/air/sch/rfz` | 28 | 28 | 28 | 0 |  | `AA`; 1 distinct, max 2 |
| `tma/air/sch/sfz` | 80 | 80 | 80 | 0 |  | `AA`, `AAA`; 2 distinct, max 3 |
| `tma/air/sch/sus` | 77 | 77 | 77 | 0 |  | `AA`; 1 distinct, max 2 |
| `tma/air/sta` | 2549 | 2549 |  |  |  |  |
| `tma/air/sta/mfx` | 2549 | 2549 | 2549 | 0 |  | `AAAAA`, `AAA` (+20 more); 447 distinct, max 7 |
| `tma/air/sta/sta_dfx` | 2235 | 2235 | 2182 | 53 |  | `9999-99-99A99:99:99A`; 1957 distinct, max 20 |
| `tma/air/sta/sta_o3a` | 109 | 109 | 31 | 78 |  | `9999-99-99A99:99:99A`; 31 distinct, max 20 |
| `tma/air/sta/sta_o4a` | 78 | 78 | 0 | 78 |  | ; 0 distinct, max 0 |
| `tma/air/sta/sta_oma` | 374 | 374 | 300 | 74 |  | `9999-99-99A99:99:99A`; 293 distinct, max 20 |
| `tma/air/sta/sta_ooa` | 202 | 202 | 125 | 77 |  | `9999-99-99A99:99:99A`; 125 distinct, max 20 |
| `tma/air/sta/sta_rwy` | 536 | 536 | 528 | 8 |  | `9999-99-99A99:99:99A`; 513 distinct, max 20 |
| `tma/air/sta/sta_sfx` | 2268 | 2268 | 2215 | 53 |  | `9999-99-99A99:99:99A`; 1980 distinct, max 20 |
| `tma/con` | 149 | 149 |  |  |  |  |
| `tma/con/aac` | 12 | 12 |  |  | `aacType` x12 |  |
| `tma/con/aac/apt` | 12 | 12 | 12 | 0 |  | `AAA`; 5 distinct, max 3 |
| `tma/con/aac/cfg` | 12 | 12 | 12 | 0 |  | `9A`, `99A` (+2 more); 5 distinct, max 17 |
| `tma/con/aac/tim` | 12 | 12 | 12 | 0 |  | `9999-99-99A99:99:99A`; 9 distinct, max 20 |
| `tma/con/aac/tra` | 12 | 12 | 12 | 0 |  | `A99`; 3 distinct, max 3 |
| `tma/con/aar` | 4 | 4 |  |  | `aarType` x4 |  |
| `tma/con/aar/apt` | 4 | 4 | 4 | 0 |  | `AAA`; 4 distinct, max 3 |
| `tma/con/aar/rat` | 4 | 4 | 4 | 0 |  | `9999`; 1 distinct, max 4 |
| `tma/con/aar/tim` | 4 | 4 | 4 | 0 |  | `9999-99-99A99:99:99A`; 2 distinct, max 20 |
| `tma/con/aar/tra` | 4 | 4 | 4 | 0 |  | `A99`; 2 distinct, max 3 |
| `tma/con/gar` | 27 | 27 |  |  | `garType` x27 |  |
| `tma/con/gar/apt` | 27 | 27 | 27 | 0 |  | `AAA`; 5 distinct, max 3 |
| `tma/con/gar/gat` | 27 | 27 | 27 | 0 |  | `AAAAA_AAAA`, `AAA_AAAA` (+1 more); 15 distinct, max 10 |
| `tma/con/gar/rat` | 27 | 27 | 27 | 0 |  | `9999`; 1 distinct, max 4 |
| `tma/con/gar/tim` | 27 | 27 | 27 | 0 |  | `9999-99-99A99:99:99A`; 3 distinct, max 20 |
| `tma/con/gar/tra` | 27 | 27 | 27 | 0 |  | `A99`; 3 distinct, max 3 |
| `tma/con/mar` | 76 | 76 |  |  | `marType` x76 |  |
| `tma/con/mar/apt` | 76 | 76 | 76 | 0 |  | `AAA`; 5 distinct, max 3 |
| `tma/con/mar/mfx` | 76 | 76 | 76 | 0 |  | `AAAAA`, `AAA_AAA` (+5 more); 36 distinct, max 7 |
| `tma/con/mar/rat` | 76 | 76 | 76 | 0 |  | `9999`; 1 distinct, max 4 |
| `tma/con/mar/tim` | 76 | 76 | 76 | 0 |  | `9999-99-99A99:99:99A`; 3 distinct, max 20 |
| `tma/con/mar/tra` | 76 | 76 | 76 | 0 |  | `A99`; 3 distinct, max 3 |
| `tma/con/rar` | 20 | 20 |  |  | `rarType` x20 |  |
| `tma/con/rar/apt` | 20 | 20 | 20 | 0 |  | `AAA`; 5 distinct, max 3 |
| `tma/con/rar/rat` | 20 | 20 | 20 | 0 |  | `9999`; 1 distinct, max 4 |
| `tma/con/rar/rwy` | 20 | 20 | 20 | 0 |  | `99`, `99A` (+2 more); 16 distinct, max 3 |
| `tma/con/rar/tim` | 20 | 20 | 20 | 0 |  | `9999-99-99A99:99:99A`; 5 distinct, max 20 |
| `tma/con/rar/tra` | 20 | 20 | 20 | 0 |  | `A99`; 3 distinct, max 3 |
| `tma/con/sac` | 4 | 4 |  |  | `sacType` x4 |  |
| `tma/con/sac/saps` | 4 | 4 |  |  |  |  |
| `tma/con/sac/saps/sap` | 122 | 4 |  |  |  |  |
| `tma/con/sac/saps/sap/san` | 122 | 4 | 122 | 0 |  | `AAA`, `AAAA`; 75 distinct, max 4 |
| `tma/con/sac/saps/sap/scf` | 122 | 4 | 122 | 0 |  | `AAA_99`, `AAA_99A`; 75 distinct, max 7 |
| `tma/con/sac/tim` | 4 | 4 | 4 | 0 |  | `9999-99-99A99:99:99A`; 2 distinct, max 20 |
| `tma/con/sac/tra` | 4 | 4 | 4 | 0 |  | `A99`, `AAA`; 4 distinct, max 3 |
| `tma/con/scc` | 4 | 4 |  |  | `sccType` x4 |  |
| `tma/con/scc/sscs` | 4 | 4 |  |  |  |  |
| `tma/con/scc/sscs/ssc` | 225 | 4 |  |  | `sscType` x225 |  |
| `tma/con/scc/sscs/ssc/ccs` | 180 | 3 |  |  |  |  |
| `tma/con/scc/sscs/ssc/ccs/cc` | 1008 | 3 |  |  | `ccType` x1008 |  |
| `tma/con/scc/sscs/ssc/ccs/cc/apreq` | 1008 | 3 | 1008 | 0 |  | `AAA`, `AAAA`; 2 distinct, max 4 |
| `tma/con/scc/sscs/ssc/ccs/cc/apt` | 1008 | 3 | 1008 | 0 |  | `AAAA`, `AAA`; 28 distinct, max 4 |
| `tma/con/scc/sscs/ssc/ccs/cc/apt_icao` | 1008 | 3 | 1008 | 0 |  | `AAAA`; 28 distinct, max 4 |
| `tma/con/scc/sscs/ssc/ccs/cc/sch` | 1008 | 3 | 1008 | 0 |  | `AAAAAA`; 1 distinct, max 6 |
| `tma/con/scc/sscs/ssc/scls` | 219 | 4 |  |  |  |  |
| `tma/con/scc/sscs/ssc/scls/scl` | 528 | 4 |  |  | `sclType` x528 |  |
| `tma/con/scc/sscs/ssc/scls/scl/scmre` | 528 | 4 | 528 | 0 |  | `AAAA`, `AAA` (+4 more); 45 distinct, max 7 |
| `tma/con/scc/sscs/ssc/scls/scl/scname` | 528 | 4 | 528 | 0 |  | `A99A_AAA`, `AAAA_AAA` (+26 more); 528 distinct, max 18 |
| `tma/con/scc/sscs/ssc/sscname` | 219 | 4 | 219 | 0 |  | `AAA_AAAA`, `AAA_AAA` (+28 more); 219 distinct, max 18 |
| `tma/con/scc/sscs/ssc/ssd` | 219 | 4 | 219 | 0 |  | `99.9`, `9.9`; 9 distinct, max 4 |
| `tma/con/scc/sscs/ssc/ssmin` | 219 | 4 | 219 | 0 |  | `9`; 1 distinct, max 1 |
| `tma/con/scc/sscs/ssc/ssn` | 225 | 4 | 225 | 0 |  | `9999`, `999` (+2 more); 205 distinct, max 4 |
| `tma/con/scc/sscs/ssc/sstyp` | 219 | 4 | 219 | 0 |  | `AAA_AAAAA_AA_AAAAA`; 1 distinct, max 18 |
| `tma/con/scc/tim` | 4 | 4 | 4 | 0 |  | `9999-99-99A99:99:99A`; 4 distinct, max 20 |
| `tma/con/scc/tra` | 4 | 4 | 4 | 0 |  | `A99`, `AAA`; 4 distinct, max 3 |
| `tma/con/tar` | 2 | 2 |  |  | `tarType` x2 |  |
| `tma/con/tar/rat` | 2 | 2 | 2 | 0 |  | `9999`; 1 distinct, max 4 |
| `tma/con/tar/tim` | 2 | 2 | 2 | 0 |  | `9999-99-99A99:99:99A`; 1 distinct, max 20 |
| `tma/con/tar/tra` | 2 | 2 | 2 | 0 |  | `A99`; 2 distinct, max 3 |
| `tma/oth` | 13 | 13 |  |  |  |  |
| `tma/oth/int` | 10 | 10 |  |  |  |  |
| `tma/oth/int/ifm` | 10 | 10 | 10 | 0 |  | `AA`; 1 distinct, max 2 |
| `tma/oth/int/ifn` | 10 | 10 | 10 | 0 |  | `AAA`, `A99`; 10 distinct, max 3 |
| `tma/oth/int/ifs` | 10 | 10 | 10 | 0 |  | `AA`; 1 distinct, max 2 |
| `tma/oth/int/ift` | 10 | 10 | 10 | 0 |  | `AAAA`; 4 distinct, max 4 |
| `tma/oth/tmg` | 3 | 3 |  |  | `tmgType` x3 |  |
| `tma/oth/tmg/tmss` | 3 | 3 |  |  |  |  |
| `tma/oth/tmg/tmss/tms` | 1259 | 3 |  |  |  |  |
| `tma/oth/tmg/tmss/tms/apt` | 1259 | 3 | 1259 | 0 |  | `AAA`, `AAAA`; 132 distinct, max 4 |
| `tma/oth/tmg/tmss/tms/ctr` | 24 | 2 | 24 | 0 |  | `AAA`; 6 distinct, max 3 |
| `tma/oth/tmg/tmss/tms/mas` | 1259 | 3 | 1259 | 0 |  | `AA`; 1 distinct, max 2 |
| `tma/oth/tmg/tmss/tms/mfx` | 1235 | 1 | 1235 | 0 |  | `AAAA`, `AAA` (+2 more); 21 distinct, max 7 |
| `tma/oth/tmg/tra` | 3 | 3 | 3 | 0 |  | `A99`, `AAA`; 3 distinct, max 3 |
| `mis` | 199 | 199 |  |  | `misSrce` x199, `misTime` x199 |  |
| `mis/hb` | 198 | 198 | 0 | 198 |  | ; 0 distinct, max 0 |
| `mis/sync` | 1 | 1 | 0 | 1 |  | ; 0 distinct, max 0 |

## Every attribute

| Attribute | Count | Value pattern |
|---|---:|---|
| `env@envSrce` | 12561 | `AAA.AAA.AAA.AAA`; 20 distinct, max 15 |
| `env@envTime` | 12561 | `9999-99-99A99:99:99.999A`; 3861 distinct, max 24 |
| `env@targetNamespace` | 12561 | `AAA:AA:AAA:AAA:AAA:AAA:AAA:AAAAAAAAAAAAAAAAAAAAAAA:9.9.9`; 1 distinct, max 56 |
| `tma@msgId` | 12561 | `99999999`, `999`, `99`; 6797 distinct, max 8 |
| `tma@msgTime` | 12561 | `9999-99-99A99:99:99.999A`; 3862 distinct, max 24 |
| `tma/air@aid` | 12384 | `AAA9999`, `AAA999`, `A999AA`; 1471 distinct, max 7 |
| `tma/air@airType` | 12384 | `AAA`; 3 distinct, max 3 |
| `tma/air@apt` | 12384 | `AAA`, `AAAA`, `9A9`; 381 distinct, max 4 |
| `tma/air@cid` | 6929 | `999`, `99A`; 886 distinct, max 3 |
| `tma/air@dap` | 12378 | `AAA`, `AAAA`, `A99`; 456 distinct, max 4 |
| `tma/air@gufi` | 12284 | `AA999999AA`, `AA9999999A`, `AA999999A9`; 1456 distinct, max 10 |
| `tma/air@tmaId` | 12384 | `A99999`; 2612 distinct, max 6 |
| `tma/air/mrp@mrpType` | 232 | `AAA`; 3 distinct, max 3 |
| `tma/con/aac@aacType` | 12 | `AAA`; 1 distinct, max 3 |
| `tma/con/aar@aarType` | 4 | `AAA`; 1 distinct, max 3 |
| `tma/con/gar@garType` | 27 | `AAA`; 1 distinct, max 3 |
| `tma/con/mar@marType` | 76 | `AAA`; 1 distinct, max 3 |
| `tma/con/rar@rarType` | 20 | `AAA`; 1 distinct, max 3 |
| `tma/con/sac@sacType` | 4 | `AAA`; 1 distinct, max 3 |
| `tma/con/scc@sccType` | 4 | `AAA`; 1 distinct, max 3 |
| `tma/con/scc/sscs/ssc@sscType` | 225 | `AAA`; 2 distinct, max 3 |
| `tma/con/scc/sscs/ssc/ccs/cc@ccType` | 1008 | `AAA`; 1 distinct, max 3 |
| `tma/con/scc/sscs/ssc/scls/scl@sclType` | 528 | `AAA`; 1 distinct, max 3 |
| `tma/con/tar@tarType` | 2 | `AAA`; 1 distinct, max 3 |
| `tma/oth/tmg@tmgType` | 3 | `AAA`; 1 distinct, max 3 |
| `mis@misSrce` | 199 | `AAA.AAA.AAA.AAA`; 20 distinct, max 15 |
| `mis@misTime` | 199 | `9999-99-99A99:99:99A`; 61 distinct, max 20 |

## How often each field appears

For each section, the share of its elements that contain the field (it can appear empty).
The air attributes are in the attribute table: `aid`, `airType`, `apt` and `tmaId` on every
`air` (12,384), `dap` on 12,378, `gufi` on 12,284, `cid` on 6,929.

| Section | Field | Elements | Share of sections | With text | Empty |
|---|---|---:|---:|---:|---:|
| `flt` | `aid` | 6618 | 100.0% | 6618 | 0 |
| `flt` | `apt` | 6618 | 100.0% | 6618 | 0 |
| `flt` | `dap` | 6613 | 99.9% | 6613 | 0 |
| `flt` | `ctm` | 497 | 7.5% | 497 | 0 |
| `flt` | `tds` | 453 | 6.8% | 262 | 191 |
| `flt` | `acs` | 439 | 6.6% | 439 | 0 |
| `flt` | `a10` | 429 | 6.5% | 429 | 0 |
| `flt` | `bcn` | 406 | 6.1% | 214 | 192 |
| `flt` | `ina` | 403 | 6.1% | 107 | 296 |
| `flt` | `cfx` | 397 | 6.0% | 397 | 0 |
| `flt` | `ara` | 366 | 5.5% | 366 | 0 |
| `flt` | `fps` | 363 | 5.5% | 363 | 0 |
| `flt` | `typ` | 246 | 3.7% | 246 | 0 |
| `flt` | `etd` | 244 | 3.7% | 244 | 0 |
| `flt` | `spd` | 233 | 3.5% | 228 | 5 |
| `flt` | `std` | 229 | 3.5% | 4 | 225 |
| `flt` | `drw` | 226 | 3.4% | 6 | 220 |
| `flt` | `trw` | 225 | 3.4% | 1 | 224 |
| `flt` | `est` | 225 | 3.4% | 225 | 0 |
| `flt` | `eng` | 224 | 3.4% | 224 | 0 |
| `flt` | `etm` | 224 | 3.4% | 0 | 224 |
| `flt` | `tcr` | 221 | 3.3% | 221 | 0 |
| `mrp` | `tra` | 232 | 100.0% | 232 | 0 |
| `mrp` | `mfx` | 232 | 100.0% | 232 | 0 |
| `mrp` | `scn` | 171 | 73.7% | 171 | 0 |
| `mrp` | `gat` | 170 | 73.3% | 170 | 0 |
| `mrp` | `dfx` | 170 | 73.3% | 170 | 0 |
| `mrp` | `sfx` | 170 | 73.3% | 170 | 0 |
| `mrp` | `oma` | 170 | 73.3% | 20 | 150 |
| `mrp` | `ooa` | 170 | 73.3% | 9 | 161 |
| `mrp` | `o3a` | 170 | 73.3% | 0 | 170 |
| `mrp` | `o4a` | 170 | 73.3% | 0 | 170 |
| `mrp` | `cat` | 170 | 73.3% | 170 | 0 |
| `mrp` | `rwy` | 85 | 36.6% | 85 | 0 |
| `mrp` | `cfg` | 79 | 34.1% | 79 | 0 |
| `eta` | `mfx` | 2916 | 100.0% | 2916 | 0 |
| `eta` | `eta_mfx` | 2606 | 89.4% | 2605 | 1 |
| `eta` | `eta_dfx` | 2606 | 89.4% | 2605 | 1 |
| `eta` | `eta_sfx` | 2606 | 89.4% | 2605 | 1 |
| `eta` | `eta_rwy` | 655 | 22.5% | 655 | 0 |
| `eta` | `eta_oma` | 434 | 14.9% | 365 | 69 |
| `eta` | `eta_ooa` | 239 | 8.2% | 166 | 73 |
| `eta` | `eta_o3a` | 119 | 4.1% | 42 | 77 |
| `eta` | `eta_o4a` | 77 | 2.6% | 0 | 77 |
| `sta` | `mfx` | 2549 | 100.0% | 2549 | 0 |
| `sta` | `sta_sfx` | 2268 | 89.0% | 2215 | 53 |
| `sta` | `sta_dfx` | 2235 | 87.7% | 2182 | 53 |
| `sta` | `sta_rwy` | 536 | 21.0% | 528 | 8 |
| `sta` | `sta_oma` | 374 | 14.7% | 300 | 74 |
| `sta` | `sta_ooa` | 202 | 7.9% | 125 | 77 |
| `sta` | `sta_o3a` | 109 | 4.3% | 31 | 78 |
| `sta` | `sta_o4a` | 78 | 3.1% | 0 | 78 |
| `sch` | `mfx` | 94 | 100.0% | 94 | 0 |
| `sch` | `sfz` | 80 | 85.1% | 80 | 0 |
| `sch` | `sus` | 77 | 81.9% | 77 | 0 |
| `sch` | `man` | 77 | 81.9% | 77 | 0 |
| `sch` | `rfz` | 28 | 29.8% | 28 | 0 |

Reading the table:

* `flt`: `aid`, `apt` (and `dap`, 99.9%) repeat the `air` attributes of the same names and
  agree with them in every one of the 6,618 sections. Nearly all `flt` sections carry only
  those three (5,278 of 6,618), so they are the "this flight changed" signal; the other 20
  fields appear when changed, 219 sections carry all of them.
* `mrp`: `tra` and `mfx` always; the other ten in 73% of sections (the same 170); `rwy` in 37%
  and `cfg` in 34%. `o3a` and `o4a` are present in all 170 and empty in all 170: the arcs three
  and four are never filled in this sample.
* `eta`/`sta`: `mfx` always; the delay-fix, scheduling-fix and (for `eta`) meter-fix times in
  about 89%; the runway time in 21 to 23%; the arcs in descending order. One `eta` has an empty
  `eta_dfx`, `eta_sfx` and `eta_mfx` each; 53 `sta` have an empty `sta_dfx` and 53 an empty `sta_sfx`.
* `sch`: `mfx` always; `sfz`, `sus`, `man` in 82 to 85%, `rfz` in 30%.
* Empty elements are common for some fields (`std` 225 of 229, `trw` 224 of 225, `drw` 220 of
  226, `ina` 296 of 403, `bcn` 192 of 406, `tds` 191 of 453, `etm` 224 of 224) and rare for
  others (`spd` 5 of 233, `sta_rwy` 8 of 536). That a field is empty rather than absent
  is information the decoder keeps (`Row::cleared`): in a change feed an empty element appears
  to say "this value is now cleared" (unconfirmed).

## What each field appears to mean

All meanings are **unconfirmed** (see "Method"). The evidence column says what supports each.

### Envelope

| Field | Appears to mean | Evidence |
|---|---|---|
| `envSrce` | the TBFM instance that sent the message, one per ARTCC | 20 values, each a Z-ARTCC identifier |
| `envTime` | time the envelope was produced, UTC, millisecond resolution | equal to `msgTime` to within 1 ms in all 12,561 messages |
| `msgTime` | time of the TBFM update | as above; repeated across the split messages of one update |
| `msgId` | sequence number of the update, per source | 8 digits for flight data, small numbers for adaptation/control/other; repeated across split messages |
| `misSrce`, `misTime` | liveness source and time, whole seconds | every 6 s per source |
| `hb`, `sync` | heartbeat; a marker (resynchronisation?) | `sync` appears once, in one source, with no content |

### `air`

| Field | Appears to mean | Evidence |
|---|---|---|
| `aid` | aircraft identification (the callsign) | shape of airline code and digits, max 7, equals `flt/aid` |
| `gufi` | globally unique flight identifier (the FIXM GUFI, as in SFDPS) | 10 characters, mixed case, 1,456 distinct, absent on 100 of 12,384 messages |
| `cid` | computer identification of the flight in the centre's host computer | 3 characters, present on 56% of messages |
| `tmaId` | TBFM's own id for the flight | 6 characters, 2,612 distinct, first letter fixed per source |
| `apt` | the arrival (metered) airport | metered (`mrp`) sections with category `ARRIVAL` carried a runway of that airport in the messages inspected by hand; always present |
| `dap` | the departure airport | absent on 6 messages; `flt` repeats it |
| `airType` | change type: `NEW` creates, `AMD` amends, `DEL` removes | 3 values |

### `flt`: flight data

| Field | Appears to mean | Evidence |
|---|---|---|
| `fps` | flight plan status: `ACTIVE`, `DEPARTED`, `ESTIMATED`, `PROPOSED` | word values |
| `acs` | aircraft (track) state: `ACTIVE`, `DROPPED`, `INACTIVE`, `LANDED`, `LOST`, `PROPOSED`, `TRACKED` | word values |
| `typ` | aircraft type with equipment suffix | shape `A999/A`, 76 distinct |
| `eng` | engine class: `JET`, `PISTON`, `TURBO_PROP` | word values |
| `bcn` | transponder (beacon) code | 4 digits, only 0 to 7 (octal), 192 of 406 empty |
| `spd` | speed, one decimal (`999.9`), or two decimals (`9.99`, 13 times, which looks like a Mach number) | unit not stated: the two forms suggest knots and Mach |
| `ara` | an altitude-like value: 4 or 5 digits, one digit (twice), or a 3-letter word (4 times) | the word looks like a visual-flight marker; role unknown |
| `ina` | an altitude-like value, 4 to 5 digits | role unknown (interim, initial, assigned?) |
| `trw` | runway-like number, 2 digits (one value in the sample) | role unknown |
| `drw` | a runway tied to an airport, shape `AAA_99A` (a trio, underscore, runway) | looks like departure runway; 220 of 226 empty |
| `tds` | a facility identifier (centre or TRACON shape) | 3 characters; role unknown (next facility? departure sector?) |
| `cfx` | coordination fix with a slash form in places | 12 characters max; shape fix name with optional digits |
| `ctm` | coordination time | ISO time without fraction |
| `etd` | estimated departure time | name and ISO time |
| `std` | scheduled/standard departure time | 4 of 229 non-empty |
| `etm`, `est` | `est` is always the same 3-letter word (an estimate source?); `etm` is empty every time | unknown |
| `a10` | a route string (dot-separated fixes and airways) | free text with dots, slashes |
| `tcr` | a list of `fix/time` pairs separated by spaces | free text |

### `mrp`: meter route (the flight's assignment to meter points)

| Field | Appears to mean | Evidence |
|---|---|---|
| `mrpType` | change type of this meter route (`NEW`, `AMD`, `DEL`) | 3 values; differs from `airType` in 30 sections |
| `tra` | the TRACON (or other facility) doing the metering | 3 characters, 16 distinct; the adaptation `tra` lists the centre's TRACONs |
| `mfx` | meter fix, the fix at which the flight is metered | also the name of `mrt` = `MFX` adaptation entries; see "Names that are not published fixes" |
| `dfx`, `sfx` | two further fix names tied to the route (delay fix? scheduling fix?) | `eta_dfx` equals `eta_sfx` in all 2,606 `eta` that carry both |
| `oma`, `ooa`, `o3a`, `o4a` | the names of the outer meter arc and outer arcs (three, four) | adaptation `mrt` types `OMA` and `OOA`; `eta_oma`/`sta_oma` times exist |
| `gat` | the arrival gate (stream) | similar shapes to the adaptation `gans/gan/nam`, 73 distinct |
| `rwy` | landing runway | shape `99A`, 18 distinct; present on 37% |
| `cfg` | the airport's runway configuration name | similar shapes to the adaptation `cfn/nam` |
| `cat` | flight category: `ARRIVAL`, `OVERFLIGHT`, `SATELLITE_ARRIVAL`, `SATELLITE_TO_EXTERNAL` | word values |
| `scn` | the spacing scenario name | similar shapes to the adaptation `scns/scn/nam` |

### `eta`, `sta`, `sch`

| Field | Appears to mean | Evidence |
|---|---|---|
| `eta` | estimated times of arrival at the meter points | ISO times, whole seconds |
| `sta` | scheduled times of arrival at the meter points | as above; `sta` exists only in the sample, with no `sta_mfx` |
| `mfx` (in `eta`/`sta`/`sch`) | the meter fix the section is about | same shape as `mrp/mfx`; the sections never share a message, so equality is not testable |
| `*_dfx`, `*_sfx` | time at the delay fix / at the scheduling fix | `eta_dfx` = `eta_sfx` in 2,606 of 2,606; `sta_dfx` = `sta_sfx` in 2,234 of 2,234 |
| `eta_mfx` | time at the meter fix | differs from `eta_sfx` in 259 of 2,571 pairs, by -210 s to +30 s (`sfx` minus `mfx`) |
| `*_oma`, `*_ooa`, `*_o3a`, `*_o4a` | time at the arcs | `eta_o4a` and `sta_o4a` are always empty |
| `*_rwy` | time at the runway threshold | present on 21 to 23% of sections |
| `sch/man` | manual-schedule flag | `YES`/`NO`, always `NO` in the sample |
| `sch/rfz` | runway frozen | always `NO` |
| `sch/sfz` | STA frozen (scheduled time frozen) | `YES` 3 times, `NO` 77: the only frozen/unfrozen signal in the feed |
| `sch/sus` | suspended | always `NO` |

### `adp`, `con`, `oth`

Not decoded to rows (the decoder classifies and counts them). Their structure is in the path
table. Appearance only: `adp` lists the centre's airports (`apns`, each with configurations
`cfns` and runways `rwns`), gates (`gans`), meter references (`mrns`: name, a type word
`MFX`/`MFA`/`OMA`/`OOA`, latitude and longitude as six digits and a hemisphere letter (one
longitude has five digits), radii and ranges), spacing scenarios (`scns`) and TRACONs (`tra`, `trn`). `con` holds the control
state: acceptance and arrival rates (`aar`, `gar`, `mar`, `rar`, `tar`, each with a
`rat`, a `tim` and a `tra`, and mostly an `apt`, with a gate, meter fix or runway in `gar`,
`mar` and `rar`), configuration changes (`aac`, `sac`) and the
miles-in-trail scenario catalogue (`scc`). `oth` holds interface status (`int`: a facility
name, a type among `ERAM`, `STAR`, `TFMS`, `WXIF`, and `UP`/`DN` state words) and a list of
per-airport metering status entries (`tmg`).

## Times

* Format: `YYYY-MM-DDTHH:MM:SS[.mmm]Z`, always UTC with `Z`. `envTime` and `msgTime` have
  exactly three fraction digits; every time inside `air` has none; `misTime` has none.
* In `air` the times run from 80 minutes before `envTime` to 17 hours after (`eta`), with a
  median about an hour ahead. `sta` times are in the same range, at most 4.8 hours ahead.
* One `1970-01-01T00:00:00Z` appears, in the control body (`scc/tim`), as an unset marker; none
  in `air`.
* The decoder returns every time as an `i64` of Unix milliseconds from a strict parser.

## Names that are not published fixes

Of the meter fixes sent by the centre whose snapshot was available (21 distinct), 12 are not
in the airspace model and every one of the 12 ends in `MP`: they look like adaptation names
for meter points (TBFM objects), not published fix identifiers. Others are mixed case and
contain lower-case letters (`mfx`, `dfx`, `sfx`, `scn`, `gat` all have mixed-case values), so
the name alphabet is printable ASCII, not upper-case letters and digits. These cannot be
resolved against NASR or CIFP by identifier (see OA-TBFM-017).

## Column set (what the decoder keeps and why)

One row per section element of an `air` (`flt`, `mrp`, `eta`, `sta`, `sch`), because a message
never mixes kinds in the sample and each section has its own field set; merging sections of
one flight is the consumer's job (it needs message order, which the feed owns). Each row
carries:

* **Envelope and message**: centre (from `envSrce`), `envTime`, `msgTime`, `msgId`, section
  kind, `airType`, and for `mrp` its own `mrpType`. They are needed to order, de-duplicate and
  judge latency (UNKNOWNS UK-004).
* **Flight identity**: `aid`, `gufi`, `cid`, `tmaId`, `apt`, `dap`. `gufi` is the link to the
  SFDPS flight-plan feed; `tmaId` is the only key the feed has on the 100 messages without a
  `gufi`; `cid` is the host computer id.
* **Meter route** (`mrp`): `tra`, `mfx`, `dfx`, `sfx`, the four arcs, `gat`, `rwy`, `cfg`,
  `cat`, `scn`.
* **Times** (`eta`, `sta`): `mfx` plus eight `eta_ms` and eight `sta_ms` slots (delay fix,
  scheduling fix, meter fix, four arcs, runway); the `sta` meter-fix slot is never filled.
* **Flags** (`sch`): `man`, `rfz`, `sfz`, `sus` as two bits each. `sfz` is the frozen state.
* **Flight data** (`flt`): `fps`, `acs`, `typ`, `eng`, `bcn`, `spd`, `ara`, `ina`, `trw`,
  `drw`, `tds`, `cfx`, `ctm`, `etd`, `std`.
* **`cleared`**: a bit per field that was sent empty (see above).
* **Not stored**: `a10` and `tcr` (long free text, no use in metering), `est` and `etm`
  (constant and always empty), and everything under `adp`, `con` and `oth`.

Widths of the name columns are the survey maxima plus headroom: aid 8 (max 7), airport 4 (4),
fix 8 (7), arcs 12 (10), gate 16 (14), configuration 20 (20), scenario 20 (17), category 24
(21), gufi 12 (10), tmaId 8 (6), cid 4 (3), cfx 14 (12), type 8 (8), status words 12 (9), engine 12
(10). A longer value is a field error: the message is refused, not truncated.

## Open questions (for the FAA TBFM and SWIM documents)

1. The meaning of every field above; in particular `dfx`, `sfx`, `ara`, `ina`, `trw`, `drw`,
   `tds`, `std`, `etm`, `est`, `eta_*` against `sta_*` (is a scheduled time the one a
   controller sees?), and the units of `spd`.
2. Whether an empty element in an amendment clears the value, and whether an absent one
   leaves it unchanged (the decoder keeps both facts; the consumer needs the rule).
3. Whether `NEW` for `mrp`/`flt` after `AMD` for the flight is a new meter route or a replay,
   and how `DEL` of a `flt` relates to the flight's other sections.
4. Why `msgId`/`msgTime` repeat over several messages (one update per section?) and whether
   the repeats are ever out of order across sources.
5. Whether SCDS delays TBFM data (UNKNOWNS UK-004 measured 7 to 9 s after `envTime`).
6. The full vocabularies of `fps`, `acs`, `eng` and `cat` (the sample shows 4, 7, 3 and 4
   values; the decoder stores them as text, so a new word is not a reject).
7. How the `MP` names and the mixed-case names relate to published fixes, if at all.
8. What `sync` means and when it is sent.
