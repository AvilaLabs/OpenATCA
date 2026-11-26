# `oa-csv`

This crate provides a bounded streaming RFC 4180 CSV reader. See the [text API profile](https://github.com/AvilaLabs/OpenATCA/blob/main/docs/core-api/foundations/text.md) for grammar, limits and errors.

**Tier: Preview.** The Cargo distribution package name is `openatca-csv`; the Rust library import
name remains `oa_csv`.

```toml
[dependencies]
# Use this version when the Preview package is available from your configured registry.
openatca-csv = "0.1.0"
```

```rust
use oa_csv::{CsvError, Reader};
use std::io::Cursor;

fn read_names() -> Result<(), CsvError> {
    let input = b"name\r\nAda\r\n";
    let mut reader = Reader::new("example.csv", Cursor::new(&input[..]))?;
    while let Some(record) = reader.next_record()? {
        let _name = record.get(reader.header(), "name")?;
    }
    Ok(())
}
```

`Reader::new` reads and validates the header first, then streams records subject to the crate's
named size limits. `Reader::with_limits` accepts an explicit limit profile. Errors include the
source label and 1-based physical record-start line. The crate has no Cargo dependencies.
