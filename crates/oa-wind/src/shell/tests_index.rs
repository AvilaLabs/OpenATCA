#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! Synthetic GRIB2 hour files with `.idx` sidecars: statistical-interval products and sidecars
//! that disagree with their file.

use crate::archive::*;
use crate::error::*;
use crate::hour::*;
use oa_grib2::testenc::{encode_simple, hrrr_spec, message, sect1, sect3_lambert, sect4, sect6};
use std::path::PathBuf;

/// (idx name, category, number) of the four wanted parameters and of RH.
const PARAMS: [(&str, u8, u8); 5] = [
    ("UGRD", 2, 2),
    ("VGRD", 2, 3),
    ("TMP", 0, 0),
    ("HGT", 3, 5),
    ("RH", 1, 1),
];

/// One message of a synthetic file.
struct Msg {
    name: &'static str,
    level_hpa: u32,
    bytes: Vec<u8>,
}

fn msg(par: usize, level_hpa: u32, template: u16) -> Msg {
    let (name, cat, num) = PARAMS[par];
    let x: Vec<u64> = (0..12)
        .map(|k| 100 * par as u64 + u64::from(level_hpa) + k)
        .collect();
    let p = encode_simple(&x, 0.0, 0, 0);
    let bytes = message(
        0,
        &[
            sect1(2026, 11, 14, 12),
            sect3_lambert(&hrrr_spec(4, 3)),
            sect4(template, cat, num, 100, 0, level_hpa * 100),
            p.sect5.clone(),
            sect6(None, false),
            p.sect7.clone(),
        ],
    );
    Msg {
        name,
        level_hpa,
        bytes,
    }
}

/// Every parameter at every level, in file order, each message plain (template 4.0).
fn messages(levels: &[u32]) -> Vec<Msg> {
    let mut v = Vec::new();
    for &l in levels {
        for par in 0..PARAMS.len() {
            v.push(msg(par, l, 0));
        }
    }
    v
}

/// The file's bytes and the true `.idx` text of its messages.
fn file_and_idx(msgs: &[Msg]) -> (Vec<u8>, Vec<(u64, String)>) {
    let mut data = Vec::new();
    let mut rows = Vec::new();
    for (k, m) in msgs.iter().enumerate() {
        rows.push((
            data.len() as u64,
            format!(
                "{}:{{}}:d=2026111412:{}:{} mb:6 hour fcst:",
                k + 1,
                m.name,
                m.level_hpa
            ),
        ));
        data.extend_from_slice(&m.bytes);
    }
    (data, rows)
}

fn idx_text(rows: &[(u64, String)]) -> String {
    rows.iter()
        .map(|(o, line)| line.replacen("{}", &o.to_string(), 1) + "\n")
        .collect()
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("oa-wind-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn write(&self, data: &[u8], idx: Option<&str>) -> PathBuf {
        let f = self.0.join("hour.grib2");
        std::fs::write(&f, data).unwrap();
        let i = self.0.join("hour.grib2.idx");
        match idx {
            Some(t) => std::fs::write(&i, t).unwrap(),
            None => {
                let _ = std::fs::remove_file(&i);
            }
        }
        f
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const LEVELS: [u32; 4] = [300, 500, 700, 850];

// REQ: OA-WIND-054
#[test]
fn a_statistical_interval_wind_is_refused() {
    // All instantaneous: loads as before.
    let plain = messages(&LEVELS);
    let (data, _) = file_and_idx(&plain);
    let h = load_hour(&data).unwrap();
    assert_eq!(h.levels_pa.len(), 4);
    // One wind of the quartet at 500 hPa from template 4.8 among instantaneous fields.
    let mut mixed = messages(&LEVELS);
    let k = mixed
        .iter()
        .position(|m| m.name == "UGRD" && m.level_hpa == 500)
        .unwrap();
    mixed[k] = msg(0, 500, 8);
    let (data, _) = file_and_idx(&mixed);
    let r = load_hour(&data);
    assert!(
        r.is_err(),
        "interval wind was accepted: {:?}",
        r.map(|h| h.levels_pa)
    );
    assert_eq!(
        r,
        Err(LoadError::StatisticalInterval {
            fields: 1,
            field: "UGRD",
            level_pa: 50_000
        })
    );
    // An interval field that the hour does not use (RH) does not matter.
    let mut rh = messages(&LEVELS);
    let k = rh
        .iter()
        .position(|m| m.name == "RH" && m.level_hpa == 500)
        .unwrap();
    rh[k] = msg(4, 500, 8);
    let (data, _) = file_and_idx(&rh);
    assert_eq!(load_hour(&data).unwrap(), h);
}

// REQ: OA-WIND-055
#[test]
fn a_sidecar_that_disagrees_with_its_file_never_thins_the_profile() {
    let msgs = messages(&LEVELS);
    let (data, rows) = file_and_idx(&msgs);
    let dir = Dir::new("stale");
    // The truth: no sidecar, then the honest sidecar.
    let bare = read_hour_file(&dir.write(&data, None)).unwrap();
    assert_eq!(bare.levels_pa.len(), 4);
    let honest = idx_text(&rows);
    assert_eq!(
        read_hour_file(&dir.write(&data, Some(&honest))).unwrap(),
        bare
    );

    // A sidecar from an older file that lacks the 700 hPa rows: its ranges are fine and decode
    // into a complete (thinner) hour.
    let missing: Vec<(u64, String)> = rows
        .iter()
        .filter(|(_, l)| !l.contains(":700 mb:"))
        .cloned()
        .collect();
    let got = read_hour_file(&dir.write(&data, Some(&idx_text(&missing))));
    assert_eq!(
        got,
        Ok(bare.clone()),
        "sidecar without a level thinned the hour"
    );

    // Every UGRD row of 300 hPa relabelled RH: the offsets are the file's, the labels are not.
    let relabelled: Vec<(u64, String)> = rows
        .iter()
        .map(|(o, l)| match l.contains(":UGRD:300 mb:") {
            true => (*o, l.replace(":UGRD:", ":RH:")),
            false => (*o, l.clone()),
        })
        .collect();
    let got = read_hour_file(&dir.write(&data, Some(&idx_text(&relabelled))));
    assert_eq!(got, Ok(bare.clone()), "relabelled sidecar thinned the hour");

    // A level moved: the 500 hPa TMP row says 550 hPa.
    let moved: Vec<(u64, String)> = rows
        .iter()
        .map(|(o, l)| (*o, l.replace(":TMP:500 mb:", ":TMP:550 mb:")))
        .collect();
    let got = read_hour_file(&dir.write(&data, Some(&idx_text(&moved))));
    assert_eq!(got, Ok(bare), "mislabelled level changed the hour");
}

// REQ: OA-WIND-055
#[test]
fn the_report_says_whether_the_sidecar_was_used_and_the_archive_counts_refusals() {
    let msgs = messages(&LEVELS);
    let (data, rows) = file_and_idx(&msgs);
    let dir = Dir::new("report");
    let (bare, used) = read_hour_file_report(&dir.write(&data, None)).unwrap();
    assert_eq!(used, IndexUse::Absent);
    let honest = idx_text(&rows);
    let (h, used) = read_hour_file_report(&dir.write(&data, Some(&honest))).unwrap();
    assert_eq!((h, used), (bare.clone(), IndexUse::Used));
    // A missing level: the offsets are not the file's message starts.
    let missing: Vec<(u64, String)> = rows
        .iter()
        .filter(|(_, l)| !l.contains(":700 mb:"))
        .cloned()
        .collect();
    let (h, used) = read_hour_file_report(&dir.write(&data, Some(&idx_text(&missing)))).unwrap();
    assert_eq!(h, bare);
    assert_eq!(
        used,
        IndexUse::Refused(LoadError::IndexMismatch("message offsets"))
    );
    // A relabelled variable on a real offset.
    let relabelled: Vec<(u64, String)> = rows
        .iter()
        .map(|(o, l)| (*o, l.replace(":UGRD:300 mb:", ":RH:300 mb:")))
        .collect();
    let (_, used) = read_hour_file_report(&dir.write(&data, Some(&idx_text(&relabelled)))).unwrap();
    assert_eq!(
        used,
        IndexUse::Refused(LoadError::IndexMismatch("message variable or level"))
    );
    // A wrong date.
    let dated: Vec<(u64, String)> = rows
        .iter()
        .map(|(o, l)| (*o, l.replace(":d=2026111412:", ":d=2026111413:")))
        .collect();
    let (_, used) = read_hour_file_report(&dir.write(&data, Some(&idx_text(&dated)))).unwrap();
    assert!(matches!(
        used,
        IndexUse::Refused(LoadError::IndexMismatch(_))
    ));
    // A claimed forecast that is not the field's.
    let lead: Vec<(u64, String)> = rows
        .iter()
        .map(|(o, l)| (*o, l.replace(":6 hour fcst:", ":anl:")))
        .collect();
    let (_, used) = read_hour_file_report(&dir.write(&data, Some(&idx_text(&lead)))).unwrap();
    assert_eq!(used, IndexUse::Refused(LoadError::IndexMismatch("time")));

    // The archive counts the fallback and answers from the whole file.
    let path = dir.write(&data, Some(&idx_text(&missing)));
    let files = index_files(&[path]).unwrap();
    let mut a = archive_files(files).unwrap();
    assert_eq!(a.index_fallbacks(), 0);
    let q = crate::Query {
        lat: 40.0,
        lon: -90.0,
        alt_ft: 18_000.0,
        time_s: a.times()[0] as f64,
    };
    // The point is outside the tiny synthetic grid, so the answer is a refusal; the file was
    // still loaded, and the fallback counted.
    let _ = archive_sample(&mut a, &q);
    assert_eq!(a.index_fallbacks(), 1);
    assert_eq!(a.held(), 1);
}

// REQ: OA-WIND-055
#[test]
fn decoded_fields_are_checked_against_the_claims() {
    let (data, rows) = file_and_idx(&messages(&LEVELS));
    let idx = oa_grib2::parse_idx(&idx_text(&rows)).unwrap();
    let claims = index_claims(&idx);
    assert_eq!(claims.len(), 16);
    let fields = oa_grib2::decode_stream(&data).fields;
    let fields: Vec<_> = fields
        .into_iter()
        .filter(|f| f.parameter != oa_grib2::Parameter::Rh)
        .collect();
    assert_eq!(check_claims(&claims, &fields), Ok(()));
    // One claim names another level.
    let mut other = claims.clone();
    other[0].level_pa += 100;
    assert_eq!(
        check_claims(&other, &fields),
        Err(LoadError::IndexMismatch("parameter or level"))
    );
    // One field fewer than claimed.
    assert_eq!(
        check_claims(&claims, &fields[1..]),
        Err(LoadError::IndexMismatch("field count"))
    );
    // A claim of a lead the file does not have, and one that cannot be read.
    let mut lead = claims.clone();
    lead[3].lead_s = Some(7200);
    assert_eq!(
        check_claims(&lead, &fields),
        Err(LoadError::IndexMismatch("time"))
    );
    lead[3].lead_s = None;
    assert!(check_claims(&lead, &fields).is_err());
}

fn real_rap_files() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("OPENATCA_DATA") else {
        println!("OPENATCA_DATA not set: real-data test skipped");
        return Vec::new();
    };
    let dir = PathBuf::from(root).join("raw/rap");
    // A sealed entry is skipped and counted, never opened (OA-SEAL-012).
    let Ok(walk) = oa_sealed::walk_unsealed(&dir) else {
        println!("{} missing: skipped", dir.display());
        return Vec::new();
    };
    println!("sealed entries skipped: {}", walk.skipped);
    let mut v: Vec<PathBuf> = walk
        .files
        .into_iter()
        .filter(|p| p.parent() == Some(dir.as_path()))
        .filter(|p| p.extension().is_some_and(|e| e == "grib2"))
        .collect();
    v.sort();
    v
}

// REQ: OA-WIND-056
#[test]
fn real_rap_subsets_refuse_the_idx_of_the_original_and_accept_their_own() {
    let files = real_rap_files();
    let Some(path) = files
        .iter()
        .find(|p| p.to_string_lossy().contains(".t12z."))
    else {
        return;
    };
    let data = oa_sealed::read(path).unwrap();
    let whole = load_hour(&data).unwrap();
    assert_eq!(whole.levels_pa.len(), 37);
    // The shipped sidecar is the original file's, whose offsets are not this subset's.
    let dir = Dir::new("real");
    let (h, used) = read_hour_file_report(&dir.write(
        &data,
        Some(&oa_sealed::read_to_string(PathBuf::from(format!("{}.idx", path.display()))).unwrap()),
    ))
    .unwrap();
    assert_eq!(h, whole);
    assert!(
        matches!(used, IndexUse::Refused(LoadError::IndexMismatch(_))),
        "{used:?}"
    );
    // An index written from the subset's own headers is accepted.
    let mut text = String::new();
    let mut k = 0;
    for span in oa_grib2::scan_messages(&data).unwrap() {
        let msg = &data[span.offset..span.offset + span.length];
        k += 1;
        for (sub, f) in oa_grib2::inspect_stream(msg).fields.iter().enumerate() {
            let t = &f.ref_time;
            text += &format!(
                "{k}.{}:{}:d={:04}{:02}{:02}{:02}:{}:{} mb:anl:\n",
                sub + 1,
                span.offset,
                t.year,
                t.month,
                t.day,
                t.hour,
                oa_grib2::parameter_name(f.parameter).unwrap_or("OTHER"),
                f.level.value / 100.0
            );
        }
    }
    let (h, used) = read_hour_file_report(&dir.write(&data, Some(&text))).unwrap();
    assert_eq!(used, IndexUse::Used);
    assert_eq!(h, whole);
}

// REQ: OA-WIND-057
#[test]
fn two_wanted_fields_of_one_message_name_one_range() {
    let idx = "\
1:0:d=2026111400:REFC:entire atmosphere:anl:
2.1:100:d=2026111400:UGRD:250 mb:anl:
2.2:100:d=2026111400:VGRD:250 mb:anl:
3:200:d=2026111400:TMP:250 mb:anl:
";
    assert_eq!(needed_ranges_text(idx, 300).unwrap(), vec![(100, 299)]);
}
