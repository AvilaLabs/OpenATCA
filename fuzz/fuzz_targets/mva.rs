#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_mva::{SectorIx, SetKind};
use std::cell::RefCell;

thread_local! {
    static PARSER: RefCell<oa_xml::Parser> =
        RefCell::new(oa_mva::new_parser().expect("default limits are valid"));
}

fuzz_target!(|data: &[u8]| {
    PARSER.with(|p| {
        let mut m = oa_mva::new_model();
        let Ok(l) = oa_mva::load_message(&mut m, &mut p.borrow_mut(), data) else {
            return;
        };
        let _ = (l.chart, l.sectors, l.rings, l.vertices);
        let _ = (oa_mva::chart_count(&m), oa_mva::chart_set_text(&m, 0));
        let _ = oa_mva::sector_range_of_chart(&m, 0);
        let (mut lat, mut lon) = (30.0, -90.0);
        for i in 0..oa_mva::sector_count(&m) {
            let s = SectorIx(i as u32);
            let _ = (
                oa_mva::facility_text(&m, s),
                oa_mva::sector_name_text(&m, s),
            );
            for r in oa_mva::ring_range(&m, s) {
                let _ = oa_mva::ring_is_hole(&m, r);
                let (la, lo) = oa_mva::ring_vertices(&m, r);
                if let (Some(&a), Some(&b)) = (la.first(), lo.first()) {
                    lat = f64::from(a) / 1e7;
                    lon = f64::from(b) / 1e7;
                }
                let _ = oa_mva::locate_in_ring(la, lo, 0, 0);
            }
            let _ = oa_mva::sector_distance_nm(&m, s, lat, lon);
        }
        for set in [SetKind::Fus3, SetKind::Fus5, SetKind::Tav, SetKind::Sub] {
            let mut out = Vec::new();
            let _ = oa_mva::sectors_containing(&m, lat, lon, set, &mut out);
            let _ = oa_mva::min_altitude_at(&m, lat, lon, set);
            let _ = oa_mva::min_altitude_with_sub(&m, lat, lon, set, "ZZZ");
            let _ = oa_mva::sectors_within_radius(&m, lat, lon, 10.0, set);
        }
    });
});
