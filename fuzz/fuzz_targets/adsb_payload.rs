#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut lenient = oa_adsb::Reports::default();
    let _ = oa_adsb::decode_payload_lenient(data, &mut lenient);
    let mut strict = oa_adsb::Reports::default();
    let _ = oa_adsb::decode_payload(data, &mut strict);
    for i in 0..oa_adsb::reports_len(&lenient) {
        let _ = oa_adsb::reports_row(&lenient, i);
    }
});
