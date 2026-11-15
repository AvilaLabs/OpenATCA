//! The digest of an input file describes the bytes that were ingested: it comes from the same read.
//! A file that changes after it was listed, or while it is read, is simulated by the opener (the
//! seam of `load_with` and `load_adsb_with`); no real recording is used. Every identifier is made
//! up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::cell::RefCell;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use oa_adsb::{encode_payload, reports_len, reports_push, Reports, ROW_EMPTY};
use oa_bench::input::{hash_files, load_adsb_with, load_with, FeedInput, Opener};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_replay::FeedKind;
use oa_sha256::{sha256, to_hex};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-bench-digest-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn th_msg(i: usize) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="2030-01-02T03:00:05.250Z"><enRoute xsi:type="ns5:NasEnRouteType"><position xsi:type="ns5:NasAircraftPositionType" positionTime="2030-01-02T03:00:04Z"><position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>36.5 -95.25</pos></location></position></position></enRoute><gufi codeSpace="urn:uuid">{i:08x}-2222-4333-8444-555555555555</gufi></flight></message>"#
    )
}

fn sfdps(n: usize) -> String {
    let msgs: String = (0..n).map(th_msg).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">{msgs}</ns5:MessageCollection>"#
    )
}

fn oaf(docs: &[String]) -> Vec<u8> {
    let mut file = Vec::new();
    for (seq, d) in docs.iter().enumerate() {
        let mut buf = vec![0u8; d.len() + 64];
        let n = encode_frame(
            &mut buf,
            1,
            0,
            seq as u64,
            1_893_553_265_000_000_000 + seq as i64 * 1_000_000_000,
            0,
            d.as_bytes(),
        )
        .unwrap();
        file.extend_from_slice(&buf[..n]);
    }
    file
}

/// An opener that hands back `bytes` for every path, whatever the disk holds, and records what it
/// handed out.
fn opener_of(bytes: Vec<u8>) -> Opener {
    Rc::new(move |_: &Path| Ok(Box::new(io::Cursor::new(bytes.clone())) as Box<dyn Read>))
}

/// A reader over `first`'s bytes up to `at`, then `second`'s from there: a file that changed while
/// it was being read. What it delivered goes to `log`.
struct Changing {
    first: Vec<u8>,
    second: Vec<u8>,
    at: usize,
    pos: usize,
    log: Rc<RefCell<Vec<u8>>>,
}

impl Read for Changing {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let src = if self.pos < self.at {
            &self.first[..self.at]
        } else {
            &self.second[..]
        };
        let n = out.len().min(src.len().saturating_sub(self.pos)).min(1000);
        out[..n].copy_from_slice(&src[self.pos..self.pos + n]);
        self.pos += n;
        self.log.borrow_mut().extend_from_slice(&out[..n]);
        Ok(n)
    }
}

fn changing_opener(
    first: Vec<u8>,
    second: Vec<u8>,
    at: usize,
    log: Rc<RefCell<Vec<u8>>>,
) -> Opener {
    Rc::new(move |_: &Path| {
        Ok(Box::new(Changing {
            first: first.clone(),
            second: second.clone(),
            at,
            pos: 0,
            log: log.clone(),
        }) as Box<dyn Read>)
    })
}

// REQ: OA-BENCH-058
#[test]
fn a_msg_file_that_changed_after_listing_is_hashed_as_ingested() {
    let dir = scratch("msg");
    let on_disk = sfdps(1);
    let read = sfdps(3);
    std::fs::write(dir.join("000000.msg"), &on_disk).unwrap();
    let l = load_with(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: dir.clone(),
        }],
        &opener_of(read.clone().into_bytes()),
    )
    .unwrap();
    assert_eq!(l.legacy.th.flight.len(), 3, "the changed bytes were scored");
    assert_eq!(l.inputs[0].sha256, to_hex(&sha256(read.as_bytes())));
    assert_eq!(l.inputs[0].bytes, read.len() as u64);
    // A read of its own (the old two-step) would have described the other bytes.
    let own = hash_files("x", &[dir.join("000000.msg")]).unwrap();
    assert_eq!(own.sha256, to_hex(&sha256(on_disk.as_bytes())));
    assert_ne!(own.sha256, l.inputs[0].sha256);
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-BENCH-058
#[test]
fn a_recording_that_changed_after_listing_is_hashed_as_replayed() {
    let dir = scratch("oaf");
    let on_disk = oaf(&[sfdps(1)]);
    let read = oaf(&[sfdps(1), sfdps(2)]);
    std::fs::write(dir.join("sfdps-0001.oaf"), &on_disk).unwrap();
    let l = load_with(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: dir.clone(),
        }],
        &opener_of(read.clone()),
    )
    .unwrap();
    assert_eq!(l.legacy.th.flight.len(), 3, "the changed bytes were scored");
    assert_eq!(l.inputs[0].sha256, to_hex(&sha256(&read)));
    assert_eq!(l.inputs[0].bytes, read.len() as u64);
    assert_ne!(
        hash_files("x", &[dir.join("sfdps-0001.oaf")])
            .unwrap()
            .sha256,
        l.inputs[0].sha256
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn clip_bytes(n: usize, shift: i32) -> Vec<u8> {
    let mut r = Reports::default();
    for k in 0..n {
        let mut row = ROW_EMPTY;
        row.icao = 0xA1B2C3;
        row.time_ns = 1_700_000_000_000_000_000 + k as i64 * 1_000_000_000;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000 + k as i32 * 100 + shift;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        row.baro_alt_ft = 30_000;
        reports_push(&mut r, &row);
    }
    let mut out = Vec::new();
    let mut at = 0;
    let mut seq = 0u64;
    while at < n {
        let end = (at + 50).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, &r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        out.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    out
}

// REQ: OA-BENCH-058
#[test]
fn an_adsb_clip_that_changes_while_it_is_read_is_hashed_as_read() {
    let first = clip_bytes(100, 0);
    let second = clip_bytes(200, 5_000);
    let log = Rc::new(RefCell::new(Vec::new()));
    // Changes after the first frame (50 reports): the reader then reads the other copy.
    let at = first.len() / 2;
    let open = changing_opener(first.clone(), second.clone(), at, log.clone());
    let clip = load_adsb_with(Path::new("clip.oaf"), &open);
    let delivered = log.borrow().clone();
    // Whatever was delivered, the digest is of exactly that, and neither copy's.
    let clip = clip.unwrap();
    assert_eq!(clip.hash.sha256, to_hex(&sha256(&delivered)));
    assert_eq!(clip.hash.bytes, delivered.len() as u64);
    assert_ne!(clip.hash.sha256, to_hex(&sha256(&first)));
    assert_ne!(clip.hash.sha256, to_hex(&sha256(&second)));
    assert!(reports_len(&clip.reports) > 0);
}

// REQ: OA-BENCH-058
#[test]
fn a_file_that_cannot_be_read_to_its_end_gives_an_error_not_a_digest() {
    struct Fails(usize);
    impl Read for Fails {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("disk went away"));
            }
            self.0 -= 1;
            out[0] = 0;
            Ok(1)
        }
    }
    let open: Opener = Rc::new(|_: &Path| Ok(Box::new(Fails(10)) as Box<dyn Read>));
    assert!(load_adsb_with(Path::new("clip.oaf"), &open).is_err());
}

/// Endless zero bytes; counts what was pulled from it.
struct Endless {
    pulled: Rc<RefCell<u64>>,
    limit: u64,
}

impl Read for Endless {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut p = self.pulled.borrow_mut();
        if *p >= self.limit {
            return Ok(0);
        }
        out.fill(0);
        *p += out.len() as u64;
        Ok(out.len())
    }
}

// REQ: OA-BENCH-092
#[test]
fn a_msg_file_longer_than_a_document_can_be_is_refused_by_its_length() {
    let dir = scratch("long");
    let f = std::fs::File::create(dir.join("000000.msg")).unwrap();
    // A sparse file: past the largest document, no real bytes on disk.
    f.set_len(16 * 1024 * 1024).unwrap();
    drop(f);
    let pulled = Rc::new(RefCell::new(0u64));
    let p2 = pulled.clone();
    let open: Opener = Rc::new(move |_| {
        Ok(Box::new(Endless {
            pulled: p2.clone(),
            limit: 16 * 1024 * 1024,
        }) as Box<dyn Read>)
    });
    let r = load_with(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: dir.clone(),
        }],
        &open,
    );
    let e = r.err().expect("an oversize file is refused");
    assert!(e.to_string().contains("000000.msg"), "{e}");
    assert_eq!(*pulled.borrow(), 0, "refused before any read");
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-BENCH-092
#[test]
fn a_msg_stream_that_outgrows_its_length_is_cut_at_the_limit() {
    let dir = scratch("grow");
    std::fs::write(dir.join("000000.msg"), b"<x/>").unwrap();
    let pulled = Rc::new(RefCell::new(0u64));
    let p2 = pulled.clone();
    let open: Opener = Rc::new(move |_| {
        Ok(Box::new(Endless {
            pulled: p2.clone(),
            limit: 64 * 1024 * 1024,
        }) as Box<dyn Read>)
    });
    let r = load_with(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: dir.clone(),
        }],
        &open,
    );
    let e = r.err().expect("a stream past the limit is refused");
    assert!(e.to_string().contains("000000.msg"), "{e}");
    let got = *pulled.borrow();
    assert!(got <= 4 * 1024 * 1024 + 64 * 1024 + 8192, "pulled {got}");
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-BENCH-092
#[test]
fn a_whole_file_read_stops_at_its_limit() {
    use oa_bench::input::read_capped_file;
    let dir = scratch("cap");
    let p = dir.join("a.bin");
    std::fs::write(&p, vec![7u8; 100]).unwrap();
    assert_eq!(read_capped_file(&p, 100).unwrap().len(), 100);
    let e = read_capped_file(&p, 99).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    assert!(e.to_string().contains("a.bin"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}
