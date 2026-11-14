# ADS-B reports in OAF1 frames (ADS1)

Status: adopted 2026-11-14. Written by `oa-adsb-clip` (crate `oa-adsb`), read by
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
  payload limit). Readers do not depend on this: a clip with any grouping, such as the earlier
  one of 60 000 reports per frame, decodes the same.

## Payload

All integers little-endian. A 16-byte header, then `count` records of 64 bytes.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic, ASCII `ADS1` |
| 4 | 2 | record length, 64 |
| 6 | 2 | reserved, 0 |
| 8 | 4 | count, u32, at most 60 000 |
| 12 | 4 | reserved, 0 |

The payload length is exactly `16 + 64 * count`.

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
| 57 | 7 | reserved, 0 | |

The fields from `callsign` on come from the aircraft object that readsb prints on some rows
only; a report without an object has them absent. They are not carried forward from earlier rows.

## Reader rules

A payload with a wrong length, magic, record length, a non-zero reserved byte, a count above the
maximum, or a record whose latitude or longitude is out of range, whose flags use bits 5 to 7 or
whose source is not 0 to 13 or 255 is refused whole (`PayloadError`) and yields no reports.

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
