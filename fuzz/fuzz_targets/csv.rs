#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_csv::{CsvError, Reader};
use std::io::BufReader;

type Outcome = (Vec<String>, Vec<(u64, Vec<String>)>, Option<CsvError>);

// Reads the whole input through `reader`, touching every accessor.
fn read_all<R: std::io::BufRead>(reader: R) -> Result<Outcome, CsvError> {
    let mut r = Reader::new("fuzz.csv", reader)?;
    let names: Vec<String> = r.header().names().to_vec();
    let h = r.header();
    let _ = (
        h.source(),
        h.len(),
        h.is_empty(),
        h.contains("A"),
        h.index_of("A"),
    );
    let _ = r.source();
    let mut rows = Vec::new();
    let mut err = None;
    loop {
        match r.next_record() {
            Ok(Some(rec)) => {
                let _ = (
                    rec.len(),
                    rec.is_empty(),
                    rec.field(0),
                    rec.get(r.header(), "A"),
                );
                rows.push((rec.line(), rec.fields().map(str::to_string).collect()));
            }
            Ok(None) => break,
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    Ok((names, rows, err))
}

// The first byte (1-64) is the BufReader capacity, so records and the BOM straddle buffer
// refills at every offset. The same bytes must give the same result with the default buffer.
fuzz_target!(|data: &[u8]| {
    let Some((&cap, body)) = data.split_first() else {
        return;
    };
    let cap = usize::from(cap % 64) + 1;
    let small = read_all(BufReader::with_capacity(cap, body));
    let normal = read_all(BufReader::new(body));
    assert_eq!(small, normal, "result depends on the buffer size ({cap})");
});
