//! The `oa-scope` command line (**shell**, B1, B3): reads a snapshot, draws the static map
//! through the pure core, writes a PPM. Every clock read and file access of the tool is here.
//!
//! ```text
//! oa-scope render --snapshot <file> --centre <lat,lon> --range-nm <n> --size <WxH> --out <file.ppm>
//! ```
//!
//! `--range-nm` is the range like a scope's: nautical miles from the centre to the nearest edge. Prints the build, raster and hash
//! times and the SHA-256 of the frame.

use crate::list::{list_new, DisplayError};
use crate::raster::{frame_digest, frame_len, rasterize, FrameSpec, RenderError};
use crate::scene::{build_scene, scene_caps, view_new};
use oa_airspace::geom::deg_to_e7;
use oa_airspace::snapshot::{decode, MAX_SNAPSHOT_BYTES};
use oa_sha256::to_hex;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

/// Largest image the tool will draw, pixels on a side.
pub const MAX_SIDE: u32 = 8192;
/// Smallest image, pixels on a side.
pub const MIN_SIDE: u32 = 64;

const USAGE: &str = "usage: oa-scope render --snapshot <file> --centre <lat,lon> \
--range-nm <n> --size <WxH> --out <file.ppm>";

// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Failed(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}\n{USAGE}"),
            CliError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CliError {}

fn failed(what: &str, e: impl fmt::Display) -> CliError {
    CliError::Failed(format!("{what}: {e}"))
}

/// The parsed `render` arguments.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, PartialEq)]
pub struct RenderArgs {
    pub snapshot: PathBuf,
    pub centre_lat_e7: i32,
    pub centre_lon_e7: i32,
    pub range_nm: f64,
    pub width: u32,
    pub height: u32,
    pub out: PathBuf,
}

/// `lat,lon` in decimal degrees to 1e-7 degree.
pub fn parse_centre(text: &str) -> Result<(i32, i32), CliError> {
    let bad = || {
        CliError::Usage(format!(
            "--centre {text:?} is not <lat,lon> in decimal degrees"
        ))
    };
    let (a, b) = text.split_once(',').ok_or_else(bad)?;
    let lat: f64 = a.trim().parse().map_err(|_| bad())?;
    let lon: f64 = b.trim().parse().map_err(|_| bad())?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(CliError::Usage(format!(
            "--centre {text:?} is outside latitude -90 to 90, longitude -180 to 180"
        )));
    }
    match (deg_to_e7(lat), deg_to_e7(lon)) {
        (Some(la), Some(lo)) => Ok((la, lo)),
        _ => Err(bad()),
    }
}

/// `WxH` in pixels.
pub fn parse_size(text: &str) -> Result<(u32, u32), CliError> {
    let bad = || CliError::Usage(format!("--size {text:?} is not <width>x<height>"));
    let (a, b) = text.split_once('x').ok_or_else(bad)?;
    let w: u32 = a.parse().map_err(|_| bad())?;
    let h: u32 = b.parse().map_err(|_| bad())?;
    if !(MIN_SIDE..=MAX_SIDE).contains(&w) || !(MIN_SIDE..=MAX_SIDE).contains(&h) {
        return Err(CliError::Usage(format!(
            "--size {text} is outside {MIN_SIDE} to {MAX_SIDE} pixels on a side"
        )));
    }
    Ok((w, h))
}

fn set_once<T>(slot: &mut Option<T>, flag: &str, v: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::Usage(format!("{flag} given more than once")));
    }
    *slot = Some(v);
    Ok(())
}

/// Parses the arguments after `render`.
pub fn parse_render_args(args: &[String]) -> Result<RenderArgs, CliError> {
    let (mut snapshot, mut centre, mut range, mut size, mut out) = (None, None, None, None, None);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| CliError::Usage(format!("missing value for {flag}")))
        };
        match flag.as_str() {
            "--snapshot" => set_once(&mut snapshot, flag, PathBuf::from(value()?))?,
            "--centre" => set_once(&mut centre, flag, parse_centre(value()?)?)?,
            "--range-nm" => {
                let t = value()?;
                let r: f64 = t
                    .parse()
                    .map_err(|_| CliError::Usage(format!("--range-nm {t:?} is not a number")))?;
                set_once(&mut range, flag, r)?;
            }
            "--size" => set_once(&mut size, flag, parse_size(value()?)?)?,
            "--out" => set_once(&mut out, flag, PathBuf::from(value()?))?,
            other => return Err(CliError::Usage(format!("unknown argument {other:?}"))),
        }
    }
    let need = |what: &str| CliError::Usage(format!("missing {what}"));
    let (centre_lat_e7, centre_lon_e7) = centre.ok_or_else(|| need("--centre"))?;
    let (width, height) = size.ok_or_else(|| need("--size"))?;
    Ok(RenderArgs {
        snapshot: snapshot.ok_or_else(|| need("--snapshot"))?,
        centre_lat_e7,
        centre_lon_e7,
        range_nm: range.ok_or_else(|| need("--range-nm"))?,
        width,
        height,
        out: out.ok_or_else(|| need("--out"))?,
    })
}

fn render_err(e: RenderError) -> CliError {
    failed("render", e)
}

fn display_err(e: DisplayError) -> CliError {
    failed("display list", e)
}

/// `render`: snapshot in, PPM out; the summary goes to `out`.
pub fn run_render(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    let a = parse_render_args(args)?;
    let meta = std::fs::metadata(&a.snapshot)
        .map_err(|e| failed(&format!("read {}", a.snapshot.display()), e))?;
    if meta.len() > MAX_SNAPSHOT_BYTES as u64 {
        return Err(CliError::Failed(format!(
            "{} is larger than the {MAX_SNAPSHOT_BYTES} byte snapshot limit",
            a.snapshot.display()
        )));
    }
    let bytes = std::fs::read(&a.snapshot)
        .map_err(|e| failed(&format!("read {}", a.snapshot.display()), e))?;
    let model = decode(&bytes).map_err(|e| failed("snapshot", e))?;
    let view = view_new(
        a.centre_lat_e7,
        a.centre_lon_e7,
        a.range_nm,
        a.width,
        a.height,
    )
    .map_err(display_err)?;
    let spec = FrameSpec {
        width: a.width,
        height: a.height,
        stride: a.width * 4,
    };
    // All allocation happens here, once.
    let mut list = list_new(scene_caps(&model));
    let mut buf = vec![0u8; frame_len(&spec)];
    let t0 = Instant::now();
    let stats = build_scene(&mut list, &model, &view).map_err(display_err)?;
    let t1 = Instant::now();
    rasterize(&list, &spec, &mut buf).map_err(render_err)?;
    let t2 = Instant::now();
    let digest = frame_digest(&spec, &buf).map_err(render_err)?;
    let t3 = Instant::now();
    let file = std::fs::File::create(&a.out)
        .map_err(|e| failed(&format!("write {}", a.out.display()), e))?;
    let mut w = std::io::BufWriter::new(file);
    crate::ppm::write_ppm(&mut w, &spec, &buf)
        .and_then(|()| w.flush())
        .map_err(|e| failed(&format!("write {}", a.out.display()), e))?;
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let _ = writeln!(
        out,
        "image    {} ({}x{})",
        a.out.display(),
        a.width,
        a.height
    );
    let _ = writeln!(out, "sha256   {}", to_hex(&digest));
    let _ = writeln!(
        out,
        "drawn    {} boundaries, {} airway legs ({} off screen), {} fixes, {} navaids, {} airports, {} labels, {} rings, not placed: {} points {} legs {} edges",
        stats.boundaries,
        stats.airway_legs,
        stats.airway_legs_off_screen,
        stats.fixes,
        stats.navaids,
        stats.airports,
        stats.labels,
        stats.rings,
        stats.points_unplaced,
        stats.legs_unplaced,
        stats.edges_unplaced
    );
    let _ = writeln!(
        out,
        "time     build {:.2} ms, raster {:.2} ms, hash {:.2} ms",
        ms(t1 - t0),
        ms(t2 - t1),
        ms(t3 - t2)
    );
    Ok(())
}

/// Dispatches on the first argument; `args` excludes the program name.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "render" => run_render(rest, out),
        Some((cmd, _)) if cmd == "--help" || cmd == "-h" => {
            let _ = writeln!(out, "{USAGE}");
            Ok(())
        }
        _ => Err(CliError::Usage("expected a command: render".into())),
    }
}
