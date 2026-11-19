//! Binary PPM (P6) output (**shell**, B1, B3): the simplest image format there is, no dependency.
//! The only code in this crate that writes a stream; it converts XRGB8888 rows to RGB triples.

use crate::raster::{check_frame, FrameSpec};
use std::io::{self, Write};

/// The P6 bytes of the visible pixels of an XRGB8888 buffer.
pub fn write_ppm<W: Write>(out: &mut W, spec: &FrameSpec, buf: &[u8]) -> io::Result<()> {
    check_frame(spec, buf.len()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    write!(out, "P6\n{} {}\n255\n", spec.width, spec.height)?;
    let (width, stride, height) = (
        spec.width as usize,
        spec.stride as usize,
        spec.height as usize,
    );
    let mut row_rgb = Vec::with_capacity(width * 3);
    for row in buf.chunks_exact(stride).take(height) {
        row_rgb.clear();
        for &[b, g, r, _] in row.as_chunks::<4>().0.iter().take(width) {
            // Stored blue, green, red, unused.
            row_rgb.extend_from_slice(&[r, g, b]);
        }
        out.write_all(&row_rgb)?;
    }
    Ok(())
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

    // REQ: OA-DSP-020
    #[test]
    fn writes_p6_with_rgb_order_and_no_row_padding() {
        // 3 by 2 pixels, stride of 4 pixels; the padding holds a marker that must not appear.
        let spec = FrameSpec {
            width: 3,
            height: 2,
            stride: 16,
        };
        let mut buf = vec![0xEE; 32];
        for y in 0..2usize {
            for x in 0..3usize {
                let o = y * 16 + x * 4;
                // Stored blue, green, red, unused.
                buf[o..o + 4].copy_from_slice(&[30 + x as u8, 20 + y as u8, 10, 0]);
            }
        }
        let mut out = Vec::new();
        write_ppm(&mut out, &spec, &buf).unwrap();
        let header = b"P6\n3 2\n255\n";
        assert_eq!(&out[..header.len()], header);
        let body = &out[header.len()..];
        assert_eq!(body.len(), 18);
        assert_eq!(&body[..6], &[10, 20, 30, 10, 20, 31]);
        assert_eq!(&body[9..12], &[10, 21, 30]);
        assert!(!body.contains(&0xEE));
    }

    // REQ: OA-DSP-020
    #[test]
    fn a_short_buffer_is_an_error_not_a_panic() {
        let spec = FrameSpec {
            width: 4,
            height: 4,
            stride: 16,
        };
        let mut out = Vec::new();
        assert!(write_ppm(&mut out, &spec, &[0; 10]).is_err());
        assert!(out.is_empty());
    }
}
