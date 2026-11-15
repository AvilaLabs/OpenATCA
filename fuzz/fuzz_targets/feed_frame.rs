#![no_main]
use libfuzzer_sys::fuzz_target;

// Raw bytes pushed into the OAF1 frame reader in two chunkings (whole, and 7 bytes at a time),
// draining frames as they come.
fn drain(data: &[u8], chunk: usize) {
    let mut buf = vec![0u8; oa_feed::MAX_FRAME_LEN];
    let Ok(mut r) = oa_feed::FrameReader::new(&mut buf) else {
        return;
    };
    let mut rest = data;
    loop {
        let n = r.push(&rest[..rest.len().min(chunk)]);
        rest = &rest[n..];
        while r.next().is_some() {}
        if n == 0 || rest.is_empty() {
            break;
        }
    }
    let _ = r.finish();
}

fuzz_target!(|data: &[u8]| {
    drain(data, usize::MAX);
    drain(data, 7);
});
