#![forbid(unsafe_code)]
//! `oa-clip-sealcheck`: opens an OAF1 clip through the seal guard and checks every row's time and
//! every frame's receive time against the sealed window. Prints counts only. Exits 0 only if
//! nothing is in the window; 1 if anything is, 2 if the clip does not parse (or is refused) or
//! the window is the fail-closed all-time one. `tools/replay_compare.sh` runs it before it builds
//! or runs a commit that may predate the row window. Shell: owns the file and stdout (B3).
//!
//! ```text
//! oa-clip-sealcheck <clip.oaf>
//! ```

use std::process::ExitCode;

use oa_sealed::DropWindow;
use oa_truth::clip::{seal_check_clip, MAX_CLIP_BYTES};

fn run(drops: &mut oa_sealed::DropReport) -> Result<ExitCode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path] = args.as_slice() else {
        return Err("usage: oa-clip-sealcheck <clip.oaf>".to_string());
    };
    let window = DropWindow::sealed();
    if window == DropWindow::all_time() {
        return Err("the seal list has no usable window (fail-closed): clip refused".to_string());
    }
    let c = seal_check_clip(path, MAX_CLIP_BYTES, window).map_err(|e| e.to_string())?;
    drops.add(c.units);
    println!(
        "clip {path}: {} frames, {} rows; in the sealed window: {} frames by receive time, {} rows; unreadable frames: {}",
        c.frames, c.rows, c.frames_in_window, c.rows_in_window, c.unreadable
    );
    if c.unreadable > 0 || c.frames == 0 {
        return Err("the clip does not parse whole: refused".to_string());
    }
    if c.frames_in_window > 0 || c.rows_in_window > 0 {
        eprintln!("oa-clip-sealcheck: the clip holds data in the sealed window: refused");
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let mut drops = oa_sealed::DropReport::to_stdout();
    match run(&mut drops) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("oa-clip-sealcheck: {e}");
            ExitCode::from(2)
        }
    }
}
