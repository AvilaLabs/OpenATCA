//! The built-in stroke font (**core**, B1): every character is a few line strokes on a small
//! integer grid, defined here as const data. There is no font engine and no outline curve; what is
//! drawn is exactly what is written below.
//!
//! The grid is 5 columns by 7 rows: x 0 to 4, y 0 to 6 from the top, so a capital is 4 units wide
//! and 6 tall (cap height). A stroke is `[x0, y0, x1, y1]`. `,` dips to y 7. The advance from one
//! character to the next is [`ADVANCE`] units. A glyph is scaled so that [`CAP_HEIGHT`] units are
//! the text's size in pixels.
//!
//! Characters that are easy to confuse look clearly different: zero is slashed, one has a flag and
//! no foot while the letter I has serifs top and bottom, S and 5, B and 8, Z and 2 differ in
//! their corners and curves. The test `confusable_pairs_differ_by_at_least_the_stated_amount`
//! checks every such pair mechanically.
//!
//! Lowercase letters draw as the capitals. A character with no glyph draws as [`MISSING`], a
//! crossed box, so an unsupported character is visible and never silently dropped.

/// One stroke: `[x0, y0, x1, y1]` in glyph units.
pub type Stroke = [u8; 4];

/// Grid units from the start of one character to the start of the next.
pub const ADVANCE: i32 = 6;
/// Cap height in grid units; text size in pixels maps to this.
pub const CAP_HEIGHT: i32 = 6;
/// Glyph width in grid units (x runs 0 to this).
pub const WIDTH: i32 = 4;
/// Lowest row any glyph uses.
pub const MAX_Y: u8 = 7;

/// A character and its strokes.
// A7.2: cold struct, size guard waived (const data).
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub ch: char,
    pub strokes: &'static [Stroke],
}

/// Drawn for any character without a glyph.
pub const MISSING: &[Stroke] = &[
    [0, 0, 4, 0],
    [4, 0, 4, 6],
    [4, 6, 0, 6],
    [0, 6, 0, 0],
    [0, 0, 4, 6],
    [4, 0, 0, 6],
];

const O_RING: [Stroke; 8] = [
    [1, 0, 3, 0],
    [3, 0, 4, 1],
    [4, 1, 4, 5],
    [4, 5, 3, 6],
    [3, 6, 1, 6],
    [1, 6, 0, 5],
    [0, 5, 0, 1],
    [0, 1, 1, 0],
];

/// Every glyph, sorted by character so a lookup is a binary search (checked by a test).
pub const GLYPHS: &[Glyph] = &[
    Glyph {
        ch: ' ',
        strokes: &[],
    },
    Glyph {
        ch: '#',
        strokes: &[[1, 0, 1, 6], [3, 0, 3, 6], [0, 2, 4, 2], [0, 4, 4, 4]],
    },
    Glyph {
        ch: '(',
        strokes: &[[3, 0, 2, 1], [2, 1, 2, 5], [2, 5, 3, 6]],
    },
    Glyph {
        ch: ')',
        strokes: &[[1, 0, 2, 1], [2, 1, 2, 5], [2, 5, 1, 6]],
    },
    Glyph {
        ch: '*',
        strokes: &[[2, 1, 2, 5], [0, 2, 4, 4], [4, 2, 0, 4]],
    },
    Glyph {
        ch: '+',
        strokes: &[[1, 3, 3, 3], [2, 1, 2, 5]],
    },
    Glyph {
        ch: ',',
        strokes: &[[2, 5, 2, 6], [2, 6, 1, 7]],
    },
    Glyph {
        ch: '-',
        strokes: &[[1, 3, 3, 3]],
    },
    Glyph {
        ch: '.',
        strokes: &[[2, 5, 2, 6]],
    },
    Glyph {
        ch: '/',
        strokes: &[[0, 6, 4, 0]],
    },
    Glyph {
        ch: '0',
        strokes: &[
            [1, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 5],
            [4, 5, 3, 6],
            [3, 6, 1, 6],
            [1, 6, 0, 5],
            [0, 5, 0, 1],
            [0, 1, 1, 0],
            [1, 5, 3, 1],
        ],
    },
    Glyph {
        ch: '1',
        strokes: &[[0, 2, 2, 0], [2, 0, 2, 6]],
    },
    Glyph {
        ch: '2',
        strokes: &[
            [0, 1, 1, 0],
            [1, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 2],
            [4, 2, 0, 6],
            [0, 6, 4, 6],
        ],
    },
    Glyph {
        ch: '3',
        strokes: &[
            [0, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 2],
            [4, 2, 3, 3],
            [1, 3, 3, 3],
            [3, 3, 4, 4],
            [4, 4, 4, 5],
            [4, 5, 3, 6],
            [3, 6, 0, 6],
        ],
    },
    Glyph {
        ch: '4',
        strokes: &[[3, 6, 3, 0], [3, 0, 0, 4], [0, 4, 4, 4]],
    },
    Glyph {
        ch: '5',
        strokes: &[
            [4, 0, 0, 0],
            [0, 0, 0, 3],
            [0, 3, 3, 3],
            [3, 3, 4, 4],
            [4, 4, 4, 5],
            [4, 5, 3, 6],
            [3, 6, 0, 6],
        ],
    },
    Glyph {
        ch: '6',
        strokes: &[
            [4, 0, 1, 0],
            [1, 0, 0, 1],
            [0, 1, 0, 5],
            [0, 5, 1, 6],
            [1, 6, 3, 6],
            [3, 6, 4, 5],
            [4, 5, 4, 4],
            [4, 4, 3, 3],
            [3, 3, 0, 3],
        ],
    },
    Glyph {
        ch: '7',
        strokes: &[[0, 0, 4, 0], [4, 0, 1, 6]],
    },
    Glyph {
        ch: '8',
        strokes: &[
            [1, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 2],
            [4, 2, 3, 3],
            [3, 3, 1, 3],
            [1, 3, 0, 2],
            [0, 2, 0, 1],
            [0, 1, 1, 0],
            [1, 3, 0, 4],
            [0, 4, 0, 5],
            [0, 5, 1, 6],
            [1, 6, 3, 6],
            [3, 6, 4, 5],
            [4, 5, 4, 4],
            [4, 4, 3, 3],
        ],
    },
    Glyph {
        ch: '9',
        strokes: &[
            [0, 5, 1, 6],
            [1, 6, 3, 6],
            [3, 6, 4, 5],
            [4, 5, 4, 1],
            [4, 1, 3, 0],
            [3, 0, 1, 0],
            [1, 0, 0, 1],
            [0, 1, 0, 2],
            [0, 2, 1, 3],
            [1, 3, 4, 3],
        ],
    },
    Glyph {
        ch: ':',
        strokes: &[[2, 1, 2, 2], [2, 4, 2, 5]],
    },
    Glyph {
        ch: '<',
        strokes: &[[4, 1, 0, 3], [0, 3, 4, 5]],
    },
    Glyph {
        ch: '>',
        strokes: &[[0, 1, 4, 3], [4, 3, 0, 5]],
    },
    Glyph {
        ch: 'A',
        strokes: &[[0, 6, 2, 0], [2, 0, 4, 6], [1, 4, 3, 4]],
    },
    Glyph {
        ch: 'B',
        strokes: &[
            [0, 0, 0, 6],
            [0, 0, 2, 0],
            [2, 0, 3, 1],
            [3, 1, 3, 2],
            [3, 2, 2, 3],
            [0, 3, 3, 3],
            [3, 3, 4, 4],
            [4, 4, 4, 5],
            [4, 5, 3, 6],
            [0, 6, 3, 6],
        ],
    },
    Glyph {
        ch: 'C',
        strokes: &[
            [4, 1, 3, 0],
            [3, 0, 1, 0],
            [1, 0, 0, 1],
            [0, 1, 0, 5],
            [0, 5, 1, 6],
            [1, 6, 3, 6],
            [3, 6, 4, 5],
        ],
    },
    Glyph {
        ch: 'D',
        strokes: &[
            [0, 0, 0, 6],
            [0, 0, 2, 0],
            [2, 0, 4, 2],
            [4, 2, 4, 4],
            [4, 4, 2, 6],
            [2, 6, 0, 6],
        ],
    },
    Glyph {
        ch: 'E',
        strokes: &[[4, 0, 0, 0], [0, 0, 0, 6], [0, 6, 4, 6], [0, 3, 3, 3]],
    },
    Glyph {
        ch: 'F',
        strokes: &[[4, 0, 0, 0], [0, 0, 0, 6], [0, 3, 3, 3]],
    },
    Glyph {
        ch: 'G',
        strokes: &[
            [4, 1, 3, 0],
            [3, 0, 1, 0],
            [1, 0, 0, 1],
            [0, 1, 0, 5],
            [0, 5, 1, 6],
            [1, 6, 3, 6],
            [3, 6, 4, 5],
            [4, 5, 4, 3],
            [4, 3, 2, 3],
        ],
    },
    Glyph {
        ch: 'H',
        strokes: &[[0, 0, 0, 6], [4, 0, 4, 6], [0, 3, 4, 3]],
    },
    Glyph {
        ch: 'I',
        strokes: &[[1, 0, 3, 0], [2, 0, 2, 6], [1, 6, 3, 6]],
    },
    Glyph {
        ch: 'J',
        strokes: &[[4, 0, 4, 5], [4, 5, 3, 6], [3, 6, 1, 6], [1, 6, 0, 5]],
    },
    Glyph {
        ch: 'K',
        strokes: &[[0, 0, 0, 6], [4, 0, 0, 3], [0, 3, 4, 6]],
    },
    Glyph {
        ch: 'L',
        strokes: &[[0, 0, 0, 6], [0, 6, 4, 6]],
    },
    Glyph {
        ch: 'M',
        strokes: &[[0, 6, 0, 0], [0, 0, 2, 3], [2, 3, 4, 0], [4, 0, 4, 6]],
    },
    Glyph {
        ch: 'N',
        strokes: &[[0, 6, 0, 0], [0, 0, 4, 6], [4, 6, 4, 0]],
    },
    Glyph {
        ch: 'O',
        strokes: &O_RING,
    },
    Glyph {
        ch: 'P',
        strokes: &[
            [0, 6, 0, 0],
            [0, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 2],
            [4, 2, 3, 3],
            [3, 3, 0, 3],
        ],
    },
    Glyph {
        ch: 'Q',
        strokes: &[
            [1, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 5],
            [4, 5, 3, 6],
            [3, 6, 1, 6],
            [1, 6, 0, 5],
            [0, 5, 0, 1],
            [0, 1, 1, 0],
            [2, 4, 4, 6],
        ],
    },
    Glyph {
        ch: 'R',
        strokes: &[
            [0, 6, 0, 0],
            [0, 0, 3, 0],
            [3, 0, 4, 1],
            [4, 1, 4, 2],
            [4, 2, 3, 3],
            [3, 3, 0, 3],
            [1, 3, 4, 6],
        ],
    },
    Glyph {
        ch: 'S',
        strokes: &[
            [4, 1, 3, 0],
            [3, 0, 1, 0],
            [1, 0, 0, 1],
            [0, 1, 0, 2],
            [0, 2, 1, 3],
            [1, 3, 3, 3],
            [3, 3, 4, 4],
            [4, 4, 4, 5],
            [4, 5, 3, 6],
            [3, 6, 1, 6],
            [1, 6, 0, 5],
        ],
    },
    Glyph {
        ch: 'T',
        strokes: &[[0, 0, 4, 0], [2, 0, 2, 6]],
    },
    Glyph {
        ch: 'U',
        strokes: &[
            [0, 0, 0, 5],
            [0, 5, 1, 6],
            [1, 6, 3, 6],
            [3, 6, 4, 5],
            [4, 5, 4, 0],
        ],
    },
    Glyph {
        ch: 'V',
        strokes: &[[0, 0, 2, 6], [2, 6, 4, 0]],
    },
    Glyph {
        ch: 'W',
        strokes: &[[0, 0, 1, 6], [1, 6, 2, 3], [2, 3, 3, 6], [3, 6, 4, 0]],
    },
    Glyph {
        ch: 'X',
        strokes: &[[0, 0, 4, 6], [4, 0, 0, 6]],
    },
    Glyph {
        ch: 'Y',
        strokes: &[[0, 0, 2, 3], [4, 0, 2, 3], [2, 3, 2, 6]],
    },
    Glyph {
        ch: 'Z',
        strokes: &[[0, 0, 4, 0], [4, 0, 0, 6], [0, 6, 4, 6]],
    },
    Glyph {
        ch: '\u{2191}',
        strokes: &[[2, 0, 2, 6], [2, 0, 0, 2], [2, 0, 4, 2]],
    },
    Glyph {
        ch: '\u{2193}',
        strokes: &[[2, 0, 2, 6], [2, 6, 0, 4], [2, 6, 4, 4]],
    },
];

/// Maximum line strokes for any built-in glyph, including lowercase folding and unknown-byte
/// fallback. Keep the raster work charge tied to this ceiling and test it against every glyph.
pub const MAX_GLYPH_STROKES: usize = 15;

/// The strokes drawn for `ch`: its glyph, the capital's glyph for a lowercase letter, or
/// [`MISSING`].
pub fn strokes_of(ch: char) -> &'static [Stroke] {
    let ch = ch.to_ascii_uppercase();
    match GLYPHS.binary_search_by(|g| g.ch.cmp(&ch)) {
        Ok(i) => GLYPHS.get(i).map_or(MISSING, |g| g.strokes),
        Err(_) => MISSING,
    }
}

/// Width of a string in grid units: from the left of its first glyph to the right of its last.
pub fn text_width_units(chars: usize) -> i32 {
    match i32::try_from(chars) {
        Ok(0) => 0,
        Ok(n) => n.saturating_mul(ADVANCE).saturating_sub(ADVANCE - WIDTH),
        Err(_) => i32::MAX,
    }
}

/// Canvas-pixel bounds, half-open, including two pixels of text clearance on each side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct TextBounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}
const _: () = assert!(core::mem::size_of::<TextBounds>() == 16);

/// The stroke renderer's aligned text origin in sixteenths of a canvas pixel.
pub fn text_origin_x16(x16: i32, size: u8, align: u8, chars: usize) -> i64 {
    let x16 = i64::from(x16);
    let width16 = i64::from(text_width_units(chars)) * i64::from(size) * 16 / i64::from(CAP_HEIGHT);
    match align {
        1 => x16 - width16 / 2,
        2 => x16 - width16,
        _ => x16,
    }
}

/// Conservative readable block bounds from the renderer's glyph advance and pixel origin.
/// `pitch` is the actual line spacing in canvas pixels, not CSS pixels. The final advance cell
/// and two pixels on every side are reserved even when the last glyph has narrower ink.
/// A single line uses the same function as a multi-line aircraft block.
pub fn text_block_bounds_px(
    anchor16: (i32, i32),
    size: u8,
    align: u8,
    longest_chars: usize,
    lines: u32,
    pitch: u32,
) -> TextBounds {
    let size = i64::from(size);
    let left = text_origin_x16(anchor16.0, size as u8, align, longest_chars) >> 4;
    let top = i64::from(anchor16.1) >> 4;
    let advance = size * i64::from(ADVANCE) / i64::from(CAP_HEIGHT);
    let width = i64::try_from(longest_chars)
        .unwrap_or(i64::MAX / 1024)
        .saturating_mul(advance);
    let line_height = i64::from(pitch).max((size * i64::from(MAX_Y) / i64::from(CAP_HEIGHT)) + 1);
    let height = i64::from(lines).saturating_mul(line_height);
    let coord = |v: i64| i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX });
    TextBounds {
        x0: coord(left - 2),
        y0: coord(top - 2),
        x1: coord(left.saturating_add(width).saturating_add(2)),
        y1: coord(top.saturating_add(height).saturating_add(2)),
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

    /// A glyph as a bitmap on the 5 by 8 grid: bit `y * 5 + x` is set when the cell is within half
    /// a cell of a stroke.
    fn bitmap(strokes: &[Stroke]) -> u64 {
        let mut bits = 0u64;
        for [x0, y0, x1, y1] in strokes {
            let (x0, y0, x1, y1) = (
                i32::from(*x0),
                i32::from(*y0),
                i32::from(*x1),
                i32::from(*y1),
            );
            for y in 0..8 {
                for x in 0..5 {
                    let (px, py) = (f64::from(x), f64::from(y));
                    let (ax, ay, bx, by) =
                        (f64::from(x0), f64::from(y0), f64::from(x1), f64::from(y1));
                    let (dx, dy) = (bx - ax, by - ay);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 == 0.0 {
                        0.0
                    } else {
                        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
                    };
                    let d = (px - (ax + t * dx)).hypot(py - (ay + t * dy));
                    if d <= 0.5 {
                        bits |= 1 << (y * 5 + x);
                    }
                }
            }
        }
        bits
    }

    fn glyph_bits(ch: char) -> u64 {
        bitmap(strokes_of(ch))
    }

    // REQ: OA-DSP-056
    #[test]
    fn raster_stroke_ceiling_covers_every_glyph_and_u8_input() {
        let observed = GLYPHS.iter().map(|g| g.strokes.len()).max();
        assert_eq!(observed, Some(MAX_GLYPH_STROKES));
        assert!(MISSING.len() <= MAX_GLYPH_STROKES);
        for byte in 0..=u8::MAX {
            assert!(strokes_of(char::from(byte)).len() <= MAX_GLYPH_STROKES);
        }
        assert_eq!(strokes_of('8').len(), MAX_GLYPH_STROKES);
    }

    // REQ: OA-DSP-007
    #[test]
    fn the_font_has_every_required_character_in_search_order() {
        for ch in ('A'..='Z')
            .chain('0'..='9')
            .chain(" .,/-+:()*#<>\u{2191}\u{2193}".chars())
        {
            assert!(
                GLYPHS.binary_search_by(|g| g.ch.cmp(&ch)).is_ok(),
                "no glyph for {ch:?}"
            );
        }
        assert!(
            GLYPHS.windows(2).all(|w| w[0].ch < w[1].ch),
            "glyphs are not sorted"
        );
        assert!(GLYPHS.iter().all(|g| g.ch == ' ' || !g.strokes.is_empty()));
    }

    // REQ: OA-DSP-007
    #[test]
    fn every_stroke_lies_on_the_grid() {
        let on_grid = |s: &Stroke| {
            s[0] <= WIDTH as u8 && s[2] <= WIDTH as u8 && s[1] <= MAX_Y && s[3] <= MAX_Y
        };
        for g in GLYPHS {
            assert!(g.strokes.iter().all(on_grid), "{:?} leaves the grid", g.ch);
            // Capitals and digits use the full cap height and stay inside it.
            if g.ch.is_ascii_alphanumeric() {
                assert!(
                    g.strokes.iter().all(|s| s[1] <= 6 && s[3] <= 6),
                    "{:?} dips",
                    g.ch
                );
            }
        }
        assert!(MISSING.iter().all(on_grid));
        assert!(text_width_units(0) == 0 && text_width_units(1) == WIDTH);
    }

    // REQ: OA-DSP-007
    #[test]
    fn lowercase_folds_and_unknown_characters_draw_the_box() {
        assert_eq!(strokes_of('a'), strokes_of('A'));
        assert_eq!(strokes_of('~'), MISSING);
        assert_eq!(strokes_of('\u{00e9}'), MISSING);
    }

    /// Cells that differ between two glyphs on the grid.
    pub fn distance(a: char, b: char) -> u32 {
        (glyph_bits(a) ^ glyph_bits(b)).count_ones()
    }

    // REQ: OA-DSP-008
    #[test]
    fn confusable_pairs_differ_by_at_least_the_stated_amount() {
        // The stated amounts, in cells of the 5 by 8 grid. At text size 12 a cell is two pixels,
        // so 4 cells is at least 16 pixels of a glyph that is 8 by 12.
        const MIN_CELLS: u32 = 4;
        // Other neighbours that read alike at a glance: a lower bar, since some pairs (P and R
        // differ only in a leg) cannot differ more without becoming different letters.
        const MIN_CELLS_NEIGHBOUR: u32 = 3;
        let required = [('0', 'O'), ('1', 'I'), ('5', 'S'), ('8', 'B'), ('2', 'Z')];
        let neighbours = [
            ('0', 'D'),
            ('0', 'Q'),
            ('6', 'G'),
            ('8', '3'),
            ('1', '7'),
            ('U', 'V'),
            ('C', 'G'),
            ('E', 'F'),
            ('P', 'R'),
            ('M', 'N'),
            ('O', 'Q'),
            ('5', '6'),
            ('9', '6'),
        ];
        for (a, b) in required {
            let d = distance(a, b);
            assert!(d >= MIN_CELLS, "{a} and {b} differ in only {d} cells");
        }
        for (a, b) in neighbours {
            let d = distance(a, b);
            assert!(
                d >= MIN_CELLS_NEIGHBOUR,
                "{a} and {b} differ in only {d} cells"
            );
        }
    }

    // REQ: OA-DSP-008
    #[test]
    fn no_two_glyphs_look_the_same() {
        for (i, a) in GLYPHS.iter().enumerate() {
            for b in &GLYPHS[i + 1..] {
                assert_ne!(
                    glyph_bits(a.ch),
                    glyph_bits(b.ch),
                    "{:?} and {:?} are identical",
                    a.ch,
                    b.ch
                );
            }
        }
    }
}
