# ADS-B reports in OAF1 frames (ADS2, reads ADS1)

Status: adopted 2026-11-14; `ADS2` (measurements) 2026-11-14. Written by `oa-adsb-clip` (crate `oa-adsb`), read by
`oa_adsb::decode_payload`. Source data: adsb.lol `globe_history` day archives (readsb trace
files, ODbL); the format of those is described in the `oa-adsb` crate documentation, with the
readsb commit it was read from.

## Why OAF1

`oa-replay` merges feeds that arrive as OAF1 frame streams (`docs/FEED_FRAMES.md`). Reusing the
frame (magic, length, CRC-32, sequence) means a clipped ADS-B history is checked, resynchronised
and recorded the same way as SWIM feeds, and can be merged by receive time later. A frame stream
of ADS-B reports is a feed of its own kind: one file per feed, as with SFDPS, TBFM and TFMS.

## Frame use

* `kind` = 1 (MESSAGE), `flags` = 2 (`FLAG_BYTES`: the payload is binary, not text).
* `seq` counts frames from 0 in the file.
* `recv_unix_ns` is the time of the **first report in the payload**, Unix nanoseconds (these are
  history frames: there is no receive wall clock); `jms_timestamp_ms` is the same in
  milliseconds.
* Reports are sorted by `(time, address)` across the whole file, so `recv_unix_ns` never
  decreases from one frame to the next.
* A frame holds the reports from its first report up to, but not including, the first report
  that is at least `--frame-ms` (default 1000) after it, so each frame spans under one second
  of report time by default and `oa-replay` merges ADS-B with the SWIM feeds at that
  granularity. A frame also ends at 60 000 reports (header plus records stay under the 4 MiB
  payload limit; 50 000 for `ADS2`, 60 000 for `ADS1`). Readers do not depend on this: a clip with any grouping, such as the earlier
  one of 60 000 reports per frame, decodes the same.

## Versions

* `ADS1`: 64-byte records, at most 60 000 per frame. Written by the first version of
  `oa-adsb-clip`; bytes 57 to 63 of a record are reserved and zero.
* `ADS2`: 80-byte records, at most 50 000 per frame (16 + 80 * 50 000 stays under the 4 MiB
  payload limit). Bytes 0 to 56 are the `ADS1` record unchanged; bytes 57 to 78 add the
  measurements below; byte 79 is reserved and zero. Only `ADS2` is written now.

`decode_payload` reads both, by magic; an `ADS1` record decodes with every measurement absent.
The OAF1 frame, its sequence and its time use are identical, so `oa-replay` and `oa-truth`
(which decode through `decode_payload` and use none of the measurements) see the same reports from an old clip as before,
and the hashes of their tables do not change. A reader that knows only `ADS1` refuses an `ADS2`
payload by its magic.

## Payload

All integers little-endian. A 16-byte header, then `count` records.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic, ASCII `ADS2` (or `ADS1`) |
| 4 | 2 | record length, 80 (64 for `ADS1`) |
| 6 | 2 | reserved, 0 |
| 8 | 4 | count, u32, at most 50 000 (60 000 for `ADS1`) |
| 12 | 4 | reserved, 0 |

The payload length is exactly `16 + record length * count`.

Record (offsets from the start of the record):

| Offset | Size | Field | Unit and absent value |
|---|---|---|---|
| 0 | 4 | icao, u32 | 24-bit address; bit 24 set when the address is not an ICAO address (readsb `~`) |
| 4 | 4 | lat, i32 | 1e-7 degree |
| 8 | 4 | lon, i32 | 1e-7 degree |
| 12 | 4 | baro altitude, i32 | feet; `-2147483648` absent (on the ground, geometric-only, unknown) |
| 16 | 4 | geometric altitude, i32 | feet; `-2147483648` absent |
| 20 | 4 | vertical rate, i32 | feet per minute (geometric if flag bit 2); `-2147483648` absent |
| 24 | 8 | time, i64 | Unix nanoseconds (the source has 10 ms resolution) |
| 32 | 2 | ground speed, u16 | 0.1 kt; `65535` absent |
| 34 | 2 | track, u16 | 0.1 degree, 0 to 3599 (true heading on the ground); `65535` absent |
| 36 | 2 | squawk, u16 | the four octal digits read as a decimal number (`7700` is 7700); `65535` absent |
| 38 | 1 | flags, u8 | bit 0 stale, bit 1 new leg, bit 2 vertical rate is geometric, bit 3 the altitude in the source row was geometric, bit 4 on the ground (altitude was `"ground"`); bits 5 to 7 zero |
| 39 | 1 | source, u8 | index into `adsb_icao, adsb_icao_nt, adsr_icao, tisb_icao, adsc, mlat, other, mode_s, adsb_other, adsr_other, tisb_trackfile, tisb_other, mode_ac, unknown` (0 to 13); `255` absent |
| 40 | 8 | callsign | ASCII, trailing spaces removed, NUL padded; all NUL absent |
| 48 | 1 | category | emitter category byte (`"A3"` is `0xA3`); 0 absent |
| 49 | 1 | nic | 0 to 11; `255` absent |
| 50 | 1 | nac_p | 0 to 15; `255` absent |
| 51 | 1 | nac_v | 0 to 7; `255` absent |
| 52 | 1 | sil | 0 to 3; `255` absent |
| 53 | 1 | sil_type | 0 unknown, 1 per hour, 2 per sample; `255` absent |
| 54 | 1 | version | ADS-B version, 0 to 7; `255` absent |
| 55 | 1 | gva | 0 to 3; `255` absent |
| 56 | 1 | sda | 0 to 3; `255` absent |
| 57 | 2 | wind speed `ws`, u16 | knots, readsb-derived; `65535` absent |
| 59 | 2 | wind direction `wd`, u16 | degrees true, the direction the wind blows from, 0 to 359 (readsb's 360 is stored as 0); `65535` absent |
| 61 | 2 | true airspeed `tas`, u16 | knots; `65535` absent |
| 63 | 2 | indicated airspeed `ias`, u16 | knots; `65535` absent |
| 65 | 2 | Mach number, u16 | 0.001; `65535` absent |
| 67 | 2 | magnetic heading, u16 | 0.01 degree, 0 to 35999; `65535` absent |
| 69 | 2 | true heading, u16 | 0.01 degree, 0 to 35999; `65535` absent |
| 71 | 2 | outer air temperature `oat`, i16 | whole degrees C, readsb-derived; `-32768` absent |
| 73 | 2 | total air temperature `tat`, i16 | whole degrees C, readsb-derived; `-32768` absent |
| 75 | 2 | roll, i16 | 0.01 degree, negative is left, within 18000 in magnitude; `-32768` absent |
| 77 | 2 | track rate, i16 | 0.01 degree per second; `-32768` absent |
| 79 | 1 | reserved, 0 | |

(`ADS1` records have bytes 57 to 63 reserved, 0, and no measurements.)

The fields from `callsign` on come from the aircraft object that readsb prints on some rows
only; a report without an object has them absent. They are not carried forward from earlier rows.
The measurements from offset 57 are per observation in the same way (`ias` and `roll` are also
taken from the source row's own columns when the row has no object). What readsb's `ws`, `wd`,
`oat` and `tat` mean, when readsb computes them and their known limits are in the `oa-adsb`
crate documentation (section "Measurements and what readsb's wind means"). A value that is
malformed or out of range in the source is left absent (counted, `measurements_bad` in the clip
tool's output), not refused.

## Reader rules

A payload with a wrong length, magic, record length, a non-zero reserved byte, a count above the
maximum of its version, or a record whose latitude or longitude is out of range, whose flags use
bits 5 to 7, whose source is not 0 to 13 or 255, whose wind direction is 360 or more, whose
heading is 36000 or more or whose roll is beyond 180 degrees is refused whole (`PayloadError`) and yields no reports.

## Clip tool

```
oa-adsb-clip --out <file|-> [--box lat_min,lat_max,lon_min,lon_max]
             [--from <time>] [--to <time>] [--frame-ms <n>] [--allow-truncated] <part>...
```

Streams the parts of one archive in order (or a single tar), keeps reports inside the box
(default: ZAU, 38.5 to 46.5 N, 83.5 to 93.5 W, edges included) and the window `[from, to)`,
sorts by time and writes frames of at most `--frame-ms` milliseconds of report time (1 to 60000,
default 1000; anything else is refused with exit 2). Reports are held in memory until the end (at most 40 Mi; the
tool stops with an error rather than dropping any). Counts, the number of distinct aircraft, the
`nac_p` figures and the position-jump check go to standard error. Exit codes: 0 done, 1 error, 2
bad arguments, 3 the parts ended early (only the first part of a split archive, for instance) and
`--allow-truncated` was not given; the frames written are valid either way.
