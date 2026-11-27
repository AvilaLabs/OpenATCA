use std::path::PathBuf;

pub struct SealedInputs {
    pub raw_sample: PathBuf,
    pub config: PathBuf,
    pub output: PathBuf,
}

pub fn sealed_inputs(manifest_dir: &str) -> SealedInputs {
    let root = std::env::var_os("OPENATCA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(manifest_dir).join("../../../openatca-data"));
    SealedInputs {
        raw_sample: root.join("raw/adsblol/2026.11.20/a.oaf"),
        config: root.join("scds/stdds-20261119-holdout/k.cfg"),
        output: root.join("scds/stdds-smoke-20261119T1/o"),
    }
}
