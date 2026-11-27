use std::path::{Path, PathBuf};
use std::process::Command;

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

pub fn assert_sealed_refusal(program: &str, args: &[&str]) {
    // seal-allow: this test executes the consumer and asserts its guard rejects sealed paths.
    let output = Command::new(program)
        .args(args)
        .output()
        .expect("run sealed-path consumer");
    assert_eq!(output.status.code(), Some(2), "{args:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("sealed"));
}

pub fn assert_no_output(path: &Path) {
    assert!(!path.exists());
}

pub fn assert_usage_refusal(program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .output()
        .expect("run usage refusal");
    assert_eq!(output.status.code(), Some(2));
}
