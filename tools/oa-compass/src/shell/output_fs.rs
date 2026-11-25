//! Shell (B1): private output creation for commands that write raw or otherwise sensitive input-derived data.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::guard::refuse_holdout_out;

fn refuse_sealed_output(path: &Path) -> io::Result<()> {
    refuse_holdout_out(path).map_err(|e| io::Error::other(e.to_string()))
}

pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    refuse_sealed_output(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

pub(super) fn create_new_private_file(path: &Path) -> io::Result<File> {
    // seal-allow: output_fs::create_new_private_file is write-only; local refuse_holdout_out runs before open, and create_new refuses existing names.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    refuse_sealed_output(path)?;
    options.open(path)
}

pub(super) fn write_new_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_new_private_file(path)?;
    file.write_all(bytes)
}
