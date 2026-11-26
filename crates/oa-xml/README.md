# `oa-xml`

This crate parses its documented bounded XML 1.0 subset; it is not a general XML platform or
serializer. See the [text API profile](https://github.com/AvilaLabs/OpenATCA/blob/main/docs/core-api/foundations/text.md)
for grammar and error behavior.

**Tier: Preview.** The published Cargo package is `openatca-xml` version `0.1.0`; its Rust library
name remains `oa_xml`.

```toml
[dependencies]
openatca-xml = "0.1.0"
```

```rust
use oa_xml::{Limits, Parser, XmlError};

fn parse_message(input: &[u8]) -> Result<(), XmlError> {
    let mut parser = Parser::new(Limits::default())?;
    let doc = parser.parse(input)?;
    let _element_count = doc.elem_name.len();
    Ok(())
}
```

The parser accepts one complete message at a time, defaults to a 1 MiB input limit, and refuses
unsupported XML constructs and resource-limit excesses with `XmlError`. It has no Cargo
dependencies. Preview status describes the current API stability tier.
