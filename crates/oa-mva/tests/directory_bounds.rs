use oa_mva::load_directory;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("oa-mva-directory-bound-{id}"));
    fs::create_dir(&path).unwrap();
    path
}

// REQ: OA-GEN-003
#[test]
fn chart_directory_refuses_more_than_sixty_four_xml_files_before_parsing() {
    let dir = scratch();
    for index in 0..65 {
        fs::write(dir.join(format!("chart-{index:02}.xml")), b"").unwrap();
    }
    let error = load_directory(&dir).unwrap_err().to_string();
    assert!(error.contains("64 XML files"));
    fs::remove_dir_all(dir).unwrap();
}
