#![no_main]
use libfuzzer_sys::fuzz_target;

// The first byte picks the AWC product; the rest is the file.
fuzz_target!(|data: &[u8]| {
    let Some((&sel, body)) = data.split_first() else {
        return;
    };
    match sel % 6 {
        0 => {
            let (mut t, mut s) = (oa_awc::new_metar_table(), oa_awc::MetarStats::default());
            let _ = oa_awc::parse_metar(body, &mut t, &mut s);
            let _ = oa_awc::finalize_metar(&mut t);
        }
        1 => {
            let (mut t, mut s) = (oa_awc::new_taf_table(), oa_awc::TafStats::default());
            let _ = oa_awc::parse_taf(body, &mut t, &mut s);
            let _ = oa_awc::finalize_taf(&mut t);
        }
        2 => {
            let (mut t, mut s) = (oa_awc::new_pirep_table(), oa_awc::PirepStats::default());
            let _ = oa_awc::parse_pirep(body, &mut t, &mut s);
            let _ = oa_awc::finalize_pirep(&mut t);
        }
        n => {
            let (mut t, mut s) = (oa_awc::new_hazard_table(), oa_awc::HazardStats::default());
            let _ = match n {
                3 => oa_awc::parse_sigmet(body, &mut t, &mut s),
                4 => oa_awc::parse_airsigmet(body, &mut t, &mut s),
                _ => oa_awc::parse_gairmet(body, &mut t, &mut s),
            };
            let _ = oa_awc::finalize_hazard(&mut t);
        }
    }
});
