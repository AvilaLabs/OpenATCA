//! The canonical output row, the per-second digest and the rows file (`oa_track::canon`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::fs;
use std::path::PathBuf;

use common::*;
use oa_sha256::sha256;
use oa_track::canon::*;
use oa_track::canon_spill::*;
use oa_track::spill::{spill_append, spill_finish, spill_new, SpilledRun};
use oa_track::{
    log_new, output_new, tracker_flush, tracker_new, tracker_update, OutRow, TrackOutput,
    TrackerConfig, TrackerStats, O_TICK,
};

fn row(t_ns: i64, addr: u32, k: f32) -> OutRow {
    OutRow {
        t_ns,
        addr,
        ident: u16::MAX,
        ident_ns: i64::MIN,
        ident_conflict: 0,
        lat_e7: 419_000_000 + addr as i32,
        lon_e7: -883_000_000,
        vel_east_mps: k,
        vel_north_mps: k + 1.0,
        pos_cov_m2: [k + 2.0, k + 3.0, k + 4.0],
        vel_cov_m2s2: [k + 5.0, k + 6.0, k + 7.0],
        turn_rate_rps: k + 8.0,
        alt_ft: k + 9.0,
        alt_var_ft2: k + 10.0,
        vrate_fpm: k + 11.0,
        mu_h: [0.9, 0.1],
        mu_v: [0.8, 0.2],
        flags: O_TICK,
    }
}

fn push(o: &mut TrackOutput, r: &OutRow) {
    o.t_ns.push(r.t_ns);
    o.addr.push(r.addr);
    o.ident.push(r.ident);
    o.ident_ns.push(r.ident_ns);
    o.ident_conflict.push(r.ident_conflict);
    o.lat_e7.push(r.lat_e7);
    o.lon_e7.push(r.lon_e7);
    o.vel_east_mps.push(r.vel_east_mps);
    o.vel_north_mps.push(r.vel_north_mps);
    o.pos_cov_m2.push(r.pos_cov_m2);
    o.vel_cov_m2s2.push(r.vel_cov_m2s2);
    o.turn_rate_rps.push(r.turn_rate_rps);
    o.alt_ft.push(r.alt_ft);
    o.alt_var_ft2.push(r.alt_var_ft2);
    o.vrate_fpm.push(r.vrate_fpm);
    o.mu_h.push(r.mu_h);
    o.mu_v.push(r.mu_v);
    o.flags.push(r.flags);
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("canon_{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn spill(dir: &std::path::Path, rows: &[OutRow]) -> SpilledRun {
    let mut o = output_new(rows.len().max(1));
    for r in rows {
        push(&mut o, r);
    }
    let mut w = spill_new(&dir.join("spill")).unwrap();
    spill_append(&mut w, &o).unwrap();
    spill_finish(w, 0, TrackerStats::default(), log_new(4), rows.len() as u64).unwrap()
}

fn digest_of(rows: &[OutRow]) -> Vec<SecondEntry> {
    let mut d = Digester::new();
    for r in rows {
        d.push(&canon_row(r)).unwrap();
    }
    d.finish()
}

// REQ: OA-TRK-360
#[test]
fn a_row_encodes_to_fixed_width_little_endian_fields_at_fixed_offsets() {
    let r = row(1_700_000_000_250_000_000, 0xA0_1234, 5.0);
    let b = canon_row(&r);
    assert_eq!(b.len(), ROW_BYTES);
    assert_eq!(b[OFF_ADDR..OFF_ADDR + 4], 0xA0_1234u32.to_le_bytes());
    assert_eq!(b[OFF_T_NS..OFF_T_NS + 8], r.t_ns.to_le_bytes());
    assert_eq!(b[OFF_LAT..OFF_LAT + 4], r.lat_e7.to_le_bytes());
    assert_eq!(b[OFF_VE..OFF_VE + 4], 5.0f32.to_bits().to_le_bytes());
    assert_eq!(b[OFF_VN..OFF_VN + 4], 6.0f32.to_bits().to_le_bytes());
    assert_eq!(
        b[OFF_POS_COV + 8..OFF_POS_COV + 12],
        9.0f32.to_bits().to_le_bytes()
    );
    assert_eq!(
        b[OFF_VEL_COV..OFF_VEL_COV + 4],
        10.0f32.to_bits().to_le_bytes()
    );
    assert_eq!(b[OFF_VRATE..OFF_VRATE + 4], 16.0f32.to_bits().to_le_bytes());
    assert_eq!(
        b[OFF_MU_V + 4..OFF_MU_V + 8],
        0.2f32.to_bits().to_le_bytes()
    );
    assert_eq!(b[OFF_FLAGS], O_TICK);
    assert_eq!(b[OFF_IDENT..OFF_IDENT + 2], u16::MAX.to_le_bytes());
    assert_eq!(b[OFF_IDENT_NS..OFF_IDENT_NS + 8], i64::MIN.to_le_bytes());
    assert_eq!(b[OFF_IDENT_CONFLICT], 0);
    assert_eq!(canon_row(&r), b, "stable across calls");
    // The fields are packed with nothing between them.
    assert_eq!(OFF_IDENT_CONFLICT + 1, ROW_BYTES);
    assert_eq!(OFF_MU_V + 8, OFF_FLAGS);
}

// REQ: OA-TRK-360
#[test]
fn every_nan_becomes_the_one_fixed_nan_and_row_at_matches_row() {
    let quiet = f32::from_bits(0x7fc0_0000);
    let payload = f32::from_bits(0x7fc0_1234);
    let neg = f32::from_bits(0xffc0_0001);
    let mut r = row(1_000_000_000, 7, 1.0);
    r.alt_ft = payload;
    r.vrate_fpm = neg;
    r.alt_var_ft2 = quiet;
    let b = canon_row(&r);
    assert_eq!(b[OFF_ALT..OFF_ALT + 4], 0x7fc0_0000u32.to_le_bytes());
    assert_eq!(b[OFF_VRATE..OFF_VRATE + 4], 0x7fc0_0000u32.to_le_bytes());
    assert_eq!(
        b[OFF_ALT_VAR..OFF_ALT_VAR + 4],
        0x7fc0_0000u32.to_le_bytes()
    );
    let mut o = output_new(2);
    push(&mut o, &r);
    assert_eq!(canon_row_at(&o, 0), Some(b));
    assert_eq!(canon_row_at(&o, 1), None);
}

// REQ: OA-TRK-361
#[test]
fn rows_group_by_second_and_a_change_to_one_second_changes_that_second_and_the_chain_after() {
    let s = 1_000_000_000i64;
    let base = vec![
        row(10 * s, 1, 1.0),
        row(10 * s + 500_000_000, 2, 1.0),
        row(11 * s, 1, 1.0),
        row(12 * s, 1, 1.0),
    ];
    let a = digest_of(&base);
    assert_eq!(
        a.iter().map(|e| (e.second, e.rows)).collect::<Vec<_>>(),
        vec![(10, 2), (11, 1), (12, 1)]
    );
    assert_eq!(a, digest_of(&base));
    // Second 11 differs: seconds 10 keep their hashes and chain, 11 and 12 change chain,
    // and 12 keeps its own second hash.
    let mut changed = base.clone();
    changed[2].vel_east_mps = 1.5;
    let b = digest_of(&changed);
    assert_eq!(a[0], b[0]);
    assert_ne!(a[1].hash, b[1].hash);
    assert_ne!(a[1].chain, b[1].chain);
    assert_eq!(a[2].hash, b[2].hash);
    assert_ne!(a[2].chain, b[2].chain);
    // Emission order inside a second is part of the digest.
    let mut swapped = base.clone();
    swapped.swap(0, 1);
    assert_ne!(a[0].hash, digest_of(&swapped)[0].hash);
    // A second is its rows hashed back to back; the chain starts from zero bytes.
    let mut bytes = canon_row(&base[0]).to_vec();
    bytes.extend_from_slice(&canon_row(&base[1]));
    assert_eq!(a[0].hash, sha256(&bytes));
    assert_eq!(a[0].chain, chain_next(&[0; 32], 10, &a[0].hash, 2));
    // A time before 1970 floors toward minus infinity.
    assert_eq!(row_second(&canon_row(&row(-1, 1, 0.0))), -1);
}

// REQ: OA-TRK-362
#[test]
fn the_files_of_a_spilled_run_hold_the_digest_and_the_rows_grouped_by_second() {
    let dir = scratch("files");
    let s = 1_000_000_000i64;
    // Emitted with second 11 before a late row of second 10.
    let rows = vec![
        row(10 * s, 1, 1.0),
        row(11 * s, 1, 2.0),
        row(10 * s + 1, 2, 3.0),
        row(12 * s, 3, 4.0),
    ];
    let run = spill(&dir, &rows);
    let (dp, rp) = (dir.join("d.txt"), dir.join("r.bin"));
    let sum = write_canon(&run, Some(&dp), Some(&rp)).unwrap();
    assert_eq!((sum.seconds, sum.rows, sum.in_order), (3, 4, false));
    let text = fs::read_to_string(&dp).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(DIGEST_HEADER));
    let want: Vec<String> = digest_of(&rows).iter().map(second_line).collect();
    let got: Vec<String> = lines.map(str::to_string).collect();
    assert_eq!(got[..got.len() - 1], want[..]);
    assert_eq!(got.last().unwrap(), &digest_end_line(&digest_of(&rows)));
    assert!(got.last().unwrap().starts_with("end 3 4 "));
    let bin = fs::read(&rp).unwrap();
    assert_eq!(bin[..ROWS_HEADER_BYTES], rows_header(4));
    assert_eq!(bin[..8], *ROWS_MAGIC);
    assert_eq!(bin.len(), ROWS_HEADER_BYTES + 4 * ROW_BYTES);
    let got: Vec<&[u8]> = bin[ROWS_HEADER_BYTES..].chunks(ROW_BYTES).collect();
    let order = [0usize, 2, 1, 3];
    for (g, i) in got.iter().zip(order) {
        assert_eq!(*g, canon_row(&rows[i]));
    }
    // In order: the file is the emission order, and the flag says so.
    let ordered = vec![rows[0], rows[2], rows[1], rows[3]];
    let run2 = spill(&scratch("files2"), &ordered);
    let rp2 = dir.join("r2.bin");
    assert!(write_canon(&run2, None, Some(&rp2)).unwrap().in_order);
    assert_eq!(fs::read(&rp2).unwrap(), bin);
}

/// A run of the tracker over a few synthetic aircraft, its spilled output's digest lines.
fn tracker_digest(dir: &std::path::Path) -> (Vec<String>, String, Vec<u8>) {
    let f = frame0();
    let cfg = TrackerConfig {
        out_capacity: 1 << 16,
        tick_ns: 1_000_000_000,
        ..big_config()
    };
    let mut rng = Rng(42);
    let mut ms = Vec::new();
    for k in 0..40 {
        let t = f64::from(k);
        for a in 0..3u32 {
            let tr = Truth {
                e: 200.0 * t + 5_000.0 * f64::from(a),
                n: 3_000.0 * f64::from(a),
                ve: 200.0,
                vn: 0.0,
                alt_ft: 30_000.0,
                vrate_fpm: 0.0,
            };
            ms.push(measure(
                &f,
                0xA0_0000 + a,
                t,
                &tr,
                SIGMA_POS,
                Some(SIGMA_VEL),
                true,
                &mut rng,
            ));
        }
    }
    let mut t = tracker_new(&cfg).unwrap();
    let mut scratch_out = output_new(cfg.out_capacity);
    let mut w = spill_new(&dir.join("spill")).unwrap();
    for m in &ms {
        tracker_update(&mut t, m, &mut scratch_out);
        spill_append(&mut w, &scratch_out).unwrap();
        oa_track::output_clear(&mut scratch_out);
    }
    tracker_flush(&mut t, 41_000_000_000, &mut scratch_out);
    spill_append(&mut w, &scratch_out).unwrap();
    let run = spill_finish(w, 0, TrackerStats::default(), log_new(4), ms.len() as u64).unwrap();
    assert!(run.rows > 100);
    let dp = dir.join("digest.txt");
    write_canon(&run, Some(&dp), Some(&dir.join("rows.bin"))).unwrap();
    let text = fs::read_to_string(&dp).unwrap();
    (
        text.lines().map(str::to_string).collect(),
        run.digest.clone(),
        fs::read(dir.join("rows.bin")).unwrap(),
    )
}

// REQ: OA-TRK-363
#[test]
fn two_tracker_runs_on_the_same_input_write_identical_digests_and_rows() {
    let (da, ha, ra) = tracker_digest(&scratch("det_a"));
    let (db, hb, rb) = tracker_digest(&scratch("det_b"));
    assert!(da.len() > 10);
    assert_eq!(da, db);
    assert_eq!(ha, hb);
    assert_eq!(ra, rb);
}

// REQ: OA-TRK-370
#[test]
fn the_field_tables_tile_the_row_and_hex_round_trips_only_lowercase() {
    // The compile-time assertions in canon.rs tile 0..ROW_BYTES; here the offsets are read back.
    let mut r = row(5_000_000_000, 0x0A_BCDE, 1.0);
    r.flags = 0xA5;
    let b = canon_row(&r);
    assert_eq!(field_at::<OFF_FLAGS, 1>(&b), [0xA5]);
    assert_eq!(
        u32::from_le_bytes(field_at::<OFF_ADDR, 4>(&b)),
        0x0A_BCDE,
        "address"
    );
    assert_eq!(row_second(&b), 5);
    let h = sha256(b"x");
    assert_eq!(from_hex32(&hex32(&h)), Some(h));
    assert_eq!(from_hex32(&hex32(&h).to_uppercase()), None);
    assert_eq!(from_hex32(&hex32(&h)[..63]), None);
    assert_eq!(from_hex32(&format!("{}g", &hex32(&h)[..63])), None);
}

// REQ: OA-TRK-371
#[test]
fn a_second_or_a_run_past_its_limit_is_an_error_and_is_not_counted() {
    let s = 1_000_000_000i64;
    let mut d = Digester::with_limits(2, 3);
    for k in 0..3 {
        d.push(&canon_row(&row(10 * s, k, 1.0))).unwrap();
    }
    let e = d.push(&canon_row(&row(10 * s, 9, 1.0))).unwrap_err();
    assert_eq!(e, LimitError::SecondTooLarge { second: 10 });
    assert!(limit_text(e).contains("second 10"));
    d.push(&canon_row(&row(11 * s, 1, 1.0))).unwrap();
    assert_eq!(
        d.push(&canon_row(&row(12 * s, 1, 1.0))).unwrap_err(),
        LimitError::TooManySeconds
    );
    // The refused rows left no trace: the digest is that of the accepted rows.
    let entries = d.finish();
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.second, e.rows))
            .collect::<Vec<_>>(),
        vec![(10, 3), (11, 1)]
    );
    // The named limits are the ones the readers check.
    assert_eq!((MAX_ROWS_PER_SECOND, MAX_SECONDS), (1 << 20, 1 << 20));
}

// REQ: OA-TRK-372
#[test]
fn a_rows_header_is_checked_in_full_and_hash_rows_equals_the_digester() {
    assert_eq!(parse_rows_header(&rows_header(7)), Ok(7));
    assert_eq!(parse_rows_header_info(&rows_header(7)).unwrap().version, 2);
    let mut v1 = [0u8; ROWS_HEADER_BYTES];
    v1[..8].copy_from_slice(ROWS_MAGIC);
    v1[8..12].copy_from_slice(&1u32.to_le_bytes());
    v1[12..16].copy_from_slice(&85u32.to_le_bytes());
    v1[16..24].copy_from_slice(&7u64.to_le_bytes());
    let old = parse_rows_header_info(&v1).unwrap();
    assert_eq!((old.version, old.row_bytes, old.rows), (1, 85, 7));
    let good = rows_header(7);
    assert!(parse_rows_header(&good[..23])
        .unwrap_err()
        .contains("header"));
    let mut longer = good.to_vec();
    longer.push(0);
    assert!(parse_rows_header(&longer).is_err());
    for (at, what) in [(0usize, "magic"), (8, "version"), (12, "row size")] {
        let mut bad = good;
        bad[at] ^= 0x40;
        assert!(
            parse_rows_header(&bad).unwrap_err().contains(what),
            "{what}"
        );
    }
    let rows: Vec<CanonRow> = (0..5)
        .map(|k| canon_row(&row(3_000_000_000, k, 2.0)))
        .collect();
    let mut d = Digester::new();
    for r in &rows {
        d.push(r).unwrap();
    }
    assert_eq!(d.finish()[0].hash, hash_rows(&rows).unwrap());
    assert_ne!(hash_rows(&rows[1..]).unwrap(), hash_rows(&rows).unwrap());
}

// REQ: OA-RDF-015
#[test]
fn spill_columns_holding_raw_nan_bits_come_out_as_the_canonical_nan() {
    // A base that spilled `x.to_bits()` leaves any NaN in the columns; the canon code that reads
    // them back is what makes the rows canonical.
    let dir = scratch("raw_nan");
    let rows = [row(1_700_000_000_000_000_000, 1, 1.0)];
    let run = spill(&dir, &rows);
    let raw = |bits: &[u32]| -> Vec<u8> { bits.iter().flat_map(|b| b.to_le_bytes()).collect() };
    fs::write(run.dir.join("ve"), raw(&[0x7fc0_0001])).unwrap();
    fs::write(run.dir.join("vn"), raw(&[0xffc0_0000])).unwrap();
    fs::write(
        run.dir.join("poscov"),
        raw(&[0x7f80_0001, 0x3f80_0000, 0xffff_ffff]),
    )
    .unwrap();
    let mut got = Vec::new();
    spill_canon_rows(&run, &mut |r| {
        got.push(*r);
        Ok(())
    })
    .unwrap();
    let b = got[0];
    let nan = 0x7fc0_0000u32.to_le_bytes();
    assert_eq!(b[OFF_VE..OFF_VE + 4], nan);
    assert_eq!(b[OFF_VN..OFF_VN + 4], nan);
    assert_eq!(b[OFF_POS_COV..OFF_POS_COV + 4], nan);
    assert_eq!(b[OFF_POS_COV + 4..OFF_POS_COV + 8], 1.0f32.to_le_bytes());
    assert_eq!(b[OFF_POS_COV + 8..OFF_POS_COV + 12], nan);
    // Non-float fields are untouched.
    assert_eq!(b[OFF_ADDR..OFF_ADDR + 4], 1u32.to_le_bytes());
    assert_eq!(b[OFF_FLAGS], O_TICK);
}
