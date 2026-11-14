// Copyright 2026 Avila Labs. Licensed under the Apache License, Version 2.0.
package org.avilalabs.openatca.scds;

import java.util.random.RandomGenerator;

/** Exponential backoff, 2 s doubling to a 5 min cap, with full random jitter (IF-S3). */
final class Backoff {
  static final long BASE_MS = 2_000;
  static final long CAP_MS = 300_000;
  /** Floor so a jittered delay is never a busy loop. */
  static final long FLOOR_MS = 250;

  private Backoff() {}

  /** Upper bound of the delay before attempt number `attempt` (0-based). */
  static long ceilingMs(int attempt) {
    long c = BASE_MS;
    for (int i = 0; i < attempt && c < CAP_MS; i++) c *= 2;
    return Math.min(c, CAP_MS);
  }

  /** A delay drawn uniformly from [FLOOR_MS, ceiling]. */
  static long delayMs(int attempt, RandomGenerator rng) {
    long ceil = ceilingMs(attempt);
    return FLOOR_MS + (long) (rng.nextDouble() * (ceil - FLOOR_MS));
  }
}
