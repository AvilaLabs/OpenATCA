// Copyright 2026 Avila Labs. Licensed under the Apache License, Version 2.0.
package org.avilalabs.openatca.scds;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;

/** Reads KEY=value lines. Values are trimmed ("KEY= value" is normal). No value is ever echoed. */
final class EnvFile {
  private final Map<String, String> values = new HashMap<>();

  static EnvFile read(Path path) throws IOException {
    EnvFile e = new EnvFile();
    int lineNo = 0;
    for (String raw : Files.readAllLines(path, StandardCharsets.UTF_8)) {
      lineNo++;
      String line = raw.trim();
      if (line.isEmpty() || line.startsWith("#")) continue;
      if (line.startsWith("export ")) line = line.substring(7).trim();
      int eq = line.indexOf('=');
      if (eq <= 0) {
        // Names the line number only: the line may hold a secret.
        throw new IOException("env file line " + lineNo + " is not KEY=value");
      }
      e.values.put(line.substring(0, eq).trim(), line.substring(eq + 1).trim());
    }
    return e;
  }

  /** The value for `key`, or null if absent or empty. */
  String optional(String key) {
    String v = values.get(key);
    return v == null || v.isEmpty() ? null : v;
  }

  /** The value for `key`; the error names the key, never a value. */
  String require(String key) throws IOException {
    String v = values.get(key);
    if (v == null || v.isEmpty()) throw new IOException("env file is missing a value for " + key);
    return v;
  }
}
