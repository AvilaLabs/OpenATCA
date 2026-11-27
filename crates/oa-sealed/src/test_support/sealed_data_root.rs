use std::path::PathBuf;

pub fn data_root(manifest_dir: &str) -> PathBuf {
    std::env::var_os("OPENATCA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(manifest_dir).join("../../../openatca-data"))
}
