# Feed frames (OAF1)

Status: adopted 2026-11-14. The wire between an input adapter process and the Rust core
(RULESET F6, INTERFACES IF-R1, IF-R5). First user: the SWIM SCDS bridge.

## Why

SCDS delivers over Solace JMS, a proprietary protocol with a Java client. Outside code never runs
in the core (F6), so `adapters/scds-bridge` consumes one subscription and writes plain frames to
its standard output. `oa-feed` reads them. The same bytes are what `--record` stores and what
`oa-feed replay` reads back, so a recording is the exact input of a live run (B6).

## Frame

All integers little-endian. A frame is a header, a payload and a CRC.

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 4 | magic | ASCII `OAF1` |
| 4 | 1 | kind | 1 MESSAGE, 2 REJECT, 3 STATUS |
| 5 | 1 | flags | bit 0: JMS redelivered; bit 1: payload came from a BytesMessage (else a TextMessage, UTF-8). Other bits are carried, not refused. |
| 6 | 2 | reserved | 0 |
| 8 | 8 | seq | u64. The bridge's own counter, 0 at bridge start, +1 per frame of any kind |
| 16 | 8 | recv_unix_ns | i64. Bridge wall clock at receipt |
| 24 | 8 | jms_timestamp_ms | i64. JMS timestamp, 0 if absent |
| 32 | 4 | payload_len | u32, at most 4 194 304 (4 MiB) |
| 36 | n | payload | `payload_len` bytes |
| 36+n | 4 | crc32 | u32. CRC-32 (IEEE 802.3, as in ZIP) of every preceding byte of the frame |

Header 36 bytes, trailer 4 bytes, so a frame is 40 to 4 194 344 bytes.

A worked example (kind 1, flags 3, seq 7, `recv_unix_ns` 1700000000123456789,
`jms_timestamp_ms` 1700000000100, payload `hello`) is the golden vector in the oa-feed tests,
produced by the bridge's Java encoder.

## Kinds

* **MESSAGE.** The SWIM message body: TextMessage as UTF-8 bytes, BytesMessage raw.
* **REJECT.** The bridge refused to frame a message. ASCII `key=value` lines: `reason=` (one of
  `oversize`, `unsupported_type:<class>`), `size=` (bytes, 0 if unknown) and `jms_id=` (the JMS
  message ID, printable ASCII). The message is acknowledged: it will not be redelivered, and
  this frame is the record that it existed.
* **STATUS.** ASCII `key=value` lines. `state=connected`, `state=disconnected`, or
  `state=reconnect_attempt` followed by `delay_ms=<n>` (the wait before the next attempt).

## Reader rules (oa-feed)

* A bad magic, an unknown kind, a length above 4 MiB or a CRC mismatch is an error reported with
  the byte offset of the frame start. The frame is never passed on (IF-R5).
* After an error the reader discards one byte and scans for the next `OAF1`, counting every byte
  it discards; the count is part of health. A stream that ends inside a frame is a separate
  `Truncated` error.
* The reader works in a caller-supplied buffer of at least one maximum frame; it allocates
  nothing after start. Input of any chunking gives the same frames.

## Bridge behaviour (adapters/scds-bridge)

* One process per subscription. Argument: the path of the env file (`SCDS_JMS_URL`, `SCDS_VPN`,
  `SCDS_USERNAME`, `SCDS_PASSWORD`, `SCDS_QUEUE`; lines are `KEY=value`, spaces around the value
  are trimmed). Errors name a key, never a value; library error text is scrubbed of the values.
* Standard output carries frames and nothing else. Logs go to standard error, one line per
  event, `key=value`.
* Acknowledgement is CLIENT_ACKNOWLEDGE and happens only after the frame has been written and
  flushed. If the reader is slow, stdout blocks, the bridge stops receiving, and the broker holds
  the backlog; the bridge buffers nothing.
* The Solace client's own connect and reconnect retries are off. The bridge retries itself with
  exponential backoff from 2 s doubling to a 5 min cap, with random spread (delay drawn uniformly
  between 250 ms and the current ceiling), and the backoff restarts after a connection that
  stayed up for a minute (IF-S3). Each attempt is logged and announced in a STATUS frame.
* A message whose payload is over 4 MiB is not framed: it is logged, acknowledged and answered
  with a REJECT. For a text message the test is on the real UTF-8 byte length (a 4 Mi-character
  message can be up to 12 MiB of UTF-8): the length is counted without copying, so an oversize
  message is refused before it is encoded, and one that fits is encoded exactly once. Any
  message type other than text or bytes gets a REJECT.
* Build: `mvn package` in `adapters/scds-bridge` makes `target/scds-bridge.jar` (shaded; every
  plugin version is pinned in the pom). The Solace client is fetched by Maven and never stored
  in this repository.

## What the Solace client buffers, and how it is bounded

sol-jms 10.30.2 exposes the guaranteed-delivery receive window on the connection factory
(`setReceiveADWindowSize`, the consumer flow's window; the Solace default is 255). Its other
receive setters are `setReceiveAdAckThreshold`, `setReceiveADAckTimerInMillis` and
`setReceiveBufferSize` (the socket receive buffer). There is no separate prefetch setting.

The window is the most messages the broker will send before the client acknowledges one. With
CLIENT_ACKNOWLEDGE and the bridge acknowledging only after a frame is written and flushed, the
messages the client library holds unacknowledged are therefore at most the window, and the
bridge itself holds one more (the one being written). Nothing else queues in the bridge.

Choice: the bridge sets the window to **32** by default (key `SCDS_ACK_WINDOW` in the env file
overrides it, 1 to 255) and logs it on connect. Not 1: with a window of 1 the broker sends the
next message only after the previous acknowledgement arrives, so throughput is about one message
per round trip to the broker. TBFM peaks near 262 msg/s, which needs a round trip under about
3.8 ms; a host reaching `ems2.swim.faa.gov` over the internet will be well above that. A window
of W sustains about W / round trip, so 32 sustains 262 msg/s up to a round trip of about 120 ms.
Worst-case memory held by the client is 32 times the largest message (observed maximum 579 KB,
so about 19 MB; the JVM heap is capped at 512 MB by `oa-feed`).

Not yet measured: the live run that produced the first recording used the default window of 255,
and the throughput at 32 has not been confirmed against the real broker. Whether the client
sends an acknowledgement immediately on `acknowledge()` or batches it by `ReceiveAdAckThreshold`
and the ack timer is likewise unverified; if a live run at 32 falls short of the feed's rate, the
first things to try are a larger window, then those two settings.
