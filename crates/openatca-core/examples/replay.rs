//! Runs the tracker on a synthetic ADS-B recording or a clip supplied by the caller.

use openatca_core::{ReplaySettings, SurveillanceReplay};

const SYNTHETIC_CLIP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/synthetic-adsb.oaf"
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os();
    let path = args.nth(1);
    if args.next().is_some() {
        return Err("usage: replay [ADS-B clip.oaf]".into());
    }
    let path = match path.as_deref() {
        Some(path) => path.to_str().ok_or("the clip path must be valid UTF-8")?,
        None => SYNTHETIC_CLIP,
    };
    let mut replay = SurveillanceReplay::open(path, ReplaySettings::default())?;
    println!("input health: {:?}", replay.input_health());

    while let Some(frame) = replay.next_frame() {
        for track in frame.tracks() {
            println!("{track:?}");
        }
        println!("{} ns tracker health: {:?}", frame.time_ns, frame.health);
    }
    Ok(())
}
