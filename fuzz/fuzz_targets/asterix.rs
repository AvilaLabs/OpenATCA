#![no_main]
use libfuzzer_sys::fuzz_target;

// Raw bytes into the ASTERIX block decoders. A refusal is fine; a panic is a finding.
fuzz_target!(|data: &[u8]| {
    let _ = oa_asterix::decode_block(data);
    if let Ok(blocks) = oa_asterix::decode_stream(data) {
        // A decoded block must encode again without a panic.
        for b in &blocks {
            let _ = oa_asterix::encode_block(b);
        }
    }
});
