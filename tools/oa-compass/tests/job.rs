//! The whole run on small synthetic inputs: what is written, what is refused, what the README
//! says (OA-CMP-030 to OA-CMP-037).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::{Path, PathBuf};

use oa_adsb::{reports_push, Reports, ROW_EMPTY};
use oa_asterix::{decode_stream, Block};
use oa_bench::legacy::{flight_ix, push_th, Legacy};
use oa_compass::core::addrcheck::{
    check_flight, contradicts, distance_km, nearest, RefPoint, Verdict,
};
use oa_compass::core::convert::{
    eram_record, track_record, EramRow, TrackRow, NOTE_ADDRESS_WITHHELD, NOTE_IDENTITY_UNCONFIRMED,
    NOTE_STATUS_ASSUMED,
};
use oa_compass::core::stats::{percentile, summarize};
use oa_compass::shell::adsb_export::AdsbExport;
use oa_compass::shell::encode::MAX_POSITION_ERROR_E7;
use oa_compass::shell::error::ExportError;
use oa_compass::shell::export::{Layout, SystemExport};
use oa_compass::shell::job::{run_job, EramSource, Job};
use oa_compass::shell::notes::{ADSB_SOURCE, OPENATCA_SOURCE};
use oa_compass::shell::run::{export_output, EramAddress, InputsMode, OpenatcaRows, TrackRunInfo};
use oa_compass::shell::slice::Stage;

/// 2026-11-15, as a day count and in Unix nanoseconds, and the start of the synthetic clip.
const DAY: i64 = 20_772;
const DAY_NS: i64 = DAY * 86_400 * 1_000_000_000;
const T0_NS: i64 = DAY_NS + 3_600 * 1_000_000_000;
/// The aircraft of the clip.
const ADDR: u32 = 0x3C_66B0;

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cmp-job-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// The clip's aircraft at second `k` of the clip: moving north at 180 m/s (350 kt, as it reports).
fn clip_position(k: i64) -> (i32, i32) {
    (410_000_000 + (k as i32) * 16_200, -880_000_000)
}

/// An OAF1 file with one aircraft reporting once a second for `secs` seconds.
fn write_clip(dir: &Path, secs: i64) -> PathBuf {
    let mut r = Reports::default();
    for k in 0..secs {
        let (la, lo) = clip_position(k);
        let row = oa_adsb::Row {
            icao: ADDR,
            time_ns: T0_NS + k * 1_000_000_000,
            lat_e7: la,
            lon_e7: lo,
            baro_alt_ft: 20_000,
            gs_dkt: 3500,
            track_ddeg: 0,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            ..ROW_EMPTY
        };
        reports_push(&mut r, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &r, 0, oa_adsb::reports_len(&r)).unwrap();
    let mut buf = vec![0u8; payload.len() + 256];
    let n = oa_feed::encode_frame(&mut buf, 1, 0, 0, T0_NS, 0, &payload).unwrap();
    let p = dir.join("clip.oaf");
    fs::write(&p, &buf[..n]).unwrap();
    p
}

/// Three ERAM flights, 20 samples each, 5 s apart from 10 s into the clip: one following the
/// aircraft of the clip under its address, one carrying the same address 2 degrees away (the
/// feed's wrong-address case), one without an address.
fn eram_legacy() -> Legacy {
    let mut leg = Legacy::default();
    let a = flight_ix(&mut leg, 1).unwrap();
    let b = flight_ix(&mut leg, 2).unwrap();
    let c = flight_ix(&mut leg, 3).unwrap();
    for (f, cs, addr) in [
        (a, *b"AGREE1\0\0", ADDR),
        (b, *b"WRONG2\0\0", ADDR),
        (c, *b"NOADDR3\0", oa_bench::legacy::NONE),
    ] {
        leg.flights.callsign[f as usize] = cs;
        leg.flights.addr[f as usize] = addr;
    }
    for k in 0..20i64 {
        let t_ms = (T0_NS / 1_000_000) + (10 + 5 * k) * 1000;
        let (la, lo) = clip_position(10 + 5 * k);
        push_th(
            &mut leg,
            a,
            t_ms,
            0,
            la + 500,
            lo,
            35_000,
            Some((0, 400)),
            false,
        );
        push_th(
            &mut leg,
            b,
            t_ms,
            0,
            la + 20_000_000,
            lo,
            35_000,
            Some((0, 400)),
            false,
        );
        push_th(
            &mut leg,
            c,
            t_ms,
            0,
            la + 30_000_000,
            lo + 5_000_000,
            35_000,
            None,
            false,
        );
    }
    leg
}

fn job(out: &Path, adsb: Option<PathBuf>, eram: bool, split: bool, rows: OpenatcaRows) -> Job {
    Job {
        layout: Layout {
            out: out.to_path_buf(),
            day: DAY,
            split,
        },
        adsb,
        sfdps: eram.then(|| EramSource::Loaded(Box::new(eram_legacy()), vec!["test input".into()])),
        mode: InputsMode::All,
        rows,
        eram_address: EramAddress::Keep,
        self_test_eram_offset_e7: 0,
        self_test_adsb_offset_e7: 0,
    }
}

fn run(j: Job) -> Result<Vec<String>, ExportError> {
    run_job(j, &mut |_| Ok(()), &mut |_| {})
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn records_062(path: &Path) -> Vec<oa_asterix::cat062::Cat062> {
    let bytes = fs::read(path).unwrap();
    decode_stream(&bytes)
        .unwrap()
        .into_iter()
        .flat_map(|b| match b {
            Block::Cat062(r) => r,
            _ => Vec::new(),
        })
        .collect()
}

// REQ: OA-CMP-030
#[test]
fn a_run_into_a_directory_with_files_is_refused_and_the_readme_lists_what_was_written() {
    let work = tmp("both");
    let clip = write_clip(&work, 120);
    let out = work.join("out");
    run(job(
        &out,
        Some(clip.clone()),
        true,
        false,
        OpenatcaRows::Ticks,
    ))
    .unwrap();
    let all = [
        "README.txt",
        "adsb.ast",
        "data_sources.json",
        "eram.ast",
        "openatca.ast",
    ];
    assert_eq!(names(&out), all);
    let readme = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        readme.contains(
            "Files written by this run in this directory: README.txt, adsb.ast, data_sources.json, eram.ast, openatca.ast."
        ),
        "{readme}"
    );
    // The same directory again, ADS-B only: refused, and nothing changed.
    let before: Vec<Vec<u8>> = all.iter().map(|n| fs::read(out.join(n)).unwrap()).collect();
    let e = run(job(&out, Some(clip), false, false, OpenatcaRows::Ticks)).unwrap_err();
    assert!(
        matches!(e, ExportError::Input(_)) && e.to_string().contains("not empty"),
        "{e}"
    );
    assert_eq!(names(&out), all);
    let after: Vec<Vec<u8>> = all.iter().map(|n| fs::read(out.join(n)).unwrap()).collect();
    assert_eq!(before, after);
    // An existing empty directory is fine.
    let empty = work.join("empty");
    fs::create_dir_all(&empty).unwrap();
    run(job(
        &empty,
        Some(write_clip(&work, 60)),
        false,
        false,
        OpenatcaRows::Ticks,
    ))
    .unwrap();
}

// REQ: OA-CMP-030
#[test]
fn the_readme_follows_the_systems_exported() {
    let work = tmp("systems");
    let clip = write_clip(&work, 120);
    // ERAM only: no mention of the OpenATCA file, the ADS-B file or their sources.
    let out = work.join("eram-only");
    run(job(&out, None, true, false, OpenatcaRows::Ticks)).unwrap();
    assert_eq!(names(&out), ["README.txt", "data_sources.json", "eram.ast"]);
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.starts_with("ERAM tracks as ASTERIX CAT062, for evaluation"),
        "{r}"
    );
    assert!(
        r.contains("Import eram.ast as ASTERIX CAT062, with the UTC Day above"),
        "{r}"
    );
    for absent in [
        "openatca.ast",
        "adsb.ast",
        "OpenATCA ROWS",
        "OpenATCA 255/11",
        "ADS-B 255/13",
    ] {
        assert!(!r.contains(absent), "{absent:?} in\n{r}");
    }
    let ds = fs::read_to_string(out.join("data_sources.json")).unwrap();
    assert!(
        ds.contains("\"ERAM\"") && !ds.contains("OpenATCA") && !ds.contains("ADS-B"),
        "{ds}"
    );
    // The ADS-B clip alone gives OpenATCA and ADS-B, and no ERAM text.
    let out = work.join("adsb-only");
    run(job(&out, Some(clip), false, false, OpenatcaRows::Ticks)).unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.starts_with(
            "OpenATCA tracks as ASTERIX CAT062 and the ADS-B reports as CAT021, for evaluation"
        ),
        "{r}"
    );
    assert!(
        r.contains("Import openatca.ast as ASTERIX CAT062 and adsb.ast as ASTERIX CAT021 (Ed 2.4)"),
        "{r}"
    );
    assert!(
        !r.contains("eram.ast") && !r.contains("ERAM duplicates"),
        "{r}"
    );
}

// REQ: OA-CMP-034
#[test]
fn a_failure_in_the_rename_phase_or_after_it_leaves_the_output_empty_or_absent() {
    let work = tmp("stages");
    let clip = write_clip(&work, 90);
    // Five files: three data files, data_sources.json and the README.
    for split in [false, true] {
        for stage in [
            Stage::Verified,
            Stage::Renamed(1),
            Stage::Renamed(3),
            Stage::Renamed(5),
        ] {
            let out = work.join(format!("out-{split}-{stage:?}"));
            let mut hook = |s: Stage| {
                if s == stage {
                    Err(ExportError::Io("injected".into()))
                } else {
                    Ok(())
                }
            };
            let r = run_job(
                job(&out, Some(clip.clone()), true, split, OpenatcaRows::Ticks),
                &mut hook,
                &mut |_| {},
            );
            assert!(r.is_err(), "{stage:?}");
            assert!(
                names(&out).is_empty(),
                "{split} {stage:?}: {:?}",
                names(&out)
            );
            assert!(!out.exists(), "a directory made by the run is removed");
        }
    }
    // A directory that existed (and was empty) stays, empty.
    let out = work.join("existing");
    fs::create_dir_all(&out).unwrap();
    let mut hook = |s: Stage| match s {
        Stage::Renamed(2) => Err(ExportError::Io("injected".into())),
        _ => Ok(()),
    };
    assert!(run_job(
        job(&out, Some(clip), true, false, OpenatcaRows::Ticks),
        &mut hook,
        &mut |_| {}
    )
    .is_err());
    assert!(out.is_dir() && names(&out).is_empty());
}

// REQ: OA-CMP-033
#[test]
fn the_commit_in_the_readme_is_the_one_embedded_at_build_time() {
    let b = oa_build_info::BUILD;
    assert!(b.starts_with("commit "), "{b}");
    // The embedded value is the crate's own repository's HEAD, read when it was built.
    if let Ok(o) = std::process::Command::new("git")
        .arg("-C")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .args(["rev-parse", "HEAD"])
        .output()
    {
        if o.status.success() {
            let head = String::from_utf8_lossy(&o.stdout).trim().to_string();
            assert!(b.contains(&head), "{b} does not name {head}");
        }
    }
    let work = tmp("build");
    let out = work.join("out");
    run(job(&out, None, true, false, OpenatcaRows::Ticks)).unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(r.contains(&format!("build {b}.")), "{r}");
}

// REQ: OA-CMP-031
#[test]
fn an_export_whose_position_error_is_over_the_limit_fails_and_commits_nothing() {
    // CAT062: a source position 100 (1e-7 degree) off the one in the record, a scale error.
    let work = tmp("limit");
    let lay = Layout {
        out: work.join("c62"),
        day: DAY,
        split: false,
    };
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay);
    let row = TrackRow {
        t_ns: T0_NS,
        lat_e7: 410_000_000,
        lon_e7: -880_000_000,
        vel_east_mps: 1.0,
        vel_north_mps: 1.0,
        pos_cov_m2: [25.0, 0.0, 25.0],
        alt_ft: 10_000.0,
        has_alt: true,
        vrate_fpm: 0.0,
        coast: false,
        radar_disagrees: false,
        uncertain: false,
        reacquired: false,
        identity_unconfirmed: false,
        address: Some(ADDR),
        monosensor: true,
    };
    let c = track_record(&row).unwrap();
    // Within the limit: fine.
    ex.push(&c, 1, (row.lat_e7 + MAX_POSITION_ERROR_E7, row.lon_e7))
        .unwrap();
    // Over it: the block that holds it does not verify.
    ex.push(
        &c,
        1,
        (row.lat_e7 + MAX_POSITION_ERROR_E7 + 100, row.lon_e7),
    )
    .unwrap();
    let e = match ex.finish() {
        Ok(_) => panic!("an error over the limit was accepted"),
        Err(e) => e,
    };
    assert!(
        matches!(e, ExportError::Verify(_)) && e.to_string().contains("over the limit"),
        "{e}"
    );
    assert!(
        names(&work.join("c62")).is_empty(),
        "{:?}",
        names(&work.join("c62"))
    );
    // CAT021, limit 1.
    let lay = Layout {
        out: work.join("c21"),
        day: DAY,
        split: false,
    };
    let mut ax = AdsbExport::new(ADSB_SOURCE, lay);
    let r = oa_compass::core::adsb::AdsbRow {
        time_ns: T0_NS,
        address: ADDR,
        non_icao: false,
        lat_e7: 410_000_000,
        lon_e7: -880_000_000,
        baro_alt_ft: None,
        geom_alt_ft: None,
        ground: false,
        vrate_fpm: None,
        rate_geometric: false,
        gs_dkt: None,
        track_ddeg: None,
        callsign: [0; 8],
        category: 0,
        nic: None,
        nac_p: None,
        nac_v: None,
        sil: None,
        sil_per_sample: None,
        version: None,
        gva: None,
        sda: None,
    };
    let c = oa_compass::core::adsb::adsb_record(&r).unwrap();
    ax.push(&c, (r.lat_e7 + 40, r.lon_e7)).unwrap();
    assert!(ax.finish().is_err());
    assert!(names(&work.join("c21")).is_empty());
}

fn track_output(rows: &[(u8, i64, u32)]) -> oa_track::TrackOutput {
    let mut o = oa_track::output_new(rows.len().max(1));
    for &(flags, t_s, addr) in rows {
        o.t_ns.push(DAY_NS + t_s * 1_000_000_000);
        o.addr.push(addr);
        o.lat_e7.push(410_000_000);
        o.lon_e7.push(-880_000_000);
        o.vel_east_mps.push(100.0);
        o.vel_north_mps.push(0.0);
        o.pos_cov_m2.push([100.0, 0.0, 100.0]);
        o.vel_cov_m2s2.push([1.0, 0.0, 1.0]);
        o.turn_rate_rps.push(0.0);
        o.alt_ft.push(30_000.0);
        o.alt_var_ft2.push(1.0);
        o.vrate_fpm.push(0.0);
        o.mu_h.push([1.0, 0.0]);
        o.mu_v.push([1.0, 0.0]);
        o.flags.push(flags | oa_track::O_HAS_ALT);
    }
    o
}

// REQ: OA-CMP-036
#[test]
fn the_updates_mode_writes_update_rows_and_counts_the_rest() {
    use oa_track::{O_COAST, O_TICK};
    let lay = |n: &str| Layout {
        out: tmp(n),
        day: DAY,
        split: false,
    };
    // Two ticks and three updates; a later batch holds an update earlier than one written.
    let o = track_output(&[
        (O_TICK, 1, 7),
        (0, 2, 7),
        (O_TICK | O_COAST, 2, 7),
        (0, 5, 7),
    ]);
    let late = track_output(&[(0, 3, 7)]);
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay("upd"));
    let mut info = TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true, OpenatcaRows::Updates).unwrap();
    export_output(&late, &mut ex, &mut info, true, OpenatcaRows::Updates).unwrap();
    assert_eq!(
        (
            info.update_rows,
            info.tick_rows_not_exported,
            info.updates_out_of_order,
            info.tick_rows
        ),
        (2, 2, 1, 0)
    );
    assert_eq!(
        info.coasting, 0,
        "a coasting tick is not exported in updates mode"
    );
    assert_eq!(ex.rows(), 2);
    ex.abort();
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay("tick"));
    let mut info = TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true, OpenatcaRows::Ticks).unwrap();
    assert_eq!(
        (
            info.tick_rows,
            info.update_rows_not_exported,
            info.update_rows
        ),
        (2, 2, 0)
    );
    assert_eq!(info.coasting, 1);
    ex.abort();
}

// REQ: OA-CMP-036
#[test]
fn the_readme_states_the_openatca_rows_mode_prominently_in_both_modes() {
    let work = tmp("modes");
    let clip = write_clip(&work, 120);
    let mut counts = Vec::new();
    for (name, rows, head, other) in [
        (
            "ticks",
            OpenatcaRows::Ticks,
            "OpenATCA ROWS: TICKS",
            "UPDATES",
        ),
        (
            "updates",
            OpenatcaRows::Updates,
            "OpenATCA ROWS: UPDATES",
            "TICKS",
        ),
    ] {
        let out = work.join(name);
        let lines = run(job(&out, Some(clip.clone()), false, false, rows)).unwrap();
        let r = fs::read_to_string(out.join("README.txt")).unwrap();
        // Before the format and encoder lines: near the top.
        let at = r.find(head).unwrap();
        assert!(at < r.find("Format:").unwrap(), "{r}");
        assert!(
            !r[..r.find("Format:").unwrap()].contains(&format!("OpenATCA ROWS: {other}")),
            "{r}"
        );
        let n = records_062(&out.join("openatca.ast")).len();
        assert!(n > 0, "{lines:?}");
        counts.push(n);
        let (kept, left) = match rows {
            OpenatcaRows::Ticks => ("tick rows exported", "update rows not exported"),
            OpenatcaRows::Updates => ("update rows exported", "tick rows not exported"),
        };
        assert!(r.contains(kept) && r.contains(left), "{r}");
    }
    // 120 reports are 120 update rows; the ticks are one a second from the first track state.
    assert_ne!(counts[0], counts[1]);
}

// REQ: OA-CMP-037
#[test]
fn the_readme_says_openatca_has_a_calculated_altitude_and_no_measured_flight_level() {
    let work = tmp("alt");
    let out = work.join("out");
    run(job(
        &out,
        Some(write_clip(&work, 90)),
        false,
        false,
        OpenatcaRows::Ticks,
    ))
    .unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.contains("CALCULATED altitude, I135 with QNH = 0. OpenATCA publishes no measured flight level (I136), so COMPASS's \"Mode C Present\" does not apply to it"),
        "{r}"
    );
    // And the records agree: I135 present, I136 never.
    let recs = records_062(&out.join("openatca.ast"));
    assert!(
        recs.iter().all(|r| r.measured_fl.is_none())
            && recs.iter().any(|r| r.baro_altitude.is_some())
    );
}

// REQ: OA-CMP-032
#[test]
fn the_status_claims_are_assumptions_or_counted_and_the_unconfirmed_flag_is_not_cnf() {
    use oa_track::{O_IDENTITY_UNCONFIRMED, O_TICK};
    // ERAM: every row carries the note that MON and CNF are defaults.
    let e = EramRow {
        pos_ms: T0_NS / 1_000_000,
        lat_e7: 410_000_000,
        lon_e7: -880_000_000,
        alt_ft: None,
        vel_dkt: None,
        coast: false,
        address: Some(ADDR),
        address_withheld: false,
        callsign: None,
    };
    let c = eram_record(&e).unwrap();
    assert!(c.notes & (1 << NOTE_STATUS_ASSUMED) != 0 && !c.values.monosensor);
    // OpenATCA: an unconfirmed-identity row is counted, noted, and written as confirmed.
    let o = track_output(&[
        (O_TICK | O_IDENTITY_UNCONFIRMED, 1, oa_track::RADAR_KEY_BASE),
        (O_TICK, 2, 7),
    ]);
    let cnf_out = tmp("cnf");
    let mut ex = SystemExport::new(
        "OpenATCA",
        "openatca.ast",
        OPENATCA_SOURCE,
        Layout {
            out: cnf_out.clone(),
            day: DAY,
            split: false,
        },
    );
    let mut info = TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true, OpenatcaRows::Ticks).unwrap();
    assert_eq!((info.tick_rows, info.unconfirmed_identity), (2, 1));
    let rep = ex.finish().unwrap().commit().unwrap();
    assert_eq!(rep.days[0].notes[NOTE_IDENTITY_UNCONFIRMED], 1);
    let recs = records_062(&cnf_out.join("openatca.ast"));
    // CNF is bit 8 of the first octet of I080 (set = tentative): never set.
    assert!(recs
        .iter()
        .all(|r| !r.track_status.as_ref().unwrap().tentative));
}

// REQ: OA-CMP-032
#[test]
fn each_file_states_its_row_nature_and_the_measured_interval_and_the_eram_track_mapping() {
    let work = tmp("nature");
    let out = work.join("out");
    run(job(
        &out,
        Some(write_clip(&work, 120)),
        true,
        false,
        OpenatcaRows::Ticks,
    ))
    .unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    // OpenATCA ticks: one second apart. ERAM samples: five seconds apart (19 pairs a flight, three flights).
    assert!(
        r.contains(
            "Measured interval between consecutive rows of one track in this file: median 1.000 s"
        ),
        "{r}"
    );
    assert!(r.contains("median 5.000 s, 90th percentile 5.000 s, longest 5.000 s, over 57 pairs of rows (0 of them at the same time)"), "{r}");
    assert!(
        r.contains("rows are SFDPS track samples at their own position times"),
        "{r}"
    );
    assert!(
        r.contains("samples written with a position time on a whole second: 60"),
        "{r}"
    );
    assert!(
        r.contains("ERAM track number (I040): the feed's track samples carry no track number"),
        "{r}"
    );
    assert!(
        r.contains("addresses carried by more than one flight (more than one ERAM track): 1"),
        "{r}"
    );
    assert!(
        r.contains("addresses on more than one flight: 3C66B0 (2 flights)"),
        "{r}"
    );
    assert!(
        r.contains("I080 MON = 0 and CNF = 0: not in the feed"),
        "{r}"
    );
    assert!(r.contains("O_IDENTITY_UNCONFIRMED"), "{r}");
}

// REQ: OA-CMP-035
#[test]
fn with_drop_a_flight_whose_track_contradicts_its_address_is_exported_without_it_and_the_file_says_so(
) {
    let work = tmp("addr");
    let out = work.join("out");
    let mut j = job(
        &out,
        Some(write_clip(&work, 120)),
        true,
        false,
        OpenatcaRows::Ticks,
    );
    j.eram_address = EramAddress::Drop;
    run(j).unwrap();
    let recs = records_062(&out.join("eram.ast"));
    assert_eq!(recs.len(), 60);
    let by = |cs: &[u8; 8]| -> Vec<&oa_asterix::cat062::Cat062> {
        recs.iter()
            .filter(|r| {
                r.derived
                    .as_ref()
                    .and_then(|d| d.ident)
                    .is_some_and(|i| i[..cs.len().min(8)] == cs[..])
            })
            .collect()
    };
    let agree = by(b"AGREE1  ");
    let wrong = by(b"WRONG2  ");
    assert_eq!((agree.len(), wrong.len()), (20, 20));
    assert!(agree
        .iter()
        .all(|r| r.derived.as_ref().unwrap().address == Some(ADDR)));
    assert!(wrong
        .iter()
        .all(|r| r.derived.as_ref().unwrap().address.is_none()));
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.contains("address check: flights whose track agrees with their address: 1"),
        "{r}"
    );
    assert!(
        r.contains("address check: flights whose track contradicts their address: 1"),
        "{r}"
    );
    assert!(r.contains("address check: rows written without I380 address because of it (--eram-contradicted-address drop): 20"), "{r}");
    assert!(r.contains("ERAM FILE ALTERED (--eram-contradicted-address drop): 20 rows of the 1 flights listed here"), "{r}");
    assert!(
        r.contains(
            "contradicts its address: flight 1 callsign WRONG2 address 3C66B0: 20 samples, 20 paired"
        ),
        "{r}"
    );
    assert!(r.contains("EVERY ADS-B report of the clip"), "{r}");
    assert!(r.contains("    I380 target address: 20\n"), "{r}");
    assert!(r.contains("20: I380 target address withheld"), "{r}");
    // Without an ADS-B clip nothing is checked, and the README says so.
    let out = work.join("noclip");
    run(job(&out, None, true, false, OpenatcaRows::Ticks)).unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(r.contains("address check: NOT run"), "{r}");
    assert!(
        records_062(&out.join("eram.ast"))
            .iter()
            .filter(|r| r.derived.as_ref().is_some_and(|d| d.address.is_some()))
            .count()
            == 40
    );
}

fn point(address: u32, k: i64, lat_e7: i32, lon_e7: i32) -> RefPoint {
    RefPoint {
        address,
        t_ns: k * 1_000_000_000,
        lat_e7,
        lon_e7,
    }
}

// REQ: OA-CMP-035
#[test]
fn the_check_rule_agrees_contradicts_or_does_not_check_by_pairs_and_median() {
    let refs: Vec<RefPoint> = (0..100)
        .map(|k| point(7, k, 400_000_000, 100_000_000))
        .collect();
    let near: Vec<(i64, i32, i32)> = (0..20)
        .map(|k| (k * 3_000_000_000, 400_010_000, 100_010_000))
        .collect();
    assert!(matches!(
        check_flight(7, &near, &refs),
        Verdict::Agrees { pairs: 20, .. }
    ));
    // 2 degrees of latitude: about 222 km.
    let far: Vec<(i64, i32, i32)> = (0..20)
        .map(|k| (k * 3_000_000_000, 420_000_000, 100_000_000))
        .collect();
    match check_flight(7, &far, &refs) {
        Verdict::Contradicts { pairs, median_km } => {
            assert_eq!(pairs, 20);
            assert!((median_km - 222.4).abs() < 1.0, "{median_km}");
        }
        v => panic!("{v:?}"),
    }
    // Nine pairs: not checked, whatever the distance. No reports of the address: not checked.
    assert_eq!(
        check_flight(7, &far[..9], &refs),
        Verdict::NotChecked { pairs: 9 }
    );
    assert_eq!(
        check_flight(8, &far, &refs),
        Verdict::NotChecked { pairs: 0 }
    );
    // A sample 11 s from the nearest report is not paired.
    let late: Vec<(i64, i32, i32)> = (0..20)
        .map(|k| (200_000_000_000 + k, 400_000_000, 100_000_000))
        .collect();
    assert_eq!(
        check_flight(7, &late, &refs),
        Verdict::NotChecked { pairs: 0 }
    );
    // The median decides, not a minority: a third of the samples far away still agrees.
    let mixed: Vec<(i64, i32, i32)> = (0..30)
        .map(|k| {
            (
                k * 2_000_000_000,
                if k % 3 == 0 { 420_000_000 } else { 400_000_000 },
                100_000_000,
            )
        })
        .collect();
    assert!(matches!(
        check_flight(7, &mixed, &refs),
        Verdict::Agrees { .. }
    ));
    assert!((distance_km((0, 0), (0, 10_000_000)) - 111.19).abs() < 0.1);
    // Round the antimeridian.
    assert!(distance_km((0, 1_799_000_000), (0, -1_799_000_000)) < 230.0);
}

// REQ: OA-CMP-032
#[test]
fn the_interval_summary_is_nearest_rank_over_the_histogram() {
    let h = [(0u32, 1u64), (128, 6), (256, 2), (1280, 1)];
    assert_eq!(percentile(&h, 1, 2), Some(128));
    assert_eq!(percentile(&h, 9, 10), Some(256));
    assert_eq!(percentile(&h, 1, 1), Some(1280));
    assert_eq!(percentile(&[], 1, 2), None);
    let s = summarize(&h);
    assert_eq!(
        (s.pairs, s.zero, s.median, s.p90, s.max),
        (10, 1, 128, 256, 1280)
    );
    assert_eq!(summarize(&[]).pairs, 0);
    let _ = NOTE_ADDRESS_WITHHELD;
}

// REQ: OA-CMP-035
#[test]
fn by_default_the_address_check_only_reports_and_the_published_address_is_written() {
    let work = tmp("keep");
    let out = work.join("out");
    run(job(
        &out,
        Some(write_clip(&work, 120)),
        true,
        false,
        OpenatcaRows::Ticks,
    ))
    .unwrap();
    let recs = records_062(&out.join("eram.ast"));
    assert_eq!(
        recs.iter()
            .filter(|r| r.derived.as_ref().is_some_and(|d| d.address == Some(ADDR)))
            .count(),
        40,
        "both flights carry the address ERAM published"
    );
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.contains("REPORT ONLY (--eram-contradicted-address keep"),
        "{r}"
    );
    assert!(!r.contains("ERAM FILE ALTERED"), "{r}");
    assert!(
        r.contains(
            "contradicts its address: flight 1 callsign WRONG2 address 3C66B0: 20 samples, 20 paired"
        ),
        "{r}"
    );
    assert!(r.contains("EVERY ADS-B report of the clip"), "{r}");
    assert!(
        r.contains("because of it (--eram-contradicted-address drop): 0"),
        "{r}"
    );
    // Under split the set B reports are in the reference set too.
    let out = work.join("split");
    let mut j = job(
        &out,
        Some(write_clip(&work, 120)),
        true,
        false,
        OpenatcaRows::Ticks,
    );
    j.mode = InputsMode::Split;
    run(j).unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.contains("under --inputs split also including set B"),
        "{r}"
    );
}

// REQ: OA-CMP-038
#[test]
fn the_check_counts_are_exact_and_use_exported_deduplicated_samples_only() {
    let work = tmp("counts");
    let out = work.join("out");
    let mut leg = Legacy::default();
    let w = flight_ix(&mut leg, 1).unwrap();
    let n = flight_ix(&mut leg, 2).unwrap();
    let g = flight_ix(&mut leg, 3).unwrap();
    leg.flights.callsign[w as usize] = *b"WRONG2\0\0";
    leg.flights.addr[w as usize] = ADDR;
    leg.flights.callsign[n as usize] = *b"NONICAO\0";
    leg.flights.addr[n as usize] = 0x0100_0000;
    leg.flights.callsign[g as usize] = *b"GOOD3\0\0\0";
    leg.flights.addr[g as usize] = ADDR;
    for k in 0..20i64 {
        let t_ms = (T0_NS / 1_000_000) + (10 + 5 * k) * 1000;
        let (la, lo) = clip_position(10 + 5 * k);
        // The wrong flight: every sample written twice (exact copies).
        for _ in 0..2 {
            push_th(
                &mut leg,
                w,
                t_ms,
                0,
                la + 20_000_000,
                lo,
                35_000,
                None,
                false,
            );
        }
        push_th(&mut leg, n, t_ms, 0, la, lo, 35_000, None, false);
        push_th(&mut leg, g, t_ms, 0, la + 500, lo, 35_000, None, false);
    }
    // Samples that are refused (latitude out of range): never paired.
    for k in 0..15i64 {
        push_th(
            &mut leg,
            w,
            (T0_NS / 1_000_000) + (200 + k) * 1000,
            0,
            950_000_000,
            0,
            35_000,
            None,
            false,
        );
    }
    let mut j = job(
        &out,
        Some(write_clip(&work, 120)),
        false,
        false,
        OpenatcaRows::Ticks,
    );
    j.sfdps = Some(EramSource::Loaded(Box::new(leg), vec!["test input".into()]));
    run(j).unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    for needle in [
        "flights with samples and an aircraft address: 3",
        "address check: flights whose track agrees with their address: 1",
        "address check: flights not checked (too few ADS-B reports of the address to compare): 0",
        "address check: flights with a non-ICAO address (above 0xFFFFFF; nothing to compare with): 1",
        "address check: flights whose track contradicts their address: 1",
        "contradicts its address: flight 0 callsign WRONG2 address 3C66B0: 20 samples, 20 paired",
        "the lower of the two middle values",
    ] {
        assert!(r.contains(needle), "{needle}\n{r}");
    }
}

// REQ: OA-CMP-038
#[test]
fn the_median_is_the_lower_middle_value_and_the_pair_arithmetic_does_not_overflow() {
    // Ten pairs: five at 0 km and five about 222 km away; the lower middle is 0 km (agrees),
    // the upper would be 222 km.
    let refs: Vec<RefPoint> = (0..40)
        .map(|k| point(7, k, 400_000_000, 100_000_000))
        .collect();
    let s: Vec<(i64, i32, i32)> = (0..10)
        .map(|k| {
            (
                k * 2_000_000_000,
                if k < 5 { 400_000_000 } else { 420_000_000 },
                100_000_000,
            )
        })
        .collect();
    assert!(matches!(
        check_flight(7, &s, &refs),
        Verdict::Agrees { pairs: 10, .. }
    ));
    // The extreme times are compared without a subtraction that overflows.
    let ext = [(i64::MIN, 0, 0), (i64::MAX, 0, 0)];
    let far = [point(7, 0, 0, 0)];
    assert_eq!(
        check_flight(7, &ext, &far),
        Verdict::NotChecked { pairs: 0 }
    );
    let big = [RefPoint {
        address: 7,
        t_ns: i64::MAX,
        lat_e7: 0,
        lon_e7: 0,
    }];
    assert_eq!(
        check_flight(7, &ext, &big),
        Verdict::NotChecked { pairs: 1 }
    );
    assert_eq!(check_flight(0x0100_0000, &ext, &big), Verdict::NonIcao);
}

// REQ: OA-CMP-039
#[test]
fn the_nearest_report_rules_for_before_after_ties_ten_seconds_and_25_km() {
    let at = |s: i64| s * 1_000_000_000;
    let refs = [point(7, 0, 1, 1), point(7, 10, 2, 2)];
    assert_eq!(
        nearest(&refs, at(3)).map(|r| r.lat_e7),
        Some(1),
        "before is closer"
    );
    assert_eq!(
        nearest(&refs, at(7)).map(|r| r.lat_e7),
        Some(2),
        "after is closer"
    );
    assert_eq!(
        nearest(&refs, at(5)).map(|r| r.lat_e7),
        Some(1),
        "a tie: the earlier wins"
    );
    assert_eq!(
        nearest(&refs, at(-4)).map(|r| r.lat_e7),
        Some(1),
        "before the first"
    );
    assert_eq!(
        nearest(&refs, at(14)).map(|r| r.lat_e7),
        Some(2),
        "after the last"
    );
    let one = [point(7, 0, 1, 1)];
    assert!(nearest(&one, at(10)).is_some(), "exactly 10 s is in");
    assert!(nearest(&one, at(10) + 1).is_none(), "10 s and 1 ns is out");
    assert!(nearest(&one, -at(10)).is_some());
    assert!(nearest(&one, -at(10) - 1).is_none());
    assert!(nearest(&[], 0).is_none());
    assert!(!contradicts(25.0), "exactly 25 km is not contradicted");
    assert!(contradicts(25.000_001), "just above is");
    // Through the whole rule: ten pairs at the same distance, a little over and under 25 km.
    let refs: Vec<RefPoint> = (0..20).map(|k| point(7, k, 0, 0)).collect();
    let samples = |lat_e7: i32| -> Vec<(i64, i32, i32)> {
        (0..10).map(|k| (k * 1_000_000_000, lat_e7, 0)).collect()
    };
    // 1e-7 degree of latitude is 1.11195 cm; 25 km is 2_248_300 of them (rounded down).
    assert!(matches!(
        check_flight(7, &samples(2_248_000), &refs),
        Verdict::Agrees { .. }
    ));
    assert!(matches!(
        check_flight(7, &samples(2_249_000), &refs),
        Verdict::Contradicts { .. }
    ));
}

// REQ: OA-CMP-040
#[test]
fn refused_rows_change_no_counter_and_written_plus_refused_is_offered() {
    use oa_track::{O_COAST, O_IDENTITY_UNCONFIRMED, O_REACQUIRED, O_TICK, O_UNCERTAIN};
    let lay = |n: &str| Layout {
        out: tmp(n),
        day: DAY,
        split: false,
    };
    let bad = O_TICK | O_COAST | O_UNCERTAIN | O_REACQUIRED | O_IDENTITY_UNCONFIRMED;
    let mut o = track_output(&[
        (O_TICK, 1, 7),
        (bad, 2, 7),
        (O_TICK | O_UNCERTAIN, 3, 7),
        (O_TICK | O_COAST, 4, 7),
    ]);
    o.lat_e7[1] = 950_000_000;
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay("cnt"));
    let mut info = TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true, OpenatcaRows::Ticks).unwrap();
    assert_eq!(info.tick_rows, 3);
    assert_eq!(
        (
            info.coasting,
            info.uncertain,
            info.reacquired,
            info.unconfirmed_identity
        ),
        (1, 1, 0, 0),
        "the refused row's flags are not counted"
    );
    let f = ex.finish().unwrap();
    let refused: u64 = f.report.refused.iter().sum();
    assert_eq!(info.tick_rows + refused, 4, "written + refused = offered");
    // Updates: a refused update does not move the time of the last update row.
    let mut o = track_output(&[(0, 1, 7), (0, 5, 7), (0, 3, 7)]);
    o.lat_e7[1] = 950_000_000;
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay("cnt2"));
    let mut info = TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true, OpenatcaRows::Updates).unwrap();
    assert_eq!(
        (
            info.update_rows,
            info.updates_out_of_order,
            info.last_update_ns
        ),
        (2, 0, Some(DAY_NS + 3_000_000_000))
    );
    ex.abort();
    // A batch is taken in time order: nothing is dropped as late.
    let o = track_output(&[(0, 3, 7), (0, 1, 7), (0, 2, 7)]);
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay("cnt3"));
    let mut info = TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true, OpenatcaRows::Updates).unwrap();
    assert_eq!((info.update_rows, info.updates_out_of_order), (3, 0));
    ex.abort();
}

// REQ: OA-CMP-041
#[test]
fn a_failed_run_removes_every_directory_level_it_created() {
    let work = tmp("levels");
    let clip = write_clip(&work, 90);
    // The hook fails after the data is written.
    let out = work.join("a").join("b").join("out");
    let mut hook = |s: Stage| match s {
        Stage::Verified => Err(ExportError::Io("injected".into())),
        _ => Ok(()),
    };
    assert!(run_job(
        job(&out, Some(clip.clone()), true, false, OpenatcaRows::Ticks),
        &mut hook,
        &mut |_| {}
    )
    .is_err());
    assert!(!work.join("a").exists(), "no level of the new path is left");
    // Split days, three levels.
    let out = work.join("c").join("d").join("out");
    assert!(run_job(
        job(&out, Some(clip.clone()), true, true, OpenatcaRows::Ticks),
        &mut hook,
        &mut |_| {}
    )
    .is_err());
    assert!(!work.join("c").exists());
    // An existing parent stays; a failure before anything is created creates nothing.
    fs::create_dir_all(work.join("keep")).unwrap();
    let out = work.join("keep").join("x").join("out");
    let mut j = job(&out, Some(clip), true, false, OpenatcaRows::Ticks);
    j.sfdps = Some(EramSource::Dir(work.join("no-such-sfdps")));
    assert!(run(j).is_err());
    assert!(work.join("keep").is_dir() && names(&work.join("keep")).is_empty());
}

// REQ: OA-CMP-042
#[test]
fn an_out_with_only_partial_files_is_refused_naming_them_and_they_stay() {
    let work = tmp("interrupted");
    let out = work.join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("eram.ast.partial"), b"x").unwrap();
    fs::write(out.join("openatca.ast.partial"), b"y").unwrap();
    let e = run(job(&out, None, true, false, OpenatcaRows::Ticks)).unwrap_err();
    let m = e.to_string();
    assert!(m.contains("an earlier run was interrupted"), "{m}");
    assert!(
        m.contains("eram.ast.partial") && m.contains("openatca.ast.partial"),
        "{m}"
    );
    assert_eq!(names(&out), ["eram.ast.partial", "openatca.ast.partial"]);
    // A directory with anything else says it is not empty.
    fs::write(out.join("notes.txt"), b"z").unwrap();
    let m = run(job(&out, None, true, false, OpenatcaRows::Ticks))
        .unwrap_err()
        .to_string();
    assert!(m.contains("not empty") && !m.contains("earlier run"), "{m}");
}

// REQ: OA-CMP-043
#[test]
fn a_file_with_no_consecutive_rows_says_so_instead_of_printing_zeros() {
    let work = tmp("nopairs");
    let out = work.join("out");
    let mut leg = Legacy::default();
    let a = flight_ix(&mut leg, 1).unwrap();
    push_th(
        &mut leg,
        a,
        T0_NS / 1_000_000,
        0,
        410_000_000,
        -880_000_000,
        35_000,
        None,
        false,
    );
    let mut j = job(&out, None, false, false, OpenatcaRows::Ticks);
    j.sfdps = Some(EramSource::Loaded(Box::new(leg), vec!["test input".into()]));
    run(j).unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.contains("no consecutive rows of one track (0 pairs)"),
        "{r}"
    );
    assert!(!r.contains("median 0.000 s"), "{r}");
}

// REQ: OA-CMP-044
#[test]
fn the_limit_plus_one_fails_and_a_scale_error_or_a_late_failure_leaves_nothing() {
    use oa_compass::shell::adsb_export::MAX_POSITION_ERROR_E7 as LIMIT_021;
    let work = tmp("limit1");
    // CAT062: limit passes, limit + 1 fails.
    for (extra, ok) in [(0, true), (1, false)] {
        let lay = Layout {
            out: work.join(format!("c62-{extra}")),
            day: DAY,
            split: false,
        };
        let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay);
        let row = TrackRow {
            t_ns: T0_NS,
            lat_e7: 0,
            lon_e7: 0,
            vel_east_mps: 1.0,
            vel_north_mps: 1.0,
            pos_cov_m2: [25.0, 0.0, 25.0],
            alt_ft: 10_000.0,
            has_alt: true,
            vrate_fpm: 0.0,
            coast: false,
            radar_disagrees: false,
            uncertain: false,
            reacquired: false,
            identity_unconfirmed: false,
            address: Some(ADDR),
            monosensor: true,
        };
        let c = track_record(&row).unwrap();
        ex.push(
            &c,
            1,
            (row.lat_e7 + MAX_POSITION_ERROR_E7 + extra, row.lon_e7),
        )
        .unwrap();
        assert_eq!(ex.finish().is_ok(), ok, "062 limit + {extra}");
    }
    // CAT021.
    for (extra, ok) in [(0, true), (1, false)] {
        let lay = Layout {
            out: work.join(format!("c21-{extra}")),
            day: DAY,
            split: false,
        };
        let mut ax = AdsbExport::new(ADSB_SOURCE, lay);
        let r = oa_compass::core::adsb::AdsbRow {
            time_ns: T0_NS,
            address: ADDR,
            non_icao: false,
            lat_e7: 0,
            lon_e7: 0,
            baro_alt_ft: None,
            geom_alt_ft: None,
            ground: false,
            vrate_fpm: None,
            rate_geometric: false,
            gs_dkt: None,
            track_ddeg: None,
            callsign: [0; 8],
            category: 0,
            nic: None,
            nac_p: None,
            nac_v: None,
            sil: None,
            sil_per_sample: None,
            version: None,
            gva: None,
            sda: None,
        };
        let c = oa_compass::core::adsb::adsb_record(&r).unwrap();
        ax.push(&c, (r.lat_e7 + LIMIT_021 + extra, r.lon_e7))
            .unwrap();
        assert_eq!(ax.finish().is_ok(), ok, "021 limit + {extra}");
    }
    // A scale error in the ERAM positions fails the whole job; nothing is left in out.
    let clip = write_clip(&work, 90);
    let out = work.join("scale").join("out");
    let mut j = job(&out, Some(clip.clone()), true, false, OpenatcaRows::Ticks);
    j.self_test_eram_offset_e7 = 100;
    let e = run(j).unwrap_err().to_string();
    assert!(e.contains("over the limit"), "{e}");
    assert!(!work.join("scale").exists(), "nothing left, no directory");
    // An ADS-B export failure after the OpenATCA partial exists.
    let out = work.join("adsbfail").join("out");
    let mut j = job(&out, Some(clip.clone()), true, false, OpenatcaRows::Ticks);
    j.self_test_adsb_offset_e7 = 100;
    assert!(run(j).is_err());
    assert!(!work.join("adsbfail").exists());
    // An ERAM load failure after the OpenATCA and ADS-B partials exist.
    let out = work.join("eramfail").join("out");
    let mut j = job(&out, Some(clip), true, false, OpenatcaRows::Ticks);
    j.sfdps = Some(EramSource::Dir(work.join("no-such-sfdps")));
    assert!(run(j).is_err());
    assert!(!work.join("eramfail").exists());
}

// REQ: OA-CMP-046
#[test]
fn the_modified_flag_of_the_build_commit_follows_the_tracked_files_of_the_built_trees() {
    let b = oa_build_info::BUILD;
    let dir = env!("CARGO_MANIFEST_DIR");
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "status",
            "--porcelain",
            "--untracked-files=no",
            "--",
            "../../crates",
            "../../tools",
            "../../Cargo.toml",
            "../../Cargo.lock",
        ])
        .output();
    // Without git the build says "unknown" and there is nothing to compare with.
    if let Ok(o) = o {
        if o.status.success() {
            let modified = !o.stdout.is_empty();
            assert_eq!(
                b.ends_with("(tracked files modified)"),
                modified,
                "{b}: the flag is the working tree's state when the crate was built"
            );
        }
    }
}

fn row_with_velocity(east: f32, north: f32) -> TrackRow {
    TrackRow {
        t_ns: T0_NS,
        lat_e7: 410_000_000,
        lon_e7: -880_000_000,
        vel_east_mps: east,
        vel_north_mps: north,
        pos_cov_m2: [25.0, 0.0, 25.0],
        alt_ft: 10_000.0,
        has_alt: true,
        vrate_fpm: 0.0,
        coast: false,
        radar_disagrees: false,
        uncertain: false,
        reacquired: false,
        identity_unconfirmed: false,
        address: Some(ADDR),
        monosensor: true,
    }
}

// REQ: OA-CMP-050
#[test]
fn i062_185_is_true_east_and_north_at_the_target_and_the_readme_says_so() {
    // Vx is the east component and Vy the north one, 0.25 m/s per count: 100 m/s east and
    // 50 m/s north are (400, 200); a westward 30 m/s is (-120, 0).
    let v = |e, n| {
        track_record(&row_with_velocity(e, n))
            .unwrap()
            .values
            .velocity
    };
    assert_eq!(v(100.0, 50.0), Some((400, 200)));
    assert_eq!(v(-30.0, 0.0), Some((-120, 0)));
    assert_eq!(v(0.0, -25.0), Some((0, -100)));
    let work = tmp("frame");
    let out = work.join("out");
    run(job(
        &out,
        Some(write_clip(&work, 30)),
        false,
        false,
        OpenatcaRows::Ticks,
    ))
    .unwrap();
    let r = fs::read_to_string(out.join("README.txt")).unwrap();
    assert!(
        r.contains("true east (Vx) and true north (Vy) at the target's own position"),
        "{r}"
    );
}

/// The track numbers (file order) of tick rows of the given (second, address) pairs, exported with
/// the tracker's dropped-instance boundaries `ends` (address, Unix ns), and the day's report.
fn numbers_with_ends(
    name: &str,
    rows: &[(i64, u32)],
    ends: &[(u32, i64)],
) -> (Vec<u16>, u64, u64, u64) {
    let out = tmp(name);
    let lay = Layout {
        out: out.clone(),
        day: DAY,
        split: false,
    };
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay);
    let mut info = TrackRunInfo::default();
    for &(a, e) in ends {
        info.ends.entry(a).or_default().push(e);
    }
    // One export call holding every row: a restart inside one batch.
    let list: Vec<(u8, i64, u32)> = rows
        .iter()
        .map(|&(t, a)| (oa_track::O_TICK, t, a))
        .collect();
    export_output(
        &track_output(&list),
        &mut ex,
        &mut info,
        true,
        OpenatcaRows::Ticks,
    )
    .unwrap();
    let f = ex.finish().unwrap();
    let d = &f.report.days[0];
    let counts = (d.tracks, d.keys, d.keys_multi_segment);
    f.commit().unwrap();
    let nums = records_062(&out.join("openatca.ast"))
        .iter()
        .map(|r| r.track_number.unwrap())
        .collect();
    (nums, counts.0, counts.1, counts.2)
}

// REQ: OA-CMP-048
#[test]
fn a_row_after_the_end_of_a_dropped_track_instance_is_a_new_track_number() {
    let drop = oa_track::score::SCORE_DROP_AFTER_NS;
    assert_eq!(drop, 120 * 1_000_000_000);
    // The instance's last update was at 3 s: its rows are on the table through 123 s (age equal to
    // the drop age) and the next instance's rows are later.
    let boundary = DAY_NS + 3_000_000_000 + drop;
    let ends = [(7, boundary)];
    let (n, tracks, keys, multi) = numbers_with_ends(
        "seg-ends",
        &[(1, 7), (2, 7), (3, 7), (123, 7), (124, 7)],
        &ends,
    );
    assert_eq!(
        n,
        [1, 1, 1, 1, 2],
        "a row at the boundary is the old instance's"
    );
    assert_eq!((tracks, keys, multi), (2, 1, 1));
    // To the nanosecond.
    let mut info = TrackRunInfo::default();
    info.ends.insert(7, vec![boundary]);
    assert_eq!(info.instance_of(7, boundary - 1), 0);
    assert_eq!(info.instance_of(7, boundary), 0);
    assert_eq!(info.instance_of(7, boundary + 1), 1);
    assert_eq!(
        info.instance_of(8, boundary + 1),
        0,
        "another address has no end"
    );
}

// REQ: OA-CMP-048
#[test]
fn a_long_gap_between_exported_rows_is_not_a_restart_without_a_tracker_end() {
    // 500 s between rows of one address, no instance dropped (for example the rows between were
    // refused or not exported): one track.
    let (n, tracks, _, multi) = numbers_with_ends("seg-gap", &[(1, 9), (500, 9)], &[]);
    assert_eq!(n, [1, 1]);
    assert_eq!((tracks, multi), (1, 0));
}

// REQ: OA-CMP-049
#[test]
fn track_segments_are_numbered_in_order_reported_and_the_space_is_checked() {
    // Aircraft 7 has two dropped instances and aircraft 8 none: the numbers follow the first
    // appearance of each segment, and the rows are exported in time order.
    let end = |s: i64| DAY_NS + s * 1_000_000_000;
    let (n, tracks, keys, multi) = numbers_with_ends(
        "segs",
        &[(1, 7), (2, 8), (100, 8), (200, 7), (400, 7)],
        &[(7, end(150)), (7, end(300))],
    );
    assert_eq!(n, [1, 2, 2, 3, 4]);
    assert_eq!((tracks, keys, multi), (4, 2, 1));
    // The 16-bit space: 65 535 numbers are given, one more is refused.
    let lay = Layout {
        out: tmp("segfull"),
        day: DAY,
        split: false,
    };
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay);
    let c = track_record(&row_with_velocity(1.0, 1.0)).unwrap();
    for k in 0..65_535u32 {
        ex.push(&c, k, (410_000_000, -880_000_000)).unwrap();
    }
    let e = ex
        .push(&c, 65_535, (410_000_000, -880_000_000))
        .unwrap_err();
    assert!(
        matches!(e, ExportError::TrackNumbers { .. })
            && e.to_string()
                .contains("more than 65535 track segments (including restarts)"),
        "{e}"
    );
    ex.abort();
}

// REQ: OA-CMP-047
#[test]
fn a_row_on_the_sealed_day_refuses_the_run_and_the_day_before_passes() {
    use oa_compass::shell::guard::SEALED_DAY;
    assert_eq!(
        oa_compass::core::time::days_from_civil(2026, 11, 19),
        Some(SEALED_DAY)
    );
    let work = tmp("sealed");
    let clip_of = |name: &str, rows: &[(i64, i64)]| -> PathBuf {
        let mut all = Reports::default();
        for &(d, s) in rows {
            let row = oa_adsb::Row {
                icao: ADDR,
                time_ns: d * 86_400 * 1_000_000_000 + s * 1_000_000_000,
                lat_e7: 410_000_000,
                lon_e7: -880_000_000,
                baro_alt_ft: 20_000,
                gs_dkt: 3500,
                track_ddeg: 0,
                nic: 8,
                nac_p: 9,
                nac_v: 2,
                sil: 3,
                version: 2,
                source: 0,
                ..ROW_EMPTY
            };
            reports_push(&mut all, &row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &all, 0, oa_adsb::reports_len(&all)).unwrap();
        let mut buf = vec![0u8; payload.len() + 256];
        let n = oa_feed::encode_frame(&mut buf, 1, 0, 0, T0_NS, 0, &payload).unwrap();
        let p = work.join(name);
        fs::write(&p, &buf[..n]).unwrap();
        p
    };
    // Split days: one row on 2026-11-18 and one on the sealed day: refused, nothing written.
    let clip = clip_of("sealed.oaf", &[(20_775, 86_399), (20_776, 0)]);
    let out = work.join("a").join("out");
    let mut j = job(&out, Some(clip), false, true, OpenatcaRows::Ticks);
    j.layout.day = 20_775;
    let e = run(j).unwrap_err().to_string();
    assert!(e.contains("sealed hold-out day"), "{e}");
    assert!(!work.join("a").exists());
    // 2026-11-18 23:59:59 alone passes.
    let clip = clip_of("ok.oaf", &[(20_775, 86_399)]);
    let out = work.join("b");
    let mut j = job(&out, Some(clip), false, true, OpenatcaRows::Ticks);
    j.layout.day = 20_775;
    run(j).unwrap();
    assert!(!names(&out).is_empty());
    // The date itself, even with no input.
    let mut j = job(&work.join("c"), None, true, false, OpenatcaRows::Ticks);
    j.layout.day = 20_776;
    assert!(run(j)
        .unwrap_err()
        .to_string()
        .contains("sealed hold-out day"));
}

/// An OAF1 clip of one aircraft with one report per (time ns, flags, latitude) given.
fn clip_rows(dir: &Path, name: &str, rows: &[(i64, u8, i32)]) -> PathBuf {
    let mut all = Reports::default();
    for &(time_ns, flags, lat_e7) in rows {
        let row = oa_adsb::Row {
            icao: ADDR,
            time_ns,
            lat_e7,
            lon_e7: -880_000_000,
            baro_alt_ft: 20_000,
            gs_dkt: 3500,
            track_ddeg: 0,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            flags,
            ..ROW_EMPTY
        };
        reports_push(&mut all, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &all, 0, oa_adsb::reports_len(&all)).unwrap();
    let mut buf = vec![0u8; payload.len() + 256];
    let n = oa_feed::encode_frame(&mut buf, 1, 0, 0, T0_NS, 0, &payload).unwrap();
    let p = dir.join(name);
    fs::write(&p, &buf[..n]).unwrap();
    p
}

/// A job with an ERAM source of one flight with a sample at each (position ms, latitude).
fn eram_job(out: &Path, day: i64, samples: &[(i64, i32)]) -> Job {
    let mut leg = Legacy::default();
    let a = flight_ix(&mut leg, 1).unwrap();
    leg.flights.callsign[a as usize] = *b"SEAL1\0\0\0";
    for &(ms, lat) in samples {
        push_th(&mut leg, a, ms, 0, lat, -880_000_000, 35_000, None, false);
    }
    let mut j = job(out, None, false, false, OpenatcaRows::Ticks);
    j.layout.day = day;
    j.sfdps = Some(EramSource::Loaded(Box::new(leg), vec!["test input".into()]));
    j
}

const SEALED_START_NS: i64 = 20_776 * 86_400 * 1_000_000_000;
const NS_256TH: i64 = 3_906_250;

fn assert_refused_and_nothing_made(j: Job, out: &Path, needle: &str) {
    let e = match run(j) {
        Ok(l) => panic!("{} was not refused: {l:?}", out.display()),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("sealed hold-out day"), "{e}");
    assert!(e.contains(needle), "{e}");
    assert!(!out.exists(), "no output directory after the refusal");
    assert!(!out.parent().unwrap().exists(), "no parent made either");
}

// REQ: OA-CMP-051
#[test]
fn the_prescan_refuses_sealed_eram_times_raw_or_rounded_before_anything_is_created() {
    let work = tmp("pre-eram");
    let ms_of = |ns: i64| ns / 1_000_000;
    let lat = 410_000_000;
    // 2026-11-19 23:59:59.999: its raw day is the sealed day.
    let out = work.join("a").join("out");
    let j = eram_job(
        &out,
        20_775,
        &[(ms_of(SEALED_START_NS + 86_399_999_000_000), lat)],
    );
    assert_refused_and_nothing_made(j, &out, "ERAM: 1 input rows");
    // 2026-11-18 23:59:59.998: raw day 11-18, but it rounds to 11-19 00:00:00: refused, because
    // the export would file it under the sealed day.
    let out = work.join("b").join("out");
    let j = eram_job(&out, 20_775, &[(ms_of(SEALED_START_NS) - 2, lat)]);
    assert_refused_and_nothing_made(j, &out, "ERAM: 1 input rows");
    // A refused-only sample (latitude out of range) on the sealed day still refuses the run.
    let out = work.join("c").join("out");
    let j = eram_job(
        &out,
        20_775,
        &[
            (ms_of(SEALED_START_NS - 86_400_000_000_000), lat),
            (ms_of(SEALED_START_NS) + 5_000, 950_000_000),
        ],
    );
    assert_refused_and_nothing_made(j, &out, "ERAM: 1 input rows");
    // 2026-11-20 00:00:00.000 is accepted (day 20 777), and 11-18 23:59:59.000 too.
    let out = work.join("d");
    run(eram_job(
        &out,
        20_777,
        &[(ms_of(SEALED_START_NS + 86_400_000_000_000), lat)],
    ))
    .unwrap();
    assert!(out.join("eram.ast").exists());
    let out = work.join("e");
    run(eram_job(
        &out,
        20_775,
        &[(ms_of(SEALED_START_NS) - 1_000, lat)],
    ))
    .unwrap();
    assert!(out.join("eram.ast").exists());
}

// REQ: OA-CMP-051
#[test]
fn the_prescan_refuses_sealed_adsb_times_including_stale_and_refused_rows() {
    let work = tmp("pre-adsb");
    let ok_row = (SEALED_START_NS - 60_000_000_000, 0u8, 410_000_000);
    // Exactly 1/256 s before midnight rounds to 00:00:00 of the sealed day: refused.
    let clip = clip_rows(
        &work,
        "a.oaf",
        &[ok_row, (SEALED_START_NS - NS_256TH, 0, 410_000_000)],
    );
    let out = work.join("a").join("out");
    let mut j = job(&out, Some(clip), false, false, OpenatcaRows::Ticks);
    j.layout.day = 20_775;
    assert_refused_and_nothing_made(j, &out, "ADS-B: 1 input rows");
    // One ns earlier rounds down, stays on 11-18, and passes.
    let clip = clip_rows(
        &work,
        "b.oaf",
        &[ok_row, (SEALED_START_NS - NS_256TH - 1, 0, 410_000_000)],
    );
    let out = work.join("b");
    let mut j = job(&out, Some(clip), false, false, OpenatcaRows::Ticks);
    j.layout.day = 20_775;
    run(j).unwrap();
    // A stale-only sealed row (the export leaves stale reports out) still refuses.
    let clip = clip_rows(
        &work,
        "c.oaf",
        &[
            ok_row,
            (
                SEALED_START_NS + 1_000_000_000,
                oa_adsb::F_STALE,
                410_000_000,
            ),
        ],
    );
    let out = work.join("c").join("out");
    let mut j = job(&out, Some(clip), false, false, OpenatcaRows::Ticks);
    j.layout.day = 20_775;
    assert_refused_and_nothing_made(j, &out, "ADS-B: 1 input rows");
    // A row the clip loader itself drops (position out of range) never becomes a report, so
    // the pre-scan cannot see it; it cannot reach any output either.
    // 2026-11-20 00:00:00.000 alone is accepted.
    let clip = clip_rows(
        &work,
        "e.oaf",
        &[(SEALED_START_NS + 86_400_000_000_000, 0, 410_000_000)],
    );
    let out = work.join("e");
    let mut j = job(&out, Some(clip), false, false, OpenatcaRows::Ticks);
    j.layout.day = 20_777;
    run(j).unwrap();
}

// REQ: OA-CMP-051
#[test]
fn the_second_check_refuses_a_sealed_day_row_at_the_eram_and_cat021_paths() {
    use oa_compass::core::adsb::{adsb_record, AdsbRow};
    use oa_compass::core::convert::EramRow;
    // ERAM path: a sample on the sealed day pushed to the export.
    let lay = Layout {
        out: tmp("sec").join("e"),
        day: 20_776,
        split: true,
    };
    let mut ex = SystemExport::new(
        "ERAM",
        "eram.ast",
        oa_compass::shell::notes::ERAM_SOURCE,
        lay,
    );
    let r = EramRow {
        pos_ms: SEALED_START_NS / 1_000_000 + 1000,
        lat_e7: 410_000_000,
        lon_e7: -880_000_000,
        alt_ft: Some(35_000),
        vel_dkt: None,
        coast: false,
        address: None,
        callsign: None,
        address_withheld: false,
    };
    let c = eram_record(&r).unwrap();
    let e = ex
        .push(&c, 1, (r.lat_e7, r.lon_e7))
        .unwrap_err()
        .to_string();
    assert!(e.contains("sealed hold-out day"), "{e}");
    ex.abort();
    // CAT021 path.
    let lay = Layout {
        out: tmp("sec").join("a"),
        day: 20_776,
        split: true,
    };
    let mut ax = AdsbExport::new(ADSB_SOURCE, lay);
    let a = AdsbRow {
        time_ns: SEALED_START_NS + 1_000_000_000,
        address: ADDR,
        non_icao: false,
        lat_e7: 0,
        lon_e7: 0,
        baro_alt_ft: None,
        geom_alt_ft: None,
        ground: false,
        vrate_fpm: None,
        rate_geometric: false,
        gs_dkt: None,
        track_ddeg: None,
        callsign: [0; 8],
        category: 0,
        nic: None,
        nac_p: None,
        nac_v: None,
        sil: None,
        sil_per_sample: None,
        version: None,
        gva: None,
        sda: None,
    };
    let c = adsb_record(&a).unwrap();
    let e = ax.push(&c, (0, 0)).unwrap_err().to_string();
    assert!(e.contains("sealed hold-out day"), "{e}");
    ax.abort();
}

/// The track numbers (file order) and README of a run over one aircraft whose reports are at the
/// given times (seconds as ns offsets from the clip start).
fn run_numbers(name: &str, times_ns: &[i64], rows: OpenatcaRows) -> (Vec<u16>, String) {
    let work = tmp(name);
    let reports: Vec<(i64, u8, i32)> = times_ns
        .iter()
        .map(|&t| (T0_NS + t, 0u8, clip_position(t / 1_000_000_000).0))
        .collect();
    let clip = clip_rows(&work, "c.oaf", &reports);
    let out = work.join("out");
    run(job(&out, Some(clip), false, false, rows)).unwrap();
    let nums = records_062(&out.join("openatca.ast"))
        .iter()
        .map(|r| r.track_number.unwrap())
        .collect();
    (nums, fs::read_to_string(out.join("README.txt")).unwrap())
}

const S: i64 = 1_000_000_000;

/// `nums` is a run of 1s then a run of 2s, both non-empty.
fn assert_two_runs(nums: &[u16]) {
    let ones = nums.iter().take_while(|&&x| x == 1).count();
    assert!(ones > 0 && ones < nums.len(), "{nums:?}");
    assert!(nums[ones..].iter().all(|&x| x == 2), "{nums:?}");
}

// REQ: OA-CMP-048
#[test]
fn a_restart_after_140_seconds_gets_a_new_number_in_ticks_and_updates_mode() {
    let burst = |from: i64| (from..from + 10).map(|k| k * S).collect::<Vec<i64>>();
    let both: Vec<i64> = burst(0).into_iter().chain(burst(140)).collect();
    for (name, rows) in [
        ("e2e-ticks", OpenatcaRows::Ticks),
        ("e2e-updates", OpenatcaRows::Updates),
    ] {
        let (n, readme) = run_numbers(name, &both, rows);
        assert_two_runs(&n);
        assert!(
            readme.contains("2 track segments (I040 numbers 1 to 2) of 1 keys, 1 keys with more than one segment"),
            "{readme}"
        );
        assert!(readme.contains("track restarts: 1 "), "{readme}");
        assert!(
            readme.contains("of the 65535 the 16-bit I040 holds"),
            "{readme}"
        );
    }
    // The whole clip is small: the rows of both instances are exported in one batch, so the 140 s
    // case is also a re-init inside one batch of tick rows. 119 s after the last update (the
    // second burst at 128 s) the track is still alive: one track.
    let near: Vec<i64> = burst(0).into_iter().chain(burst(128)).collect();
    let (n, readme) = run_numbers("e2e-119", &near, OpenatcaRows::Ticks);
    assert!(n.iter().all(|&x| x == 1), "{n:?}");
    assert!(readme.contains("1 track segments"), "{readme}");
    let alive: Vec<i64> = burst(0).into_iter().chain(burst(100)).collect();
    let (n, readme) = run_numbers("e2e-alive", &alive, OpenatcaRows::Ticks);
    assert!(n.iter().all(|&x| x == 1), "{n:?}");
    assert!(
        readme.contains(
            "1 track segments (I040 numbers 1 to 1) of 1 keys, 0 keys with more than one segment"
        ),
        "{readme}"
    );
}

// REQ: OA-CMP-048
#[test]
fn the_export_follows_the_tracker_where_it_drops_a_track_not_a_clock_rule() {
    let first: Vec<i64> = (0..10).map(|k| k * S).collect();
    // The tracker drops a track only at a sweep and measures the age from its own time basis, so
    // the report time at which a second burst restarts the track is the tracker's, measured here:
    // 130 s after the first burst's start the track still lives, 131 s it does not. (The boundary
    // to the nanosecond is in the unit test of the instance rule above.)
    let after = |at_s: i64| -> Vec<i64> {
        first
            .iter()
            .copied()
            .chain((0..10).map(|k| (at_s + k) * S))
            .collect()
    };
    for rows in [OpenatcaRows::Updates, OpenatcaRows::Ticks] {
        let (n, _) = run_numbers("age-130", &after(130), rows);
        assert!(n.iter().all(|&x| x == 1), "{rows:?}: {n:?}");
        let (n, _) = run_numbers("age-131", &after(131), rows);
        assert_two_runs(&n);
    }
}

// REQ: OA-CMP-048
#[test]
fn the_tracker_holds_one_track_per_address_so_simultaneous_reports_make_one_track() {
    // One address reporting from two places a second apart: the tracker's key index has one slot
    // per address, so at any time at most one row of that address exists, and the numbering has
    // nothing to separate.
    let work = tmp("simul");
    let mut reports: Vec<(i64, u8, i32)> = Vec::new();
    for k in 0..30i64 {
        let lat = clip_position(k).0;
        reports.push((T0_NS + k * S, 0, lat));
        reports.push((T0_NS + k * S + S / 2, 0, lat + 20_000));
    }
    let clip = clip_rows(&work, "c.oaf", &reports);
    let out = work.join("out");
    run(job(&out, Some(clip), false, false, OpenatcaRows::Ticks)).unwrap();
    let recs = records_062(&out.join("openatca.ast"));
    let mut by_time = std::collections::BTreeMap::new();
    for r in &recs {
        *by_time.entry(r.time.unwrap()).or_insert(0u32) += 1;
    }
    assert!(!by_time.is_empty());
    assert!(
        by_time.values().all(|&c| c == 1),
        "two rows of one address at one time"
    );
}

// REQ: OA-CMP-047
#[test]
fn an_out_inside_a_sealed_folder_is_refused_and_nothing_is_created() {
    let work = tmp("sealed-out");
    let out = work.join("scds").join("rec-20261119").join("export");
    let e = run(job(&out, None, true, false, OpenatcaRows::Ticks)).unwrap_err();
    assert!(e.to_string().contains("sealed hold-out recording"), "{e}");
    assert!(!work.join("scds").exists());
    #[cfg(unix)]
    {
        let real = work.join("real").join("stdds-20261119-holdout");
        fs::create_dir_all(&real).unwrap();
        let link = work.join("lnk");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = run(job(&link.join("o"), None, true, false, OpenatcaRows::Ticks)).unwrap_err();
        assert!(e.to_string().contains("sealed hold-out recording"), "{e}");
        assert!(names(&real).is_empty());
    }
}

// REQ: OA-CMP-050
#[test]
fn the_velocity_frame_sentence_names_the_systems_present_and_claims_east_north_for_openatca_only() {
    let work = tmp("velsent");
    let readme = |name: &str, clip: bool, eram: bool| {
        let out = work.join(name);
        let adsb = clip.then(|| write_clip(&work, 30));
        run(job(&out, adsb, eram, false, OpenatcaRows::Ticks)).unwrap();
        fs::read_to_string(out.join("README.txt")).unwrap()
    };
    let oa = "OpenATCA velocity (I185): the track's, true east (Vx) and true north (Vy)";
    let er = "ERAM velocity (I185): the east and north components as the SFDPS feed gives them; their frame is not verified here.";
    let r = readme("oa", true, false);
    assert!(r.contains(oa) && !r.contains("ERAM velocity"), "{r}");
    let r = readme("er", false, true);
    assert!(r.contains(er) && !r.contains("OpenATCA velocity"), "{r}");
    let r = readme("both", true, true);
    assert!(r.contains(oa) && r.contains(er), "{r}");
}
