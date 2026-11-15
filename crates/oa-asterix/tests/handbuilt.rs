//! Records built by hand from the specification's own definitions. Every expected byte is
//! derived in a comment from the LSB or bit layout, not taken from the encoder's output.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_asterix::cat021::*;
use oa_asterix::cat048::*;
use oa_asterix::cat062::*;
use oa_asterix::units::*;
use oa_asterix::*;

/// One block of category `cat` holding `recs`: CAT, LEN (3 + the records), the records.
fn blk(cat: u8, recs: &[&[u8]]) -> Vec<u8> {
    let body: Vec<u8> = recs.iter().flat_map(|r| r.iter().copied()).collect();
    let len = (3 + body.len()) as u16;
    let mut v = vec![cat];
    v.extend_from_slice(&len.to_be_bytes());
    v.extend_from_slice(&body);
    v
}

/// CAT021 record: FRNs 1 (I021/010), 2 (040), 7 (131), 11 (080), 17 (090), 19 (070), 21 (145).
///
/// FSPEC (Part 1 5.2.7, bit 8 first, bit 1 = FX): octet 1 covers FRN 1 to 7: FRN 1 = 0x80,
/// FRN 2 = 0x40, FRN 7 = 0x02, FX = 0x01, so 0xC3. Octet 2 covers FRN 8 to 14: FRN 11 is the
/// fourth bit, 0x10, plus FX, so 0x11. Octet 3 covers FRN 15 to 21: FRN 17 = 0x20, FRN 19 =
/// 0x08, FRN 21 = 0x02, no FX, so 0x2A.
///
/// I021/010: SAC 25 = 0x19, SIC 3 = 0x03.
/// I021/040: ATP = 0 (bits 8/6), ARC = 1 (100 ft, bits 5/4), RC = 0, RAB = 0, FX = 0:
/// 0b000_01_0_0_0 = 0x08.
/// I021/131: latitude 49.2633 degrees = 492 633 000 e-7 degree. LSB = 180/2^30 degrees, so the
/// count is 492 633 000e-7 / (180 / 2^30) = 293 867 031.10, rounded 293 867 031 = 0x11840E17.
/// Longitude 10.614239 degrees: 63 316 401.91, rounded 63 316 402 = 0x03C621B2.
/// I021/080: address 0x3C66B0.
/// I021/090: primary NACv = 1 (bits 8/6 = 001), NIC = 8 (bits 5/2 = 1000), FX = 1:
/// 0b001_1000_1 = 0x31. First extension: NICBARO = 1 (bit 8), SIL = 3 (bits 7/6 = 11), NACp = 8
/// (bits 5/2 = 1000), FX = 0: 0b1_11_1000_0 = 0xF0.
/// I021/070: Mode 3/A 1000 octal = 0o1000 = 0x200, bits 16/13 spare = 0: 0x0200.
/// I021/145: FL 250, LSB 1/4 FL: 250 / 0.25 = 1000 = 0x03E8.
/// Length: 3 FSPEC + 2 + 1 + 8 + 3 + 2 + 2 + 2 = 23 octets, the block 26 = 0x001A.
const CAT021_RECORD: [u8; 23] = [
    0xC3, 0x11, 0x2A, 0x19, 0x03, 0x08, 0x11, 0x84, 0x0E, 0x17, 0x03, 0xC6, 0x21, 0xB2, 0x3C, 0x66,
    0xB0, 0x31, 0xF0, 0x02, 0x00, 0x03, 0xE8,
];

fn cat021_expected() -> Cat021 {
    Cat021 {
        source: SourceId { sac: 25, sic: 3 },
        descriptor: Some(Trd021 {
            atp: 0,
            arc: 1,
            range_check: false,
            field_monitor: false,
            ext: vec![],
        }),
        position_hr: Some(RawPosition {
            lat: 0x1184_0E17,
            lon: 0x03C6_21B2,
        }),
        address: Some(0x3C_66B0),
        quality: Some(Quality021 {
            nacv: 1,
            nic: 8,
            ext: vec![0xF0],
        }),
        mode3a: Some(0o1000),
        flight_level: Some(1000),
        ..Cat021::default()
    }
}

// REQ: OA-AST-008
#[test]
fn cat021_hand_built_record_decodes_and_encodes() {
    let bytes = blk(21, &[&CAT021_RECORD]);
    assert_eq!(&bytes[..3], &[21, 0x00, 0x1A]);
    let (b, n) = decode_block(&bytes).unwrap();
    assert_eq!(n, 26);
    let Block::Cat021(recs) = &b else { panic!() };
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0], cat021_expected());
    let r = &recs[0];
    // The units: the raw counts times the named LSBs give back the values the bytes were
    // derived from.
    let p = r.position_hr.unwrap();
    assert!((f64::from(p.lat) * CAT021_POS_HR_DEG - 49.2633).abs() <= CAT021_POS_HR_DEG / 2.0);
    assert!((f64::from(p.lon) * CAT021_POS_HR_DEG - 10.614239).abs() <= CAT021_POS_HR_DEG / 2.0);
    assert_eq!(f64::from(r.flight_level.unwrap()) * CAT021_FL, 250.0);
    assert_eq!(r.mode3a, Some(0x200));
    let q = r.quality.as_ref().unwrap();
    assert_eq!(
        (
            q.nacv,
            q.nic,
            quality_nacp(q),
            quality_sil(q),
            quality_nicbaro(q)
        ),
        (1, 8, Some(8), Some(3), Some(1))
    );
    assert_eq!(quality_pic(q), None);
    let t = r.descriptor.as_ref().unwrap();
    assert_eq!(descriptor_ground(t), None);
    assert_eq!(encode_block(&b).unwrap(), bytes);
}

/// CAT048 record: FRNs 1 (I048/010), 2 (140), 3 (020), 4 (040), 5 (070), 6 (090), 7 (130),
/// 8 (220), 9 (240), 11 (161), 12 (042), 14 (170).
///
/// FSPEC: octet 1 covers FRN 1 to 7, all set: 0xFE plus FX = 0xFF. Octet 2 covers FRN 8 to 14:
/// FRN 8 = 0x80, 9 = 0x40, 11 = 0x10, 12 = 0x08, 14 = 0x02: 0xDA.
/// I048/010: SAC 10 = 0x0A, SIC 20 = 0x14.
/// I048/140: 12:00:00 = 43 200 s; LSB 1/128 s; 43 200 * 128 = 5 529 600 = 0x546000.
/// I048/020: TYP = 3 (SSR + PSR, bits 8/6 = 011), SIM RDP SPI RAB FX = 0: 0b011_00000 = 0x60.
/// I048/040: RHO 10 NM, LSB 1/256 NM: 10 * 256 = 2560 = 0x0A00. THETA 90 degrees, LSB
/// 360/2^16: 90 / 360 * 65 536 = 16 384 = 0x4000.
/// I048/070: Mode 3/A 7700 octal = 0o7700 = 0xFC0, V G L spare = 0: 0x0FC0.
/// I048/090: FL 350, LSB 1/4 FL: 350 * 4 = 1400 = 0x0578, V and G = 0.
/// I048/130: primary 0b1000_0100 = 0x84 (subitem 1 SRL and subitem 6 RPD present, FX = 0),
/// SRL = 16 (0x10) counts of 360/2^13 degrees, RPD = -1 count = 0xFF (two's complement).
/// I048/220: address 0xABCDEF.
/// I048/240: "DLH7YA  " in six-bit characters: D=4, L=12, H=8, 7=55, Y=25, A=1, space=32,
/// space=32: 000100 001100 001000 110111 011001 000001 100000 100000 = 10 C2 37 64 18 20.
/// I048/161: track 1234 = 0x04D2.
/// I048/042: X = 100 NM, LSB 1/128 NM: 12 800 = 0x3200; Y = -50 NM: -6400 = 0xE700.
/// I048/170: CNF = 0 (confirmed), RAD = 00 (combined), DOU = 0, MAH = 0, CDM = 01 (climbing),
/// FX = 0: 0b0_00_0_0_01_0 = 0x02.
/// Length: 2 FSPEC + 2 + 3 + 1 + 4 + 2 + 2 + 3 + 3 + 6 + 2 + 4 + 1 = 35, the block 38 = 0x26.
const CAT048_RECORD: [u8; 35] = [
    0xFF, 0xDA, 0x0A, 0x14, 0x54, 0x60, 0x00, 0x60, 0x0A, 0x00, 0x40, 0x00, 0x0F, 0xC0, 0x05, 0x78,
    0x84, 0x10, 0xFF, 0xAB, 0xCD, 0xEF, 0x10, 0xC2, 0x37, 0x64, 0x18, 0x20, 0x04, 0xD2, 0x32, 0x00,
    0xE7, 0x00, 0x02,
];

fn cat048_expected() -> Cat048 {
    Cat048 {
        source: SourceId { sac: 10, sic: 20 },
        time_of_day: Some(5_529_600),
        descriptor: Some(Trd048 {
            typ: 3,
            ..Trd048::default()
        }),
        polar: Some(Polar {
            rho: 2560,
            theta: 16384,
        }),
        mode3a: Some(Mode3a {
            code: 0o7700,
            ..Mode3a::default()
        }),
        flight_level: Some(FlightLevel048 {
            raw: 1400,
            ..FlightLevel048::default()
        }),
        plot: Some(PlotCharacteristics {
            ssr_runlength: Some(16),
            range_diff: Some(-1),
            ..PlotCharacteristics::default()
        }),
        address: Some(0xAB_CDEF),
        ident: Some(*b"DLH7YA  "),
        track_number: Some(1234),
        cartesian: Some(Cartesian { x: 12800, y: -6400 }),
        track_status: Some(TrackStatus048 {
            cdm: 1,
            ..TrackStatus048::default()
        }),
        ..Cat048::default()
    }
}

// REQ: OA-AST-009
#[test]
fn cat048_hand_built_record_decodes_and_encodes() {
    let bytes = blk(48, &[&CAT048_RECORD]);
    assert_eq!(&bytes[..3], &[48, 0x00, 0x26]);
    let (b, n) = decode_block(&bytes).unwrap();
    assert_eq!(n, 38);
    let Block::Cat048(recs) = &b else { panic!() };
    assert_eq!(recs[0], cat048_expected());
    let r = &recs[0];
    assert_eq!(f64::from(r.time_of_day.unwrap()) * CAT048_TIME_S, 43_200.0);
    let p = r.polar.unwrap();
    assert_eq!(f64::from(p.rho) * CAT048_RHO_NM, 10.0);
    assert_eq!(f64::from(p.theta) * CAT048_THETA_DEG, 90.0);
    assert_eq!(f64::from(r.flight_level.unwrap().raw) * CAT048_FL, 350.0);
    let c = r.cartesian.unwrap();
    assert_eq!(
        (f64::from(c.x) * CAT048_XY_NM, f64::from(c.y) * CAT048_XY_NM),
        (100.0, -50.0)
    );
    assert_eq!(encode_block(&b).unwrap(), bytes);
}

/// CAT062 record: FRNs 1 (I062/010), 4 (070), 5 (105), 6 (100), 7 (185), 9 (060), 11 (380),
/// 12 (040), 13 (080), 14 (290), 17 (136), 18 (130), 20 (220).
///
/// FSPEC: octet 1 (FRN 1 to 7): FRN 1 = 0x80, 4 = 0x10, 5 = 0x08, 6 = 0x04, 7 = 0x02, FX =
/// 0x01: 0x9F. Octet 2 (FRN 8 to 14): FRN 9 = 0x40, 11 = 0x10, 12 = 0x08, 13 = 0x04, 14 =
/// 0x02, FX: 0x5F. Octet 3 (FRN 15 to 21): FRN 17 = 0x20, 18 = 0x10, 20 = 0x04: 0x34.
/// I062/010: SAC 20 = 0x14, SIC 5 = 0x05.
/// I062/070: 12:00:00 = 5 529 600 counts of 1/128 s = 0x546000.
/// I062/105: LSB 180/2^25 degrees. Latitude 41.9786: 41.9786 * 2^25 / 180 = 7 825 378.2,
/// rounded 7 825 378 = 0x007767E2. Longitude -87.9048: -16 386 642.4, rounded -16 386 642 =
/// 0xFF05F5AE (two's complement in 32 bits).
/// I062/100: LSB 0.5 m. X = 10 000 m = 20 000 counts = 0x004E20; Y = -5000 m = -10 000 =
/// 0xFFD8F0 in 24 bits.
/// I062/185: LSB 0.25 m/s. Vx = 200 m/s = 800 = 0x0320; Vy = -100 m/s = -400 = 0xFE70.
/// I062/060: V G CH = 0, spare 0, code 2000 octal = 0o2000 = 0x400: 0x0400.
/// I062/380: primary 0b1100_0100 = 0xC4 (subitems 1, 2 and 6, FX = 0): address 0xA1B2C3;
/// identification "AAL123  " = A=1 A=1 L=12 1=49 2=50 3=51 space space = 000001 000001 001100
/// 110001 110010 110011 100000 100000 = 04 13 31 CB 38 20; selected altitude SAS = 1, source = 2 (MCP/FCU, bits 15/14 = 10), altitude FL350 = 35 000 ft at LSB
/// 25 ft = 1400 = 0x0578 in 13 bits: 0x8000 | 0x4000 | 0x0578 = 0xC578.
/// I062/040: track 0x1234.
/// I062/080: MON 0, SPI 0, MRH 1 (0x20), SRC = 001 (GNSS, bits 5/3: 0x04), CNF 0, FX 0: 0x24.
/// I062/290: primary 0b1010_0000 = 0xA0 (subitem 1 TRK and 3 SSR): TRK age 4 s at 1/4 s = 16 =
/// 0x10; SSR age 2.5 s = 10 = 0x0A.
/// I062/136: FL 350 = 1400 = 0x0578. I062/130: 35 000 ft at 6.25 ft = 5600 = 0x15E0.
/// I062/220: 1500 ft/min at 6.25 = 240 = 0x00F0.
/// Length: 3 FSPEC + 2 + 3 + 8 + 6 + 4 + 2 + 12 + 2 + 1 + 3 + 2 + 2 + 2 = 52, block 55 = 0x37.
const CAT062_RECORD: [u8; 52] = [
    0x9F, 0x5F, 0x34, 0x14, 0x05, 0x54, 0x60, 0x00, 0x00, 0x77, 0x67, 0xE2, 0xFF, 0x05, 0xF5, 0xAE,
    0x00, 0x4E, 0x20, 0xFF, 0xD8, 0xF0, 0x03, 0x20, 0xFE, 0x70, 0x04, 0x00, 0xC4, 0xA1, 0xB2, 0xC3,
    0x04, 0x13, 0x31, 0xCB, 0x38, 0x20, 0xC5, 0x78, 0x12, 0x34, 0x24, 0xA0, 0x10, 0x0A, 0x05, 0x78,
    0x15, 0xE0, 0x00, 0xF0,
];

fn cat062_expected() -> Cat062 {
    Cat062 {
        source: SourceId { sac: 20, sic: 5 },
        time: Some(5_529_600),
        position: Some(RawPosition {
            lat: 7_825_378,
            lon: -16_386_642,
        }),
        cartesian: Some(CartPosition {
            x: 20_000,
            y: -10_000,
        }),
        velocity: Some(Velocity { vx: 800, vy: -400 }),
        mode3a: Some(Mode3a {
            code: 0o2000,
            ..Mode3a::default()
        }),
        derived: Some(AircraftDerived {
            address: Some(0xA1_B2C3),
            ident: Some(*b"AAL123  "),
            selected_altitude: Some(SelectedAltitude {
                source_provided: true,
                source: 2,
                altitude: 1400,
            }),
            other: vec![],
        }),
        track_number: Some(0x1234),
        track_status: Some(TrackStatus062 {
            geometric_more_reliable: true,
            altitude_source: 1,
            ..TrackStatus062::default()
        }),
        update_ages: Some(UpdateAges {
            track: Some(16),
            ssr: Some(10),
            ..UpdateAges::default()
        }),
        measured_fl: Some(1400),
        geometric_altitude: Some(5600),
        climb_rate: Some(240),
        ..Cat062::default()
    }
}

// REQ: OA-AST-010
#[test]
fn cat062_hand_built_record_decodes_and_encodes() {
    let bytes = blk(62, &[&CAT062_RECORD]);
    assert_eq!(&bytes[..3], &[62, 0x00, 0x37]);
    let (b, n) = decode_block(&bytes).unwrap();
    assert_eq!(n, 55);
    let Block::Cat062(recs) = &b else { panic!() };
    assert_eq!(recs[0], cat062_expected());
    let r = &recs[0];
    let p = r.position.unwrap();
    assert!((f64::from(p.lat) * CAT062_POS_DEG - 41.9786).abs() <= CAT062_POS_DEG / 2.0);
    assert!((f64::from(p.lon) * CAT062_POS_DEG + 87.9048).abs() <= CAT062_POS_DEG / 2.0);
    let c = r.cartesian.unwrap();
    assert_eq!(
        (
            f64::from(c.x) * CAT062_CART_M,
            f64::from(c.y) * CAT062_CART_M
        ),
        (10_000.0, -5000.0)
    );
    let v = r.velocity.unwrap();
    assert_eq!(
        (
            f64::from(v.vx) * CAT062_VEL_M_S,
            f64::from(v.vy) * CAT062_VEL_M_S
        ),
        (200.0, -100.0)
    );
    assert_eq!(f64::from(r.measured_fl.unwrap()) * CAT062_FL, 350.0);
    assert_eq!(
        f64::from(r.geometric_altitude.unwrap()) * CAT062_GEOM_ALT_FT,
        35_000.0
    );
    assert_eq!(f64::from(r.climb_rate.unwrap()) * CAT062_ROC_FPM, 1500.0);
    let a = r.update_ages.unwrap();
    assert_eq!(f64::from(a.track.unwrap()) * CAT062_AGE_S, 4.0);
    assert_eq!(f64::from(a.ssr.unwrap()) * CAT062_AGE_S, 2.5);
    assert_eq!(
        f64::from(
            r.derived
                .as_ref()
                .unwrap()
                .selected_altitude
                .unwrap()
                .altitude
        ) * CAT062_SELECTED_ALT_FT,
        35_000.0
    );
    assert_eq!(encode_block(&b).unwrap(), bytes);
}

// REQ: OA-AST-001
#[test]
fn block_framing_len_counts_header_and_records_tile_the_block() {
    // Two records in one block: LEN = 3 + 23 + 23.
    let two = blk(21, &[&CAT021_RECORD, &CAT021_RECORD]);
    assert_eq!(&two[..3], &[21, 0x00, 0x31]);
    let (b, n) = decode_block(&two).unwrap();
    assert_eq!(n, two.len());
    let Block::Cat021(recs) = &b else { panic!() };
    assert_eq!(recs.len(), 2);
    assert_eq!(encode_block(&b).unwrap(), two);
    // A stream of three blocks of different categories decodes block by block; the bytes after
    // LEN belong to the next block.
    let mut stream = blk(21, &[&CAT021_RECORD]);
    stream.extend(blk(48, &[&CAT048_RECORD]));
    stream.extend(blk(62, &[&CAT062_RECORD]));
    let blocks = decode_stream(&stream).unwrap();
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], Block::Cat021(_)));
    assert!(matches!(blocks[1], Block::Cat048(_)));
    assert!(matches!(blocks[2], Block::Cat062(_)));
    let (_, n) = decode_block(&stream).unwrap();
    assert_eq!(n, 26);
    // Trailing bytes after the last block are the start of a block and are refused as one.
    let mut bad = stream.clone();
    bad.push(21);
    let e = decode_stream(&bad).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Truncated);
    assert_eq!(e.offset, bad.len());
    // The block limit is the two-octet LEN.
    assert_eq!(MAX_BLOCK_LEN, 65_535);
    assert_eq!(MAX_RECORDS_PER_BLOCK, 21_844);
}

// REQ: OA-AST-002
#[test]
fn fspec_bit_order_and_multi_octet_extension() {
    // FRN 1 alone: 0x80. Only the data source identifier.
    let rec = [0x80, 0x19, 0x03];
    let (b, _) = decode_block(&blk(21, &[&rec])).unwrap();
    let Block::Cat021(r) = b else { panic!() };
    assert_eq!(r[0].source, SourceId { sac: 25, sic: 3 });
    // FRN 7 is 0x02 of octet 1 (bit 2), FRN 8 is 0x80 of octet 2: both FX chains.
    // FRN 8 (I021/072, 3 octets) = 0x80 in octet 2.
    let rec = [0x81, 0x80, 0x19, 0x03, 0x00, 0x01, 0x00];
    let (b, _) = decode_block(&blk(21, &[&rec])).unwrap();
    let Block::Cat021(r) = b else { panic!() };
    assert_eq!(r[0].toa_velocity, Some(256));
    // FRN 22 is the first bit of the fourth FSPEC octet: I021/152 magnetic heading, FRN 22 =
    // 0x80 of octet 4.
    let rec = [0x81, 0x01, 0x01, 0x80, 0x19, 0x03, 0x12, 0x34];
    let (b, _) = decode_block(&blk(21, &[&rec])).unwrap();
    let Block::Cat021(r) = b else { panic!() };
    assert_eq!(r[0].magnetic_heading, Some(0x1234));
    // Encoding writes the shortest FSPEC and sets FX only between octets.
    let out = encode_block(&Block::Cat021(r)).unwrap();
    assert_eq!(&out[3..7], &[0x81, 0x01, 0x01, 0x80]);
}

// REQ: OA-AST-013
#[test]
fn unit_constants_and_conversions() {
    assert!((CAT021_POS_DEG - 2.145_767_21e-5).abs() < 1e-12);
    assert!((CAT021_POS_HR_DEG - 1.676_380_634e-7).abs() < 1e-15);
    assert!((CAT062_POS_DEG - 5.364_418e-6).abs() < 1e-11);
    assert_eq!(CAT021_TIME_S, 1.0 / 128.0);
    assert_eq!(CAT021_GEOM_HEIGHT_FT, 6.25);
    // One IAS count is 2^-14 NM/s = 3600 / 16 384 kt, about 0.22 kt.
    assert!((CAT021_GS_NM_S * 3600.0 - 0.2197).abs() < 1e-4);
    // Positions: the LSB is wider than 1e-7 degree for every width up to 30, so the count
    // survives the trip through e-7 degree exactly.
    for bits in [23, 25, 30] {
        for raw in [0, 1, -1, 123_456, -987_654, 293_867_031 >> (30 - bits)] {
            assert_eq!(
                e7_to_raw(raw_to_e7(raw, bits).unwrap(), bits),
                raw,
                "bits {bits}"
            );
        }
    }
    // The 131 count of the hand-built record: 492 633 000 e-7 degree.
    assert_eq!(e7_to_raw(492_633_000, 30), 0x1184_0E17);
    assert_eq!(e7_to_raw(106_142_390, 30), 0x03C6_21B2);
    // The other direction: at most half an LSB of error, about 0.84e-7 degree for 131.
    let back = raw_to_e7(0x1184_0E17, 30).unwrap();
    assert!((back - 492_633_000).abs() <= 1);
    // Times: 1/128 s to milliseconds, rounded.
    assert_eq!(time128_to_ms(128), 1000);
    assert_eq!(time128_to_ms(1), 8); // 7.8125 ms
    assert_eq!(ms_to_time128(1000), 128);
    assert_eq!(ms_to_time128(8), 1);
}
