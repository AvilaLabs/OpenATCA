use std::path::PathBuf;

pub fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("oa-sealed-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}
