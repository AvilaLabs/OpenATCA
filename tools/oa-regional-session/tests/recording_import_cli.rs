#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "oa-recording-import-{}-{nonce}",
        std::process::id()
    ))
}

fn write_oaf(path: &Path, kind: u8, sequence: u64, receipt_ns: i64, jms_ms: i64, payload: &[u8]) {
    let mut frame = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
    let len =
        oa_feed::encode_frame(&mut frame, kind, 0, sequence, receipt_ns, jms_ms, payload).unwrap();
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

// REQ: OA-REG-013
#[test]
fn synthetic_oaf_import_runs_through_tracking_and_stage_contract() {
    let scratch = scratch_dir();
    fs::create_dir(&scratch).unwrap();
    let sfdps_path = scratch.join("synthetic-sfdps.oaf");
    let adsb_path = scratch.join("synthetic-adsb.oaf");
    let imported = scratch.join("imported");
    let staged = scratch.join("staged");
    let run = scratch.join("run");

    let xml = br#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="2030-01-02T03:04:08.000Z"><gufi codeSpace="urn:uuid">21111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message></ns5:MessageCollection>"#;
    let receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:05.250Z").unwrap();
    let receipt_ns = receipt_ms
        .checked_mul(1_000_000)
        .unwrap()
        .checked_add(100_000)
        .unwrap();
    write_oaf(
        &sfdps_path,
        oa_feed::Kind::Message as u8,
        1,
        receipt_ns,
        receipt_ms,
        xml,
    );

    // Two valid airborne reports one second apart exercise actual tracker confirmation. Ground
    // and missing-altitude rows retain their distinct exclusions; a final malformed row is
    // quarantined by exact source ordinal without discarding its healthy siblings.
    // Keep the newest accepted track report one second before the 03:04:06 issue cutoff, so this
    // test exercises current surveillance rather than the product-withheld Slow-feed mode.
    let base_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:04.000Z").unwrap();
    let base_ns = base_ms.checked_mul(1_000_000).unwrap();
    let mut reports = oa_adsb::Reports::default();
    for row in [
        adsb_row(base_ns, 41.900_000_0, -88.300_000_0),
        // 450 kt east advances about 0.00279 degrees of longitude here in one second.
        adsb_row(base_ns + 1_000_000_000, 41.900_000_0, -88.297_210_0),
        {
            let mut row = adsb_row(base_ns + 1_250_000_000, 41.900_200_0, -88.298_000_0);
            row.flags |= oa_adsb::F_GROUND;
            row.baro_alt_ft = oa_adsb::NONE_I32;
            row
        },
        {
            let mut row = adsb_row(base_ns + 1_500_000_000, 41.900_300_0, -88.297_000_0);
            row.baro_alt_ft = oa_adsb::NONE_I32;
            row
        },
        adsb_row(base_ns + 2_000_000_000, 41.900_400_0, -88.296_000_0),
    ] {
        oa_adsb::reports_push(&mut reports, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &reports, 0, 5).unwrap();
    let malformed_at = oa_adsb::PAYLOAD_HEADER_LEN + 4 * oa_adsb::RECORD_LEN;
    *payload.get_mut(malformed_at + 7).unwrap() = 0x7f;
    write_oaf(
        &adsb_path,
        oa_feed::Kind::Message as u8,
        1,
        receipt_ns,
        receipt_ms,
        &payload,
    );

    // A standalone copy avoids the Cargo target hardlink failing OA-SEAL-002.
    let importer = scratch.join("regional-recording-import");
    fs::copy(env!("CARGO_BIN_EXE_regional-recording-import"), &importer).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&importer, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let import_result = Command::new(&importer)
        .args([
            "--adsb",
            adsb_path.to_str().unwrap(),
            "--sfdps",
            sfdps_path.to_str().unwrap(),
            "--centre",
            "ZAU1",
            "--out",
            imported.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        import_result.status.success(),
        "{}",
        String::from_utf8_lossy(&import_result.stderr)
    );
    assert!(imported.join("COMPLETE").is_file());

    let normalized = fs::read_to_string(imported.join("normalized.tsv")).unwrap();
    assert!(
        normalized.lines().any(|line| line.contains("\tFLIGHT\t")),
        "{normalized}"
    );
    let position = normalized
        .lines()
        .skip(1)
        .find(|line| line.contains("\tPOS\t"))
        .unwrap();
    let fields = position.split('\t').collect::<Vec<_>>();
    // OAF1 archive rows use their own report clocks, not the container's later receipt.
    // Kills moving every row to the final frame cutoff, which loses confirmation cadence.
    let position_cutoffs: Vec<i64> = normalized
        .lines()
        .skip(1)
        .filter(|line| line.contains("\tPOS\t"))
        .map(|line| line.split('\t').nth(1).unwrap().parse().unwrap())
        .collect();
    assert_eq!(position_cutoffs, [base_ns, base_ns + 1_000_000_000]);
    assert_eq!(
        fields.get(5).copied(),
        Some("00000000000000000000000000000000")
    );
    assert_eq!(fields.get(6).copied(), Some("ABCDEF"));
    let provenance = fs::read_to_string(imported.join("provenance.tsv")).unwrap();
    assert!(provenance.contains(&format!("\t{receipt_ns}\t")));
    let counts = fs::read_to_string(imported.join("counts.tsv")).unwrap();
    assert!(counts.contains("adsb_ground_excluded: 1"), "{counts}");
    assert!(
        counts.contains("adsb_unknown_ground_refused: 1"),
        "{counts}"
    );
    assert!(
        counts.contains("adsb_unknown_gufi_positions: 2"),
        "{counts}"
    );
    assert!(counts.contains("adsb_rows_dropped: 1"), "{counts}");
    assert!(counts.contains("quarantine_records\t3"), "{counts}");
    assert!(
        counts.contains("manual_handling_required\t3\tstatus=REVIEW_REQUIRED"),
        "{counts}"
    );
    assert!(counts.contains("sfdps_rows_refused: 1"), "{counts}");
    assert!(counts.contains("sfdps_rows_decoded: 2"), "{counts}");
    let quarantine = fs::read_to_string(imported.join("quarantine.tsv")).unwrap();
    assert!(quarantine.contains("OA-REGIONAL-QUARANTINE\t1"));
    assert!(
        quarantine.contains("ITEM\tSURVEILLANCE\t0\t1\t0\t4\t4\t"),
        "{quarantine}"
    );
    assert!(
        quarantine.contains("ITEM\tSURVEILLANCE\t0\t1\t0\t5\t5\t"),
        "{quarantine}"
    );
    assert!(
        quarantine.contains("ITEM\tFLIGHT\t1\t1\t0\t2\t2\t"),
        "{quarantine}"
    );
    let manifest_text = fs::read_to_string(imported.join("manifest.tsv")).unwrap();
    assert!(manifest_text.contains("quarantine_sha256\t"));

    let stage_result = Command::new(env!("CARGO_BIN_EXE_oa-regional-session"))
        .args([
            "stage",
            "--input",
            imported.join("normalized.tsv").to_str().unwrap(),
            "--out",
            staged.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        stage_result.status.success(),
        "{}",
        String::from_utf8_lossy(&stage_result.stderr)
    );
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
        .args([
            "run",
            "--input",
            imported.join("normalized.tsv").to_str().unwrap(),
            "--synthetic-model",
            "--out",
            run.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        run_result.status.success(),
        "{}",
        String::from_utf8_lossy(&run_result.stderr)
    );
    let manifest = fs::read(run.join("manifest.tsv")).unwrap();
    let complete = fs::read_to_string(run.join("COMPLETE")).unwrap();
    assert_eq!(
        complete.trim(),
        oa_sha256::to_hex(&oa_sha256::sha256(&manifest))
    );
    let evidence = fs::read_to_string(run.join("frame-0001.txt")).unwrap();
    assert!(evidence.contains("address: 11259375"), "{evidence}");
    let ca_stats = evidence.split("ca_stats: CycleStats {").nth(1).unwrap();
    assert!(ca_stats.contains("tracks: 1,"), "{evidence}");
    assert!(evidence.contains("ca_input_short: 0"), "{evidence}");
    assert!(evidence.contains("confirmed: 1"), "{evidence}");
    assert!(evidence.contains("tracks: ["), "{evidence}");
    fs::remove_dir_all(Path::new(&scratch)).unwrap();
}

// REQ: OA-MIG-415
#[test]
fn hour_count_cli_accepts_fourteen_sfdps_segments_and_counts_source_messages_only() {
    const NS_PER_HOUR: i64 = 3_600_000_000_000;
    const UTC_DAY_START_NS: i64 = 1_893_542_400_000_000_000;
    const BUSY_HOUR: usize = 14;
    const OTHER_HOUR: usize = 13;
    const HOUR_ROW_TAG: &str = "hour";
    const FLIGHT_ROW_TAG: &str = "flight";

    let scratch = scratch_dir();
    fs::create_dir(&scratch).unwrap();
    let importer = scratch.join("regional-recording-import");
    fs::copy(env!("CARGO_BIN_EXE_regional-recording-import"), &importer).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&importer, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut args = Vec::new();
    let mut busy_hour_messages = 0u64;
    let mut other_hour_messages = 0u64;
    let mut sfdps_segments = 0usize;
    for index in 0..14usize {
        let path = scratch.join(format!("synthetic-sfdps-{index}.oaf"));
        let hour = if index == 0 {
            other_hour_messages += 1;
            OTHER_HOUR
        } else {
            busy_hour_messages += 1;
            BUSY_HOUR
        };
        let hour_offset = i64::try_from(hour).unwrap() * NS_PER_HOUR;
        let within_hour = i64::try_from(index).unwrap() * 1_000_000_000;
        let receipt_ns = UTC_DAY_START_NS + hour_offset + within_hour;
        write_oaf(
            &path,
            oa_feed::Kind::Message as u8,
            u64::try_from(index + 1).unwrap(),
            receipt_ns,
            receipt_ns / 1_000_000,
            b"synthetic message frame",
        );
        args.push("--sfdps".into());
        args.push(path.into_os_string());
        sfdps_segments += 1;
    }
    let adsb_path = scratch.join("synthetic-adsb.oaf");
    let adsb_receipt = UTC_DAY_START_NS + i64::try_from(BUSY_HOUR).unwrap() * NS_PER_HOUR;
    write_oaf(
        &adsb_path,
        oa_feed::Kind::Message as u8,
        1,
        adsb_receipt,
        adsb_receipt / 1_000_000,
        b"synthetic adsb frame",
    );
    args.push("--adsb".into());
    args.push(adsb_path.into_os_string());
    args.push("--count-hours".into());
    args.push(UTC_DAY_START_NS.to_string().into());
    let output_path = scratch.join("hour-counts.tsv");
    args.push("--hours-out".into());
    args.push(output_path.clone().into_os_string());

    let result = Command::new(&importer).args(args).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(output_path).unwrap();
    let busy_hour_label = format!("{BUSY_HOUR:02}");
    let other_hour_label = format!("{OTHER_HOUR:02}");
    let read_hour_count = |label: &str| {
        report
            .lines()
            .filter_map(|line| {
                let fields = line.split('\t').collect::<Vec<_>>();
                (fields.first() == Some(&"hour") && fields.get(1) == Some(&label))
                    .then(|| fields.get(4).and_then(|value| value.parse::<u64>().ok()))
                    .flatten()
            })
            .next()
    };
    assert_eq!(read_hour_count(&busy_hour_label), Some(busy_hour_messages));
    assert_eq!(
        read_hour_count(&other_hour_label),
        Some(other_hour_messages)
    );
    let stream_rows = report
        .lines()
        .filter(|line| line.split('\t').next() == Some("stream"))
        .count();
    let expected_stream_rows = sfdps_segments + 1;
    assert_eq!(stream_rows, expected_stream_rows);
    let output_tags = report
        .lines()
        .filter_map(|line| line.split('\t').next())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(output_tags.contains(HOUR_ROW_TAG));
    assert!(!output_tags.contains(FLIGHT_ROW_TAG));
    fs::remove_dir_all(Path::new(&scratch)).unwrap();
}

// REQ: OA-MIG-415
#[test]
fn window_import_is_half_open_streamed_and_emits_an_hour_envelope() {
    const NS_PER_HOUR: i64 = 3_600_000_000_000;
    const NS_PER_MINUTE: i64 = 60_000_000_000;
    const UTC_DAY_START_NS: i64 = 1_794_614_400_000_000_000;
    const WINDOW_HOUR: i64 = 14;
    const NORMALIZED_HEADER: &str = "OA-RS\t1";
    const HOUR_MANIFEST_HEADER: &str = "OA_RECORDING_HOUR_MANIFEST\t1";
    const HOUR_COMPLETE_NAME: &str = "HOUR_COMPLETE";
    const QUARANTINE_HEADER: &str = "OA-RS-QUARANTINE-AGGREGATE\t1";

    let scratch = scratch_dir();
    fs::create_dir(&scratch).unwrap();
    let sfdps_inside = scratch.join("sfdps-inside.oaf");
    let sfdps_at_end = scratch.join("sfdps-at-end.oaf");
    let adsb_inside = scratch.join("adsb-inside.oaf");
    let imported = scratch.join("window-import");
    let importer = scratch.join("regional-recording-import");
    fs::copy(env!("CARGO_BIN_EXE_regional-recording-import"), &importer).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&importer, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let start_ns = UTC_DAY_START_NS + WINDOW_HOUR * NS_PER_HOUR;
    let end_ns = start_ns + NS_PER_MINUTE;
    let source_time_inside = "2026-11-14T14:00:00.000Z";
    let source_time_at_end = "2026-11-14T14:01:00.000Z";
    let xml = |timestamp: &str| {
        format!(
            r#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="{timestamp}"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message></ns5:MessageCollection>"#
        )
    };
    let payload_inside = xml(source_time_inside);
    let payload_at_end = xml(source_time_at_end);
    let receipt_ms = start_ns / 1_000_000;
    write_oaf(
        &sfdps_inside,
        oa_feed::Kind::Message as u8,
        1,
        start_ns,
        receipt_ms,
        payload_inside.as_bytes(),
    );
    write_oaf(
        &sfdps_at_end,
        oa_feed::Kind::Message as u8,
        2,
        end_ns,
        end_ns / 1_000_000,
        payload_at_end.as_bytes(),
    );
    let mut reports = oa_adsb::Reports::default();
    oa_adsb::reports_push(
        &mut reports,
        &adsb_row(start_ns, 41.900_000_0, -88.300_000_0),
    );
    let mut adsb_payload = Vec::new();
    oa_adsb::encode_payload(&mut adsb_payload, &reports, 0, 1).unwrap();
    write_oaf(
        &adsb_inside,
        oa_feed::Kind::Message as u8,
        1,
        start_ns,
        receipt_ms,
        &adsb_payload,
    );

    let result = Command::new(&importer)
        .args([
            "--sfdps",
            sfdps_inside.to_str().unwrap(),
            "--sfdps",
            sfdps_at_end.to_str().unwrap(),
            "--adsb",
            adsb_inside.to_str().unwrap(),
            "--start",
            &start_ns.to_string(),
            "--end",
            &end_ns.to_string(),
            "--centre",
            "ZAU1",
            "--out",
            imported.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let normalized = fs::read_to_string(imported.join("normalized.tsv")).unwrap();
    let normalized_lines = normalized.lines().collect::<Vec<_>>();
    assert_eq!(normalized_lines.first().copied(), Some(NORMALIZED_HEADER));
    let event_lines = normalized_lines
        .iter()
        .filter(|line| line.starts_with("v1\t"))
        .count();
    let expected_in_window_events = 2usize;
    assert_eq!(event_lines, expected_in_window_events);

    let counts = fs::read_to_string(imported.join("counts.tsv")).unwrap();
    let expected_outside_messages = format!("outside_window_messages\t{}", 1u64);
    assert!(counts.lines().any(|line| line == expected_outside_messages));
    let provenance = fs::read_to_string(imported.join("provenance.tsv")).unwrap();
    let expected_stream_lines = 3usize;
    assert_eq!(
        provenance
            .lines()
            .filter(|line| line.starts_with("STREAM\t"))
            .count(),
        expected_stream_lines
    );
    let quarantine = fs::read_to_string(imported.join("quarantine.tsv")).unwrap();
    assert_eq!(quarantine.lines().next(), Some(QUARANTINE_HEADER));
    let manifest = fs::read_to_string(imported.join("hour-manifest.tsv")).unwrap();
    assert_eq!(manifest.lines().next(), Some(HOUR_MANIFEST_HEADER));
    assert!(imported.join(HOUR_COMPLETE_NAME).is_file());
    assert!(!imported.join("COMPLETE").exists());
    let manifest_bytes = fs::read(imported.join("hour-manifest.tsv")).unwrap();
    let complete = fs::read_to_string(imported.join(HOUR_COMPLETE_NAME)).unwrap();
    assert_eq!(
        complete.trim(),
        oa_sha256::to_hex(&oa_sha256::sha256(&manifest_bytes))
    );

    let incomplete_pair_output = scratch.join("missing-end");
    let incomplete_pair = Command::new(&importer)
        .args([
            "--sfdps",
            sfdps_inside.to_str().unwrap(),
            "--adsb",
            adsb_inside.to_str().unwrap(),
            "--start",
            &start_ns.to_string(),
            "--centre",
            "ZAU1",
            "--out",
            incomplete_pair_output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!incomplete_pair.status.success());
    assert!(!incomplete_pair_output.exists());
    fs::remove_dir_all(Path::new(&scratch)).unwrap();
}
