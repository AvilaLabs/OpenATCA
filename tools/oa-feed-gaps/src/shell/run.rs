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
    let clip = match &a.clip_log {
        Some(p) => Some(parse_clip_log(&read_log(p)?)),
        None => None,
    };
    Ok(render(a.window, &[sfdps, adsb], clip.as_ref()))
}

/// Writes `text` to a temporary file next to `out`, then renames it over `out`.
pub fn write_atomic(out: &Path, text: &str) -> io::Result<()> {
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".partial");
    let tmp = std::path::PathBuf::from(tmp);
    let r = fs::write(&tmp, text.as_bytes()).and_then(|()| fs::rename(&tmp, out));
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
