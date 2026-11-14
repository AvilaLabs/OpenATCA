//! gzip member framing (RFC 1952) around the DEFLATE compressor. Core (pure).
//!
//! The header carries no timestamp and no name, so the same input and level always give the
//! same bytes (B6). The operating-system byte is 255 (unknown).

use crate::deflate::Compressor;
use oa_zip::crc32_update;

/// Bytes in the header this writer emits.
pub const GZIP_HEADER_LEN: usize = 10;
/// Bytes in the trailer (CRC-32 and input size modulo 2^32).
pub const GZIP_TRAILER_LEN: usize = 8;

/// A streaming gzip writer for one member at a time; after [`finish`](Self::finish) it is
/// ready for the next member (concatenated members are a valid gzip file). A subsystem under D5.
// A7.2: cold struct (one per stream), size guard waived
pub struct GzipWriter {
    comp: Compressor,
    crc: u32,
    isize: u32,
    started: bool,
}

impl GzipWriter {
    /// Allocate the compressor state for `level` (see [`Compressor::new`]).
    pub fn new(level: u8) -> GzipWriter {
        GzipWriter {
            comp: Compressor::new(level),
            crc: 0,
            isize: 0,
            started: false,
        }
    }

    /// The header is written lazily, so even an empty member carries one.
    fn start<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        if self.started {
            return Ok(());
        }
        // XFL: 2 = best compression, 4 = fastest, per RFC 1952.
        let xfl = match self.comp.level() {
            9 => 2,
            1 => 4,
            _ => 0,
        };
        out(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, xfl, 255])?;
        self.started = true;
        Ok(())
    }

    /// Compress `input` into the current member. After an error from `out` discard the writer.
    pub fn write<E>(
        &mut self,
        input: &[u8],
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        self.start(out)?;
        self.crc = crc32_update(self.crc, input);
        // ISIZE is the length modulo 2^32 by definition, so truncation is intended.
        self.isize = self.isize.wrapping_add(input.len() as u32);
        self.comp.write(input, out)
    }

    /// Sync flush (see [`Compressor::flush`]): everything written so far becomes decodable.
    pub fn flush<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        self.start(out)?;
        self.comp.flush(out)
    }

    /// Finish the member with its CRC-32 and size trailer.
    pub fn finish<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        self.start(out)?;
        self.comp.finish(out)?;
        let mut t = [0u8; GZIP_TRAILER_LEN];
        let (c, s) = t.split_at_mut(4);
        c.copy_from_slice(&self.crc.to_le_bytes());
        s.copy_from_slice(&self.isize.to_le_bytes());
        out(&t)?;
        self.crc = 0;
        self.isize = 0;
        self.started = false;
        Ok(())
    }
}
