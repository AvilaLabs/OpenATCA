//! Helpers shared by the integration tests: a second, independent FSPEC writer and block
//! builder, so expected bytes do not come from the code under test.
#![allow(dead_code)]

/// FSPEC for the given FRNs (ascending): bit 8 first, bit 1 is FX, shortest length.
pub fn fspec(frns: &[u8]) -> Vec<u8> {
    let top = *frns.iter().max().unwrap() as usize;
    let n = (top - 1) / 7 + 1;
    let mut v = vec![0u8; n];
    for &f in frns {
        let i = f as usize - 1;
        v[i / 7] |= 0x80 >> (i % 7);
    }
    for b in v.iter_mut().take(n - 1) {
        *b |= 1;
    }
    v
}

/// A record: FSPEC for the FRNs, then the item bytes in order.
pub fn rec(items: &[(u8, &[u8])]) -> Vec<u8> {
    let frns: Vec<u8> = items.iter().map(|i| i.0).collect();
    let mut v = fspec(&frns);
    for (_, b) in items {
        v.extend_from_slice(b);
    }
    v
}

/// A block of category `cat` holding the records.
pub fn blk(cat: u8, recs: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = recs.iter().flatten().copied().collect();
    let mut v = vec![cat];
    v.extend_from_slice(&((3 + body.len()) as u16).to_be_bytes());
    v.extend_from_slice(&body);
    v
}

use oa_asterix::cat021::*;
use oa_asterix::cat048::*;
use oa_asterix::cat062::*;
use oa_asterix::*;

/// The hand-built records of `handbuilt.rs` (derivations there).
pub const CAT021_HAND: [u8; 23] = [
    0xC3, 0x11, 0x2A, 0x19, 0x03, 0x08, 0x11, 0x84, 0x0E, 0x17, 0x03, 0xC6, 0x21, 0xB2, 0x3C, 0x66,
    0xB0, 0x31, 0xF0, 0x02, 0x00, 0x03, 0xE8,
];
pub const CAT048_HAND: [u8; 35] = [
    0xFF, 0xDA, 0x0A, 0x14, 0x54, 0x60, 0x00, 0x60, 0x0A, 0x00, 0x40, 0x00, 0x0F, 0xC0, 0x05, 0x78,
    0x84, 0x10, 0xFF, 0xAB, 0xCD, 0xEF, 0x10, 0xC2, 0x37, 0x64, 0x18, 0x20, 0x04, 0xD2, 0x32, 0x00,
    0xE7, 0x00, 0x02,
];
pub const CAT062_HAND: [u8; 52] = [
    0x9F, 0x5F, 0x34, 0x14, 0x05, 0x54, 0x60, 0x00, 0x00, 0x77, 0x67, 0xE2, 0xFF, 0x05, 0xF5, 0xAE,
    0x00, 0x4E, 0x20, 0xFF, 0xD8, 0xF0, 0x03, 0x20, 0xFE, 0x70, 0x04, 0x00, 0xC4, 0xA1, 0xB2, 0xC3,
    0x04, 0x13, 0x31, 0xCB, 0x38, 0x20, 0xC5, 0x78, 0x12, 0x34, 0x24, 0xA0, 0x10, 0x0A, 0x05, 0x78,
    0x15, 0xE0, 0x00, 0xF0,
];

const SRC: [u8; 2] = [0x19, 0x03];

/// A CAT021 record of untyped items only (besides the source): one of each item kind.
/// Fixed (015, 016, 008, 132, 260, 400), compound (220, 295), compound with an extended and a
/// repetitive subitem (110), extended (271), repetitive (250), RE and SP.
pub fn cat021_raw_items() -> Vec<(u8, Vec<u8>)> {
    let mut rep250 = vec![0x02];
    rep250.extend(1..=16u8);
    let mut c110 = vec![0xC0, 0x80, 0x02];
    c110.extend((0..30u8).map(|i| i * 3));
    vec![
        (4, vec![0x07]),
        (31, vec![0xF0, 0, 1, 0, 2, 0, 3, 4]),
        (34, c110),
        (35, vec![0x10]),
        (36, vec![0x25]),
        (37, vec![0x21, 0xA0]),
        (38, vec![0xC4]),
        (39, rep250),
        (40, vec![1, 2, 3, 4, 5, 6, 7]),
        (41, vec![0x05]),
        (42, vec![0x81, 0x40, 0x11, 0x22]),
        (48, vec![0x03, 0xAA, 0xBB]),
        (49, vec![0x02, 0x55]),
    ]
}

/// A CAT048 record of untyped items only.
pub fn cat048_raw_items() -> Vec<(u8, Vec<u8>)> {
    let mut c120 = vec![0xC0, 0x00, 0x05, 0x01];
    c120.extend([1, 2, 3, 4, 5, 6]);
    vec![
        (13, vec![0x01, 0x02, 0x03, 0x04]),
        (15, vec![9, 8, 7, 6]),
        (16, vec![0x03, 0x08]),
        (17, vec![0x01, 0x23]),
        (18, vec![0xC0, 0x34, 0x00, 0x56]),
        (19, vec![0x01, 0x00]),
        (20, c120),
        (21, vec![0x20, 0x70]),
        (22, vec![1, 2, 3, 4, 5, 6, 7]),
        (23, vec![0x23]),
        (24, vec![0x0A, 0x52]),
        (25, vec![0x05]),
        (26, vec![0x01, 0x23]),
        (27, vec![0x03, 1, 2]),
        (28, vec![0x02, 0xFF]),
    ]
}

/// A CAT062 record of untyped items only.
pub fn cat062_raw_items() -> Vec<(u8, Vec<u8>)> {
    let mut c390 = vec![0xC1, 0x08, 0xAA, 0xBB, 1, 2, 3, 4, 5, 6, 7, 0x02];
    c390.extend(10..18u8);
    let mut c295 = vec![0x81, 0x01, 0x01, 0x01, 0x40];
    c295.extend([0x11, 0x22]);
    vec![
        (3, vec![0x09]),
        (8, vec![0x01, 0x02]),
        (10, vec![0x40, 1, 2, 3, 4, 5, 6]),
        (15, vec![0x54]),
        (16, c295),
        (21, c390),
        (22, vec![0x11, 0x0A]),
        (23, vec![0x01]),
        (24, vec![0xA0, 0x11, 1, 2, 3, 4, 5, 6]),
        (25, vec![0x07, 0x00]),
        (26, vec![1, 2, 3, 4, 5, 6]),
        (27, vec![0x41, 0x80, 0xAB, 0xCD, 0xEF]),
        (28, vec![0xC0, 0x00, 0x01, 0x00, 0x02, 0x03, 0x04]),
        (34, vec![0x04, 1, 2, 3]),
        (35, vec![0x03, 9, 9]),
    ]
}

/// A record with the source and the raw items, as bytes.
pub fn raw_record(items: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut all: Vec<(u8, &[u8])> = vec![(1, &SRC[..])];
    all.extend(items.iter().map(|(f, b)| (*f, b.as_slice())));
    rec(&all)
}

/// A CAT021 record with every typed item set.
pub fn cat021_full() -> Cat021 {
    Cat021 {
        source: SourceId { sac: 7, sic: 9 },
        descriptor: Some(Trd021 {
            atp: 2,
            arc: 3,
            range_check: true,
            field_monitor: true,
            ext: vec![0x45, 0x20],
        }),
        track_number: Some(4095),
        toa_position: Some(0xFF_FFFF),
        position: Some(RawPosition {
            lat: -3_000_000,
            lon: 8_388_607,
        }),
        position_hr: Some(RawPosition {
            lat: 536_870_912,
            lon: -2_147_483_648,
        }),
        toa_velocity: Some(1),
        air_speed: Some(FlagU15 {
            flag: true,
            raw: 0x7FFF,
        }),
        true_airspeed: Some(FlagU15 {
            flag: false,
            raw: 480,
        }),
        address: Some(0xFF_FFFF),
        tomr_position: Some(77),
        tomr_position_hp: Some(HighPrecisionTime {
            fsi: 2,
            fraction: 0x3FFF_FFFF,
        }),
        tomr_velocity: Some(78),
        tomr_velocity_hp: Some(HighPrecisionTime {
            fsi: 1,
            fraction: 5,
        }),
        geometric_height: Some(-240),
        quality: Some(Quality021 {
            nacv: 7,
            nic: 15,
            ext: vec![0xF1, 0x35, 0x98],
        }),
        mops: Some(Mops {
            not_supported: true,
            version: 2,
            link: 2,
        }),
        mode3a: Some(0o7777),
        roll_angle: Some(-1800),
        flight_level: Some(-60),
        magnetic_heading: Some(0xFFFF),
        target_status: Some(0xFF),
        baro_rate: Some(FlagS15 {
            range_exceeded: true,
            raw: -16384,
        }),
        geometric_rate: Some(FlagS15 {
            range_exceeded: false,
            raw: 16383,
        }),
        ground_vector: Some(GroundVector {
            range_exceeded: true,
            speed: 0x7FFF,
            track: 0xFFFF,
        }),
        track_angle_rate: Some(-512),
        time_transmission: Some(0x80_0000),
        target_id: Some(*b"N123AB  "),
        emitter_category: Some(24),
        selected_altitude: Some(SelectedAltitude {
            source_provided: true,
            source: 3,
            altitude: -4096,
        }),
        final_state: Some(FinalState {
            manage_vertical: true,
            altitude_hold: false,
            approach: true,
            altitude: 4095,
        }),
        raw: cat021_raw_items()
            .into_iter()
            .map(|(frn, bytes)| RawItem { frn, bytes })
            .collect(),
    }
}

/// A CAT048 record with every typed item set.
pub fn cat048_full() -> Cat048 {
    Cat048 {
        source: SourceId { sac: 0xFF, sic: 0 },
        time_of_day: Some(0xFF_FFFF),
        descriptor: Some(Trd048 {
            typ: 7,
            simulated: true,
            rdp_chain2: true,
            spi: true,
            field_monitor: true,
            ext: vec![0x41, 0x80],
        }),
        polar: Some(Polar {
            rho: 0xFFFF,
            theta: 1,
        }),
        mode3a: Some(Mode3a {
            not_validated: true,
            garbled: true,
            l_or_change: true,
            code: 0o7777,
        }),
        flight_level: Some(FlightLevel048 {
            not_validated: true,
            garbled: false,
            raw: -8192,
        }),
        plot: Some(PlotCharacteristics {
            ssr_runlength: Some(255),
            ssr_replies: Some(3),
            ssr_amplitude: Some(-100),
            psr_runlength: Some(0),
            psr_amplitude: Some(127),
            range_diff: Some(-128),
            azimuth_diff: Some(5),
        }),
        address: Some(0x12_3456),
        ident: Some(*b"        "),
        mb_data: Some(vec![
            MbRegister {
                data: [1, 2, 3, 4, 5, 6, 7],
                bds: 0x40,
            },
            MbRegister {
                data: [0xFF; 7],
                bds: 0x60,
            },
        ]),
        track_number: Some(4095),
        cartesian: Some(Cartesian {
            x: -32768,
            y: 32767,
        }),
        track_status: Some(TrackStatus048 {
            tentative: true,
            rad: 2,
            doubtful: true,
            manoeuvre: true,
            cdm: 3,
            ext: vec![0x81, 0x80],
        }),
        raw: cat048_raw_items()
            .into_iter()
            .map(|(frn, bytes)| RawItem { frn, bytes })
            .collect(),
    }
}

/// A CAT062 record with every typed item set.
pub fn cat062_full() -> Cat062 {
    Cat062 {
        source: SourceId { sac: 1, sic: 255 },
        time: Some(0xFF_FFFF),
        position: Some(RawPosition {
            lat: -16_777_216,
            lon: 2_147_483_647,
        }),
        cartesian: Some(CartPosition {
            x: -8_388_608,
            y: 8_388_607,
        }),
        velocity: Some(Velocity {
            vx: -32768,
            vy: 32767,
        }),
        mode3a: Some(Mode3a {
            not_validated: true,
            garbled: true,
            l_or_change: true,
            code: 0o1234,
        }),
        derived: Some(AircraftDerived {
            address: Some(0),
            ident: Some(*b"ABC 1234"),
            selected_altitude: Some(SelectedAltitude {
                source_provided: false,
                source: 0,
                altitude: 0,
            }),
            other: vec![
                RawSub {
                    number: 3,
                    bytes: vec![0x12, 0x34],
                },
                RawSub {
                    number: 8,
                    bytes: vec![0x80],
                },
                RawSub {
                    number: 9,
                    bytes: {
                        let mut v = vec![1];
                        v.extend(0..15u8);
                        v
                    },
                },
                RawSub {
                    number: 28,
                    bytes: vec![0, 5],
                },
            ],
        }),
        track_number: Some(0xFFFF),
        track_status: Some(TrackStatus062 {
            monosensor: true,
            spi: true,
            geometric_more_reliable: true,
            altitude_source: 7,
            tentative: true,
            ext: vec![0x81, 0x01, 0x01, 0x01, 0x01, 0x80],
        }),
        update_ages: Some(UpdateAges {
            track: Some(1),
            psr: Some(2),
            ssr: Some(3),
            mode_s: Some(4),
            ads_c: Some(0x0506),
            ads_b_es: Some(7),
            ads_b_vdl: Some(8),
            ads_b_uat: Some(9),
            loop_age: Some(10),
            mlat: Some(11),
        }),
        measured_fl: Some(-60),
        geometric_altitude: Some(-240),
        baro_altitude: Some(BaroAltitude {
            qnh: true,
            raw: -16384,
        }),
        climb_rate: Some(-320),
        raw: cat062_raw_items()
            .into_iter()
            .map(|(frn, bytes)| RawItem { frn, bytes })
            .collect(),
    }
}

/// Valid blocks of every category: hand-built, raw items only, and every typed item.
pub fn corpus() -> Vec<Vec<u8>> {
    let mut v = vec![
        blk(21, &[CAT021_HAND.to_vec()]),
        blk(48, &[CAT048_HAND.to_vec()]),
        blk(62, &[CAT062_HAND.to_vec()]),
        blk(21, &[raw_record(&cat021_raw_items())]),
        blk(48, &[raw_record(&cat048_raw_items())]),
        blk(62, &[raw_record(&cat062_raw_items())]),
    ];
    v.push(encode_block(&Block::Cat021(vec![cat021_full(), Cat021::default()])).unwrap());
    v.push(encode_block(&Block::Cat048(vec![cat048_full(), Cat048::default()])).unwrap());
    v.push(encode_block(&Block::Cat062(vec![cat062_full(), Cat062::default()])).unwrap());
    v
}
