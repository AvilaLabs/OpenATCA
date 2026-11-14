// Copyright 2026 Avila Labs. Licensed under the Apache License, Version 2.0.
package org.avilalabs.openatca.scds;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.zip.CRC32;

/** OAF1 frame encoder. Layout is specified in docs/FEED_FRAMES.md; all integers little-endian. */
final class Frame {
  static final int KIND_MESSAGE = 1;
  static final int KIND_REJECT = 2;
  static final int KIND_STATUS = 3;
  static final int FLAG_REDELIVERED = 1;
  static final int FLAG_BYTES = 2;
  static final int HEADER_LEN = 36;
  static final int MAX_PAYLOAD = 4 * 1024 * 1024;

  private Frame() {}

  /** Encodes one frame: header, payload, then the CRC-32 of every preceding byte. */
  static byte[] encode(int kind, int flags, long seq, long recvUnixNs, long jmsTimestampMs, byte[] payload) {
    ByteBuffer b = ByteBuffer.allocate(HEADER_LEN + payload.length + 4).order(ByteOrder.LITTLE_ENDIAN);
    b.put((byte) 'O').put((byte) 'A').put((byte) 'F').put((byte) '1');
    b.put((byte) kind).put((byte) flags).putShort((short) 0);
    b.putLong(seq).putLong(recvUnixNs).putLong(jmsTimestampMs);
    b.putInt(payload.length);
    b.put(payload);
    CRC32 crc = new CRC32();
    crc.update(b.array(), 0, b.position());
    b.putInt((int) crc.getValue());
    return b.array();
  }
}
