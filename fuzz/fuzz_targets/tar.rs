#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_tar::{Next, TarReader};

fuzz_target!(|data: &[u8]| {
    let mut r = TarReader::new(vec![data], oa_tar::DEFAULT_LIMITS);
    let mut out = Vec::new();
    // Bounded: every entry consumes at least one block, so this ends with the input.
    for _ in 0..data.len() / 512 + 2 {
        match r.next_entry() {
            Ok(Next::Entry(_)) => {
                out.clear();
                if r.read_entry_to_vec(&mut out).is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
});
