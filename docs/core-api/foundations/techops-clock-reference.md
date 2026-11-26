# Tech Ops GPS receiver reference

`oa-techops::parse_ubx_nav_timegps` accepts one complete 24-byte UBX frame and returns a
`UbxGpsReference`. Its exact input profile is **u-blox M10 SPG 5.10 Interface Description,
UBX-21035062, revision R03 (2023-06-27), §3.15.21**, `UBX-NAV-TIMEGPS` class `0x01`, ID `0x20`,
16-byte payload. Official public source: [u-blox interface description PDF](https://content.u-blox.com/sites/default/files/u-blox-M10-SPG-5.10_InterfaceDescription_UBX-21035062.pdf).

The decoder checks exact framing and payload size, UBX checksum, time-of-week and week validity,
the supported field ranges, and the receiver's GPS-UTC leap value against the checked-in IERS
history and conversion horizon. It converts week, iTOW and fTOW into `GpsInstant`, which callers
can supply as `ClockSample.gps_reference` to `assess_daily_clock`. The receiver's `tAcc` estimate
is preserved as `accuracy_ns`; it is not an authenticated uncertainty bound or an alarm override.
The daily comparison's `reference_day` anchors the GPS references; processor UTC samples are
compared as absolute instants and may cross midnight, so small offsets at a date boundary remain
measurable and large wrong-date offsets remain visible as alarms.

The API parses a single already-buffered frame; it does not scan/assemble a byte stream, read a
device, identify or authenticate a receiver, establish reference independence, or supply sample
age. The UBX checksum detects transmission corruption only. Synthetic vectors establish parser
behavior, not receiver interoperability or a live GPS-derived time source. The daily report remains
`UNVERIFIED_CALLER_SUPPLIED` until the external acquisition and provenance controls are evidenced.
