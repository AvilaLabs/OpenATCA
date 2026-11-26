//! Round-trip properties over generated and mutated messages: whatever [`parse`] accepts,
//! [`encode`] reproduces byte for byte, and whatever [`encode`] produces parses back to the same
//! value. The generator is a fixed-seed xorshift: no dependency, reproducible.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SEEDS: &[&str] = &[
    "(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120A070 CYSC DCT YSC V447 MPV DCT KMPV-KMPV0053-EET/KZBW0003)",
    "(CHGKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD1905-MMEX-DOF/121115-10/SFGHRWXZ/SB2-18/PBN/D2 NAV/RNVD1E2A1 EET/MMZT0023 MMEX0057 RMK/TCAS EQUIPPED)",
    "(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW-07/AAL73)",
    "(ESTKZMP/CZWG992KZMP/CZWG991-DAL122/A4322-KFAR-HML/2042F350-CYOW)",
    "(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)",
    "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX-PBN/D2 NAV/RNVD1E2A1 DOF/121130)",
    "(ABIKZWY/CZQM005-IBE6175/A2537-LEMD-41N040W/0700F330-CYUL-9/B744/H-15/M084F350 41N030W 41N040W 41N050W 40N060W 38N065W DANER A699 MAPLE DCT GOALL)",
    "(MISCZQM/KZBW999-/S1-RMK/COMBINING SECTOR 21 INTO 22 AT 1415Z)",
    "(IRSKZBW/CZQM232CZQM/KZBW491)",
    "(TRQCZVR/KZSE491-RMK/SHUTDOWN FOR SOFTWARE CHANGE)",
    "(LRMKZLC/CZWG035CZWG/KZLC021-RMK/06/07/AAL98295)",
    "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-13242934462034N0780521WN043327629F349)",
    "(RLAKZBW/CZUL202CZUL/KZBW445-KBTV1A)",
    "(RTAKZMP/CZWG222KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-KZMP42)",
    "(POJCZWG/KZMP438KZMP/CZWG812-DLH499-KSLC-KBUF-CZWGBB)",
    "(TOCKZAK/CZVR115KZAK/CZVR278-QFA135/A2217-PHNL0344-CZVR)",
];

// REQ: OA-NAM-002
#[test]
fn parse_then_encode_is_byte_exact_on_every_seed() {
    for s in SEEDS {
        let m = parse(s.as_bytes()).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(encode(&m).unwrap(), s.as_bytes(), "{s}");
    }
}

// REQ: OA-NAM-002
#[test]
fn byte_mutations_never_break_the_round_trip_or_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let alphabet = b"()-/ 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+.";
    let (mut accepted, mut refused) = (0u32, 0u32);
    for _ in 0..60_000 {
        let mut b = SEEDS[rng.below(SEEDS.len())].as_bytes().to_vec();
        for _ in 0..=rng.below(3) {
            match rng.below(4) {
                0 if !b.is_empty() => {
                    let i = rng.below(b.len());
                    b[i] = alphabet[rng.below(alphabet.len())];
                }
                1 if !b.is_empty() => {
                    let i = rng.below(b.len());
                    b.remove(i);
                }
                2 => {
                    let i = rng.below(b.len() + 1);
                    b.insert(i, alphabet[rng.below(alphabet.len())]);
                }
                _ => {
                    let n = rng.below(b.len() + 1);
                    b.truncate(n);
                }
            }
        }
        match parse(&b) {
            Ok(m) => {
                accepted += 1;
                let e =
                    encode(&m).unwrap_or_else(|e| panic!("{}: {e}", String::from_utf8_lossy(&b)));
                assert_eq!(e, b, "{}", String::from_utf8_lossy(&b));
                assert_eq!(parse(&e).unwrap(), m);
            }
            Err(_) => refused += 1,
        }
        // The tolerant parser never panics either, and when it accepts, its value encodes and
        // parses back.
        if let Ok((m, _)) = parse_tolerant(&b) {
            assert_eq!(parse(&encode(&m).unwrap()).unwrap(), m);
        }
    }
    assert!(accepted > 100 && refused > 1000, "{accepted} {refused}");
}

// REQ: OA-NAM-002, OA-NAM-006
#[test]
fn arbitrary_bytes_never_panic() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..20_000 {
        let n = rng.below(80);
        let b: Vec<u8> = (0..n).map(|_| (rng.next() & 0xFF) as u8).collect();
        let _ = parse(&b);
        let _ = parse_tolerant(&b);
        let _ = frame::unframe(&b);
    }
}

// REQ: OA-NAM-001, OA-NAM-002
#[test]
fn every_kind_has_a_spec_and_a_mnemonic_that_parses_back() {
    for k in ALL_KINDS {
        assert_eq!(
            schema::kind_of(schema::spec(k).unwrap().mnemonic.as_bytes()),
            Some(k)
        );
    }
    assert_eq!(ALL_KINDS.len(), 26);
}
