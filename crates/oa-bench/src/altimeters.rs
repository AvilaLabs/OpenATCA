//! Altimeter settings of airport fields from a directory of AWC METAR files (`oa-awc`). Shell:
//! reads files; the lookup is pure. The ground test of the takeoff and landing truth needs them
//! to bring ADS-B pressure altitude to feet above mean sea level (`truth::AirportRef`).

use std::io;
use std::path::Path;

use oa_awc::{finalize_metar, metar_altim_hpa, metar_len, new_metar_table, pack_ident};
use oa_awc::{parse_metar, MetarStats, MetarTable};

use oa_sha256::sha256;

use crate::input::{manifest_hash, read_capped_file};
use crate::report::InputHash;

/// Hectopascals per inch of mercury.
const HPA_PER_INHG: f64 = 33.863_886_666_7;
/// The largest METAR file read, bytes.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// The most METAR files read.
const MAX_FILES: usize = 10_000;

/// METAR observations with their trusted altimeter settings.
// A7.2: cold struct, size guard waived
pub struct Altimeters {
    table: MetarTable,
    /// Files read.
    pub files: usize,
    /// Files that were not valid METAR documents and were left out.
    pub refused_files: usize,
    /// Observation rows kept after de-duplication.
    pub rows: usize,
}

/// Reads every `metar*.json` file of `dir` (name order) and hashes them. Each file is read once;
/// the digest and the parse are of those same bytes.
pub fn load_metar_dir(dir: &Path) -> io::Result<(Altimeters, InputHash)> {
    load_metar_dir_with(dir, &|p| read_capped_file(p, MAX_FILE_BYTES))
}

/// As [`load_metar_dir`], reading each file's bytes with `read` (called once per file, in name
/// order). The seam for a test that returns other bytes on a second read.
pub fn load_metar_dir_with(
    dir: &Path,
    read: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> io::Result<(Altimeters, InputHash)> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("metar") && n.ends_with(".json"))
        })
        .collect();
    paths.sort();
    if paths.len() > MAX_FILES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: more than {MAX_FILES} METAR files", dir.display()),
        ));
    }
    let mut table = new_metar_table();
    let mut stats = MetarStats::default();
    let mut refused_files = 0usize;
    let mut sums = Vec::with_capacity(paths.len());
    for p in &paths {
        let bytes = read(p)?;
        sums.push((sha256(&bytes), bytes.len() as u64));
        if parse_metar(&bytes, &mut table, &mut stats).is_err() {
            refused_files += 1;
        }
    }
    finalize_metar(&mut table);
    let hash = manifest_hash(&format!("metar ({})", dir.display()), &paths, &sums);
    let rows = metar_len(&table);
    Ok((
        Altimeters {
            table,
            files: paths.len(),
            refused_files,
            rows,
        },
        hash,
    ))
}

impl Altimeters {
    /// Every trusted altimeter setting of the field, `(observation time Unix ms, inHg)`. The
    /// identifier is tried as given and, for a three-letter FAA identifier, with a `K` in front
    /// (the METAR station of a contiguous-US field). Empty when the station is unknown.
    pub fn at_field(&self, name: &str) -> Vec<(i64, f64)> {
        let name = name.trim().to_ascii_uppercase();
        let mut cands = vec![name.clone()];
        if name.len() == 3 {
            cands.push(format!("K{name}"));
        }
        for c in cands {
            let Some(id) = pack_ident(&c, 3) else {
                continue;
            };
            let t = &self.table;
            let lo = t.station.partition_point(|&s| s < id);
            let hi = t.station.partition_point(|&s| s <= id);
            let out: Vec<(i64, f64)> = (lo..hi)
                .filter_map(|i| {
                    let hpa = metar_altim_hpa(t, i)?;
                    let o = t.obs_s.get(i)?;
                    Some((o.checked_mul(1000)?, hpa / HPA_PER_INHG))
                })
                .collect();
            if !out.is_empty() {
                return out;
            }
        }
        Vec::new()
    }
}
