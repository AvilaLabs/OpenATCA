use std::path::PathBuf;

mod root {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-support/sealed_data_root.rs"
    ));
}

pub struct SealedInputs {
    pub raw_sample: PathBuf,
    pub config: PathBuf,
    pub output: PathBuf,
}

pub fn sealed_inputs(manifest_dir: &str) -> SealedInputs {
    let root = root::data_root(manifest_dir);
    SealedInputs {
        raw_sample: root.join("raw/adsblol/2026.11.20/a.oaf"),
        config: root.join("scds/stdds-20261119-holdout/k.cfg"),
        output: root.join("scds/stdds-smoke-20261119T1/o"),
    }
}
