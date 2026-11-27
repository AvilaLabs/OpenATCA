//! The column-file spill of a tracker run: where it writes, what it removes, and what it reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};

use oa_sha256::{to_hex, Sha256};
use oa_track::spill::{
    spill_append, spill_counts, spill_create, spill_finish, spill_new, spill_rows, SpillMismatch,
    SpilledRun,
};
use oa_track::{log_new, output_hash_into, output_new, TrackerStats, O_HAS_ALT};

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("spill_{name}"));
    let _ = fs::remove_dir_all(&d);
    d
}

/// A finished spill of `n` rows in `dir`.
fn make_run(dir: &Path, n: usize) -> SpilledRun {
    let mut o = output_new(n.max(1));
    for i in 0..n {
        o.t_ns.push(1_000_000_000 * i as i64);
        o.addr.push(0xA0_0001 + (i as u32 % 3));
        o.ident.push(u16::MAX);
        o.ident_ns.push(i64::MIN);
        o.ident_conflict.push(0);
        o.lat_e7.push(419_000_000);
        o.lon_e7.push(-883_000_000);
        o.vel_east_mps.push(1.0);
        o.vel_north_mps.push(2.0);
        o.pos_cov_m2.push([1.0, 0.0, 1.0]);
        o.vel_cov_m2s2.push([1.0, 0.0, 1.0]);
        o.turn_rate_rps.push(0.0);
        o.alt_ft.push(30_000.0);
        o.alt_var_ft2.push(100.0);
        o.vrate_fpm.push(0.0);
        o.mu_h.push([0.9, 0.1]);
        o.mu_v.push([0.8, 0.2]);
        o.flags.push(0);
    }
    let mut w = spill_new(dir).unwrap();
    spill_append(&mut w, &o).unwrap();
    spill_finish(w, 0, TrackerStats::default(), log_new(4), n as u64).unwrap()
}

fn shorten(dir: &Path, col: &str, by: u64) {
    let f = fs::OpenOptions::new()
        .write(true)
        .open(dir.join(col))
        .unwrap();
    let len = f.metadata().unwrap().len();
    f.set_len(len - by).unwrap();
}

// REQ: OA-TRK-079
#[test]
fn a_spill_refuses_a_directory_that_has_anything_in_it() {
    let dir = scratch("nonempty");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("precious.txt"), b"keep").unwrap();
    assert!(spill_new(&dir).is_err());
    assert_eq!(fs::read(dir.join("precious.txt")).unwrap(), b"keep");
    // A column file name already taken is not overwritten either.
    let dir2 = scratch("taken");
    fs::create_dir_all(&dir2).unwrap();
    fs::write(dir2.join("t_ns"), b"mine").unwrap();
    assert!(spill_new(&dir2).is_err());
    assert_eq!(fs::read(dir2.join("t_ns")).unwrap(), b"mine");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&dir2);
}

// REQ: OA-TRK-079
#[test]
fn dropping_a_run_removes_only_the_files_it_made_and_then_its_empty_directory() {
    let dir = scratch("drop");
    let run = make_run(&dir, 5);
    assert!(dir.join("flags").exists());
    // Something else put into the directory while the run lived survives it.
    fs::write(dir.join("notes.txt"), b"mine").unwrap();
    drop(run);
    assert_eq!(fs::read(dir.join("notes.txt")).unwrap(), b"mine");
    assert!(!dir.join("flags").exists() && !dir.join("t_ns").exists());
    let _ = fs::remove_dir_all(&dir);
    // With nothing else in it, the directory goes too.
    let dir = scratch("drop_clean");
    drop(make_run(&dir, 5));
    assert!(!dir.exists());
}

// REQ: OA-TRK-080
#[test]
fn a_truncated_column_is_an_error_whichever_column_it_is() {
    for col in ["flags", "poscov", "t_ns", "muv"] {
        let dir = scratch(&format!("trunc_{col}"));
        let run = make_run(&dir, 20);
        assert_eq!(spill_rows(&run, false, 0, u32::MAX).unwrap().0.len(), 20);
        assert_eq!(spill_counts(&run, false).unwrap().values().sum::<u64>(), 20);
        shorten(&dir, col, 1);
        let e = spill_rows(&run, false, 0, u32::MAX).unwrap_err();
        // The error is typed: it names the column and the bytes found and expected.
        let m = e
            .get_ref()
            .and_then(|x| x.downcast_ref::<SpillMismatch>())
            .unwrap_or_else(|| panic!("{col}: not a SpillMismatch: {e}"));
        assert_eq!(m.column, col);
        assert_eq!(m.found_bytes + 1, m.expected_bytes);
        assert!(spill_counts(&run, false).is_err(), "{col}: counts");
    }
}

// REQ: OA-TRK-086
#[test]
fn rows_without_an_altitude_have_one_digest_in_memory_and_on_disk() {
    // Rows with no altitude hold NaN in the altitude columns; one with and one without a
    // geometric-only altitude (flag clear, a value present) are in the mix too.
    let mut o = output_new(8);
    for i in 0..8usize {
        let has = i % 3 == 0;
        o.t_ns.push(1_000_000_000 * i as i64);
        o.addr.push(0xA0_0001);
        o.ident.push(u16::MAX);
        o.ident_ns.push(i64::MIN);
        o.ident_conflict.push(0);
        o.lat_e7.push(419_000_000);
        o.lon_e7.push(-883_000_000);
        o.vel_east_mps.push(1.0);
        o.vel_north_mps.push(2.0);
        o.pos_cov_m2.push([1.0, 0.0, 1.0]);
        o.vel_cov_m2s2.push([1.0, 0.0, 1.0]);
        o.turn_rate_rps.push(0.0);
        o.alt_ft
            .push(if has || i == 4 { 30_000.0 } else { f32::NAN });
        o.alt_var_ft2.push(if has { 100.0 } else { f32::NAN });
        o.vrate_fpm.push(if has { 64.0 } else { f32::NAN });
        o.mu_h.push([0.9, 0.1]);
        o.mu_v.push([0.8, 0.2]);
        o.flags.push(if has { O_HAS_ALT } else { 0 });
    }
    let mut h = Sha256::new();
    output_hash_into(&mut h, &o);
    let memory = to_hex(&h.finalize());
    let dir = scratch("canon");
    let mut w = spill_new(&dir).unwrap();
    spill_append(&mut w, &o).unwrap();
    let run = spill_finish(w, 0, TrackerStats::default(), log_new(4), 8).unwrap();
    assert_eq!(run.digest, memory);
}

// REQ: OA-TRK-280
#[test]
fn a_writer_that_loses_the_race_for_a_directory_leaves_the_winners_files() {
    let dir = scratch("race");
    // Both passed the emptiness check; the first made its files, the second fails at the first
    // column it makes.
    let a = spill_create(&dir).unwrap();
    assert!(spill_create(&dir).is_err());
    for c in ["t_ns", "addr", "flags", "muv"] {
        assert!(dir.join(c).exists(), "winner's {c} was removed");
    }
    drop(a);
    let _ = fs::remove_dir_all(&dir);
}

// REQ: OA-TRK-280
#[test]
fn a_failed_start_removes_only_the_files_it_made_and_keeps_a_directory_it_did_not() {
    let dir = scratch("fail_existing");
    fs::create_dir_all(&dir).unwrap();
    // A column name taken by a directory makes the creation fail partway.
    fs::create_dir(dir.join("lat")).unwrap();
    assert!(spill_create(&dir).is_err());
    assert!(dir.join("lat").is_dir());
    assert!(!dir.join("t_ns").exists() && !dir.join("addr").exists());
    // An empty directory that was there before the call is still there.
    let dir2 = scratch("fail_empty");
    fs::create_dir_all(&dir2).unwrap();
    fs::write(dir2.join("t_ns"), b"x").unwrap();
    assert!(spill_create(&dir2).is_err());
    assert_eq!(fs::read(dir2.join("t_ns")).unwrap(), b"x");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&dir2);
}
