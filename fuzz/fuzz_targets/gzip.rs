#![no_main]
use libfuzzer_sys::fuzz_target;

// gzip members and raw DEFLATE, with an output cap of 64 MiB.
fuzz_target!(|data: &[u8]| {
    let limits = oa_zip::InflateLimits {
        max_output: 64 << 20,
        max_ratio: 1000,
        ratio_exempt_bytes: 1 << 16,
    };
    let mut g = oa_zip::GzipReader::new(limits);
    let _ = g.push::<()>(data, &mut |_| Ok(()));
    let _ = g.finish();
    let mut inf = oa_zip::Inflater::new(limits);
    let _ = inf.push::<()>(data, &mut |_| Ok(()));
    let _ = oa_zip::inflate(data, 0, 64 << 20);
});
