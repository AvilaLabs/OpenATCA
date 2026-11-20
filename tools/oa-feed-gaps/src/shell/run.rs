//! Shell (B1): one check, and the report file.

use std::fs;
use std::io;
use std::path::Path;

use crate::core::clip::parse_clip_log;
use crate::core::report::render;
use crate::shell::args::Args;
use crate::shell::read::{read_adsb, read_sfdps};

/// Reads both inputs and renders the report; the flag is whether the window is damaged.
pub fn check(a: &Args) -> io::Result<(String, bool)> {
    let sfdps = read_sfdps(&a.sfdps_dir, a.window)?;
    let adsb = read_adsb(&a.adsb, a.window)?;
    let clip = parse_clip_log(&read_log(&a.clip_log)?);
    Ok(render(a.window, &[sfdps, adsb], &clip))
}

/// Refuses an output path that already exists (a link included).
pub fn refuse_existing(out: &Path) -> io::Result<()> {
    // seal-allow: a stat of the tool's own output path, which is never read
    if fs::symlink_metadata(out).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{}: already exists", out.display()),
        ));
    }
    Ok(())
}

/// Writes `text` to a new temporary file next to `out`, syncs it, then renames it to `out`. An
/// existing `out` or temporary file is an error and is never replaced.
pub fn write_atomic(out: &Path, text: &str) -> io::Result<()> {
    refuse_existing(out)?;
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".partial");
    let tmp = std::path::PathBuf::from(tmp);
    // A temporary file that already exists is someone else's: error, and leave it.
    // seal-allow: create-new write of the tool's own report; nothing is read
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    let r = (|| {
        io::Write::write_all(&mut f, text.as_bytes())?;
        f.sync_all()?;
        refuse_existing(out)?;
        fs::rename(&tmp, out)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// The clip tool's log, through the shared guard.
fn read_log(path: &Path) -> io::Result<String> {
    oa_sealed::refuse_io(path)?;
    let mut s = String::new();
    io::Read::read_to_string(&mut oa_sealed::open(path)?, &mut s)?;
    Ok(s)
}
