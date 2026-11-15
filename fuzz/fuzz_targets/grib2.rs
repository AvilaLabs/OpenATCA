#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = oa_grib2::message::scan_messages(data);
    let d = oa_grib2::message::decode_stream(data);
    for f in d.fields.iter().take(4) {
        let _ = oa_grib2::sample_bilinear(f, 45.0, -90.0);
    }
    let _ = oa_grib2::message::inspect_stream(data);
    let _ = oa_grib2::message::decode_message(data);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = oa_grib2::parse_idx(text);
    }
});
