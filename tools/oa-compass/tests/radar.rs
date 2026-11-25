//! Synthetic CAT048 midnight replay compatibility, without external recordings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use oa_asterix::{decode_block, Block, SourceId};
use oa_sensorsim::plots::{plots_push, to_cat048, Plot, Plots, NONE_ADDR, NONE_ALT, NONE_CODE};
use oa_sensorsim::profile::MSSR_EUROCONTROL_1997;

fn temp(name: &str) -> PathBuf {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "oa-compass-radar-{name}-{}-{t}",
        std::process::id()
    ))
}

fn synthetic_sensor_stream(secs: &[i64], no_height_at: &[i64], source: SourceId) -> Vec<u8> {
    let mut plots = Plots::default();
    for &sec in secs {
        plots_push(
            &mut plots,
            &Plot {
                time_ns: sec * 1_000_000_000,
                scan: sec.div_euclid(5),
                kind: 0,
                range_m: 20.0 * 1852.0,
                az_deg: 90.0,
                alt_ft: if no_height_at.contains(&sec) {
                    NONE_ALT
                } else {
                    10_000
                },
                mode_a: NONE_CODE,
                addr: NONE_ADDR,
                truth_addr: NONE_ADDR,
                truth_range_m: 20.0 * 1852.0,
                truth_az_deg: 90.0,
            },
        );
    }
    to_cat048(&plots, source).unwrap()
}

fn success_stream() -> Vec<u8> {
    synthetic_sensor_stream(
        &[
            20, 25, 30, 35, 86_380, 86_385, 86_390, 86_395, 86_400, 86_405, 86_410, 86_415, 86_420,
        ],
        &[],
        SourceId { sac: 12, sic: 34 },
    )
}

fn radar_command(input: &Path, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_oa-compass"))
        .args([
            "radar",
            "--cat048",
            input.to_str().unwrap(),
            "--date",
            "2026.11.20",
            "--out",
            output.to_str().unwrap(),
            "--site-lat-deg",
            "41.9",
            "--site-lon-deg",
            "-88.3",
            "--site-height-m",
            "200",
            "--scan-period-ms",
            "5000",
        ])
        .output()
        .unwrap()
}

#[test]
fn radar_command_rolls_over_and_writes_compass_cat062() {
    let root = temp("midnight");
    let input = root.with_extension("ast");
    let output = root.with_extension("out");
    assert_eq!(MSSR_EUROCONTROL_1997.range_sigma_m, 70.0);
    assert_eq!(MSSR_EUROCONTROL_1997.az_sigma_deg, 0.08);
    assert_eq!(MSSR_EUROCONTROL_1997.alt_quantum_ft, 100);
    fs::write(&input, success_stream()).unwrap();
    let result = radar_command(&input, &output);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("2026.11.20/radar.ast").is_file());
    assert!(output.join("2026.11.21/radar.ast").is_file());
    for day in ["2026.11.20", "2026.11.21"] {
        let bytes = fs::read(output.join(day).join("radar.ast")).unwrap();
        let (block, used) = decode_block(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        let Block::Cat062(rows) = block else {
            panic!("not CAT062")
        };
        assert!(!rows.is_empty());
        assert!(rows
            .iter()
            .all(|r| r.source == SourceId { sac: 255, sic: 14 }));
        assert!(output.join(day).join("data_sources.json").is_file());
    }
    let readme = fs::read_to_string(output.join("2026.11.21/README.txt")).unwrap();
    assert!(readme.contains("midnight rollovers: 1"));
    assert!(readme.contains("CAT034 is not supported"));
    assert!(readme.contains("10,000,000 CAT062 rows"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(output.join("2026.11.21"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in ["radar.ast", "data_sources.json", "README.txt"] {
            assert_eq!(
                fs::metadata(output.join("2026.11.21").join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{name} must have private mode 0600"
            );
        }
    }
    fs::remove_dir_all(output).unwrap();
    fs::remove_file(input).unwrap();
}

#[cfg(unix)]
#[test]
fn radar_command_refuses_existing_symlink_output_without_changing_target_permissions() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let root = temp("symlink-output");
    let input = root.with_extension("ast");
    let output = root.with_extension("out");
    let target = root.with_extension("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
    symlink(&target, &output).unwrap();
    fs::write(&input, success_stream()).unwrap();
    let result = radar_command(&input, &output);
    assert!(!result.status.success());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert!(fs::symlink_metadata(&output)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(output).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_file(input).unwrap();
}

#[test]
fn radar_command_refuses_cat034_instead_of_silently_skipping_it() {
    let root = temp("cat034");
    let input = root.with_extension("ast");
    let output = root.with_extension("out");
    fs::write(&input, [34u8, 0, 3]).unwrap();
    let result = radar_command(&input, &output);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("CAT034 is unsupported"));
    assert!(!output.exists());
    fs::remove_file(input).unwrap();
}

#[test]
fn radar_command_refuses_out_of_order_second_rollover_and_mixed_sac_sic() {
    let root = temp("invalid-order");
    let input = root.with_extension("ast");
    let out = root.with_extension("out");
    let source = SourceId { sac: 12, sic: 34 };
    for (name, bytes, expected) in [
        (
            "order",
            synthetic_sensor_stream(&[120, 115], &[], source),
            "time of day regressed",
        ),
        (
            "rollover",
            synthetic_sensor_stream(
                &[86_395, 86_400, 86_400 + 23 * 3600, 2 * 86_400],
                &[],
                source,
            ),
            "single permitted midnight rollover",
        ),
    ] {
        fs::write(&input, bytes).unwrap();
        let output = out.with_extension(name);
        let result = radar_command(&input, &output);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "expected {expected:?}; stderr was {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!output.exists());
    }
    let mut mixed = synthetic_sensor_stream(&[120], &[], source);
    mixed.extend(synthetic_sensor_stream(
        &[125],
        &[],
        SourceId { sac: 13, sic: 34 },
    ));
    fs::write(&input, mixed).unwrap();
    let result = radar_command(&input, &out);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("multiple CAT048 SAC/SIC"));
    assert!(!out.exists());
    fs::remove_file(input).unwrap();
}

#[test]
fn radar_command_distinguishes_missing_flight_level_from_conversion_refusal() {
    let root = temp("no-height");
    let input = root.with_extension("ast");
    let output = root.with_extension("out");
    let secs = [0, 5, 10, 15, 20, 25];
    fs::write(
        &input,
        synthetic_sensor_stream(&secs, &[0, 25], SourceId { sac: 12, sic: 34 }),
    )
    .unwrap();
    let result = radar_command(&input, &output);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let readme = fs::read_to_string(output.join("2026.11.20/README.txt")).unwrap();
    assert!(readme.contains("plots with missing or invalid I048/090 flight level (input condition, separate from conversion refusals): 2"));
    assert!(readme.contains("no-height conversion refusals during anonymous initiation: 1"));
    assert!(readme.contains("no-height conversion refusals during association: 0"));
    let associated = readme
        .lines()
        .find_map(|line| line.strip_prefix("anonymous plot assignments applied: "))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    assert!(
        associated > 0,
        "fixture should exercise conversion using an associated track's estimated altitude"
    );
    let bytes = fs::read(output.join("2026.11.20/radar.ast")).unwrap();
    let mut offset = 0;
    let mut saw_baro_only = false;
    while offset < bytes.len() {
        let (block, used) = decode_block(&bytes[offset..]).unwrap();
        let Block::Cat062(rows) = block else {
            panic!("not CAT062")
        };
        saw_baro_only |= rows.iter().any(|r| {
            r.baro_altitude.as_ref().is_some_and(|baro| !baro.qnh)
                && r.measured_fl.is_none()
                && r.geometric_altitude.is_none()
                && r.track_status
                    .as_ref()
                    .is_some_and(|status| !status.geometric_more_reliable)
        });
        offset += used;
    }
    assert!(
        saw_baro_only,
        "CAT048 flight level must not be emitted as measured FL/geometric altitude or QNH-corrected altitude"
    );
    fs::remove_dir_all(output).unwrap();
    fs::remove_file(input).unwrap();
}
