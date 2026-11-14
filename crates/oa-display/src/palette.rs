//! The fixed palette (**core**, B1). A primitive names a colour by index; the index is checked
//! against [`PALETTE_LEN`] when a primitive is pushed and again before a list is drawn. The table is
//! documented in the crate README section (`lib.rs`).

/// Number of palette entries. Index 0 is the background.
pub const PALETTE_LEN: usize = 16;

pub const BACKGROUND: u8 = 0;
pub const TEXT: u8 = 1;
pub const ARTCC_HIGH: u8 = 2;
pub const ARTCC_LOW: u8 = 3;
pub const AIRWAY: u8 = 4;
pub const FIX: u8 = 5;
pub const AIRPORT: u8 = 6;
pub const RING: u8 = 7;
pub const DIM_TEXT: u8 = 8;
/// Reserved for alerts (conflict, MSAW); nothing in the static map uses it.
pub const ALERT: u8 = 9;

/// XRGB8888 values: `0x00RRGGBB`. Entries 10 to 15 are reserved and black.
pub const PALETTE: [u32; PALETTE_LEN] = [
    0x000B_0F14, // background
    0x00E8_EDF2, // text
    0x0056_B4E9, // ARTCC HIGH (Okabe-Ito sky blue)
    0x00E6_9F00, // ARTCC LOW (Okabe-Ito orange)
    0x008A_97A6, // airway
    0x00F0_E442, // fix, navaid (Okabe-Ito yellow)
    0x00F2_F5F8, // airport (white, apart from the grey airway under every colour-vision type)
    0x002F_3B48, // range ring
    0x007E_8B99, // dim text
    0x00FF_5A4F, // alert
    0,
    0,
    0,
    0,
    0,
    0,
];

/// The four bytes of palette entry `i` as stored in an XRGB8888 buffer on a little-endian
/// machine (the byte order of a KMS dumb buffer): blue, green, red, unused. `None` for an index
/// outside the palette.
pub fn bytes_of(i: u8) -> Option<[u8; 4]> {
    PALETTE.get(usize::from(i)).map(|c| c.to_le_bytes())
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

    fn rgb(i: u8) -> [f64; 3] {
        let c = PALETTE[usize::from(i)];
        [
            f64::from((c >> 16) & 255),
            f64::from((c >> 8) & 255),
            f64::from(c & 255),
        ]
    }

    fn linear(v: f64) -> f64 {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    fn gamma(v: f64) -> f64 {
        let v = v.clamp(0.0, 1.0);
        255.0
            * if v <= 0.003_130_8 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
    }

    fn luminance(i: u8) -> f64 {
        let [r, g, b] = rgb(i);
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    fn contrast(a: u8, b: u8) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// The colour as seen with one kind of dichromacy (Machado, Oliveira and Fernandes 2009,
    /// severity 1.0, in linear RGB), back in sRGB.
    fn simulate(i: u8, m: &[[f64; 3]; 3]) -> [f64; 3] {
        let c = rgb(i).map(linear);
        let mut out = [0.0; 3];
        for (o, row) in out.iter_mut().zip(m) {
            *o = gamma(row[0] * c[0] + row[1] * c[1] + row[2] * c[2]);
        }
        out
    }

    fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    // REQ: OA-DSP-016
    #[test]
    fn every_foreground_colour_has_aa_contrast_on_the_background() {
        for (name, i) in [
            ("text", TEXT),
            ("ARTCC HIGH", ARTCC_HIGH),
            ("ARTCC LOW", ARTCC_LOW),
            ("airway", AIRWAY),
            ("fix", FIX),
            ("airport", AIRPORT),
            ("dim text", DIM_TEXT),
            ("alert", ALERT),
        ] {
            let c = contrast(BACKGROUND, i);
            assert!(c >= 4.5, "{name} has contrast {c:.2}:1");
        }
        // The rings are decoration, not data: visible but quiet.
        let c = contrast(BACKGROUND, RING);
        assert!((1.3..3.0).contains(&c), "ring contrast {c:.2}:1");
        // The background is dark.
        assert!(luminance(BACKGROUND) < 0.01);
    }

    // REQ: OA-DSP-016
    #[test]
    fn data_colours_stay_apart_under_every_form_of_colour_blindness() {
        const MIN_APART: f64 = 55.0;
        let protan = [
            [0.152_286, 1.052_583, -0.204_868],
            [0.114_503, 0.786_281, 0.099_216],
            [-0.003_882, -0.048_116, 1.051_998],
        ];
        let deutan = [
            [0.367_322, 0.860_646, -0.227_968],
            [0.280_085, 0.672_501, 0.047_413],
            [-0.011_820, 0.042_940, 0.968_881],
        ];
        let tritan = [
            [1.255_528, -0.076_749, -0.178_779],
            [-0.078_411, 0.930_809, 0.147_602],
            [0.004_733, 0.691_367, 0.303_900],
        ];
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let data = [ARTCC_HIGH, ARTCC_LOW, AIRWAY, FIX, AIRPORT, ALERT];
        for (kind, m) in [
            ("normal", identity),
            ("protan", protan),
            ("deutan", deutan),
            ("tritan", tritan),
        ] {
            let mut worst = f64::MAX;
            let mut pair = (0, 0);
            for (k, &a) in data.iter().enumerate() {
                for &b in &data[k + 1..] {
                    let d = dist(simulate(a, &m), simulate(b, &m));
                    if d < worst {
                        worst = d;
                        pair = (a, b);
                    }
                }
            }
            eprintln!("{kind}: closest pair {pair:?} is {worst:.0} apart");
            assert!(
                worst >= MIN_APART,
                "{kind}: colours {pair:?} are only {worst:.0} apart"
            );
        }
        // The two boundary colours, which share a line style, differ in more than hue: they must
        // differ clearly in plain RGB distance too.
        assert!(dist(rgb(ARTCC_HIGH), rgb(ARTCC_LOW)) > 120.0);
    }

    // REQ: OA-DSP-016
    #[test]
    fn palette_bytes_are_blue_green_red_unused() {
        assert_eq!(bytes_of(ARTCC_HIGH), Some([0xE9, 0xB4, 0x56, 0x00]));
        assert_eq!(bytes_of(PALETTE_LEN as u8), None);
        assert!(
            PALETTE.iter().all(|c| c >> 24 == 0),
            "the unused byte is zero"
        );
    }
}
