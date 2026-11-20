//! Independent real captures: ASTERIX from another organisation's system (the
//! CroatiaControlLtd/asterix sample set, GPL-2.0, kept outside this repository). Read from
//! `$OPENATCA_DATA/raw/asterix-samples/`, skipped with a note when absent. No byte of them is
//! in this repository; `tests/findings.rs` rebuilds each finding by hand from the specification.
//!
//! What the files are (determined from their bytes and checked against the upstream decoder's
//! own output in `raw/asterix-samples-expected/`):
//!
//! * `cat048.raw`, `cat062cat065.raw`, `cat21_re.ast`: bare data blocks end to end, no record
//!   header (`cat21_re.ast` included; its first octet is the category).
//! * `asterix.pcap`, `cat_034_048.pcap`, `cat_062_065.pcap`: libpcap, Ethernet, IPv4, UDP, one
//!   or more data blocks per datagram.
//! * `cat_001_002.pcap`: the same, but each datagram is a series of frames `[LEN16 counting
//!   itself][four octets][one data block]`; the four octets are not ASTERIX (upstream reads them
//!   as a time stamp); the meaning is not documented in the sample set and is not used here.
//! * `parsegps.gps`: not ASTERIX at the top level. 2200 octets of 0xCD (the Microsoft debug
//!   heap's fill byte), then data blocks each followed by ten octets (seven constant here, then
//!   a 16-bit big-endian time in 1/128 s, then a zero); upstream names the format "GPS".
//!   Inferred, then checked: the times equal upstream's printed time stamps. Only the blocks
//!   inside are read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/pcap.rs"]
mod pcap;

use oa_asterix::cat048::*;
use oa_asterix::cat062::*;
use oa_asterix::uap::{slot_of, uap_for, Slot};
use oa_asterix::units::*;
use oa_asterix::wire::parse_block;
use oa_asterix::*;
use pcap::*;
use std::path::PathBuf;

fn dirs() -> Option<(PathBuf, PathBuf)> {
    let root = PathBuf::from(std::env::var_os("OPENATCA_DATA")?).join("raw");
    let d = root.join("asterix-samples");
    d.is_dir()
        .then(|| (d, root.join("asterix-samples-expected")))
}

fn read(name: &str) -> Option<Vec<u8>> {
    let (d, _) = dirs()?;
    Some(oa_sealed::read(d.join(name)).unwrap())
}

fn read_expected(name: &str) -> Option<String> {
    let (_, e) = dirs()?;
    oa_sealed::read_to_string(e.join(name)).ok()
}

/// Counts for one capture file.
#[derive(Default, Debug)]
struct Tally {
    blocks: usize,
    /// Records decoded, by category 21, 48, 62.
    records: [usize; 3],
    /// Supported blocks that decoded and re-encoded to the same bytes.
    exact: usize,
    /// Blocks that decoded but whose re-encoding differs.
    mismatch: Vec<String>,
    /// Supported blocks the decoder refused.
    refused: Vec<String>,
    /// Blocks of categories we do not support, by category.
    skipped: Vec<(u8, usize)>,
    /// Octets that are not a whole block.
    stray: usize,
}

/// The data blocks of one buffer, framing only (category, LEN); `Err` with the offset of the
/// first octet that is not a whole block.
fn split_blocks(buf: &[u8]) -> Result<Vec<&[u8]>, usize> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let rest = &buf[pos..];
        if rest.len() < 3 {
            return Err(pos);
        }
        let len = usize::from(u16::from_be_bytes([rest[1], rest[2]]));
        if len < 3 || len > rest.len() {
            return Err(pos);
        }
        out.push(&rest[..len]);
        pos += len;
    }
    Ok(out)
}

/// Decodes and re-encodes every supported block, counts the rest by category.
fn tally(blocks: &[&[u8]], t: &mut Tally) {
    for (i, b) in blocks.iter().enumerate() {
        t.blocks += 1;
        let cat = b[0];
        if !matches!(cat, 21 | 48 | 62) {
            match t.skipped.iter_mut().find(|s| s.0 == cat) {
                Some(s) => s.1 += 1,
                None => t.skipped.push((cat, 1)),
            }
            continue;
        }
        match decode_block(b) {
            Ok((blk, n)) => {
                assert_eq!(n, b.len());
                let (ix, k) = match &blk {
                    Block::Cat021(r) => (0, r.len()),
                    Block::Cat048(r) => (1, r.len()),
                    Block::Cat062(r) => (2, r.len()),
                };
                t.records[ix] += k;
                match encode_block(&blk) {
                    Ok(e) if e == *b => t.exact += 1,
                    Ok(_) => t
                        .mismatch
                        .push(format!("block {i}: CAT{cat:03} bytes differ")),
                    Err(e) => t.mismatch.push(format!("block {i}: encode refused: {e}")),
                }
            }
            Err(e) => t.refused.push(format!("block {i}: {e}")),
        }
    }
}

/// The 0xCD fill, then blocks each followed by ten octets (see the module notes). Returns the
/// blocks and the 16-bit time (1/128 s) that follows each.
fn gps_blocks(file: &[u8]) -> (Vec<&[u8]>, Vec<u16>) {
    let mut pos = file.iter().position(|&b| b != 0xCD).unwrap();
    let (mut blocks, mut times) = (Vec::new(), Vec::new());
    while pos < file.len() {
        let len = usize::from(u16::from_be_bytes([file[pos + 1], file[pos + 2]]));
        blocks.push(&file[pos..pos + len]);
        let gap = &file[pos + len..pos + len + 10];
        times.push(u16::from_be_bytes([gap[7], gap[8]]));
        pos += len + 10;
    }
    assert_eq!(pos, file.len(), "gps blocks and gaps tile the file");
    (blocks, times)
}

/// The frames of a `cat_001_002.pcap` datagram: `[LEN16 counting itself][4 octets][block]`.
fn frame_blocks(payload: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < payload.len() {
        let len = usize::from(u16::from_be_bytes([payload[pos], payload[pos + 1]]));
        let frame = &payload[pos..pos + len];
        let blocks = split_blocks(&frame[6..]).unwrap();
        assert_eq!(blocks.iter().map(|b| b.len()).sum::<usize>(), len - 6);
        out.extend(blocks);
        pos += len;
    }
    out
}

/// The blocks of a capture file, in order.
fn file_blocks<'a>(name: &str, bytes: &'a [u8]) -> Vec<&'a [u8]> {
    match name {
        "parsegps.gps" => gps_blocks(bytes).0,
        "cat_001_002.pcap" => read_pcap(bytes)
            .unwrap()
            .into_iter()
            .flat_map(frame_blocks)
            .collect(),
        n if n.ends_with(".pcap") => read_pcap(bytes)
            .unwrap()
            .into_iter()
            .flat_map(|p| split_blocks(p).unwrap())
            .collect(),
        _ => split_blocks(bytes).unwrap(),
    }
}

fn survey(name: &str) -> Option<Tally> {
    let bytes = read(name)?;
    let mut t = Tally::default();
    tally(&file_blocks(name, &bytes), &mut t);
    println!("{name}: {t:?}");
    Some(t)
}

/// Pins one file's counts: blocks, records (021, 048, 062), exact, refused, skipped.
fn expect(
    name: &str,
    blocks: usize,
    records: [usize; 3],
    exact: usize,
    refused: usize,
    skipped: &[(u8, usize)],
) {
    let Some(t) = survey(name) else {
        println!("OPENATCA_DATA has no asterix-samples: skipped");
        return;
    };
    assert_eq!(
        (
            t.blocks,
            t.records,
            t.exact,
            t.refused.len(),
            t.skipped.as_slice()
        ),
        (blocks, records, exact, refused, skipped),
        "{name}"
    );
    assert!(t.mismatch.is_empty(), "{name}: {:?}", t.mismatch);
    assert_eq!(t.stray, 0);
}

// REQ: OA-AST-023
#[test]
fn bare_files_decode_and_round_trip() {
    // One CAT048 block with one record.
    expect("cat048.raw", 1, [0, 1, 0], 1, 0, &[]);
    // The Reserved Expansion Field (FRN 48) is kept raw: both blocks round-trip.
    expect("cat21_re.ast", 2, [2, 0, 0], 2, 0, &[]);
    // CAT062 and CAT065: the 062 block has an I062/390 with a primary subfield of three octets
    // whose third sets no bit; accepted since the fix, kept as it came.
    expect("cat062cat065.raw", 2, [0, 0, 2], 1, 0, &[(65, 1)]);
}

// REQ: OA-AST-023
#[test]
fn pcap_files_decode_and_round_trip() {
    expect("cat_062_065.pcap", 2, [0, 0, 2], 1, 0, &[(65, 1)]);
    // 120 blocks: 34 of CAT034 skipped by category, the rest CAT048 (128 records).
    expect("cat_034_048.pcap", 120, [0, 128, 0], 86, 0, &[(34, 34)]);
    // Frames of CAT001 and CAT002: framing parses, nothing decoded.
    let Some(t) = survey("cat_001_002.pcap") else {
        println!("OPENATCA_DATA has no asterix-samples: skipped");
        return;
    };
    assert_eq!(t.records, [0, 0, 0]);
    assert_eq!(t.stray, 0);
    assert_eq!((t.blocks, t.skipped.as_slice()), (6, &[(1, 5), (2, 1)][..]));
}

// REQ: OA-AST-023
#[test]
fn gps_container_blocks_decode_and_round_trip() {
    // 15 blocks: 12 of CAT048 (one record each in the 15 records) and 3 of CAT034.
    expect("parsegps.gps", 15, [0, 15, 0], 12, 0, &[(34, 3)]);
}

// REQ: OA-AST-028
#[test]
fn asterix_pcap_is_not_the_edition_we_implement() {
    let Some(bytes) = read("asterix.pcap") else {
        println!("OPENATCA_DATA has no asterix-samples: skipped");
        return;
    };
    let packets = read_pcap(&bytes).unwrap();
    assert_eq!(packets.len(), 100);
    let mut refused = 0;
    let mut prev_time = 0u32;
    for p in &packets {
        // Refused under the Ed 1.21 layout ...
        assert!(decode_block(p).is_err());
        refused += 1;
        // ... and consistent under the pre-standard layout (CAT062 Ed 0.17 UAP): the record
        // tiles the block exactly, the data source is the same in every datagram, and the
        // time of track (FRN 3) never runs backwards.
        let (end, time) = walk_cat062_ed017(p);
        assert_eq!(end, p.len());
        assert_eq!(&p[p.len() - end + 6..p.len() - end + 8], &[25, 100][..]);
        assert!(time >= prev_time);
        assert!(time < 86_400 * 128);
        prev_time = time;
    }
    assert_eq!(refused, 100);
}

/// Walks one CAT062 block in the layout of the pre-standard Ed 0.17 UAP: FRN 1 data source 2,
/// 2 service id 1, 3 time 3, 4 track number 2, 5 position 6, 6 cartesian 4, 7 mode 3/A 2,
/// 8 130 2, 9 135 2, 10 136 2, 11 180 4, 12 200 1, 13 220 2, 14 240 1, 15 210 2, 16 track
/// status (extended, one octet), 21 flight plan data (compound, subitem lengths of Ed 1.21).
/// Returns where the record ends and the time of track in 1/128 s.
fn walk_cat062_ed017(block: &[u8]) -> (usize, u32) {
    assert_eq!(block[0], 62);
    assert_eq!(
        usize::from(u16::from_be_bytes([block[1], block[2]])),
        block.len()
    );
    let mut at = 3;
    let mut fspec = Vec::new();
    loop {
        let b = block[at];
        at += 1;
        fspec.push(b);
        if b & 1 == 0 {
            break;
        }
    }
    let present = |frn: usize| fspec[(frn - 1) / 7] & (0x80 >> ((frn - 1) % 7)) != 0;
    let mut time = 0;
    for frn in 1..=21 {
        if !present(frn) {
            continue;
        }
        let n = match frn {
            1 | 4 | 7..=10 | 13 | 15 => 2,
            2 | 12 | 14 => 1,
            3 => 3,
            5 => 6,
            6 | 11 => 4,
            16 => {
                let mut n = 1;
                while block[at + n - 1] & 1 == 1 {
                    n += 1;
                }
                n
            }
            21 => {
                let subs = [2, 7, 4, 1, 4, 1, 4, 4, 3, 2, 2, 0, 6, 1, 7, 7, 2, 7];
                let mut primary = 1;
                while block[at + primary - 1] & 1 == 1 {
                    primary += 1;
                }
                let mut n = primary;
                for i in 0..primary * 7 {
                    if block[at + i / 7] & (0x80 >> (i % 7)) != 0 {
                        n += if i == 11 {
                            1 + 4 * usize::from(block[at + n])
                        } else {
                            subs[i]
                        };
                    }
                }
                n
            }
            _ => panic!("FRN {frn} not in the sample"),
        };
        if frn == 3 {
            time = block[at..at + 3]
                .iter()
                .fold(0, |a, &b| a << 8 | u32::from(b));
        }
        at += n;
    }
    (at, time)
}

// ---- Upstream expected output ---------------------------------------------------------------

/// One item of the upstream text output: its number, its hex and its `key: value` lines.
#[derive(Debug, Default)]
struct UItem {
    id: String,
    hex: String,
    kv: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct URecord {
    hex: String,
    items: Vec<UItem>,
}

#[derive(Debug, Default)]
struct UBlock {
    cat: u8,
    len: usize,
    time: f64,
    records: Vec<URecord>,
}

/// Parses `-P`/`-R` text output of the upstream decoder.
fn parse_upstream(text: &str) -> Vec<UBlock> {
    let mut out: Vec<UBlock> = Vec::new();
    for line in text.lines() {
        if let Some(r) = line.strip_prefix("Category: ") {
            out.push(UBlock {
                cat: r.trim().parse().unwrap(),
                ..UBlock::default()
            });
        } else if let Some(b) = out.last_mut() {
            if let Some(r) = b.records.last_mut() {
                if let Some(rest) = line.strip_prefix("HexData: ") {
                    if r.items.is_empty() && r.hex.is_empty() {
                        r.hex = rest.trim().to_string();
                    }
                } else if let Some(rest) = line.strip_prefix("Item ") {
                    r.items.push(UItem {
                        id: rest[..3].to_string(),
                        ..UItem::default()
                    });
                } else if let Some(i) = r.items.last_mut() {
                    if let Some(h) = line.strip_prefix("[ ") {
                        i.hex = h.trim_end_matches(" ]").split(' ').collect();
                    } else if let Some(kv) = line.strip_prefix('\t') {
                        if let Some((k, v)) = kv.split_once(": ") {
                            i.kv.push((k.to_string(), v.to_string()));
                        }
                    }
                }
            }
            if line.starts_with("Data Record") {
                b.records.push(URecord::default());
            }
            if b.records.is_empty() {
                if let Some(v) = line.strip_prefix("Len: ") {
                    b.len = v.trim().parse().unwrap();
                } else if let Some(v) = line.strip_prefix("Timestamp: ") {
                    b.time = v.trim().parse().unwrap();
                }
            }
        }
    }
    out
}

/// The value of `key` in `item` of `rec`, up to the first " (".
fn uv<'a>(rec: &'a URecord, item: &str, key: &str) -> Option<&'a str> {
    let i = rec.items.iter().find(|i| i.id == item)?;
    let (_, v) = i.kv.iter().find(|(k, _)| k == key)?;
    Some(v.split(" (").next().unwrap_or(v))
}

fn un(rec: &URecord, item: &str, key: &str) -> Option<i64> {
    uv(rec, item, key)?.parse().ok()
}

/// Agreement counters: values compared, and each difference as text.
#[derive(Default, Debug)]
struct Agree {
    compared: usize,
    differ: Vec<String>,
}

impl Agree {
    fn eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, ours: T, theirs: T) {
        self.compared += 1;
        if ours != theirs {
            self.differ
                .push(format!("{what}: ours {ours:?}, upstream {theirs:?}"));
        }
    }
}

/// Our item bytes (id, hex) of every record of a block, from the framing layer.
fn our_items(block: &[u8]) -> Vec<Vec<(u16, String)>> {
    let raw = parse_block(block, 0).unwrap();
    let uap = uap_for(raw.cat).unwrap();
    raw.records
        .iter()
        .map(|r| {
            r.spans
                .iter()
                .filter_map(|s| match slot_of(uap, s.frn) {
                    Some(Slot::Item { id, .. }) => Some((
                        *id,
                        block[s.start..s.end]
                            .iter()
                            .map(|b| format!("{b:02X}"))
                            .collect(),
                    )),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// Compares a CAT048 record with the upstream text of the same record.
fn compare_048(a: &mut Agree, tag: &str, ours: &Cat048, up: &URecord) {
    let t = |k: &str| format!("{tag} {k}");
    a.eq(
        &t("010 SAC"),
        Some(i64::from(ours.source.sac)),
        un(up, "010", "System Area Code"),
    );
    a.eq(
        &t("010 SIC"),
        Some(i64::from(ours.source.sic)),
        un(up, "010", "System Identification Code"),
    );
    a.eq(
        &t("140"),
        ours.time_of_day.map(i64::from),
        un(up, "140", "Time Of Day"),
    );
    match &ours.descriptor {
        Some(d) => {
            a.eq(&t("020 TYP"), Some(i64::from(d.typ)), un(up, "020", "TYP"));
            a.eq(
                &t("020 SIM"),
                Some(i64::from(d.simulated)),
                un(up, "020", "SIM"),
            );
            a.eq(
                &t("020 RDP"),
                Some(i64::from(d.rdp_chain2)),
                un(up, "020", "RDP"),
            );
            a.eq(&t("020 SPI"), Some(i64::from(d.spi)), un(up, "020", "SPI"));
            a.eq(
                &t("020 RAB"),
                Some(i64::from(d.field_monitor)),
                un(up, "020", "RAB"),
            );
        }
        None => a.eq(&t("020 absent"), None, un(up, "020", "TYP")),
    }
    match &ours.polar {
        Some(p) => {
            a.eq(&t("040 RHO"), Some(i64::from(p.rho)), un(up, "040", "RHO"));
            a.eq(
                &t("040 THETA"),
                Some(i64::from(p.theta)),
                un(up, "040", "THETA"),
            );
        }
        None => a.eq(&t("040 absent"), None, un(up, "040", "RHO")),
    }
    match &ours.mode3a {
        Some(m) => {
            a.eq(
                &t("070 V"),
                Some(i64::from(m.not_validated)),
                un(up, "070", "V"),
            );
            a.eq(&t("070 G"), Some(i64::from(m.garbled)), un(up, "070", "G"));
            a.eq(
                &t("070 L"),
                Some(i64::from(m.l_or_change)),
                un(up, "070", "L"),
            );
            a.eq(
                &t("070 code"),
                Some(format!("{:04o}", m.code)),
                uv(up, "070", "Mode-3/A reply code").map(str::to_string),
            );
        }
        None => a.eq(&t("070 absent"), None, un(up, "070", "V")),
    }
    match &ours.flight_level {
        Some(f) => {
            a.eq(
                &t("090 V"),
                Some(i64::from(f.not_validated)),
                un(up, "090", "V"),
            );
            a.eq(&t("090 G"), Some(i64::from(f.garbled)), un(up, "090", "G"));
            a.eq(
                &t("090 FL"),
                Some(i64::from(f.raw)),
                un(up, "090", "FlightLevel"),
            );
        }
        None => a.eq(&t("090 absent"), None, un(up, "090", "FlightLevel")),
    }
    a.eq(
        &t("220"),
        ours.address.map(|x| format!("{x:06X}")),
        uv(up, "220", "AircraftAddress").map(str::to_string),
    );
    match (&ours.ident, up.items.iter().find(|i| i.id == "240")) {
        (Some(id), Some(i)) => a.eq(
            &t("240"),
            String::from_utf8(id.to_vec()).unwrap(),
            i.kv[0].1.clone(),
        ),
        // Six zero octets, kept raw: upstream prints eight spaces.
        (None, Some(i)) => {
            let z = ours.raw.iter().any(|r| r.frn == 9 && r.bytes == [0; 6]);
            a.eq(
                &t("240 all-zero"),
                (z, "        ".to_string()),
                (true, i.kv[0].1.clone()),
            );
        }
        (Some(_), None) => a.eq(&t("240 absent"), true, false),
        (None, None) => a.eq(&t("240 absent"), true, true),
    }
    a.eq(
        &t("161"),
        ours.track_number.map(i64::from),
        un(up, "161", "Track Number"),
    );
    match &ours.cartesian {
        Some(c) => {
            a.eq(
                &t("042 X"),
                Some(i64::from(c.x)),
                un(up, "042", "X component"),
            );
            a.eq(
                &t("042 Y"),
                Some(i64::from(c.y)),
                un(up, "042", "Y component"),
            );
        }
        None => a.eq(&t("042 absent"), None, un(up, "042", "X component")),
    }
    match &ours.plot {
        Some(p) => {
            a.eq(
                &t("130 SRL"),
                p.ssr_runlength.map(i64::from),
                un(up, "130", "SSR Plot Runlength"),
            );
            a.eq(
                &t("130 SRR"),
                p.ssr_replies.map(i64::from),
                un(up, "130", "SSR-Number of received reply"),
            );
            a.eq(
                &t("130 SAM"),
                p.ssr_amplitude.map(i64::from),
                un(up, "130", "SSR-Reply amplitude"),
            );
            a.eq(
                &t("130 PRL"),
                p.psr_runlength.map(i64::from),
                un(up, "130", "Primary Plot Runlength"),
            );
            a.eq(
                &t("130 PAM"),
                p.psr_amplitude.map(i64::from),
                un(up, "130", "Amplitude of Primary plot"),
            );
        }
        None => a.eq(
            &t("130 absent"),
            None,
            up.items.iter().find(|i| i.id == "130").map(|_| 0),
        ),
    }
    match &ours.track_status {
        Some(s) => {
            a.eq(
                &t("170 CNF"),
                Some(i64::from(s.tentative)),
                un(up, "170", "CNF"),
            );
            a.eq(&t("170 RAD"), Some(i64::from(s.rad)), un(up, "170", "RAD"));
            a.eq(
                &t("170 DOU"),
                Some(i64::from(s.doubtful)),
                un(up, "170", "DOU"),
            );
            a.eq(
                &t("170 MAH"),
                Some(i64::from(s.manoeuvre)),
                un(up, "170", "MAH"),
            );
            a.eq(&t("170 CDM"), Some(i64::from(s.cdm)), un(up, "170", "CDM"));
        }
        None => a.eq(&t("170 absent"), None, un(up, "170", "CNF")),
    }
}

/// Block, record and item agreement of one file with its upstream text output.
fn compare_text(name: &str, expected: &str) -> Agree {
    let bytes = read(name).unwrap();
    let ours = file_blocks(name, &bytes);
    let up = parse_upstream(&read_expected(expected).unwrap());
    let mut a = Agree::default();
    a.eq("blocks", ours.len(), up.len());
    for (bi, (b, u)) in ours.iter().zip(&up).enumerate() {
        let tag = format!("{name} block {bi}");
        a.eq(&format!("{tag} CAT"), b[0], u.cat);
        a.eq(&format!("{tag} LEN"), b.len() - 3, u.len);
        if !matches!(b[0], 48 | 62) {
            continue;
        }
        let (blk, _) = decode_block(b).unwrap();
        let items = our_items(b);
        let recs: Vec<(Vec<u8>, Option<Cat048>)> = match blk {
            Block::Cat048(r) => r
                .into_iter()
                .map(|x| {
                    (
                        encode_block(&Block::Cat048(vec![x.clone()])).unwrap(),
                        Some(x),
                    )
                })
                .collect(),
            Block::Cat062(r) => r
                .into_iter()
                .map(|x| (encode_block(&Block::Cat062(vec![x])).unwrap(), None))
                .collect(),
            Block::Cat021(_) => unreachable!(),
        };
        a.eq(&format!("{tag} records"), recs.len(), u.records.len());
        for (ri, ((enc, typed), ur)) in recs.iter().zip(&u.records).enumerate() {
            let tag = format!("{tag} rec {ri}");
            a.eq(&format!("{tag} bytes"), hex(&enc[3..]), ur.hex.clone());
            let ours_ids: Vec<(String, String)> = items[ri]
                .iter()
                .map(|(i, h)| (format!("{i:03}"), h.clone()))
                .collect();
            let up_ids: Vec<(String, String)> = ur
                .items
                .iter()
                .map(|i| (i.id.clone(), i.hex.clone()))
                .collect();
            a.eq(&format!("{tag} items"), ours_ids, up_ids);
            if let Some(t) = typed {
                compare_048(&mut a, &tag, t, ur);
            }
        }
    }
    println!("{name}: {} compared, {} differ", a.compared, a.differ.len());
    a
}

// REQ: OA-AST-024
#[test]
fn cat048_values_agree_with_upstream_output() {
    for (file, exp, n) in [
        ("cat_034_048.pcap", "cat_034_048.txt", 4_347),
        ("parsegps.gps", "parsegps.txt", 1),
    ] {
        if read(file).is_none() || read_expected(exp).is_none() {
            println!("OPENATCA_DATA has no asterix-samples(-expected): skipped");
            return;
        }
        let a = compare_text(file, exp);
        assert!(
            a.differ.is_empty(),
            "{file}: {:?}",
            &a.differ[..a.differ.len().min(10)]
        );
        assert!(a.compared >= n, "{file}: {} compared", a.compared);
        println!("{file}: {} compared", a.compared);
    }
}

// REQ: OA-AST-024
#[test]
fn gps_times_and_framing_agree_with_upstream_output() {
    let (Some(bytes), Some(text)) = (read("parsegps.gps"), read_expected("parsegps.txt")) else {
        println!("OPENATCA_DATA has no asterix-samples(-expected): skipped");
        return;
    };
    let (blocks, times) = gps_blocks(&bytes);
    let up = parse_upstream(&text);
    assert_eq!(blocks.len(), up.len());
    for (t, u) in times.iter().zip(&up) {
        // Upstream prints six decimals of t/128.
        assert_eq!((u.time * 128.0).round(), f64::from(*t), "block time");
    }
}

// REQ: OA-AST-024
#[test]
fn cat001_002_framing_agrees_with_upstream_output() {
    let (Some(bytes), Some(text)) = (read("cat_001_002.pcap"), read_expected("cat_001_002.txt"))
    else {
        println!("OPENATCA_DATA has no asterix-samples(-expected): skipped");
        return;
    };
    let ours = file_blocks("cat_001_002.pcap", &bytes);
    let up = parse_upstream(&text);
    assert_eq!(ours.len(), up.len());
    for (b, u) in ours.iter().zip(&up) {
        assert_eq!((b[0], b.len() - 3), (u.cat, u.len));
    }
}

// REQ: OA-AST-024
#[test]
fn cat062_values_agree_with_upstream_json() {
    let (Some(bytes), Some(text)) = (
        read("cat_062_065.pcap"),
        read_expected("cat_062_065_json.txt"),
    ) else {
        println!("OPENATCA_DATA has no asterix-samples(-expected): skipped");
        return;
    };
    let ours: Vec<Cat062> = file_blocks("cat_062_065.pcap", &bytes)
        .into_iter()
        .filter(|b| b[0] == 62)
        .flat_map(|b| match decode_block(b).unwrap().0 {
            Block::Cat062(r) => r,
            _ => unreachable!(),
        })
        .collect();
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("\"cat\":62")).collect();
    assert_eq!(ours.len(), 2);
    assert_eq!(lines.len(), 2);
    let mut a = Agree::default();
    for (r, line) in ours.iter().zip(lines) {
        let doc = oa_json::parse(line.as_bytes(), &oa_json::DEFAULT_LIMITS).unwrap();
        let root = oa_json::NodeIx(0);
        let c = doc.get(root, "CAT062").unwrap();
        let num = |item: &str, key: &str| {
            let it = doc.get(c, item)?;
            doc.as_f64(doc.get(it, key)?)
        };
        let text_of = |item: &str, key: &str| {
            let it = doc.get(c, item)?;
            doc.str(doc.get(it, key)?).map(str::to_string)
        };
        let rec_hex = doc
            .str(doc.get(root, "hexdata").unwrap())
            .unwrap()
            .to_string();
        a.eq(
            "bytes",
            hex(&encode_block(&Block::Cat062(vec![r.clone()])).unwrap()[3..]),
            rec_hex,
        );
        a.eq("010 SAC", Some(f64::from(r.source.sac)), num("I010", "SAC"));
        a.eq("010 SIC", Some(f64::from(r.source.sic)), num("I010", "SIC"));
        a.eq(
            "070",
            r.time.map(|t| f64::from(t) * CAT062_TIME_S),
            num("I070", "ToT"),
        );
        let close =
            |a: &mut Agree, what: &str, ours: Option<f64>, theirs: Option<f64>, tol: f64| {
                let ok = match (ours, theirs) {
                    (Some(x), Some(y)) => (x - y).abs() <= tol,
                    (None, None) => true,
                    _ => false,
                };
                a.eq(what, ok, true);
            };
        let p = r.position.unwrap();
        close(
            &mut a,
            "105 lat",
            Some(f64::from(p.lat) * CAT062_POS_DEG),
            num("I105", "Lat"),
            1e-7,
        );
        close(
            &mut a,
            "105 lon",
            Some(f64::from(p.lon) * CAT062_POS_DEG),
            num("I105", "Lon"),
            1e-7,
        );
        let c100 = r.cartesian.unwrap();
        a.eq(
            "100 X",
            Some(f64::from(c100.x) * CAT062_CART_M),
            num("I100", "X"),
        );
        a.eq(
            "100 Y",
            Some(f64::from(c100.y) * CAT062_CART_M),
            num("I100", "Y"),
        );
        let v = r.velocity.unwrap();
        a.eq(
            "185 Vx",
            Some(f64::from(v.vx) * CAT062_VEL_M_S),
            num("I185", "Vx"),
        );
        a.eq(
            "185 Vy",
            Some(f64::from(v.vy) * CAT062_VEL_M_S),
            num("I185", "Vy"),
        );
        let m = r.mode3a.unwrap();
        a.eq(
            "060 code",
            Some(format!("{:04o}", m.code)),
            text_of("I060", "Mode3A"),
        );
        a.eq(
            "060 V",
            Some(f64::from(u8::from(m.not_validated))),
            num("I060", "V"),
        );
        a.eq(
            "060 G",
            Some(f64::from(u8::from(m.garbled))),
            num("I060", "G"),
        );
        a.eq(
            "060 CH",
            Some(f64::from(u8::from(m.l_or_change))),
            num("I060", "CH"),
        );
        let d = r.derived.as_ref().unwrap();
        a.eq(
            "380 ADR",
            d.address.map(|x| format!("{x:06X}")),
            doc.get(c, "I380")
                .and_then(|i| doc.get(i, "ADR"))
                .and_then(|i| doc.get(i, "ADR"))
                .and_then(|n| doc.str(n))
                .map(str::to_string),
        );
        a.eq(
            "380 ACID",
            d.ident.map(|x| String::from_utf8(x.to_vec()).unwrap()),
            doc.get(c, "I380")
                .and_then(|i| doc.get(i, "ID"))
                .and_then(|i| doc.get(i, "ACID"))
                .and_then(|n| doc.str(n))
                .map(str::to_string),
        );
        a.eq("040", r.track_number.map(f64::from), num("I040", "TrkN"));
        let s = r.track_status.as_ref().unwrap();
        let st = doc.get(c, "I080").unwrap();
        // I080 repeats FX; the named bits are looked up in document order.
        let first = |k: &str| {
            doc.members(st)
                .find(|(n, _)| doc.str(*n) == Some(k))
                .and_then(|(_, v)| doc.as_f64(v))
        };
        a.eq(
            "080 MON",
            Some(f64::from(u8::from(s.monosensor))),
            first("MON"),
        );
        a.eq("080 SPI", Some(f64::from(u8::from(s.spi))), first("SPI"));
        a.eq(
            "080 MRH",
            Some(f64::from(u8::from(s.geometric_more_reliable))),
            first("MRH"),
        );
        a.eq("080 SRC", Some(f64::from(s.altitude_source)), first("SRC"));
        a.eq(
            "080 CNF",
            Some(f64::from(u8::from(s.tentative))),
            first("CNF"),
        );
        let ages = r.update_ages.unwrap();
        a.eq(
            "290 PSR",
            ages.psr.map(|x| f64::from(x) * CAT062_AGE_S),
            num("I290", "PSR").or(doc
                .get(c, "I290")
                .and_then(|i| doc.get(i, "PSR"))
                .and_then(|i| doc.get(i, "PSR"))
                .and_then(|n| doc.as_f64(n))),
        );
        a.eq(
            "136",
            r.measured_fl.map(|x| f64::from(x) * CAT062_FL * 100.0),
            doc.get(c, "I136")
                .and_then(|i| doc.get(i, "MFL"))
                .and_then(|n| doc.as_f64(n)),
        );
        a.eq(
            "130",
            r.geometric_altitude
                .map(|x| f64::from(x) * CAT062_GEOM_ALT_FT),
            num("I130", "Alt"),
        );
        let b = r.baro_altitude.unwrap();
        a.eq(
            "135 QNH",
            Some(f64::from(u8::from(b.qnh))),
            num("I135", "QNH"),
        );
        a.eq(
            "135 CTBA",
            Some(f64::from(b.raw) * CAT062_FL * 100.0),
            num("I135", "CTBA"),
        );
        a.eq(
            "220",
            r.climb_rate.map(|x| f64::from(x) * CAT062_ROC_FPM),
            num("I220", "RoC"),
        );
    }
    println!(
        "cat_062_065 json: {} compared, {} differ",
        a.compared,
        a.differ.len()
    );
    assert!(a.differ.is_empty(), "{:?}", a.differ);
    assert_eq!(a.compared, 56);
}

// ---- The pcap reader, on hand-made files ----------------------------------------------------

/// An Ethernet II, IPv4 (no options), UDP frame around `payload`.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut f = vec![
        0x01, 0x00, 0x5E, 0x01, 0x01, 0x01, 2, 3, 4, 5, 6, 7, 0x08, 0x00,
    ];
    let total = 20 + 8 + payload.len();
    f.extend_from_slice(&[
        0x45,
        0,
        (total >> 8) as u8,
        total as u8,
        0,
        0,
        0x40,
        0,
        64,
        17,
        0,
        0,
    ]);
    f.extend_from_slice(&[10, 0, 0, 1, 233, 1, 1, 1]);
    let udp = 8 + payload.len();
    f.extend_from_slice(&[0x12, 0x34, 0x56, 0x78, (udp >> 8) as u8, udp as u8, 0, 0]);
    f.extend_from_slice(payload);
    f
}

/// A pcap file: `magic` read as written, little or big endian, `link` type, the frames.
fn pcap_file(magic: [u8; 4], le: bool, link: u32, frames: &[Vec<u8>]) -> Vec<u8> {
    let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut f = magic.to_vec();
    f.extend_from_slice(&w16(2));
    f.extend_from_slice(&w16(4));
    f.extend_from_slice(&[0; 8]);
    f.extend_from_slice(&w32(65_535));
    f.extend_from_slice(&w32(link));
    for fr in frames {
        f.extend_from_slice(&w32(1));
        f.extend_from_slice(&w32(2));
        f.extend_from_slice(&w32(fr.len() as u32));
        f.extend_from_slice(&w32(fr.len() as u32));
        f.extend_from_slice(fr);
    }
    f
}

const LE_MAGIC: [u8; 4] = [0xD4, 0xC3, 0xB2, 0xA1];
const BE_MAGIC: [u8; 4] = [0xA1, 0xB2, 0xC3, 0xD4];

// REQ: OA-AST-022
#[test]
fn pcap_reader_extracts_udp_payloads() {
    let frames = [frame(&[21, 0, 3]), frame(&[48, 0, 4, 9])];
    for (magic, le) in [
        (LE_MAGIC, true),
        (BE_MAGIC, false),
        ([0x4D, 0x3C, 0xB2, 0xA1], true),
        ([0xA1, 0xB2, 0x3C, 0x4D], false),
    ] {
        let f = pcap_file(magic, le, 1, &frames);
        let p = read_pcap(&f).unwrap();
        assert_eq!(p, vec![&[21u8, 0, 3][..], &[48, 0, 4, 9][..]]);
    }
    // No packets is a valid file; Ethernet padding past the IP datagram is not payload.
    assert!(read_pcap(&pcap_file(LE_MAGIC, true, 1, &[]))
        .unwrap()
        .is_empty());
    let mut padded = frame(&[1, 2, 3]);
    padded.extend_from_slice(&[0; 12]);
    let p = pcap_file(LE_MAGIC, true, 1, &[padded]);
    assert_eq!(read_pcap(&p).unwrap(), vec![&[1u8, 2, 3][..]]);
}

// REQ: OA-AST-022
#[test]
fn pcap_reader_refuses_everything_else_with_a_typed_error() {
    let good = frame(&[1, 2, 3]);
    let one = |f: Vec<u8>| read_pcap(&pcap_file(LE_MAGIC, true, 1, &[f])).unwrap_err();
    // File level.
    assert_eq!(read_pcap(&[]).unwrap_err(), PcapError::Truncated);
    assert_eq!(read_pcap(&[0; 24]).unwrap_err(), PcapError::BadMagic);
    let mut f = pcap_file(LE_MAGIC, true, 1, std::slice::from_ref(&good));
    f[4] = 3;
    assert_eq!(read_pcap(&f).unwrap_err(), PcapError::BadVersion);
    assert_eq!(
        read_pcap(&pcap_file(LE_MAGIC, true, 113, std::slice::from_ref(&good))).unwrap_err(),
        PcapError::LinkType(113)
    );
    let f = pcap_file(LE_MAGIC, true, 1, std::slice::from_ref(&good));
    assert_eq!(
        read_pcap(&f[..f.len() - 1]).unwrap_err(),
        PcapError::Truncated
    );
    assert_eq!(read_pcap(&f[..30]).unwrap_err(), PcapError::Truncated);
    // A packet cut by the snaplen: original length above the captured length.
    let mut f = pcap_file(LE_MAGIC, true, 1, std::slice::from_ref(&good));
    f[24 + 12] += 1;
    assert_eq!(read_pcap(&f).unwrap_err(), PcapError::PacketCut);
    // A captured length past the bound.
    let mut f = pcap_file(LE_MAGIC, true, 1, std::slice::from_ref(&good));
    f[24 + 8..24 + 12].copy_from_slice(&(MAX_PACKET_LEN as u32 + 1).to_le_bytes());
    assert_eq!(read_pcap(&f).unwrap_err(), PcapError::PacketTooLong);
    // Frame level: ARP, IPv6, a header below five words, a datagram longer than the frame,
    // a fragment, TCP, a UDP length below eight and past the datagram.
    let mut arp = good.clone();
    arp[12..14].copy_from_slice(&[0x08, 0x06]);
    assert_eq!(one(arp), PcapError::EtherType(0x0806));
    let mut v6 = good.clone();
    v6[14] = 0x65;
    assert_eq!(one(v6), PcapError::BadIpHeader);
    let mut ihl = good.clone();
    ihl[14] = 0x44;
    assert_eq!(one(ihl), PcapError::BadIpHeader);
    let mut long = good.clone();
    long[17] += 1;
    assert_eq!(one(long), PcapError::BadIpHeader);
    let mut frag = good.clone();
    frag[20] = 0x20;
    assert_eq!(one(frag), PcapError::Fragmented);
    let mut frag = good.clone();
    frag[21] = 0x08;
    assert_eq!(one(frag), PcapError::Fragmented);
    let mut tcp = good.clone();
    tcp[23] = 6;
    assert_eq!(one(tcp), PcapError::Protocol(6));
    let mut short = good.clone();
    short[39] = 7;
    assert_eq!(one(short), PcapError::BadUdp);
    let mut past = good.clone();
    past[39] = 12;
    assert_eq!(one(past), PcapError::BadUdp);
    assert_eq!(one(vec![0; 10]), PcapError::Truncated);
}
