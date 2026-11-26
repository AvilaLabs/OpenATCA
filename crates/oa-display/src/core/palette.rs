//! The fixed palette (**core**, B1). A primitive names a colour by index; the index is checked
//! against [`PALETTE_LEN`] when a primitive is pushed and again before a list is drawn. The table is
//! documented in the crate README section (`lib.rs`).

/// Number of palette entries. Index 0 is the background.
pub const PALETTE_LEN: usize = 16;

/// One exact sRGB entry from FAA-HF-STD-010A Tables 5.13.1-I or 5.13.2-I.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SrgbColor {
    pub name: &'static str,
    pub rgb: [u8; 3],
}

/// FAA-HF-STD-010A Table 5.13.1-I defaults for critical foreground objects viewed below 40 lux.
/// The names are the standard's suggestions, not required operational meanings.
pub const FAA_HF_STD_010A_FOREGROUND: [SrgbColor; 11] = [
    SrgbColor {
        name: "White",
        rgb: [255, 255, 255],
    },
    SrgbColor {
        name: "Pink",
        rgb: [246, 132, 216],
    },
    SrgbColor {
        name: "Gray",
        rgb: [179, 179, 179],
    },
    SrgbColor {
        name: "Blue",
        rgb: [94, 141, 246],
    },
    SrgbColor {
        name: "Orange",
        rgb: [254, 147, 13],
    },
    SrgbColor {
        name: "Red",
        rgb: [255, 19, 32],
    },
    SrgbColor {
        name: "Green",
        rgb: [35, 225, 98],
    },
    SrgbColor {
        name: "Yellow",
        rgb: [223, 243, 52],
    },
    SrgbColor {
        name: "Magenta",
        rgb: [216, 34, 255],
    },
    SrgbColor {
        name: "Aqua",
        rgb: [7, 205, 237],
    },
    SrgbColor {
        name: "Brown",
        rgb: [197, 149, 91],
    },
];

/// FAA-HF-STD-010A Table 5.13.2-I defaults for weather severity viewed below 40 lux.
/// Severity levels 1/2, 3/4, and 5/6 require pattern coding if all seven levels are shown.
pub const FAA_HF_STD_010A_WEATHER: [SrgbColor; 4] = [
    SrgbColor {
        name: "Black",
        rgb: [0, 0, 0],
    },
    SrgbColor {
        name: "Wx-Green",
        rgb: [23, 57, 40],
    },
    SrgbColor {
        name: "Wx-Yellow",
        rgb: [90, 74, 20],
    },
    SrgbColor {
        name: "Wx-Red",
        rgb: [93, 46, 89],
    },
];

/// sRGB software request that a shell may translate to the workstation's display command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwarePresetCommand {
    SelectSrgbPreset,
}

/// The operator sRGB-preset action is represented as data for the impure display shell to send.
pub const fn operator_srgb_preset_command() -> HardwarePresetCommand {
    HardwarePresetCommand::SelectSrgbPreset
}

pub const BACKGROUND: u8 = 0;
pub const TEXT: u8 = 1;
pub const ARTCC_HIGH: u8 = 2;
pub const ARTCC_LOW: u8 = 3;
pub const AIRWAY: u8 = 4;
pub const FIX: u8 = 5;
pub const AIRPORT: u8 = 6;
pub const RING: u8 = 7;
pub const DIM_TEXT: u8 = 8;
/// Reserved for conditions that need attention (a conflict, MSAW, data the map could not place),
/// never for expected data.
pub const ALERT: u8 = 9;
/// Boundaries of neighbouring facilities: the same hues as the own boundary, dimmer.
pub const NEIGHBOUR_HIGH: u8 = 10;
pub const NEIGHBOUR_LOW: u8 = 11;
/// Live targets: the position symbol, velocity leader and data block (light mint).
pub const TARGET: u8 = 12;

/// XRGB8888 values: `0x00RRGGBB`. Entries 13 to 15 are reserved and black.
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
    0x002F_6F96, // neighbour ARTCC HIGH (dim sky blue)
    0x009A_6B00, // neighbour ARTCC LOW (dim orange)
    0x007F_E0B0, // target (light mint: apart from every data colour under all three deficiencies)
    0,
    0,
    0,
];

/// The four bytes of palette entry `i` as stored in an XRGB8888 buffer on a little-endian
/// machine (the byte order of a KMS dumb buffer): blue, green, red, unused. `None` for an index
/// outside the palette.
pub fn bytes_of(i: u8) -> Option<[u8; 4]> {
    PALETTE.get(usize::from(i)).map(|c| u32::to_le_bytes(*c))
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

    fn xyz(rgb: [u8; 3], peak_white_cd_m2: f64) -> [f64; 3] {
        let [r, g, b] = rgb.map(|channel| linear(f64::from(channel)));
        // The published sRGB-to-XYZ matrix is rounded. Its X/Z row sums already equal
        // the D65 whitepoint values; normalize only Y, whose row sum is 1.0000001.
        let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
        let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
        let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
        [
            x * peak_white_cd_m2,
            (y * (1.0 / (0.212_672_9 + 0.715_152_2 + 0.072_175_0))) * peak_white_cd_m2,
            z * peak_white_cd_m2,
        ]
    }

    fn lab_f(value: f64) -> f64 {
        const DELTA: f64 = 6.0 / 29.0;
        if value > DELTA.powi(3) {
            value.cbrt()
        } else {
            value / (3.0 * DELTA.powi(2)) + 4.0 / 29.0
        }
    }

    fn cielab(rgb: [u8; 3], peak_white_cd_m2: f64) -> [f64; 3] {
        let [x, y, z] = xyz(rgb, peak_white_cd_m2);
        let fx = lab_f(x / (0.950_47 * peak_white_cd_m2));
        let fy = lab_f(y / peak_white_cd_m2);
        let fz = lab_f(z / (1.088_83 * peak_white_cd_m2));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    fn cieluv(rgb: [u8; 3], peak_white_cd_m2: f64) -> [f64; 3] {
        let [x, y, z] = xyz(rgb, peak_white_cd_m2);
        let denominator = x + 15.0 * y + 3.0 * z;
        let (u_prime, v_prime) = if denominator == 0.0 {
            (0.0, 0.0)
        } else {
            (4.0 * x / denominator, 9.0 * y / denominator)
        };
        let relative_y = y / peak_white_cd_m2;
        let lightness = if relative_y > (6.0_f64 / 29.0).powi(3) {
            116.0 * relative_y.cbrt() - 16.0
        } else {
            (29.0_f64 / 3.0).powi(3) * relative_y
        };
        // FAA-HF-STD-010A §5.7 specifies these four-decimal D65 reference coordinates.
        // They intentionally differ slightly from u'v' derived from the full-precision XYZ white.
        const FAA_D65_U_PRIME_N: f64 = 0.1978;
        const FAA_D65_V_PRIME_N: f64 = 0.4683;
        [
            lightness,
            13.0 * lightness * (u_prime - FAA_D65_U_PRIME_N),
            13.0 * lightness * (v_prime - FAA_D65_V_PRIME_N),
        ]
    }

    fn uv_size_coefficients(angle_arcmin: f64) -> Option<[f64; 3]> {
        if !angle_arcmin.is_finite() || angle_arcmin <= 0.0 {
            return None;
        }
        if angle_arcmin >= 60.0 {
            return Some([1.0; 3]);
        }
        let kl = 1.0366 - (0.15263 - 0.05766 * angle_arcmin).exp();
        let ku = if angle_arcmin <= 32.0 {
            0.008991 * angle_arcmin - 0.0065
        } else {
            0.0257 * angle_arcmin - 0.5403
        };
        let kv = if angle_arcmin <= 32.0 {
            0.005446 * angle_arcmin - 0.042
        } else {
            0.031 * angle_arcmin - 0.8594
        };
        Some([kl, ku, kv])
    }

    fn delta_uv_sc(a: [u8; 3], b: [u8; 3], peak_white_cd_m2: f64, angle_arcmin: f64) -> f64 {
        let left = cieluv(a, peak_white_cd_m2);
        let right = cieluv(b, peak_white_cd_m2);
        let [kl, ku, kv] = uv_size_coefficients(angle_arcmin).unwrap();
        ((kl * (left[0] - right[0])).powi(2)
            + (ku * (left[1] - right[1])).powi(2)
            + (kv * (left[2] - right[2])).powi(2))
        .sqrt()
    }

    fn delta_e_ab(a: [u8; 3], b: [u8; 3], peak_white_cd_m2: f64) -> f64 {
        dist(cielab(a, peak_white_cd_m2), cielab(b, peak_white_cd_m2))
    }

    fn minimum_pairwise<T>(colors: &[T], mut distance: impl FnMut(&T, &T) -> f64) -> f64 {
        let mut minimum = f64::MAX;
        for (index, left) in colors.iter().enumerate() {
            for right in &colors[index + 1..] {
                minimum = minimum.min(distance(left, right));
            }
        }
        minimum
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
            ("target", TARGET),
        ] {
            let c = contrast(BACKGROUND, i);
            assert!(c >= 4.5, "{name} has contrast {c:.2}:1");
        }
        // Neighbouring boundaries are dimmer than the own boundary but still plainly visible.
        for i in [NEIGHBOUR_HIGH, NEIGHBOUR_LOW] {
            assert!(contrast(BACKGROUND, i) >= 3.0);
        }
        assert!(luminance(NEIGHBOUR_HIGH) < luminance(ARTCC_HIGH) * 0.6);
        assert!(luminance(NEIGHBOUR_LOW) < luminance(ARTCC_LOW) * 0.6);
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
        let data = [
            ARTCC_HIGH,
            ARTCC_LOW,
            NEIGHBOUR_HIGH,
            NEIGHBOUR_LOW,
            AIRWAY,
            TARGET,
            FIX,
            AIRPORT,
            ALERT,
        ];
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

    // REQ: OA-DSP-016
    #[test]
    fn faa_hf_std_010a_palette_values_match_the_published_tables() {
        let foreground = [
            ("White", [255, 255, 255]),
            ("Pink", [246, 132, 216]),
            ("Gray", [179, 179, 179]),
            ("Blue", [94, 141, 246]),
            ("Orange", [254, 147, 13]),
            ("Red", [255, 19, 32]),
            ("Green", [35, 225, 98]),
            ("Yellow", [223, 243, 52]),
            ("Magenta", [216, 34, 255]),
            ("Aqua", [7, 205, 237]),
            ("Brown", [197, 149, 91]),
        ];
        let weather = [
            ("Black", [0, 0, 0]),
            ("Wx-Green", [23, 57, 40]),
            ("Wx-Yellow", [90, 74, 20]),
            ("Wx-Red", [93, 46, 89]),
        ];
        for (actual, (name, rgb)) in FAA_HF_STD_010A_FOREGROUND.iter().zip(foreground) {
            assert_eq!((actual.name, actual.rgb), (name, rgb));
        }
        for (actual, (name, rgb)) in FAA_HF_STD_010A_WEATHER.iter().zip(weather) {
            assert_eq!((actual.name, actual.rgb), (name, rgb));
        }
        assert_eq!(
            operator_srgb_preset_command(),
            HardwarePresetCommand::SelectSrgbPreset
        );
    }

    // REQ: OA-DSP-016
    #[test]
    fn faa_difference_math_uses_cieluv_size_correction_and_cielab_thresholds() {
        // White/black vectors anchor the CIE conversions. 200 cd/m² is the standard's
        // minimum peak-white hardware requirement; ambient reflection remains a measured-device input.
        let white = [255, 255, 255];
        let black = [0, 0, 0];
        // CIE D65's full-precision XYZ reference white. The independent chromaticity
        // calculation below checks neutral RGB against the matrix whitepoint, while
        // CIELUV still uses FAA §5.7's mandated four-decimal u'n/v'n values.
        let d65_xyz = [0.950_47, 1.0, 1.088_83];
        let d65_denominator = d65_xyz[0] + 15.0 * d65_xyz[1] + 3.0 * d65_xyz[2];
        let d65_u_prime = 4.0 * d65_xyz[0] / d65_denominator;
        let d65_v_prime = 9.0 * d65_xyz[1] / d65_denominator;
        let [gray_x, gray_y, gray_z] = xyz([128, 128, 128], 200.0);
        let gray_denominator = gray_x + 15.0 * gray_y + 3.0 * gray_z;
        assert!((gray_y / 200.0 - linear(128.0)).abs() < 1.0e-14);
        assert!((4.0 * gray_x / gray_denominator - d65_u_prime).abs() < 1.0e-15);
        assert!((9.0 * gray_y / gray_denominator - d65_v_prime).abs() < 1.0e-15);
        assert!((delta_e_ab(white, black, 200.0) - 100.0).abs() < 1.0e-8);
        let white_luv = cieluv(white, 200.0);
        let expected_white_luv = [
            100.0,
            1300.0 * (d65_u_prime - 0.1978),
            1300.0 * (d65_v_prime - 0.4683),
        ];
        for (actual, expected) in white_luv.into_iter().zip(expected_white_luv) {
            assert!((actual - expected).abs() < 1.0e-12);
        }
        let expected_white_black_delta =
            (100.0_f64.powi(2) + expected_white_luv[1].powi(2) + expected_white_luv[2].powi(2))
                .sqrt();
        let white_black_delta = delta_uv_sc(white, black, 200.0, 60.0);
        assert!(white_black_delta > 100.0);
        assert!((white_black_delta - expected_white_black_delta).abs() < 1.0e-12);
        assert_eq!(uv_size_coefficients(60.0), Some([1.0; 3]));
        assert_eq!(uv_size_coefficients(0.0), None);
        assert_eq!(uv_size_coefficients(f64::NAN), None);

        // The standard's palette uses one pixel/luminance encoding for the role list; checking
        // all pairs is conservative where the eventual scene's adjacency/search map is unavailable.
        // These ideal-sRGB software calculations do not replace the required ambient/display measurements.
        let foreground_rgb: Vec<[u8; 3]> = FAA_HF_STD_010A_FOREGROUND
            .iter()
            .map(|entry| entry.rgb)
            .collect();
        let min_search = minimum_pairwise(&foreground_rgb, |a, b| delta_uv_sc(*a, *b, 200.0, 60.0));
        let min_adjacent = minimum_pairwise(&foreground_rgb, |a, b| delta_e_ab(*a, *b, 200.0));
        assert!(min_search >= 28.0, "minimum ΔE*uv-sc is {min_search}");
        assert!(min_adjacent >= 9.9, "minimum ΔE*ab is {min_adjacent}");
    }
}
