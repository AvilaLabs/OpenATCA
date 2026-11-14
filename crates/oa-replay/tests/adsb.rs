//! Tests of the ADS-B feed: the aircraft table, its bounds and hash, malformed payloads, the merge
//! order with ADS-B present, and a mixed SFDPS, TBFM and ADS-B replay built from hand-made
//! fixtures. Every address, time and position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_adsb::{
    encode_payload, frame_ranges, reports_len, reports_push, Reports, Row, F_GROUND, NONE_I32,
    NONE_U16, NONE_U8, NON_ICAO, PAYLOAD_MAGIC, RECORD_LEN, ROW_EMPTY,
};
use oa_feed::Kind;
use oa_replay::{
    adsb_slot_of, apply_adsb, errors_total, merge_key, merge_streams, new_adsb_table, new_pipeline,
    process_frame, replay_hash, run_streams, snapshot, spec_from_bytes, state_hash, AdsbApplied,
    ErrKind, FeedKind, Pipeline, PipelineConfig, ADSB_CAPACITY, ERR_NAMES, FEED_COUNT, FEED_NAMES,
};

const T0: i64 = DAY * 1_000_000 + 3_500_000_000;

fn row(icao: u32, t_ns: i64) -> Row {
    Row {
        icao,
        time_ns: t_ns,
        lat_e7: 420_000_000 + (icao as i32 & 0xffff) * 100,
        lon_e7: -880_000_000,
        baro_alt_ft: 35_000,
        gs_dkt: 4500,
        track_ddeg: 900,
        flags: 0,
        source: 0,
        ..ROW_EMPTY
    }
}

fn reports(rows: &[Row]) -> Reports {
    let mut r = Reports::default();
    for x in rows {
        reports_push(&mut r, x);
    }
    r
}

/// The payload of `rows` (one frame's worth).
fn payload(rows: &[Row]) -> Vec<u8> {
    let r = reports(rows);
    let mut out = Vec::new();
    encode_payload(&mut out, &r, 0, reports_len(&r)).unwrap();
    out
}

fn apply_all(t: &mut oa_replay::AdsbTable, rows: &[Row]) -> Vec<AdsbApplied> {
    let r = reports(rows);
    (0..reports_len(&r)).map(|i| apply_adsb(t, &r, i)).collect()
}

// REQ: OA-RPL-030
#[test]
fn the_table_keeps_the_latest_report_and_carries_object_fields_forward() {
    let mut t = new_adsb_table(16);
    let mut a = row(0xA0_0001, T0);
    a.callsign = *b"UAL12\0\0\0";
    a.squawk = 1200;
    a.category = 0xA3;
    a.nic = 8;
    a.nac_p = 9;
    a.sil_type = 2;
    assert_eq!(apply_all(&mut t, &[a]), [AdsbApplied::New]);
    // A later report without the aircraft object: position replaces, object fields stay.
    let mut b = row(0xA0_0001, T0 + NS);
    b.lat_e7 = 421_000_000;
    b.baro_alt_ft = NONE_I32;
    b.flags = F_GROUND;
    b.gs_dkt = NONE_U16;
    b.nac_p = 10;
    assert_eq!(apply_all(&mut t, &[b]), [AdsbApplied::Updated]);
    let s = adsb_slot_of(&t, 0xA0_0001).unwrap();
    assert_eq!(t.live, 1);
    assert_eq!(t.time_ns[s], T0 + NS);
    assert_eq!(t.lat_e7[s], 421_000_000);
    assert_eq!(
        t.baro_alt_ft[s], NONE_I32,
        "the report's own altitude, absent"
    );
    assert_eq!(t.gs_dkt[s], NONE_U16);
    assert_eq!(t.track_ddeg[s], 900);
    assert_eq!(t.flags[s], F_GROUND);
    assert_eq!(&t.callsign[s], b"UAL12\0\0\0");
    assert_eq!((t.squawk[s], t.category[s]), (1200, 0xA3));
    // nic, nac_p, nac_v, sil, sil_type, version, gva, sda
    assert_eq!(
        t.quality[s],
        [8, 10, NONE_U8, NONE_U8, 2, NONE_U8, NONE_U8, NONE_U8]
    );
    assert_eq!((t.reports[s], t.out_of_order[s]), (2, 0));
    // The same address with the non-ICAO bit is another aircraft.
    assert_eq!(
        apply_all(&mut t, &[row(0xA0_0001 | NON_ICAO, T0)]),
        [AdsbApplied::New]
    );
    assert_eq!(t.live, 2);
    assert_eq!(t.stats.aircraft, 2);
    assert_eq!(t.stats.applied, 3);
}

// REQ: OA-RPL-031
#[test]
fn the_table_is_bounded_refuses_when_full_and_never_reallocates() {
    assert_eq!(ADSB_CAPACITY, 65_536);
    let mut t = new_adsb_table(4);
    let before = (
        t.key.as_ptr() as usize,
        t.key.capacity(),
        t.quality.as_ptr() as usize,
        t.quality.capacity(),
    );
    let rows: Vec<Row> = (1..=6u32).map(|n| row(n, T0 + i64::from(n))).collect();
    let got = apply_all(&mut t, &rows);
    assert_eq!(&got[..4], &[AdsbApplied::New; 4]);
    assert_eq!(&got[4..], &[AdsbApplied::TableFull; 2]);
    assert_eq!((t.live, t.stats.refused_full), (4, 2));
    assert!(adsb_slot_of(&t, 5).is_none());
    // Known aircraft keep updating when the table is full; a refused one is refused again.
    assert_eq!(
        apply_all(&mut t, &[row(2, T0 + 99), row(5, T0 + 99)]),
        [AdsbApplied::Updated, AdsbApplied::TableFull]
    );
    assert_eq!(t.stats.refused_full, 3);
    let after = (
        t.key.as_ptr() as usize,
        t.key.capacity(),
        t.quality.as_ptr() as usize,
        t.quality.capacity(),
    );
    assert_eq!(before, after);
    assert_eq!((t.key.len(), t.time_ns.len()), (4, 4));
    // Through the pipeline the refusals are counted per feed.
    let mut p = small_pipeline(8);
    p.adsb = new_adsb_table(2);
    let rows: Vec<Row> = (1..=3u32).map(|n| row(n, T0)).collect();
    process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &payload(&rows));
    let f = FeedKind::Adsb as usize;
    assert_eq!(
        (p.rows_seen[f], p.rows_applied[f], p.rows_refused_full[f]),
        (3, 2, 1)
    );
}

// REQ: OA-RPL-032
#[test]
fn a_report_older_than_the_held_one_is_refused_whole_and_counted() {
    let mut t = new_adsb_table(8);
    let mut new = row(7, T0 + 10 * NS);
    new.callsign = *b"NEW\0\0\0\0\0";
    apply_all(&mut t, &[new]);
    let mut old = row(7, T0 + 5 * NS);
    old.callsign = *b"OLD\0\0\0\0\0";
    old.lat_e7 = 1;
    assert_eq!(apply_all(&mut t, &[old]), [AdsbApplied::OutOfOrder]);
    let s = adsb_slot_of(&t, 7).unwrap();
    assert_eq!(&t.callsign[s], b"NEW\0\0\0\0\0");
    assert_ne!(t.lat_e7[s], 1);
    assert_eq!(
        (t.reports[s], t.out_of_order[s], t.stats.out_of_order),
        (1, 1, 1)
    );
    // An equal time applies.
    let mut same = row(7, T0 + 10 * NS);
    same.lat_e7 = 2;
    assert_eq!(apply_all(&mut t, &[same]), [AdsbApplied::Updated]);
    assert_eq!(t.lat_e7[s], 2);
    // Through the pipeline: counted per feed, and not an applied row.
    let mut p = small_pipeline(8);
    let rows = [row(9, T0 + 3 * NS), row(9, T0 + NS)];
    process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &payload(&rows));
    let f = FeedKind::Adsb as usize;
    assert_eq!((p.rows_applied[f], p.rows_out_of_order[f]), (1, 1));
}

fn run_frames(p: &mut Pipeline, frames: &[(FeedKind, i64, Vec<u8>)]) {
    for (f, t, b) in frames {
        process_frame(p, *f, Kind::Message, *t, b);
    }
}

// REQ: OA-RPL-033
#[test]
fn the_adsb_table_is_in_the_state_hash_in_address_order() {
    let rows: Vec<Row> = (1..=20u32)
        .map(|n| row(0x400000 + n * 37, T0 + i64::from(n) * NS))
        .collect();
    let hash_of = |order: &[usize]| {
        let mut p = small_pipeline(8);
        let frames: Vec<_> = order
            .iter()
            .map(|&i| (FeedKind::Adsb, T0, payload(&[rows[i]])))
            .collect();
        run_frames(&mut p, &frames);
        replay_hash(&p)
    };
    let fwd: Vec<usize> = (0..20).collect();
    let mut rev = fwd.clone();
    rev.reverse();
    // First-seen order does not matter: aircraft hash in address order.
    assert_eq!(hash_of(&fwd), hash_of(&rev));
    assert_eq!(hash_of(&fwd), hash_of(&fwd));
    // Any column or count changes it.
    let base = hash_of(&fwd);
    let mut p = small_pipeline(8);
    let mut tweaked = rows.clone();
    tweaked[7].track_ddeg += 1;
    for r in &tweaked {
        process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &payload(&[*r]));
    }
    assert_ne!(replay_hash(&p), base);
    let mut p = small_pipeline(8);
    for r in &rows {
        process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &payload(&[*r]));
    }
    assert_eq!(replay_hash(&p), base);
    process_frame(
        &mut p,
        FeedKind::Adsb,
        Kind::Message,
        T0,
        &payload(&[rows[3]]),
    );
    assert_ne!(replay_hash(&p), base, "a repeat report changes the counts");
    // The ADS-B table is in the hash: it differs from the flight table's alone, and checkpoints
    // and the final snapshot use it.
    assert_ne!(replay_hash(&p), state_hash(&p.table));
    assert_eq!(snapshot(&p).hash, replay_hash(&p));
}

// REQ: OA-RPL-034
#[test]
fn replays_without_adsb_hash_exactly_as_before() {
    // Without an ADS-B frame the replay hash is the flight table's own hash, so every checkpoint
    // and final hash of an SWIM-only replay is unchanged (the table golden of OA-RPL-026 pins
    // state_hash itself).
    let mut p = small_pipeline(64);
    let mut frames = Vec::new();
    for k in 0..30i64 {
        let n = (k % 5) as u32 + 1;
        let x = sf_doc(&[sf(n, k * 1000)
            .plan(&format!("KA{n:06}AB"))
            .call("TST1")
            .pos(30.0, -80.0, k * 1000)]);
        frames.push((FeedKind::Sfdps, T0 + k * 1_500_000_000, x.into_bytes()));
        let y = tb_doc(
            k * 1000,
            "ZTT",
            &[tb(k * 1000, Some(&format!("KA{n:06}AB")), "T00001").mrp("MFXA1", "09L", "NEW")],
        );
        frames.push((FeedKind::Tbfm, T0 + k * 1_700_000_000, y.into_bytes()));
    }
    frames.sort_by_key(|f| f.1);
    run_frames(&mut p, &frames);
    assert!(p.checkpoints.len() > 3);
    assert_eq!(p.adsb.live, 0);
    assert_eq!(snapshot(&p).hash, state_hash(&p.table));
    assert_eq!(replay_hash(&p), state_hash(&p.table));
    // The checkpoint hashes equal a replay in which the pipeline knows nothing of ADS-B: replay
    // again and compare with state_hash taken after each frame.
    let mut q = small_pipeline(64);
    let mut want = Vec::new();
    let mut next = i64::MIN;
    for (f, t, b) in &frames {
        if next == i64::MIN {
            next = (t.div_euclid(10 * NS) + 1) * 10 * NS;
        } else if *t >= next {
            want.push(state_hash(&q.table));
            next = (t.div_euclid(10 * NS) + 1) * 10 * NS;
        }
        process_frame(&mut q, *f, Kind::Message, *t, b);
    }
    let got: Vec<_> = q.checkpoints.iter().map(|c| c.hash).collect();
    assert_eq!(got, want);
    // And an ADS-B stream that carries no frame at all changes nothing.
    let a = run_chains_hash(vec![
        (FeedKind::Sfdps, vec![sfdps_bytes()]),
        (FeedKind::Adsb, vec![Vec::new()]),
    ]);
    let b = run_chains_hash(vec![(FeedKind::Sfdps, vec![sfdps_bytes()])]);
    assert_eq!(a, b);
}

fn sfdps_bytes() -> Vec<u8> {
    let mut v = Vec::new();
    for k in 0..12i64 {
        let x = sf_doc(&[sf(k as u32 % 4 + 1, k * 1000)
            .plan(&format!("KA{:06}AB", k % 4 + 1))
            .pos(30.0, -80.0, k * 1000)]);
        v.extend(frame(1, k as u64, T0 + k * 3 * NS, x.as_bytes()));
    }
    v
}

fn run_chains_hash(
    streams: Vec<(FeedKind, Vec<Vec<u8>>)>,
) -> (Vec<oa_sha256::Digest>, oa_sha256::Digest) {
    let specs = streams
        .into_iter()
        .map(|(f, files)| spec_from_bytes(f, files))
        .collect();
    let mut p = small_pipeline(64);
    run_streams(&mut p, specs, 4096, &mut |_| {}).unwrap();
    (
        p.checkpoints.iter().map(|c| c.hash).collect(),
        snapshot(&p).hash,
    )
}

// REQ: OA-RPL-035
#[test]
fn malformed_adsb_payloads_are_counted_by_type_and_never_applied() {
    let good = payload(&[row(1, T0), row(2, T0)]);
    let mut cases: Vec<(Vec<u8>, ErrKind)> = Vec::new();
    cases.push((good[..good.len() - 1].to_vec(), ErrKind::AdsbLength));
    cases.push((b"ADS".to_vec(), ErrKind::AdsbLength));
    let mut m = good.clone();
    m[0] = b'X';
    cases.push((m, ErrKind::AdsbMagic));
    let mut m = good.clone();
    m[4] = 63;
    cases.push((m, ErrKind::AdsbLayout));
    let mut m = good.clone();
    m[12] = 1;
    cases.push((m, ErrKind::AdsbLayout));
    let mut m = good.clone();
    m[8..12].copy_from_slice(&60_001u32.to_le_bytes());
    cases.push((m, ErrKind::AdsbTooManyRows));
    // Record 1: latitude out of range; flag bits 5 to 7; a reserved byte; an unknown source.
    for (at, v) in [(4usize, 0x7fu8), (38, 0x20), (57, 1), (39, 14)] {
        let mut m = good.clone();
        let o = 16 + RECORD_LEN + at;
        m[o] = v;
        if at == 4 {
            m[o + 3] = 0x7f;
        }
        cases.push((m, ErrKind::AdsbBadRecord));
    }
    assert_eq!(&good[..4], &PAYLOAD_MAGIC);
    for (bytes, kind) in cases {
        let mut p = small_pipeline(8);
        process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &bytes);
        assert_eq!(errors_total(&p.errors), 1, "{kind:?}");
        assert_eq!(p.errors[kind as usize], 1, "{}", ERR_NAMES[kind as usize]);
        assert_eq!((p.adsb.live, p.adsb.stats.applied), (0, 0), "{kind:?}");
        assert_eq!(p.rows_seen[FeedKind::Adsb as usize], 0);
        assert_eq!(p.messages[FeedKind::Adsb as usize], 1);
    }
    // Garbage and every single-byte corruption of a payload: never a panic, never a partly
    // applied payload (the table holds either both records or none).
    for i in 0..good.len() {
        let mut m = good.clone();
        m[i] ^= 0xff;
        let mut p = small_pipeline(8);
        process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &m);
        assert!(
            p.adsb.live == 0 || p.adsb.live == 2 || p.adsb.live == 1,
            "{i}"
        );
        if errors_total(&p.errors) > 0 {
            assert_eq!(p.adsb.live, 0, "byte {i}");
        }
    }
    let mut p = small_pipeline(8);
    let mut rng = Rng(5);
    for _ in 0..50 {
        let n = rng.below(300);
        let junk: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        process_frame(&mut p, FeedKind::Adsb, Kind::Message, T0, &junk);
    }
    assert_eq!(p.adsb.live, 0);
    // STATUS and REJECT frames carry no reports.
    process_frame(&mut p, FeedKind::Adsb, Kind::Status, T0, &good);
    assert_eq!(p.adsb.live, 0);
    assert_eq!(p.other_frames[FeedKind::Adsb as usize], 1);
}

// REQ: OA-RPL-036
#[test]
fn adsb_sorts_after_every_swim_feed_at_an_equal_receive_time() {
    assert_eq!(FeedKind::Adsb as usize, FEED_COUNT - 1);
    assert_eq!(FEED_NAMES[FeedKind::Adsb as usize], "adsb");
    let kinds = [
        FeedKind::Sfdps,
        FeedKind::Tbfm,
        FeedKind::Tfms,
        FeedKind::Fns,
        FeedKind::Adsb,
    ];
    assert!(kinds.windows(2).all(|w| w[0] < w[1]));
    assert!(merge_key(5, FeedKind::Fns, 9, 9, 9) < merge_key(5, FeedKind::Adsb, 0, 0, 0));
    // One frame per feed at the same instant, streams named in every rotation: the same order.
    let one = |feed: FeedKind| spec_from_bytes(feed, vec![frame(1, 0, T0, b"x")]);
    for rot in 0..kinds.len() {
        let mut order = kinds.to_vec();
        order.rotate_left(rot);
        let mut got = Vec::new();
        merge_streams(order.iter().map(|&k| one(k)).collect(), 64, &mut |m| {
            got.push(m.feed)
        })
        .unwrap();
        assert_eq!(got, kinds, "rotation {rot}");
    }
    // Random streams with many equal times, ADS-B among them: exactly the global sort.
    for seed in 1..=10u64 {
        let mut rng = Rng(seed * 104_729);
        let feeds = [
            FeedKind::Adsb,
            FeedKind::Sfdps,
            FeedKind::Adsb,
            FeedKind::Tbfm,
        ];
        let mut files = Vec::new();
        for _ in 0..feeds.len() {
            let (mut t, mut v) = (0i64, Vec::new());
            for seq in 0..(10 + rng.below(20)) as u64 {
                t += rng.below(3) as i64;
                v.extend(frame(1, seq, t, b"x"));
            }
            files.push(v);
        }
        let specs = files
            .iter()
            .zip(feeds)
            .map(|(b, f)| spec_from_bytes(f, vec![b.clone()]))
            .collect();
        let mut got = Vec::new();
        merge_streams(specs, 23, &mut |m| {
            got.push(merge_key(m.recv_ns, m.feed, m.seq, m.stream, m.offset))
        })
        .unwrap();
        let mut want = got.clone();
        want.sort_unstable();
        assert_eq!(got, want, "seed {seed}");
    }
}

/// Hand-made recordings: SFDPS and TBFM flights over Illinois and ADS-B reports of five
/// aircraft, some at exactly the receive time of an SWIM frame.
fn mixed() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut s, mut t, mut a) = (Vec::new(), Vec::new(), Vec::new());
    for k in 0..40i64 {
        let n = (k % 4) as u32 + 1;
        let x = sf_doc(&[sf(n, k * 1000)
            .plan(&format!("KA{n:06}AB"))
            .call(&format!("TST{n}"))
            .pos(41.0 + f64::from(n) / 10.0, -88.0, k * 1000)]);
        s.extend(frame(1, k as u64, T0 + k * 2 * NS, x.as_bytes()));
        let y = tb_doc(
            k * 1000,
            "ZTT",
            &[
                tb(k * 1000, Some(&format!("KA{n:06}AB")), &format!("T{n:05}"))
                    .mrp("MFXA1", "09L", "NEW")
                    .eta("MFXA1", 600_000),
            ],
        );
        t.extend(frame(1, k as u64, T0 + k * 3 * NS, y.as_bytes()));
    }
    // Two reports a second per aircraft over 40 s; two aircraft per frame; frame times in step.
    for k in 0..40i64 {
        let mut rows = Vec::new();
        for j in 0..5u32 {
            let mut r = row(0xA1_0000 + j, T0 + k * 2 * NS + i64::from(j));
            r.lat_e7 += (k as i32) * 1000;
            if k % 8 == 0 {
                r.callsign = *b"ADS\0\0\0\0\0";
                r.nac_p = 9;
            }
            rows.push(r);
        }
        a.extend(frame(1, k as u64, rows[0].time_ns, &payload(&rows)));
    }
    // One damaged payload and one STATUS frame.
    a.extend(frame(1, 40, T0 + 90 * NS, b"ADS1 but not really"));
    a.extend(frame(3, 41, T0 + 91 * NS, b"state=connected"));
    (s, t, a)
}

fn run_mixed(order: &[usize], chunk: usize) -> (Pipeline, u64) {
    let (s, t, a) = mixed();
    let all = [
        (FeedKind::Sfdps, s),
        (FeedKind::Tbfm, t),
        (FeedKind::Adsb, a),
    ];
    let specs = order
        .iter()
        .map(|&i| spec_from_bytes(all[i].0, vec![all[i].1.clone()]))
        .collect();
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let r = run_streams(&mut p, specs, chunk, &mut |_| {}).unwrap();
    (p, r.bytes)
}

// REQ: OA-RPL-037
#[test]
fn a_mixed_swim_and_adsb_replay_is_deterministic_in_any_order_and_chunking() {
    let (p, bytes) = run_mixed(&[0, 1, 2], 64 * 1024);
    let f = FeedKind::Adsb as usize;
    assert_eq!(p.frames[f], 42);
    assert_eq!(p.messages[f], 41);
    assert_eq!(p.other_frames[f], 1);
    assert_eq!(p.rows_seen[f], 200);
    assert_eq!(p.rows_applied[f], 200);
    assert_eq!(p.adsb.live, 5);
    assert_eq!(p.errors[ErrKind::AdsbLayout as usize], 1);
    assert_eq!(errors_total(&p.errors), 1);
    assert_eq!(p.frames[0], 40);
    assert_eq!(p.frames[1], 40);
    assert_eq!(p.table.live, 4);
    assert!(p.checkpoints.len() > 5);
    let s = adsb_slot_of(&p.adsb, 0xA1_0003).unwrap();
    assert_eq!(p.adsb.reports[s], 40);
    assert_eq!(&p.adsb.callsign[s][..3], b"ADS");
    assert_eq!(
        p.adsb.lat_e7[s],
        420_000_000 + 0xA1_0003 % 0x10000 * 100 + 39_000
    );
    let want = snapshot(&p);
    for (order, chunk) in [
        ([2usize, 1, 0], 64 * 1024),
        ([1, 2, 0], 1),
        ([0, 2, 1], 37),
        ([0, 1, 2], 4097),
    ] {
        let (q, b2) = run_mixed(&order, chunk);
        assert_eq!(b2, bytes);
        assert_eq!(snapshot(&q), want, "{order:?} {chunk}");
        assert_eq!(q.checkpoints, p.checkpoints);
        assert_eq!(q.errors, p.errors);
        assert_eq!(q.rows_applied, p.rows_applied);
    }
    // Dropping the ADS-B stream gives the SWIM-only state: the flight table is the same either
    // way, ADS-B only adds to the hash.
    let (s, t, _) = mixed();
    let mut q = new_pipeline(PipelineConfig::default()).unwrap();
    run_streams(
        &mut q,
        vec![
            spec_from_bytes(FeedKind::Sfdps, vec![s]),
            spec_from_bytes(FeedKind::Tbfm, vec![t]),
        ],
        4096,
        &mut |_| {},
    )
    .unwrap();
    // Pinned from the code before ADS-B existed: the same two recordings gave this hash then.
    assert_eq!(
        oa_sha256::to_hex(&snapshot(&q).hash),
        "fa8d94b079bf7f4c69aae2db5b1077fddfa633838f7bd3c363bdc3b968ac459e"
    );
    assert_ne!(snapshot(&q).hash, want.hash);
    assert_eq!(q.table.live, p.table.live);
}

// REQ: OA-RPL-040
#[test]
fn a_one_second_frame_adsb_clip_interleaves_with_a_swim_feed_at_one_second() {
    // 30 s of ADS-B at 10 reports a second, framed as `oa-adsb-clip` does by default, and a
    // TFMS frame every 3 s.
    let rows: Vec<Row> = (0..300i64)
        .map(|i| row(0xA2_0000 + (i % 3) as u32, T0 + i * NS / 10))
        .collect();
    let r = reports(&rows);
    let ranges = frame_ranges(&r, 1000);
    assert_eq!(ranges.len(), 30);
    let mut a = Vec::new();
    for (seq, &(from, to)) in ranges.iter().enumerate() {
        let mut p = Vec::new();
        encode_payload(&mut p, &r, from, to).unwrap();
        a.extend(frame(1, seq as u64, rows[from].time_ns, &p));
    }
    let mut t = Vec::new();
    for k in 0..10i64 {
        t.extend(frame(1, k as u64, T0 + k * 3 * NS + NS / 2, b"x"));
    }
    let mut got: Vec<(FeedKind, i64)> = Vec::new();
    merge_streams(
        vec![
            spec_from_bytes(FeedKind::Adsb, vec![a]),
            spec_from_bytes(FeedKind::Tfms, vec![t]),
        ],
        4096,
        &mut |m| got.push((m.feed, m.recv_ns)),
    )
    .unwrap();
    assert_eq!(got.len(), 40);
    assert!(got.windows(2).all(|w| w[0].1 <= w[1].1));
    // Three ADS-B frames sit between neighbouring TFMS frames, and no ADS-B frame is more than
    // a second after the previous one.
    let adsb: Vec<i64> = got
        .iter()
        .filter(|g| g.0 == FeedKind::Adsb)
        .map(|g| g.1)
        .collect();
    assert!(adsb.windows(2).all(|w| w[1] - w[0] <= NS));
    let mut run = 0;
    let mut runs = Vec::new();
    for g in &got {
        if g.0 == FeedKind::Adsb {
            run += 1;
        } else {
            runs.push(run);
            run = 0;
        }
    }
    assert!(runs.iter().all(|&n| n <= 3), "{runs:?}");
    assert_eq!(runs.iter().sum::<i32>() + run, 30);
}
