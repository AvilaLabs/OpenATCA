#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_dir() -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("oa-recording-import-{}-{nonce}", std::process::id()))
}

#[test]
fn synthetic_oaf_import_is_provenanced_and_passes_the_ordinary_stage_cli() {
    let scratch = scratch_dir();
    fs::create_dir(&scratch).unwrap();
    let source_path = scratch.join("synthetic-sfdps.oaf");
    let imported = scratch.join("imported");
    let staged = scratch.join("staged");

    let xml = br#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message></ns5:MessageCollection>"#;
    let receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:05.250Z").unwrap();
    let receipt_ns = receipt_ms.checked_mul(1_000_000).unwrap().checked_add(100_000).unwrap();
    let issue_ns = receipt_ns.div_euclid(1_000_000_000).checked_add(1).unwrap() * 1_000_000_000;
    let mut frame = vec![0u8; oa_feed::HEADER_LEN + xml.len() + oa_feed::TRAILER_LEN];
    let frame_len = oa_feed::encode_frame(&mut frame, oa_feed::Kind::Message as u8, 0, 1,
        receipt_ns, receipt_ms, xml).unwrap();
    frame.truncate(frame_len);
    fs::write(&source_path, frame).unwrap();

    // A separate copy avoids the Cargo target's hardlinked executable failing OA-SEAL-002.
    let importer = scratch.join("regional-recording-import");
    fs::copy(env!("CARGO_BIN_EXE_regional-recording-import"), &importer).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&importer, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let import_result = Command::new(&importer)
        .args(["--sfdps", source_path.to_str().unwrap(), "--centre", "ZAU1", "--out", imported.to_str().unwrap()])
        .output().unwrap();
    assert!(import_result.status.success(), "{}", String::from_utf8_lossy(&import_result.stderr));
    assert!(imported.join("COMPLETE").is_file());

    let normalized = fs::read_to_string(imported.join("normalized.tsv")).unwrap();
    let row = normalized.lines().nth(1).unwrap();
    let fields = row.split('\t').collect::<Vec<_>>();
    let issue_text = issue_ns.to_string();
    assert_eq!(fields.get(1).copied(), Some(issue_text.as_str()));
    assert_eq!(fields.get(4).copied(), Some("FLIGHT"));
    assert_eq!(fields.get(9).copied(), Some("OWNER"));
    let provenance = fs::read_to_string(imported.join("provenance.tsv")).unwrap();
    assert!(provenance.contains(&format!("\t{receipt_ns}\t")));

    let stage_result = Command::new(env!("CARGO_BIN_EXE_oa-regional-session"))
        .args(["stage", "--input", imported.join("normalized.tsv").to_str().unwrap(), "--out", staged.to_str().unwrap()])
        .output().unwrap();
    assert!(stage_result.status.success(), "{}", String::from_utf8_lossy(&stage_result.stderr));
    assert!(staged.join("COMPLETE").is_file());
    fs::remove_dir_all(Path::new(&scratch)).unwrap();
}
