#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(g) = oa_tiff::read_geotiff(data) {
        let _ = oa_tiff::grid_elevation_at(&g, 45.0, -90.0);
    }
});
