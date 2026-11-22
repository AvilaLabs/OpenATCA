//! Opt-in digest of ordered, validated replay frames passed through the pipeline.
//!
//! This is an offline diagnostic for comparing pipeline input with the resulting state digest.
//! It is not a source identity, completeness proof, or equivalence claim across independently
//! clocked feeds. Payload bytes are absorbed into the digest and are never formatted or logged.

use oa_sha256::{Digest, Sha256};

use crate::source::Merged;

const DOMAIN: &[u8] = b"OpenATCA/oa-replay/ordered-input/v1\0";
const FRAME: &[u8] = b"FRM1";

/// Streaming fingerprint state, bounded independently of replay length.
pub struct InputFingerprint {
    hash: Option<Sha256>,
    frames: u64,
    payload_bytes: u64,
}

impl Default for InputFingerprint {
    fn default() -> Self {
        let mut hash = Sha256::new();
        let ok = hash.update(DOMAIN).is_ok();
        Self {
            hash: ok.then_some(hash),
            frames: 0,
            payload_bytes: 0,
        }
    }
}

impl InputFingerprint {
    /// Absorb one validated frame that was not omitted by the pipeline's sealed-document gate.
    pub fn observe(&mut self, frame: &Merged<'_>) {
        let Some(hash) = self.hash.as_mut() else {
            return;
        };
        let Ok(payload_len) = u32::try_from(frame.payload.len()) else {
            self.hash = None;
            return;
        };
        let tags = [
            frame.feed as u8,
            frame.kind as u8,
            frame.flags,
            u8::from(frame.mono_ns.is_some()),
        ];
        let parts: [&[u8]; 8] = [
            FRAME,
            &tags,
            &frame.recv_ns.to_le_bytes(),
            &frame.mono_ns.unwrap_or(0).to_le_bytes(),
            &frame.seq.to_le_bytes(),
            &frame.stream.to_le_bytes(),
            &payload_len.to_le_bytes(),
            frame.payload,
        ];
        for part in parts {
            if hash.update(part).is_err() {
                self.hash = None;
                return;
            }
        }
        self.frames = self.frames.saturating_add(1);
        self.payload_bytes = self.payload_bytes.saturating_add(u64::from(payload_len));
    }

    /// Number of frames included in the digest.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Payload bytes included in the digest; bytes are not retained.
    pub fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }

    /// Final digest, or `None` if the SHA-256 implementation's input limit was exceeded.
    pub fn finalize(&self) -> Option<Digest> {
        self.hash.clone().map(Sha256::finalize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::FeedKind;
    use oa_feed::Kind;

    fn frame<'a>(payload: &'a [u8], recv_ns: i64, mono_ns: Option<u64>, seq: u64) -> Merged<'a> {
        Merged {
            feed: FeedKind::Sfdps,
            kind: Kind::Message,
            flags: 0,
            recv_ns,
            mono_ns,
            seq,
            stream: 0,
            offset: 0,
            frame_len: payload.len() as u64,
            payload,
        }
    }

    #[test]
    fn encoding_is_ordered_unambiguous_and_presence_sensitive() {
        let mut a = InputFingerprint::default();
        a.observe(&frame(b"ab", 1, None, 2));
        a.observe(&frame(b"c", 3, Some(0), 4));
        let mut b = InputFingerprint::default();
        b.observe(&frame(b"a", 1, None, 2));
        b.observe(&frame(b"bc", 3, Some(0), 4));
        assert_ne!(a.finalize(), b.finalize());

        let mut c = InputFingerprint::default();
        c.observe(&frame(b"ab", 1, Some(0), 2));
        c.observe(&frame(b"c", 3, Some(0), 4));
        let mut d = InputFingerprint::default();
        d.observe(&frame(b"ab", 1, None, 2));
        d.observe(&frame(b"c", 3, Some(0), 4));
        assert_ne!(c.finalize(), d.finalize());

        let mut e = InputFingerprint::default();
        e.observe(&frame(b"ab", 1, None, 2));
        e.observe(&Merged {
            stream: 1,
            ..frame(b"c", 3, Some(0), 4)
        });
        assert_ne!(d.finalize(), e.finalize());
    }
}
