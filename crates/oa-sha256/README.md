# `oa-sha256`

This crate implements SHA-256 digest operations for integrity and content identity. A digest does
not authenticate content, establish provenance, or authorize a sender. Use it only to compare
content bytes with a separately trusted expected digest. See the [integrity API profile](https://github.com/AvilaLabs/OpenATCA/blob/main/docs/core-api/foundations/integrity.md).

**Tier: Preview.** The published Cargo package is `openatca-sha256` version `0.1.0`; its Rust
library name remains `oa_sha256`.

```toml
[dependencies]
openatca-sha256 = "0.1.0"
```

```rust
use oa_sha256::{sha256, to_hex};

let digest = sha256(b"content to identify");
let lowercase_hex = to_hex(&digest);
```

`sha256` is the one-shot byte-slice API. `Sha256` supports bounded streaming updates; `to_hex_into`
and `Hex` format into caller-owned storage, while `to_hex` allocates a `String`. The crate has no
Cargo dependencies. Preview status describes the current API stability tier; it does not make a
digest an authentication or authorization mechanism.
