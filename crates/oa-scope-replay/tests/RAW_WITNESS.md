# Raw replay restart witness

The offline raw-only CLI can write a durable replay witness with
`--raw-witness-out <file>` and verify it on a later invocation with
`--verify-raw-witness <file>`. Both flags require `--raw-only` and an explicit
`--raw-receipt` basis. The witness output path must not already exist. Verification
reads the witness without changing it and requires a fresh output directory.

The versioned `OARW001` file binds the exact snapshot bytes, exact OAF clip bytes,
declared receipt basis, effective raw/display settings, frame count, and each frame's
ordered exact nanosecond timestamp and pixel digest. A SHA-256 checksum covers the
complete preceding file body. Input files are regular files; the clip remains bounded
by the raw replay's 512 MiB limit, and the witness is limited to 1,000,000 bytes and
10,000 frames. Unknown fields, noncanonical order, partial frame lists, wrong times,
and checksum damage are refused before output creation. The witness is written and
synced to an exclusively created `.oarw-partial` sibling. The directory is synced
before a final file is created exclusively and copied from the bounded in-memory encoding. The final
file and its directory entry are synced while the partial still exists. The complete partial
is retained after success as an explicit recovery input; publication deletes neither file.
The files have distinct inodes, so each remains a single-link input for the sealed-data guard.
Publication is fail-closed and recoverable, not atomically visible: a crash during the
final copy can leave an incomplete final file and a complete partial. Verification
refuses the incomplete final by checksum; explicitly pass the `.oarw-partial` path to
`--verify-raw-witness` to verify the synced recovery copy. It never falls back silently,
overwrites either path, or deletes a preexisting partial.

Verification replays the source from its beginning and compares each frame against the
recorded ordered prefix before publishing that frame. A frame mismatch stops the run;
already matched earlier frames may have been published, and the error says so. This
is restart verification evidence, not restored process state or a supervisor protocol.
The checksum detects accidental or unsophisticated edits; it does not authenticate who
created the file or protect against a writer who can recompute the checksum.

The receipt basis remains explicit. `recorded` uses the OAF frame receipt timestamp;
`archive-reconstructed` uses the latest decoded observation timestamp in that frame as
an availability proxy. Observation measurement timestamps remain the values used for
the displayed reports. The witness changes no publisher generation, duplicate, or
fencing rule and provides no authority, peer identity, live feed reconnection, or
external side-effect guarantee.
