//! The seeded generator. Core (pure).
//!
//! xoshiro256** 1.0 (Blackman and Vigna, "Scrambled linear pseudorandom number generators",
//! ACM TOMS 2021; reference code <https://prng.di.unimi.it/xoshiro256starstar.c>), seeded with
//! SplitMix64 as its authors recommend (<https://prng.di.unimi.it/splitmix64.c>). Both are
//! written here from the published algorithms; the tests check them against the published
//! output sequences. Uniform doubles take the top 53 bits; normals are Box-Muller.
//!
//! Every stream is a pure function of its seed words, so a simulation run reseeds per
//! (aircraft, scan) and its output for one aircraft does not depend on which others exist.
//! The one platform dependence is the C library's `ln`, `sqrt` and `cos` in [`Rng::normal`],
//! which can differ in the last bit between libm versions (as in oa-truth).

/// SplitMix64 step: advances `state` and returns the next output.
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The SplitMix64 output finaliser as a hash of one word (the step from state `x`).
fn mix(x: u64) -> u64 {
    let mut s = x;
    splitmix64(&mut s)
}

/// xoshiro256** state. Plain data (A1); A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// The generator with state words `s` exactly as given (all-zero is replaced by the
    /// SplitMix64 expansion of 0, since the all-zero state is a fixed point).
    pub fn from_state(s: [u64; 4]) -> Rng {
        if s == [0; 4] {
            return Rng::from_seed(0);
        }
        Rng { s }
    }

    /// The generator whose four state words are the first four SplitMix64 outputs of `seed`.
    pub fn from_seed(seed: u64) -> Rng {
        let mut sm = seed;
        let s = [
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
        ];
        Rng { s }
    }

    /// An independent stream for `(seed, a, b)`: the seed, `a` and `b` are folded through the
    /// SplitMix64 finaliser one after the other and the result seeds [`Rng::from_seed`].
    pub fn from_stream(seed: u64, a: u64, b: u64) -> Rng {
        let h = mix(mix(mix(seed) ^ a) ^ b);
        Rng::from_seed(h)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.s;
        let result = s1.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s1 << 17;
        let s2 = s2 ^ s0;
        let s3 = s3 ^ s1;
        let s1 = s1 ^ s2;
        let s0 = s0 ^ s3;
        let s2 = s2 ^ t;
        let s3 = s3.rotate_left(45);
        self.s = [s0, s1, s2, s3];
        result
    }

    /// Uniform in `[0, 1)` with 53 bits.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in `(0, 1]`, safe to take the logarithm of.
    pub fn unit_open(&mut self) -> f64 {
        1.0 - self.unit()
    }

    /// True with probability `p` (`p` at or below 0 never, at or above 1 always).
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// Standard normal by Box-Muller (one value per call; the sine half is discarded).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.unit_open();
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // REQ: OA-SIM-001
    #[test]
    fn splitmix64_matches_published_sequence() {
        // Seed 1234567: the sequence in the xoshiro/SplitMix reference tests (rand_xoshiro).
        let mut s = 1_234_567u64;
        let want = [
            6_457_827_717_110_365_317u64,
            3_203_168_211_198_807_973,
            9_817_491_932_198_370_423,
            4_593_380_528_125_082_431,
            16_408_922_859_458_223_821,
        ];
        for w in want {
            assert_eq!(splitmix64(&mut s), w);
        }
    }

    // REQ: OA-SIM-001
    #[test]
    fn xoshiro256ss_matches_published_sequence() {
        // State {1, 2, 3, 4}: the sequence in the xoshiro256** reference tests (rand_xoshiro).
        let mut r = Rng::from_state([1, 2, 3, 4]);
        let want = [
            11_520u64,
            0,
            1_509_978_240,
            1_215_971_899_390_074_240,
            1_216_172_134_540_287_360,
            607_988_272_756_665_600,
            16_172_922_978_634_559_625,
            8_476_171_486_693_032_832,
            10_595_114_339_597_558_777,
            2_904_607_092_377_533_576,
        ];
        for w in want {
            assert_eq!(r.next_u64(), w);
        }
    }

    // REQ: OA-SIM-001
    #[test]
    fn streams_are_independent_and_repeatable() {
        let a = Rng::from_stream(7, 1, 2);
        assert_eq!(a, Rng::from_stream(7, 1, 2));
        assert_ne!(a, Rng::from_stream(7, 1, 3));
        assert_ne!(a, Rng::from_stream(7, 2, 2));
        assert_ne!(a, Rng::from_stream(8, 1, 2));
        assert_ne!(Rng::from_stream(7, 1, 2), Rng::from_stream(7, 2, 1));
    }

    // REQ: OA-SIM-001
    #[test]
    fn unit_and_normal_have_the_right_moments() {
        let mut r = Rng::from_seed(42);
        let n = 200_000;
        let (mut su, mut sn, mut sn2) = (0.0, 0.0, 0.0);
        for _ in 0..n {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            su += u;
            let z = r.normal();
            sn += z;
            sn2 += z * z;
        }
        let nf = f64::from(n);
        assert!((su / nf - 0.5).abs() < 0.005);
        assert!((sn / nf).abs() < 0.01);
        assert!((sn2 / nf - 1.0).abs() < 0.02);
    }
}
