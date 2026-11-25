//! Shell (B1): bounded CAT048-only radar replay and CAT062 export. No ADS-B or ERAM input is read.
//! CAT034 is refused explicitly because this build has no CAT034 decoder.

use std::fs;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use oa_asterix::{decode_block, Block, SourceId, MAX_BLOCK_LEN};
use oa_track::fusion::{
    fusion_advance, fusion_config, fusion_finish, fusion_new, fusion_plots, fusion_tracker,
    fusion_tracker_config, FusionConfig, FusionStats,
};
use oa_track::fusion_params::BUCKET_NS;
use oa_track::radar::{RadarPlot, RadarSite, NONE_ALT, NONE_CODE, PLOT_NO_ADDR};
use oa_track::score::DRAIN_ROWS;
use oa_track::{output_clear, output_len, output_new, tracker_stats, DEFAULT_CONFIG};

use super::args::parse_date;
use super::error::ExportError;
use super::export::{date_text, Layout, SystemExport};
use super::guard::{open_guarded, refuse_holdout_out, refuse_sealed_row};
use super::output_fs::{create_private_dir, write_new_private_file};
use super::run::{export_output, note_ends, OpenatcaRows, TrackRunInfo};

const USAGE: &str = "usage: oa-compass radar --cat048 <file.ast> --date <YYYY.MM.DD> --out <new-dir> --site-lat-deg <deg> --site-lon-deg <deg> --site-height-m <ellipsoid-m> --scan-period-ms <ms>";
const MAX_INPUT_BYTES: u64 = 1_073_741_824;
const MAX_PLOTS: u64 = 10_000_000;
const MAX_OUTPUT_ROWS: u64 = 10_000_000;
const RADAR_OUTPUT_SOURCE: SourceId = SourceId { sac: 255, sic: 14 };
const RADAR_PROFILE_NAME: &str = "MSSR_EUROCONTROL_1997";
const NS_PER_DAY: i64 = 86_400_000_000_000;
const TOD_UNITS_PER_DAY: u32 = 86_400 * 128;
const ROLLOVER_LATE: u32 = 18 * 60 * 60 * 128;
const ROLLOVER_EARLY: u32 = 6 * 60 * 60 * 128;

#[derive(Debug)]
struct Args {
    input: PathBuf,
    out: PathBuf,
    day: i64,
    lat: f64,
    lon: f64,
    height_m: f64,
    scan_ms: i64,
}

fn parse_f64(flag: &str, value: &str) -> Result<f64, String> {
    let v = value
        .parse::<f64>()
        .map_err(|_| format!("{flag} must be a finite number"))?;
    if !v.is_finite() {
        return Err(format!("{flag} must be a finite number"));
    }
    Ok(v)
}

fn parse_i64(flag: &str, value: &str) -> Result<i64, String> {
    value
        .parse::<i64>()
        .map_err(|_| format!("{flag} must be an integer"))
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut input = None;
    let mut out = None;
    let mut date = None;
    let mut lat = None;
    let mut lon = None;
    let mut height_m = None;
    let mut scan_ms = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args.get(i).ok_or("missing argument")?;
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        if value.starts_with("--") {
            return Err(format!("{flag} needs a value"));
        }
        let slot = match flag.as_str() {
            "--cat048" => &mut input,
            "--out" => &mut out,
            "--date" => &mut date,
            "--site-lat-deg" => &mut lat,
            "--site-lon-deg" => &mut lon,
            "--site-height-m" => &mut height_m,
            "--scan-period-ms" => &mut scan_ms,
            _ => return Err(format!("unknown argument {flag}")),
        };
        if slot.is_some() {
            return Err(format!("{flag} is given twice"));
        }
        *slot = Some(value.clone());
        i += 2;
    }
    let input = input.ok_or("--cat048 is required")?;
    let out = out.ok_or("--out is required")?;
    let date = date.ok_or("--date is required")?;
    let day = parse_date(&date).ok_or_else(|| format!("--date {date} is not YYYY.MM.DD"))?;
    let lat = parse_f64("--site-lat-deg", &lat.ok_or("--site-lat-deg is required")?)?;
    let lon = parse_f64("--site-lon-deg", &lon.ok_or("--site-lon-deg is required")?)?;
    let height_m = parse_f64(
        "--site-height-m",
        &height_m.ok_or("--site-height-m is required")?,
    )?;
    let scan_ms = parse_i64(
        "--scan-period-ms",
        &scan_ms.ok_or("--scan-period-ms is required")?,
    )?;
    if !(100..=600_000).contains(&scan_ms) {
        return Err("--scan-period-ms must be 100..=600000".into());
    }
    Ok(Args {
        input: input.into(),
        out: out.into(),
        day,
        lat,
        lon,
        height_m,
        scan_ms,
    })
}

pub fn radar_main(args: &[String]) -> ExitCode {
    let result = parse_args(args).map_err(ExportError::Input).and_then(run);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-compass radar: {e}\n{USAGE}");
            ExitCode::from(1)
        }
    }
}

fn run(a: Args) -> Result<(), ExportError> {
    refuse_holdout_out(&a.out)?;
    let input = open_guarded(&a.input)?;
    create_private_dir(&a.out).map_err(|e| super::error::io_err(&a.out, &e))?;
    let mut export = SystemExport::new_private(
        "OpenATCA radar-only",
        "radar.ast",
        RADAR_OUTPUT_SOURCE,
        Layout {
            out: a.out.clone(),
            day: a.day,
            split: true,
        },
    );
    let site0 = RadarSite {
        lat_deg: a.lat,
        lon_deg: a.lon,
        height_m: a.height_m,
        sigma_range_m: 70.0,
        sigma_az_deg: 0.08,
        range_quantum_m: 0.0,
        alt_quantum_ft: 100,
        period_ns: a.scan_ms.saturating_mul(1_000_000),
        epoch_ns: a.day.saturating_mul(NS_PER_DAY),
        sensor: 0,
    };
    let base = DEFAULT_CONFIG;
    let tracker_cfg = fusion_tracker_config(&base, &[site0]);
    let mut cfg: FusionConfig = fusion_config(tracker_cfg);
    // A single radar cannot estimate relative sensor registration. No estimation is performed.
    cfg.registration = false;
    let result = run_stream_with_limits(
        &a,
        input,
        site0,
        cfg,
        &mut export,
        RunLimits {
            max_bytes: MAX_INPUT_BYTES,
            max_records: MAX_PLOTS,
            max_output_rows: MAX_OUTPUT_ROWS,
        },
    );
    let reports = match result {
        Ok(v) => v,
        Err(e) => {
            export.abort();
            let _ = fs::remove_dir_all(&a.out);
            return Err(e);
        }
    };
    if reports.2 == 0 {
        export.abort();
        let _ = fs::remove_dir_all(&a.out);
        return Err(ExportError::Input(format!(
            "no CAT048 records with polar positions were offered; {}/{} records lacked polar position",
            reports.3, reports.1
        )));
    }
    export.counts.extend(reports.0.clone());
    let finished = match export.finish() {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_dir_all(&a.out);
            return Err(e);
        }
    };
    let metadata = (|| -> Result<(), ExportError> {
        for d in &finished.report.days {
            let dir = a.out.join(date_text(d.day));
            let sources = "{\"content_type\":\"data_sources\",\"content_version\":\"0.2\",\"data_sources\":[{\"ds_type\":\"Tracker\",\"name\":\"OpenATCA radar-only\",\"sac\":255,\"sic\":14}]}\n";
            let counters = reports
                .0
                .iter()
                .map(|(n, v)| format!("{n}: {v}\n"))
                .collect::<String>();
            let readme = format!(
            "OpenATCA radar-only replay; CAT062 Ed 1.21 file radar.ast.\nInput: CAT048 Ed 1.32 only; no ADS-B or ERAM input.\nUTC output day: {}.\nRadar site: {:.7},{:.7}; antenna height {:.1} m above ellipsoid.\nProfile: {} (70 m range sigma, 0.08 degree azimuth sigma, 100 ft Mode C quantum); not a measured site calibration.\nScan period: {} ms; scan epoch is UTC midnight of the supplied start date, not a north-marker observation.\nI048/140 1/128-second counts are converted to nanoseconds by integer floor. Records without CAT048 time refuse the run. Rows without polar position are skipped and counted. Missing/invalid flight level is counted separately; this command supplies no site or user fallback, while fusion may use the estimated height of an associated track, otherwise it refuses for NoHeight. Actual conversion refusals are reported separately. Valid CAT048 flight level is passed as the model's ellipsoidal height; pressure-to-geometric altitude difference and atmospheric refraction remain unmodelled.\nCAT034 is not supported: any CAT034 block refuses the run. Timestamps must be nondecreasing; equal timestamps are allowed. A single midnight rollover is accepted only from >=18:00 to <=06:00 TOD; all other time regressions and spans over 24 hours refuse. Output is split by UTC day. Input is capped at 1 GiB, 10,000,000 records (65,535 bytes per ASTERIX block), and 10,000,000 CAT062 rows. On Unix, output directories are created with mode 0700 and files with mode 0600; existing output paths are refused and never chmodded. Cleanup covers handled errors only; abrupt termination may leave a private partial tree that this command will not reuse.\nCounts (whole input stream):\n{}\nComplete FusionStats and TrackerStats diagnostics:\n{}\nNo result establishes accuracy on real radar data or equivalence to the separate synthetic fusion stress test.\n",
            date_text(d.day), a.lat, a.lon, a.height_m, RADAR_PROFILE_NAME, a.scan_ms, counters, reports.5
        );
            for (name, bytes) in [
                ("data_sources.json", sources.as_bytes()),
                ("README.txt", readme.as_bytes()),
            ] {
                let p = dir.join(name);
                write_new_private_file(&p, bytes).map_err(|e| super::error::io_err(&p, &e))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = metadata {
        let _ = fs::remove_dir_all(&a.out);
        return Err(e);
    }
    if let Err(e) = finished.commit() {
        let _ = fs::remove_dir_all(&a.out);
        return Err(e);
    }
    println!("CAT048 records: {}; plots offered: {}; records skipped (no polar): {}; midnight rollovers: {}", reports.1, reports.2, reports.3, reports.4);
    Ok(())
}

struct RunCounts(Vec<(String, u64)>, u64, u64, u64, u64, String);

#[derive(Clone, Copy)]
struct RunLimits {
    max_bytes: u64,
    max_records: u64,
    max_output_rows: u64,
}

fn run_stream_with_limits(
    a: &Args,
    file: fs::File,
    mut site: RadarSite,
    cfg: FusionConfig,
    export: &mut SystemExport,
    limits: RunLimits,
) -> Result<RunCounts, ExportError> {
    let mut r = BufReader::with_capacity(1 << 20, file);
    let mut f = None;
    let mut out = output_new(cfg.tracker.out_capacity);
    let mut info = TrackRunInfo::default();
    let mut logged = 0usize;
    let mut bytes_in = 0u64;
    let (mut records, mut offered, mut no_polar, mut no_valid_fl, mut rollovers) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut simulated = 0u64;
    let (mut source, mut previous_tod, mut previous_ns, mut first_ns, mut last_bucket) =
        (None, None, None, None, None);
    let mut blockbuf = Vec::with_capacity(65_535);
    loop {
        let mut head = [0u8; 3];
        match r.read(&mut head[..1]) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(super::error::io_err(&a.input, &e)),
        }
        r.read_exact(&mut head[1..])
            .map_err(|e| super::error::io_err(&a.input, &e))?;
        let len = usize::from(u16::from_be_bytes([head[1], head[2]]));
        if !(3..=MAX_BLOCK_LEN).contains(&len) {
            return Err(ExportError::Input(format!(
                "invalid ASTERIX block length {len}"
            )));
        }
        bytes_in = bytes_in
            .checked_add(len as u64)
            .ok_or_else(|| ExportError::Input("input byte count overflow".into()))?;
        if bytes_in > limits.max_bytes {
            return Err(ExportError::Input(format!(
                "input exceeds {} byte limit",
                limits.max_bytes
            )));
        }
        blockbuf.clear();
        blockbuf.extend_from_slice(&head);
        blockbuf.resize(len, 0);
        r.read_exact(
            blockbuf
                .get_mut(3..)
                .ok_or_else(|| ExportError::Input("invalid ASTERIX block buffer".into()))?,
        )
        .map_err(|e| super::error::io_err(&a.input, &e))?;
        if head[0] == 34 {
            return Err(ExportError::Input("CAT034 is unsupported by oa-asterix; this command refuses the stream rather than skipping sensor-status blocks".into()));
        }
        if head[0] != 48 {
            return Err(ExportError::Input(format!(
                "CAT{} block in CAT048-only input",
                head[0]
            )));
        }
        let (block, used) = decode_block(&blockbuf)
            .map_err(|e| ExportError::Input(format!("CAT048 decode: {e}")))?;
        if used != len {
            return Err(ExportError::Input(
                "CAT048 decoder did not consume the complete block".into(),
            ));
        }
        let Block::Cat048(recs) = block else {
            return Err(ExportError::Input(
                "decoder returned a non-CAT048 block".into(),
            ));
        };
        for rec in recs {
            records = records
                .checked_add(1)
                .ok_or_else(|| ExportError::Input("record count overflow".into()))?;
            if records > limits.max_records {
                return Err(ExportError::Input(format!(
                    "record count exceeds {} limit",
                    limits.max_records
                )));
            }
            simulated += u64::from(rec.descriptor.is_some_and(|d| d.simulated));
            match source {
                Some(s) if s != rec.source => {
                    return Err(ExportError::Input(
                        "multiple CAT048 SAC/SIC sources; only one radar is allowed".into(),
                    ))
                }
                None => {
                    source = Some(rec.source);
                    site.sensor = (u16::from(rec.source.sac) << 8) | u16::from(rec.source.sic);
                    f = Some(
                        fusion_new(&cfg, &[site])
                            .map_err(|e| ExportError::Input(format!("radar config: {e}")))?,
                    );
                }
                _ => {}
            }
            let tod = rec.time_of_day.ok_or_else(|| {
                ExportError::Input("CAT048 record lacks I048/140 time of day".into())
            })?;
            if tod >= TOD_UNITS_PER_DAY {
                return Err(ExportError::Input(format!(
                    "invalid CAT048 time of day {tod}"
                )));
            }
            let day_offset = if let Some(prev) = previous_tod {
                if tod < prev {
                    if rollovers != 0 || prev < ROLLOVER_LATE || tod > ROLLOVER_EARLY {
                        return Err(ExportError::Input("time of day regressed outside the single permitted midnight rollover rule".into()));
                    }
                    rollovers += 1;
                    1i64
                } else if rollovers > 0 {
                    1
                } else {
                    0
                }
            } else {
                0
            };
            let tod_ns = (i64::from(tod) * 1_000_000_000) / 128;
            let abs_ns = a
                .day
                .checked_mul(NS_PER_DAY)
                .and_then(|x| x.checked_add(day_offset * NS_PER_DAY))
                .and_then(|x| x.checked_add(tod_ns))
                .ok_or_else(|| ExportError::Input("timestamp overflow".into()))?;
            refuse_sealed_row("CAT048 radar input", abs_ns.div_euclid(NS_PER_DAY))?;
            if let Some(prev) = previous_ns {
                if abs_ns < prev {
                    return Err(ExportError::Input(
                        "CAT048 timestamps are not nondecreasing".into(),
                    ));
                }
                if abs_ns - first_ns.unwrap_or(abs_ns) > NS_PER_DAY {
                    return Err(ExportError::Input(
                        "CAT048 stream spans more than 24 hours".into(),
                    ));
                }
            } else {
                first_ns = Some(abs_ns);
            }
            previous_tod = Some(tod);
            previous_ns = Some(abs_ns);
            let Some(polar) = rec.polar else {
                no_polar += 1;
                continue;
            };
            if !rec
                .flight_level
                .is_some_and(|fl| !fl.not_validated && !fl.garbled)
            {
                no_valid_fl += 1;
            }
            let bucket = abs_ns.div_euclid(BUCKET_NS);
            let Some(fusion) = f.as_mut() else {
                return Err(ExportError::Input(
                    "CAT048 stream contained no source record".into(),
                ));
            };
            if last_bucket.is_some_and(|old| bucket > old) {
                fusion_advance(fusion, bucket.saturating_mul(BUCKET_NS), &mut out);
                note_ends(
                    fusion_tracker(fusion),
                    cfg.tracker.drop_after_ns,
                    &mut logged,
                    &mut info,
                )?;
                if output_len(&out) > DRAIN_ROWS.min(cfg.tracker.out_capacity / 2) {
                    export_output(&out, export, &mut info, true, OpenatcaRows::Ticks)?;
                    output_clear(&mut out);
                    check_output_limit(export, limits.max_output_rows)?;
                }
            }
            let (range_m, az_deg) = (
                f64::from(polar.rho) * 1852.0 / 256.0,
                f64::from(polar.theta) * 360.0 / 65_536.0,
            );
            let alt_ft = rec
                .flight_level
                .filter(|fl| !fl.not_validated && !fl.garbled)
                .map_or(NONE_ALT, |fl| i32::from(fl.raw) * 25);
            let addr = rec.address.map_or(PLOT_NO_ADDR, |a| a & 0x00ff_ffff);
            let plot = RadarPlot {
                time_ns: abs_ns,
                range_m,
                az_deg,
                alt_ft,
                addr,
                label: PLOT_NO_ADDR,
                mode_a: rec.mode3a.map_or(NONE_CODE, |m| m.code),
            };
            fusion_plots(fusion, 0, &[plot]);
            offered += 1;
            last_bucket = Some(bucket);
        }
    }
    let mut fusion = f.ok_or_else(|| ExportError::Input("empty CAT048 input".into()))?;
    if let Some(end) = previous_ns {
        fusion_finish(&mut fusion, end, &mut out);
    }
    note_ends(
        fusion_tracker(&fusion),
        cfg.tracker.drop_after_ns,
        &mut logged,
        &mut info,
    )?;
    export_output(&out, export, &mut info, true, OpenatcaRows::Ticks)?;
    check_output_limit(export, limits.max_output_rows)?;
    info.overflow = out.overflow;
    if info.overflow > 0 {
        return Err(ExportError::Input(format!(
            "tracker output overflow: {}",
            info.overflow
        )));
    }
    let stats: &FusionStats = oa_track::fusion::fusion_stats(&fusion);
    let ts = tracker_stats(fusion_tracker(&fusion));
    if stats.pending_overflow > 0
        || stats.bucket_plot_overflow > 0
        || stats.track_rec_overflow > 0
        || stats.scratch_full > 0
        || stats.bucket_work_capped > 0
        || stats.assoc_refused > 0
        || stats.track_refused > 0
        || stats.cand_capacity > 0
        || stats.assoc_incomplete_batches > 0
        || stats.assoc_incomplete_deferred > 0
        || stats.assoc_refused_deferred > 0
        || stats.init_capped > 0
        || stats.init_plots_refused > 0
        || stats.plot_scan_full > 0
        || stats.wide_full > 0
        || stats.init_wide_full > 0
        || ts.refused.iter().any(|&n| n > 0)
    {
        return Err(ExportError::Input(format!("radar-only replay incomplete: bounded loss/work cap or tracker refusal; no CAT062 output committed. FusionStats={stats:?}; TrackerStats={ts:?}")));
    }
    let counts = vec![
        ("CAT048 records".into(), records),
        ("input bytes read".into(), bytes_in),
        ("records marked simulated by I048/020".into(), simulated),
        ("polar plots offered to radar tracker".into(), offered),
        (
            "records skipped because polar position was absent".into(),
            no_polar,
        ),
        (
            "plots with missing or invalid I048/090 flight level (input condition, separate from conversion refusals)".into(),
            no_valid_fl,
        ),
        (
            "plots refused by fusion geometry/height checks".into(),
            stats.plots_refused,
        ),
        (
            "no-height conversion refusals during anonymous initiation".into(),
            stats.anonymous_init_conversion_refusals.no_height,
        ),
        (
            "no-height conversion refusals during association".into(),
            stats.pair_conversion_refusals.no_height,
        ),
        ("plots unassigned to a track".into(), stats.unassigned),
        ("plots late after bucket closure".into(), stats.late),
        ("plots held for ambiguity".into(), stats.held),
        (
            "plots refused by tracker, by reason".into(),
            ts.refused.iter().copied().sum(),
        ),
        ("hard-address plots applied".into(), stats.applied_hard),
        (
            "anonymous plot assignments applied".into(),
            stats.applied_assoc,
        ),
        ("prediction refusals".into(), stats.prediction_refused),
        ("hard address mismatches".into(), stats.hard_mismatch),
        (
            "candidate plots refused by capacity".into(),
            stats.cand_capacity,
        ),
        (
            "candidate plots refused by initiation".into(),
            stats.init_plots_refused,
        ),
        (
            "association-incomplete batches".into(),
            stats.assoc_incomplete_batches,
        ),
        (
            "association-incomplete deferred plots".into(),
            stats.assoc_incomplete_deferred,
        ),
        (
            "association-refused deferred plots".into(),
            stats.assoc_refused_deferred,
        ),
        ("assignment hard extras".into(), stats.hard_extra),
        ("assignment held for ambiguity".into(), stats.held),
        ("capacity evicted held plots".into(), stats.hold_evicted),
        ("candidate starts".into(), stats.cand_started),
        ("candidate confirmations".into(), stats.cand_confirmed),
        (
            "confirmed tracks refused by capacity".into(),
            stats.track_refused,
        ),
        ("tracker ticks exported".into(), info.tick_rows),
        ("midnight rollovers".into(), rollovers),
    ];
    let diagnostics = format!("FusionStats: {stats:#?}\nTrackerStats: {ts:#?}\n");
    Ok(RunCounts(
        counts,
        records,
        offered,
        no_polar,
        rollovers,
        diagnostics,
    ))
}

fn check_output_limit(export: &SystemExport, limit: u64) -> Result<(), ExportError> {
    let rows = export.rows();
    if rows > limit {
        return Err(ExportError::Input(format!(
            "CAT062 output row count {rows} exceeds {limit} row limit"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use oa_sensorsim::plots::{plots_push, to_cat048, Plot, Plots, NONE_ADDR, NONE_CODE};
    use std::path::Path;

    fn temp(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "oa-compass-radar-cap-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn sample_stream() -> Vec<u8> {
        let mut plots = Plots::default();
        plots_push(
            &mut plots,
            &Plot {
                time_ns: 0,
                scan: 0,
                kind: 0,
                range_m: 20.0 * 1852.0,
                az_deg: 90.0,
                alt_ft: 10_000,
                mode_a: NONE_CODE,
                addr: NONE_ADDR,
                truth_addr: NONE_ADDR,
                truth_range_m: 20.0 * 1852.0,
                truth_az_deg: 90.0,
            },
        );
        to_cat048(&plots, SourceId { sac: 12, sic: 34 }).unwrap()
    }

    fn limits(root: &Path, input: &Path, limits: RunLimits) -> Result<RunCounts, ExportError> {
        fs::create_dir(root).unwrap();
        fs::write(input, sample_stream()).unwrap();
        let args = Args {
            input: input.to_path_buf(),
            out: root.to_path_buf(),
            day: parse_date("2026.11.20").unwrap(),
            lat: 41.9,
            lon: -88.3,
            height_m: 200.0,
            scan_ms: 5_000,
        };
        let site = RadarSite {
            lat_deg: args.lat,
            lon_deg: args.lon,
            height_m: args.height_m,
            sigma_range_m: 70.0,
            sigma_az_deg: 0.08,
            range_quantum_m: 0.0,
            alt_quantum_ft: 100,
            period_ns: 5_000_000_000,
            epoch_ns: args.day * NS_PER_DAY,
            sensor: (12 << 8) | 34,
        };
        let cfg = fusion_config(fusion_tracker_config(&DEFAULT_CONFIG, &[site]));
        let mut export = SystemExport::new(
            "OpenATCA radar-only",
            "radar.ast",
            RADAR_OUTPUT_SOURCE,
            Layout {
                out: root.to_path_buf(),
                day: args.day,
                split: true,
            },
        );
        let file = fs::File::open(input).unwrap();
        let result = run_stream_with_limits(&args, file, site, cfg, &mut export, limits);
        if result.is_err() {
            export.abort();
        }
        result
    }

    #[test]
    fn input_byte_and_record_caps_are_checked_before_replay() {
        let root = temp("input");
        let input = root.with_extension("ast");
        let out1 = root.with_extension("bytes");
        let byte_result = limits(
            &out1,
            &input,
            RunLimits {
                max_bytes: 1,
                max_records: MAX_PLOTS,
                max_output_rows: MAX_OUTPUT_ROWS,
            },
        );
        assert!(matches!(byte_result, Err(ExportError::Input(s)) if s.contains("byte limit")));
        fs::remove_dir_all(&out1).unwrap();

        let out2 = root.with_extension("records");
        let record_result = limits(
            &out2,
            &input,
            RunLimits {
                max_bytes: MAX_INPUT_BYTES,
                max_records: 0,
                max_output_rows: MAX_OUTPUT_ROWS,
            },
        );
        assert!(
            matches!(record_result, Err(ExportError::Input(s)) if s.contains("record count exceeds 0"))
        );
        fs::remove_dir_all(&out2).unwrap();
        fs::remove_file(input).unwrap();
    }

    #[test]
    fn output_row_cap_refuses_before_commit() {
        use super::super::notes::OPENATCA_SOURCE;
        use crate::core::convert::{track_record, TrackRow};
        let root = temp("output");
        fs::create_dir(&root).unwrap();
        let mut export = SystemExport::new(
            "OpenATCA",
            "openatca.ast",
            OPENATCA_SOURCE,
            Layout {
                out: root.clone(),
                day: parse_date("2026.11.20").unwrap(),
                split: false,
            },
        );
        let row = TrackRow {
            t_ns: parse_date("2026.11.20").unwrap() * NS_PER_DAY,
            lat_e7: 419_000_000,
            lon_e7: -883_000_000,
            vel_east_mps: 100.0,
            vel_north_mps: 0.0,
            pos_cov_m2: [100.0, 0.0, 100.0],
            alt_ft: 10_000.0,
            has_alt: true,
            vrate_fpm: 0.0,
            coast: false,
            radar_disagrees: false,
            uncertain: false,
            reacquired: false,
            identity_unconfirmed: false,
            address: Some(0xABCDEF),
            monosensor: true,
        };
        export
            .push(&track_record(&row).unwrap(), 1, (row.lat_e7, row.lon_e7))
            .unwrap();
        assert!(
            matches!(check_output_limit(&export, 0), Err(ExportError::Input(s)) if s.contains("row count 1 exceeds 0"))
        );
        export.abort();
        fs::remove_dir_all(root).unwrap();
    }
}
