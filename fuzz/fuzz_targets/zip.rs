#![no_main]
use libfuzzer_sys::fuzz_target;

// Small limits keep one run cheap; the zip code is the same at any limit.
const LIMITS: oa_zip::Limits = oa_zip::Limits {
    max_entries: 1024,
    max_entry_bytes: 1 << 24,
    max_total_bytes: 1 << 26,
    max_ratio: 1000,
    ratio_exempt_bytes: 1 << 16,
    max_name_bytes: 1024,
};

fuzz_target!(|data: &[u8]| {
    for limits in [oa_zip::DEFAULT_LIMITS, LIMITS] {
        if let Ok(a) = oa_zip::Archive::parse_with_limits(data, limits) {
            for e in a.entries().iter().take(16) {
                let _ = oa_zip::is_safe_path(e.name());
                let _ = a.extract_entry(e);
            }
        }
    }
});
