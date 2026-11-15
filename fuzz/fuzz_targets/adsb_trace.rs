#![no_main]
use libfuzzer_sys::fuzz_target;

// A readsb / adsb.lol trace file (JSON bytes) through the trace decoder, with the ZAU box and
// with a filter that keeps everything.
fuzz_target!(|data: &[u8]| {
    let all = oa_adsb::Filter {
        lat_min_e7: i32::MIN,
        lat_max_e7: i32::MAX,
        lon_min_e7: i32::MIN,
        lon_max_e7: i32::MAX,
        from_ns: i64::MIN,
        to_ns: i64::MAX,
    };
    for f in [&oa_adsb::ZAU_BOX, &all] {
        let mut out = oa_adsb::Reports::default();
        let mut st = oa_adsb::Stats::default();
        let _ = oa_adsb::decode_trace(data, f, &mut out, &mut st);
    }
});
