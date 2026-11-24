#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_dir() -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("oa-recording-import-{}-{nonce}", std::process::id()))
}

fn write_oaf(path: &Path, kind: u8, sequence: u64, receipt_ns: i64, jms_ms: i64, payload: &[u8]) {
    let mut frame = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
    let len = oa_feed::encode_frame(&mut frame, kind, 0, sequence, receipt_ns, jms_ms, payload).unwrap();
    frame.truncate(len);
    fs::write(path, frame).unwrap();
}

fn adsb_row(time_ns: i64, latitude: f64, longitude: f64) -> oa_adsb::Row {
    oa_adsb::Row {
        time_ns,
        icao: 0xABCDEF,
        lat_e7: (latitude * 1.0e7) as i32,
        lon_e7: (longitude * 1.0e7) as i32,
        baro_alt_ft: 35_000,
        gs_dkt: 4500,
        track_ddeg: 900,
        vrate_fpm: 0,
        nic: 8,
        nac_p: 9,
        nac_v: 2,
        sil: 3,
        version: 2,
        source: 0,
        ..oa_adsb::ROW_EMPTY
    }
}

#[test]
fn synthetic_oaf_import_runs_through_tracking_and_stage_contract() {
    let scratch = scratch_dir();
    fs::create_dir(&scratch).unwrap();
    let sfdps_path = scratch.join("synthetic-sfdps.oaf");
    let adsb_path = scratch.join("synthetic-adsb.oaf");
    let imported = scratch.join("imported");
    let staged = scratch.join("staged");
    let run = scratch.join("run");

    let xml = br#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message></ns5:MessageCollection>"#;
    let receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:05.250Z").unwrap();
    let receipt_ns = receipt_ms.checked_mul(1_000_000).unwrap().checked_add(100_000).unwrap();
    let issue_ns = receipt_ns.div_euclid(1_000_000_000).checked_add(1).unwrap() * 1_000_000_000;
    write_oaf(&sfdps_path, oa_feed::Kind::Message as u8, 1, receipt_ns, receipt_ms, xml);

    // Two valid airborne reports one second apart exercise actual tracker confirmation. The
    // following ground and no-altitude rows prove the importer accounts for its exclusions.
    let base_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:03.000Z").unwrap();
    let base_ns = base_ms.checked_mul(1_000_000).unwrap();
    let mut reports = oa_adsb::Reports::default();
    for row in [
        adsb_row(base_ns, 41.900_000_0, -88.300_000_0),
        adsb_row(base_ns + 1_000_000_000, 41.900_100_0, -88.299_000_0),
        {
            let mut row = adsb_row(base_ns + 1_500_000_000, 41.900_200_0, -88.298_000_0);
            row.flags |= oa_adsb::F_GROUND;
            row.baro_alt_ft = oa_adsb::NONE_I32;
            row
        },
        {
            let mut row = adsb_row(base_ns + 2_000_000_000, 41.900_300_0, -88.297_000_0);
            row.baro_alt_ft = oa_adsb::NONE_I32;
            row
        },
    ] {
        oa_adsb::reports_push(&mut reports, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &reports, 0, 4).unwrap();
    write_oaf(&adsb_path, oa_feed::Kind::Message as u8, 1, receipt_ns, receipt_ms, &payload);

    // A standalone copy avoids the Cargo target hardlink failing OA-SEAL-002.
    let importer = scratch.join("regional-recording-import");
    fs::copy(env!("CARGO_BIN_EXE_regional-recording-import"), &importer).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&importer, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let import_result = Command::new(&importer)
        .args(["--adsb", adsb_path.to_str().unwrap(), "--sfdps", sfdps_path.to_str().unwrap(), "--centre", "ZAU1", "--out", imported.to_str().unwrap()])
        .output().unwrap();
    assert!(import_result.status.success(), "{}", String::from_utf8_lossy(&import_result.stderr));
    assert!(imported.join("COMPLETE").is_file());

    let normalized = fs::read_to_string(imported.join("normalized.tsv")).unwrap();
    let position = normalized.lines().skip(1).find(|line| line.contains("\tPOS\t")).unwrap();
    let fields = position.split('\t').collect::<Vec<_>>();
    let issue_text = issue_ns.to_string();
    assert_eq!(fields.get(1).copied(), Some(issue_text.as_str()));
    assert_eq!(fields.get(5).copied(), Some("00000000000000000000000000000000"));
    assert_eq!(fields.get(6).copied(), Some("ABCDEF"));
    let provenance = fs::read_to_string(imported.join("provenance.tsv")).unwrap();
    assert!(provenance.contains(&format!("\t{receipt_ns}\t")));
    let counts = fs::read_to_string(imported.join("counts.tsv")).unwrap();
    assert!(counts.contains("adsb_ground_excluded: 1"), "{counts}");
    assert!(counts.contains("adsb_unknown_ground_refused: 1"), "{counts}");
    assert!(counts.contains("adsb_unknown_gufi_positions: 2"), "{counts}");

    let stage_result = Command::new(env!("CARGO_BIN_EXE_oa-regional-session"))
        .args(["stage", "--input", imported.join("normalized.tsv").to_str().unwrap(), "--out", staged.to_str().unwrap()])
        .output().unwrap();
    assert!(stage_result.status.success(), "{}", String::from_utf8_lossy(&stage_result.stderr));
    let stage_report = fs::read_to_string(staged.join("session-report.tsv")).unwrap();
    assert!(stage_report.contains("STAGED_INPUT_ONLY"));
    assert!(!staged.join("COMPLETE").exists());

    let session = scratch.join("oa-regional-session");
    fs::copy(env!("CARGO_BIN_EXE_oa-regional-session"), &session).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&session, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let run_result = Command::new(&session)
        .args(["run", "--input", imported.join("normalized.tsv").to_str().unwrap(), "--synthetic-model", "--out", run.to_str().unwrap()])
        .output().unwrap();
    assert!(run_result.status.success(), "{}", String::from_utf8_lossy(&run_result.stderr));
    let manifest = fs::read(run.join("manifest.tsv")).unwrap();
    let complete = fs::read_to_string(run.join("COMPLETE")).unwrap();
    assert_eq!(complete.trim(), oa_sha256::to_hex(&oa_sha256::sha256(&manifest)));
    let evidence = fs::read_to_string(run.join("frame-0000.txt")).unwrap();
    assert!(evidence.contains("address: 11259375"), "{evidence}");
    assert!(evidence.contains("ca_input_short: 1"), "{evidence}");
    assert!(evidence.contains("tracks: ["), "{evidence}");
    fs::remove_dir_all(Path::new(&scratch)).unwrap();
}
