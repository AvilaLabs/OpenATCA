# Raw replay restart witness

The offline raw-only CLI can write a durable replay witness with
`--raw-witness-out <file>` and verify it on a later invocation with
`--verify-raw-witness <file>`. Both flags require `--raw-only` and an explicit
`--raw-receipt` basis. The witness output path must not already exist. Verification
reads the witness without changing it and normally requires a fresh output directory. Every
publication writer, including `--overwrite` and `--hashes-only`, holds the output lock through
its final manifest. `--overwrite` checks inputs against generated names and completes a bounded
directory preflight before removing any prior output.

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

`--resume-raw-publication` can be combined with `--verify-raw-witness` to continue an
interrupted publication. The witness must come from an earlier complete run: witness
creation is withheld on refused or unavailable work, and an interrupted first run does
not yet have a closed witness. Before opening output, the command checks snapshot and
clip bytes, receipt basis, effective settings, frame count and exact sequence against
that witness. It then accepts only a contiguous completed PPM prefix. Every existing
PPM is recomputed from frame zero and compared byte-for-byte; a gap, extra entry,
truncated or changed PPM, wrong witness, or mismatching existing `hashes.txt` is refused.
Only missing suffix frames are created, in order. Each is written to a private staging name,
file-synced, then hard-linked atomically to its final name (which cannot be replaced); the
directory is synced before the stage link is removed and synced again. The hashes manifest uses
the same publication sequence and is created last as the completion marker. Recovery under the
lock removes an exact-name single-link partial stage without reading it. If a crash occurred
between linking stage and final names, recovery first proves from `symlink_metadata` that both are
regular files with the same device/inode and exactly two links, then removes the staging link;
the final is read later through the sealed regular-file guard after its link count is one.
Unexpected staging names, extra links, unmatched final files, non-prefix stages, and tampered
canonical prefix files are refused without cleanup. A crash after all frames but before the
manifest can be resumed by rechecking the full sequence and publishing the manifest. The public
publisher API also requires existing prefix frames to be verified in order before a suffix can be
published or the manifest finalized.

The publication API holds an exclusive OS file lock for one active publisher on the
same host and a local filesystem that honors those locks. The empty lock file remains
in the output directory; the OS lock is released when the process exits. Do not remove or
replace the lock-file directory entry while a publisher is active: an uncooperative external
actor can unlink it and create a second lock inode. This is not a distributed lease, generation
fence, or production fencing protocol. A publisher on a different host or a filesystem with
unreliable lock semantics is outside this guarantee.
Recovery recomputes the complete replay from its original inputs and verifies the
completed output prefix; it is not a process-state or tracker-state checkpoint. The
checksum detects accidental or unsophisticated edits; it does not authenticate who
created the witness or protect against a writer who can recompute its checksum.

The receipt basis remains explicit. `recorded` uses the OAF frame receipt timestamp;
`archive-reconstructed` uses the latest decoded observation timestamp in that frame as
an availability proxy. Observation measurement timestamps remain the values used for
the displayed reports. The witness changes no publisher generation, duplicate, or
fencing rule and provides no authority, peer identity, live feed reconnection, or
external side-effect guarantee.

## Requirement mapping

These definitions are scoped to this package and are not yet entered in the root
requirement registry:

- **OA-DSP-074** A verified OARW001 witness binds exact snapshot and clip bytes, receipt
  basis, effective settings and ordered frame digests; any mismatch is refused before
  publication recovery begins.
- **OA-DSP-075** Recovery accepts only a byte-exact contiguous completed PPM prefix,
  publishes only its missing suffix with create-new and sync, and refuses tampering,
  truncation, gaps, wrong provenance or a nonmatching completion manifest.
- **OA-DSP-076** One same-host publisher holds the local OS lock at a time; interruption
  releases that lock and preserves completed files for recomputation-based recovery. It
  provides no distributed or production fencing.
