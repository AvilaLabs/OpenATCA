// Copyright 2026 Avila Labs. Licensed under the Apache License, Version 2.0.
package org.avilalabs.openatca.scds;

import com.solacesystems.jms.SolConnectionFactory;
import com.solacesystems.jms.SolJmsUtility;
import com.solacesystems.jms.message.SolMessage;
import java.io.BufferedOutputStream;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.security.SecureRandom;
import java.time.Instant;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.Session;
import javax.jms.TextMessage;

/**
 * SCDS bridge: one process, one subscription. Consumes the queue and writes OAF1 frames to stdout
 * (nothing else goes there). Logs are one line per event, key=value, on stderr, never with a
 * credential. A message is acknowledged only after its frame is written and flushed; if stdout
 * blocks, consumption stops and the broker holds the backlog (RULESET F6, INTERFACES IF-R1/S3).
 */
public final class Bridge {
  private static final SecureRandom RNG = new SecureRandom();
  private static final PrintStream ERR = System.err;
  private static final long RECEIVE_TIMEOUT_MS = 5_000;
  /** Solace stream compression level (1-9); SCDS requires compressed consumption. */
  private static final int COMPRESSION_LEVEL = 9;
  /** A connection that stayed up this long resets the backoff. */
  private static final long STABLE_MS = 60_000;

  /** Credential values, kept only to scrub library error text before it is logged. */
  private static volatile String[] secrets = new String[0];

  /**
   * Default consumer ack window: the most messages the broker may send before we acknowledge
   * one. Bounds what the client holds unacknowledged. Not 1: at a window of 1 the rate is one
   * message per round trip to the broker, far below TBFM's ~262 msg/s peak unless the round trip
   * is under ~4 ms. 32 sustains 262 msg/s up to a ~120 ms round trip.
   */
  static final int DEFAULT_ACK_WINDOW = 32;

  private final OutputStream out;
  private long seq = 0;

  private Bridge(OutputStream out) {
    this.out = out;
  }

  public static void main(String[] args) {
    if (args.length != 1) {
      log("event=fatal reason=usage detail=\"bridge <envfile>\"");
      System.exit(2);
    }
    // Stray prints must never corrupt the frame stream.
    OutputStream raw = new BufferedOutputStream(new FileOutputStream(FileDescriptor.out), 1 << 16);
    System.setOut(ERR);
    // The Solace client logs through java.util.logging at INFO; keep stderr to our own events.
    java.util.logging.Logger.getLogger("").setLevel(java.util.logging.Level.WARNING);
    scrubLibraryLogs();
    EnvFile env;
    try {
      env = EnvFile.read(Path.of(args[0]));
      for (String k : new String[] {"SCDS_JMS_URL", "SCDS_VPN", "SCDS_USERNAME", "SCDS_PASSWORD", "SCDS_QUEUE"}) {
        env.require(k);
      }
      ackWindow(env); // validates the optional window before any connection
    } catch (IOException e) {
      log("event=fatal reason=env detail=\"" + clean(e.getMessage()) + "\"");
      System.exit(2);
      return;
    }
    try {
      secrets = new String[] {
        env.require("SCDS_JMS_URL"), env.require("SCDS_USERNAME"), env.require("SCDS_PASSWORD"), env.require("SCDS_VPN")
      };
    } catch (IOException e) {
      System.exit(2);
    }
    Bridge bridge = new Bridge(raw);
    try {
      bridge.run(env);
    } catch (IOException e) {
      // stdout is gone: the reader has exited, so there is nothing left to serve.
      log("event=fatal reason=stdout detail=\"" + clean(e.getMessage()) + "\"");
      System.exit(1);
    }
  }

  private void run(EnvFile env) throws IOException {
    int attempt = 0;
    while (true) {
      long started = System.currentTimeMillis();
      try {
        log("event=connect_attempt attempt=" + attempt);
        session(env);
      } catch (JMSException | RuntimeException e) {
        log("event=connection_failed attempt=" + attempt + " error=" + e.getClass().getSimpleName()
            + " detail=\"" + clean(e.getMessage()) + "\"");
      }
      if (System.currentTimeMillis() - started >= STABLE_MS) attempt = 0;
      long delay = Backoff.delayMs(attempt, RNG);
      status("state=disconnected");
      status("state=reconnect_attempt\ndelay_ms=" + delay);
      log("event=reconnect_wait attempt=" + attempt + " delay_ms=" + delay);
      attempt++;
      try {
        Thread.sleep(delay);
      } catch (InterruptedException ie) {
        Thread.currentThread().interrupt();
        return;
      }
    }
  }

  /** Ack window from the optional SCDS_ACK_WINDOW key (1 to 255), else the default. */
  static int ackWindow(EnvFile env) throws IOException {
    String v = env.optional("SCDS_ACK_WINDOW");
    if (v == null) return DEFAULT_ACK_WINDOW;
    try {
      int w = Integer.parseInt(v);
      if (w >= 1 && w <= 255) return w;
    } catch (NumberFormatException ignored) {
      // falls through to the error below
    }
    throw new IOException("env file value for SCDS_ACK_WINDOW must be a whole number 1 to 255");
  }

  /** One connection's lifetime; returns or throws when it is lost. */
  private void session(EnvFile env) throws IOException, JMSException {
    SolConnectionFactory cf;
    try {
      cf = SolJmsUtility.createConnectionFactory();
    } catch (JMSException | RuntimeException e) {
      throw e;
    } catch (Exception e) {
      throw new IllegalStateException("connection factory: " + e.getClass().getSimpleName());
    }
    cf.setHost(env.require("SCDS_JMS_URL"));
    cf.setVPN(env.require("SCDS_VPN"));
    cf.setUsername(env.require("SCDS_USERNAME"));
    cf.setPassword(env.require("SCDS_PASSWORD"));
    // Our own loop owns reconnection, so the client library must not retry behind our back.
    cf.setConnectRetries(0);
    cf.setReconnectRetries(0);
    cf.setReceiveADWindowSize(ackWindow(env));
    // SCDS usage standards: "Keep Data Compression enabled. Consuming compressed data is the only
    // authorized method for obtaining SCDS data products."
    cf.setCompressionLevel(COMPRESSION_LEVEL);
    Connection c = cf.createConnection();
    try {
      Session s = c.createSession(false, Session.CLIENT_ACKNOWLEDGE);
      MessageConsumer mc = s.createConsumer(s.createQueue(env.require("SCDS_QUEUE")));
      c.start();
      status("state=connected");
      log("event=connected ack_window=" + ackWindow(env) + " compression=" + COMPRESSION_LEVEL);
      while (true) {
        Message m = mc.receive(RECEIVE_TIMEOUT_MS);
        if (m == null) continue;
        long recvNs = nowNs();
        handle(m, recvNs);
        m.acknowledge(); // only after the frame is written and flushed
      }
    } finally {
      try {
        c.close();
      } catch (Exception ignored) {
        // already broken; the reconnect loop takes over
      }
    }
  }

  private void handle(Message m, long recvNs) throws IOException, JMSException {
    int flags = m.getJMSRedelivered() ? Frame.FLAG_REDELIVERED : 0;
    long ts = Math.max(0, m.getJMSTimestamp());
    String id = clean(m.getJMSMessageID());
    byte[] payload;
    if (m instanceof TextMessage tm) {
      // getText() builds the whole string, so an oversize message is refused from its received
      // size first; the exact UTF-8 check below then covers the rest.
      long wire = receivedSize(m);
      if (wire > Frame.MAX_PAYLOAD + HEADER_ALLOWANCE) {
        reject("oversize", wire, id, flags, ts, recvNs);
        return;
      }
      String text;
      try {
        text = tm.getText() == null ? "" : tm.getText();
      } catch (OutOfMemoryError e) {
        // Still refused and acknowledged by the caller, so the broker does not redeliver it
        // into another out-of-memory restart.
        reject("oversize", wire, id, flags, ts, recvNs);
        return;
      }
      // The real UTF-8 length (up to 3 bytes per char), counted without a copy, so an oversize
      // message is refused before it is encoded; a message that fits is encoded exactly once.
      long utf8 = utf8Length(text);
      if (utf8 > Frame.MAX_PAYLOAD) {
        reject("oversize", utf8, id, flags, ts, recvNs);
        return;
      }
      payload = text.getBytes(StandardCharsets.UTF_8);
    } else if (m instanceof BytesMessage bm) {
      long len = bm.getBodyLength();
      if (len > Frame.MAX_PAYLOAD) {
        reject("oversize", len, id, flags | Frame.FLAG_BYTES, ts, recvNs);
        return;
      }
      payload = new byte[(int) len];
      bm.readBytes(payload);
      flags |= Frame.FLAG_BYTES;
    } else {
      reject("unsupported_type:" + m.getClass().getSimpleName(), 0, id, flags, ts, recvNs);
      return;
    }
    if (payload.length > Frame.MAX_PAYLOAD) {
      reject("oversize", payload.length, id, flags, ts, recvNs);
      return;
    }
    write(Frame.KIND_MESSAGE, flags, ts, recvNs, payload);
  }

  /** Bytes of message framing and headers that may sit on top of the body in the received size. */
  static final long HEADER_ALLOWANCE = 64 * 1024;

  /** Size of the received message as the client counts it, without reading the body; -1 if unknown. */
  private static long receivedSize(Message m) {
    if (m instanceof SolMessage sm) {
      try {
        return sm.getMessageSize();
      } catch (JMSException | RuntimeException e) {
        return -1;
      }
    }
    return -1;
  }

  /**
   * Replaces the client library's log handlers with one that writes through {@link #clean}, so a
   * connection property or credential in library text never reaches stderr.
   */
  private static void scrubLibraryLogs() {
    java.util.logging.Logger root = java.util.logging.Logger.getLogger("");
    for (java.util.logging.Handler h : root.getHandlers()) root.removeHandler(h);
    java.util.logging.Handler scrub =
        new java.util.logging.Handler() {
          @Override
          public void publish(java.util.logging.LogRecord r) {
            if (r == null || !isLoggable(r)) return;
            String msg;
            try {
              msg = new java.util.logging.SimpleFormatter().formatMessage(r);
            } catch (RuntimeException e) {
              msg = "";
            }
            String thrown = r.getThrown() == null ? "" : r.getThrown().getClass().getSimpleName();
            log("event=library level=" + r.getLevel() + " logger=" + clean(r.getLoggerName()) + " detail=\""
                + clean(msg) + "\" thrown=" + thrown);
          }

          @Override
          public void flush() {}

          @Override
          public void close() {}
        };
    scrub.setLevel(java.util.logging.Level.WARNING);
    root.addHandler(scrub);
  }

  /** Length of `s` in UTF-8, matching String.getBytes (a lone surrogate encodes as '?'). */
  static long utf8Length(String s) {
    long n = 0;
    for (int i = 0; i < s.length(); i++) {
      char c = s.charAt(i);
      if (c < 0x80) {
        n += 1;
      } else if (c < 0x800) {
        n += 2;
      } else if (Character.isHighSurrogate(c) && i + 1 < s.length() && Character.isLowSurrogate(s.charAt(i + 1))) {
        n += 4;
        i++;
      } else if (Character.isSurrogate(c)) {
        n += 1;
      } else {
        n += 3;
      }
    }
    return n;
  }

  private void reject(String reason, long size, String id, int flags, long ts, long recvNs) throws IOException {
    log("event=reject reason=" + reason + " size=" + size + " jms_id=" + id);
    String p = "reason=" + reason + "\nsize=" + size + "\njms_id=" + id + "\n";
    write(Frame.KIND_REJECT, flags, ts, recvNs, p.getBytes(StandardCharsets.US_ASCII));
  }

  private void status(String body) {
    try {
      write(Frame.KIND_STATUS, 0, 0, nowNs(), (body + "\n").getBytes(StandardCharsets.US_ASCII));
    } catch (IOException e) {
      log("event=fatal reason=stdout detail=\"" + clean(e.getMessage()) + "\"");
      System.exit(1);
    }
  }

  private void write(int kind, int flags, long ts, long recvNs, byte[] payload) throws IOException {
    out.write(Frame.encode(kind, flags, seq++, recvNs, ts, payload));
    out.flush();
  }

  private static long nowNs() {
    Instant i = Instant.now();
    return i.getEpochSecond() * 1_000_000_000L + i.getNano();
  }

  /** Printable ASCII only, no quotes or line breaks: safe for a log line and a REJECT payload. */
  private static String clean(String s) {
    if (s == null) return "";
    for (String secret : secrets) {
      if (!secret.isEmpty()) s = s.replace(secret, "<redacted>");
    }
    StringBuilder b = new StringBuilder(Math.min(s.length(), 200));
    for (int i = 0; i < s.length() && b.length() < 200; i++) {
      char ch = s.charAt(i);
      b.append(ch >= 0x20 && ch < 0x7f && ch != '"' ? ch : '_');
    }
    return b.toString();
  }

  private static void log(String line) {
    ERR.println("ts=" + Instant.now() + " " + line);
  }
}
