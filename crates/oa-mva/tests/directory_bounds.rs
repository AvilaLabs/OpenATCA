use oa_mva::load_directory;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> std::io::Result<PathBuf> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("oa-mva-directory-bound-{id}"));
    fs::create_dir(&path)?;
    Ok(path)
}

// REQ: OA-GEN-003
#[test]
fn chart_directory_refuses_more_than_sixty_four_xml_files_before_parsing(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = scratch()?;
    for index in 0..65 {
        fs::write(dir.join(format!("chart-{index:02}.xml")), b"")?;
    }
    let error = match load_directory(&dir) {
        Ok(_) => return Err("expected the XML file budget to refuse the directory".into()),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("64 XML files"));
    fs::remove_dir_all(dir)?;
    Ok(())
}
