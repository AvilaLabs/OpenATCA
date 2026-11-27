use std::path::Path;

pub fn assert_no_output(path: &Path) {
    assert!(!path.exists());
}
