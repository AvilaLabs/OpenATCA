//! CAT021 records built from oa-adsb rows (a hand-written trace in readsb's format, and, when
//! `OPENATCA_DATA` points at the data directory, real adsb.lol rows) round-trip through the
//! encoder and decoder, and the error of every quantised field stays within half its LSB.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::*;
use oa_asterix::cat021::*;
use oa_asterix::units::*;
use oa_asterix::*;

/// The first two rows of oa-adsb's own full-row fixture: the first has an aircraft object, the
/// second has none.
const TRACE: &str = r#"{"icao":"3c66b0","r":"D-AIUP","t":"A320","dbFlags":0,"desc":"AIRBUS A-320","timestamp": 1663259853.016,"trace":[
[7016.59,49.263300,10.614239,25125,446.5,309.0,0,-2176,{"type":"adsb_icao","flight":"DLH7YA  ","alt_geom":25875,"ias":335,"tas":484,"mach":0.796,"track":309.00,"roll":-10.72,"baro_rate":-2176,"geom_rate":-2208,"squawk":"1000","category":"A3","nav_qnh":1012.8,"nic":8,"rc":186,"version":2,"nic_baro":1,"nac_p":8,"nac_v":0,"sil":3,"sil_type":"perhour","gva":2,"sda":2,"alert":0,"spi":0},"adsb_icao",25875,-2208,335,-10.7],
[7024.85,49.273589,10.593278,24825,446.0,306.6,0,-2176,null,"adsb_icao",25550,-2144,337,-1.6]]}"#;

const WIDE: Filter = Filter {
    lat_min_e7: -900_000_000,
    lat_max_e7: 900_000_000,
    lon_min_e7: -1_800_000_000,
    lon_max_e7: 1_800_000_000,
    from_ns: i64::MIN,
    to_ns: i64::MAX,
};

/// A squawk read as four decimal digits becomes the 12-bit octal-digit code.
fn squawk_code(s: u16) -> Option<u16> {
    if s == NONE_U16 {
        return None;
    }
    let d = [s / 1000, s / 100 % 10, s / 10 % 10, s % 10];
    if d.iter().any(|&x| x > 7) {
        return None;
    }
    Some(d[0] << 9 | d[1] << 6 | d[2] << 3 | d[3])
}

/// A CAT021 record from row `i`, with the fields the row has.
fn record(r: &Reports, i: usize) -> Cat021 {
    let mut c = Cat021 {
        source: SourceId { sac: 25, sic: 3 },
        address: Some(r.icao[i] & 0xFF_FFFF),
        descriptor: Some(Trd021 {
            atp: if r.icao[i] & NON_ICAO == 0 { 0 } else { 3 },
            arc: 2,
            ..Trd021::default()
        }),
        position_hr: Some(RawPosition {
            lat: e7_to_raw(r.lat_e7[i], CAT021_POS_HR_BITS),
            lon: e7_to_raw(r.lon_e7[i], CAT021_POS_HR_BITS),
        }),
        ..Cat021::default()
    };
    if r.baro_alt_ft[i] != NONE_I32 {
        // I021/145: LSB 1/4 FL = 25 ft.
        c.flight_level = Some((f64::from(r.baro_alt_ft[i]) / 25.0).round() as i16);
    }
    if r.geom_alt_ft[i] != NONE_I32 {
        c.geometric_height =
            Some((f64::from(r.geom_alt_ft[i]) / CAT021_GEOM_HEIGHT_FT).round() as i16);
    }
    if r.nac_p[i] != NONE_U8 && r.sil[i] != NONE_U8 && r.nic[i] != NONE_U8 {
        // I021/090 first extension: NICBARO 0, SIL, NACp.
        let ext = r.sil[i] << 5 | r.nac_p[i] << 1;
        c.quality = Some(Quality021 {
            nacv: if r.nac_v[i] == NONE_U8 { 0 } else { r.nac_v[i] },
            nic: r.nic[i],
            ext: vec![ext],
        });
    }
    if r.version[i] != NONE_U8 {
        c.mops = Some(Mops {
            version: r.version[i],
            link: 2,
            ..Mops::default()
        });
    }
    c.mode3a = squawk_code(r.squawk[i]);
    if r.callsign[i] != [0; 8] {
        let mut id = [b' '; 8];
        for (k, &ch) in r.callsign[i].iter().enumerate() {
            if ch != 0 {
                id[k] = ch;
            }
        }
        // Real callsigns sometimes carry characters outside the six-bit set (a hyphen, lower
        // case); the encoder refuses those, so such a row is sent without an identification.
        if id
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b' ')
        {
            c.target_id = Some(id);
        }
    }
    if r.gs_dkt[i] != NONE_U16 && r.track_ddeg[i] != NONE_U16 {
        // I021/160: speed LSB 2^-14 NM/s = 3600/16384 kt; track LSB 360/2^16 degrees.
        let speed = (f64::from(r.gs_dkt[i]) / 10.0 * 16_384.0 / 3600.0).round() as u32;
        if speed < 0x8000 {
            c.ground_vector = Some(GroundVector {
                range_exceeded: false,
                speed: speed as u16,
                track: (f64::from(r.track_ddeg[i]) / 10.0 / 360.0 * 65_536.0).round() as u16,
            });
        }
    }
    if r.vrate_fpm[i] != NONE_I32 {
        let raw = (f64::from(r.vrate_fpm[i]) / CAT021_VRATE_FPM).round() as i32;
        if raw.abs() < 16384 {
            let v = FlagS15 {
                range_exceeded: false,
                raw: raw as i16,
            };
            if r.flags[i] & F_RATE_GEOM != 0 {
                c.geometric_rate = Some(v);
            } else {
                c.baro_rate = Some(v);
            }
        }
    }
    c
}

/// Largest errors seen, in the unit of the field.
#[derive(Debug, Default)]
struct Worst {
    lat_deg: f64,
    lon_deg: f64,
    lat_e7: i32,
    lon_e7: i32,
    fl_ft: f64,
    height_ft: f64,
    speed_kt: f64,
    track_deg: f64,
    rate_fpm: f64,
    rows: usize,
}

/// Round-trips the rows through blocks of at most 500 records and compares.
fn check(r: &Reports) -> Worst {
    let mut w = Worst::default();
    let n = reports_len(r);
    let mut i = 0;
    while i < n {
        let end = (i + 500).min(n);
        let recs: Vec<Cat021> = (i..end).map(|k| record(r, k)).collect();
        let bytes = encode_block(&Block::Cat021(recs.clone())).unwrap();
        let (b, len) = decode_block(&bytes).unwrap();
        assert_eq!(len, bytes.len());
        let Block::Cat021(back) = b else { panic!() };
        // The decoder returns exactly what was encoded, and re-encodes to the same bytes.
        assert_eq!(back, recs);
        assert_eq!(encode_block(&Block::Cat021(back.clone())).unwrap(), bytes);
        for (k, d) in (i..end).zip(&back) {
            w.rows += 1;
            assert_eq!(d.address, Some(r.icao[k] & 0xFF_FFFF));
            let p = d.position_hr.unwrap();
            // Position: the count is the nearest to the row's value, so the error is at most
            // half an LSB of I021/131 (0.84e-7 degree, 9 mm), and 1e-7 degree after the
            // conversion back to integer e-7 degree.
            let dlat = (f64::from(p.lat) * CAT021_POS_HR_DEG - f64::from(r.lat_e7[k]) * 1e-7).abs();
            let dlon = (f64::from(p.lon) * CAT021_POS_HR_DEG - f64::from(r.lon_e7[k]) * 1e-7).abs();
            assert!(dlat <= CAT021_POS_HR_DEG / 2.0 + 1e-12, "lat {dlat}");
            assert!(dlon <= CAT021_POS_HR_DEG / 2.0 + 1e-12, "lon {dlon}");
            w.lat_deg = w.lat_deg.max(dlat);
            w.lon_deg = w.lon_deg.max(dlon);
            let elat = (raw_to_e7(p.lat, CAT021_POS_HR_BITS) - r.lat_e7[k]).abs();
            let elon = (raw_to_e7(p.lon, CAT021_POS_HR_BITS) - r.lon_e7[k]).abs();
            assert!(elat <= 1 && elon <= 1, "e7 {elat} {elon}");
            w.lat_e7 = w.lat_e7.max(elat);
            w.lon_e7 = w.lon_e7.max(elon);
            if let Some(fl) = d.flight_level {
                let e = (f64::from(fl) * 25.0 - f64::from(r.baro_alt_ft[k])).abs();
                assert!(e <= 12.5, "flight level error {e} ft");
                w.fl_ft = w.fl_ft.max(e);
            }
            if let Some(h) = d.geometric_height {
                let e = (f64::from(h) * CAT021_GEOM_HEIGHT_FT - f64::from(r.geom_alt_ft[k])).abs();
                assert!(e <= 3.125 + 1e-9, "height error {e} ft");
                w.height_ft = w.height_ft.max(e);
            }
            if let Some(q) = &d.quality {
                assert_eq!(quality_nacp(q), Some(r.nac_p[k]));
                assert_eq!(quality_sil(q), Some(r.sil[k]));
                assert_eq!(q.nic, r.nic[k]);
            }
            if let Some(m) = &d.mops {
                assert_eq!(m.version, r.version[k]);
            }
            if let Some(code) = d.mode3a {
                assert_eq!(Some(code), squawk_code(r.squawk[k]));
            }
            if let Some(g) = &d.ground_vector {
                let kt = f64::from(g.speed) * CAT021_GS_NM_S * 3600.0;
                let e = (kt - f64::from(r.gs_dkt[k]) / 10.0).abs();
                assert!(
                    e <= CAT021_GS_NM_S * 3600.0 / 2.0 + 1e-9,
                    "speed error {e} kt"
                );
                w.speed_kt = w.speed_kt.max(e);
                let deg = f64::from(g.track) * CAT021_ANGLE_DEG;
                let mut e = (deg - f64::from(r.track_ddeg[k]) / 10.0).abs();
                e = e.min(360.0 - e);
                assert!(e <= CAT021_ANGLE_DEG / 2.0 + 1e-9, "track error {e} deg");
                w.track_deg = w.track_deg.max(e);
            }
            for v in [d.baro_rate, d.geometric_rate].into_iter().flatten() {
                let e = (f64::from(v.raw) * CAT021_VRATE_FPM - f64::from(r.vrate_fpm[k])).abs();
                assert!(e <= CAT021_VRATE_FPM / 2.0 + 1e-9, "rate error {e} fpm");
                w.rate_fpm = w.rate_fpm.max(e);
            }
        }
        i = end;
    }
    w
}

// REQ: OA-AST-019
#[test]
fn adsb_rows_round_trip_with_lsb_limited_error() {
    let mut out = Reports::default();
    let mut st = Stats::default();
    decode_trace(TRACE.as_bytes(), &WIDE, &mut out, &mut st).unwrap();
    assert_eq!(reports_len(&out), 2);
    let w = check(&out);
    assert_eq!(w.rows, 2);
    // Row 0 by hand: address 3C66B0, NACp 8, NIC 8, SIL 3, squawk 1000 -> 0o1000, FL 1005
    // (25125 ft / 25), geometric height 4140 (25875 / 6.25), callsign DLH7YA.
    let c = record(&out, 0);
    assert_eq!(c.address, Some(0x3C_66B0));
    assert_eq!(c.flight_level, Some(1005));
    assert_eq!(c.geometric_height, Some(4140));
    assert_eq!(c.mode3a, Some(0o1000));
    assert_eq!(c.target_id, Some(*b"DLH7YA  "));
    assert_eq!(c.mops.unwrap().version, 2);
    let q = c.quality.unwrap();
    assert_eq!(
        (q.nic, quality_nacp(&q), quality_sil(&q)),
        (8, Some(8), Some(3))
    );
    // Row 1 has no aircraft object: no quality indicators, no address-independent extras.
    let c1 = record(&out, 1);
    assert!(c1.quality.is_none() && c1.mops.is_none() && c1.target_id.is_none());
    // Statement of the error bounds used above: positions are within half an LSB of the
    // 131 item (9 mm), flight level within 12.5 ft, geometric height within 3.125 ft,
    // ground speed within 0.11 kt, track within 0.0028 degree, vertical rate within 3.1 ft/min.
    eprintln!("fixture worst errors: {w:?}");
}

// REQ: OA-AST-020
#[test]
fn real_adsb_lol_rows_round_trip() {
    let Some(dir) = std::env::var_os("OPENATCA_DATA") else {
        eprintln!("OPENATCA_DATA is not set: skipping the real ADS-B round trip");
        return;
    };
    let dir = std::path::Path::new(&dir).join("raw/adsblol");
    let part = std::fs::read_dir(&dir).ok().and_then(|rd| {
        rd.flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "aa"))
    });
    let Some(part) = part else {
        eprintln!("no .aa part under {}: skipping", dir.display());
        return;
    };
    let f = std::fs::File::open(&part).unwrap();
    // Take the first 40 000 rows and stop the scan there.
    const WANT: usize = 40_000;
    let mut rows = Reports::default();
    let _ = scan_archive(vec![f], &WIDE, &mut |r: &Reports| -> Result<(), ()> {
        reports_extend(&mut rows, r);
        if reports_len(&rows) >= WANT {
            Err(())
        } else {
            Ok(())
        }
    });
    assert!(
        reports_len(&rows) >= 1000,
        "only {} rows read",
        reports_len(&rows)
    );
    let w = check(&rows);
    eprintln!("real rows: {} round-tripped, worst errors {w:?}", w.rows);
}
