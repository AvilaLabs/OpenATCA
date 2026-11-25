# SHA-256 and content identity

**Current surface.** `oa-sha256` is a split core/shell crate. Core exports `Sha256::new()`,
`update(&[u8]) -> Result<(), MessageTooLong>`, `finalize() -> [u8;32]`, one-shot `sha256`,
`BLOCK_LEN=64`, `DIGEST_LEN=32`, `MAX_MESSAGE_BYTES=u64::MAX/8`, and `hex_byte`, `to_hex_into`,
and `Hex`. Shell exports `sha256_reader(Read)`, `READ_CHUNK_LEN=64 KiB`, its I/O/hash error, and
allocating `to_hex`.

**Units/limits/errors.** Input is exact bytes; digest is 32 bytes, hex is 64 lowercase ASCII
characters. Streaming accepts at most `MAX_MESSAGE_BYTES`; further updates return
`MessageTooLong`. Reader errors are explicit shell errors; it reads incrementally in 64 KiB chunks
and does not impose a total-byte cap below SHA-256's representable limit. A caller needing a
smaller cap must enforce it. There is no path-taking API and no implicit file access.

**Tier/stability proposal.** L0 integrity-only, FIPS 180-4 SHA-256. This is not authentication,
signature verification, a seal, provenance, or authorization: any party able to replace content
can recompute the digest. Standard response files and a differential test are present. Before
Stable, complete the §8 evidence and record exact FIPS edition/test vectors, malformed/maximum
length behavior, fuzz and mutation review, Kani for byte/integer logic, cross-architecture output,
and Rust/Python examples. The one-shot and streaming APIs should be cross-checked over chunk
boundaries and maximum-length arithmetic.

No hidden clock or seal checks occur. A higher-level caller decides which expected digest is
trusted and binds the digest to a signed/otherwise authenticated manifest.
