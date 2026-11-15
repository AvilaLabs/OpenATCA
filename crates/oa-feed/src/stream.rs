//! Reading a whole OAF1 stream through one [`FrameReader`]. Shell: owns the buffers and the
//! reads; the framing is [`crate::reader`]'s.
//!
//! The one loop the loaders of recorded ADS-B clips share, so that none of them can forget the
//! end of the stream: a frame cut short at the end (a recording that stopped mid-write, a copy
//! that did not finish) is reported by [`FrameReader::finish`] and counted here with the other
//! damaged frames, never dropped silently.

use std::io::{self, Read};

use crate::frame::{Frame, MAX_FRAME_LEN};
use crate::reader::FrameReader;

/// Bytes read from the source at once.
const CHUNK_LEN: usize = 1 << 20;

/// Reads `src` to its end and calls `f` for every well-formed frame, in stream order. Returns the
/// number of damaged frames: every frame the reader refused (bad magic, kind, length or CRC) plus
/// one for a partial frame left at the end of the stream. The buffers are allocated here, once.
pub fn read_frames<R: Read>(src: &mut R, f: &mut dyn FnMut(&Frame<'_>)) -> io::Result<u64> {
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut bad = 0u64;
    let mut chunk = vec![0u8; CHUNK_LEN];
    loop {
        let n = match src.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut at = 0usize;
        while at < n {
            let Some(rest) = chunk.get(at..n) else { break };
            let took = rd.push(rest);
            at += took;
            while let Some(fr) = rd.next() {
                match fr {
                    Ok(fr) => f(&fr),
                    Err(_) => bad += 1,
                }
            }
            if took == 0 {
                break;
            }
        }
    }
    if rd.finish().is_some() {
        bad += 1;
    }
    Ok(bad)
}
