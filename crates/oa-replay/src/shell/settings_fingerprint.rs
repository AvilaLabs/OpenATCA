//! Canonical digest of effective offline replay settings and ordered feed-stream kinds.
//!
//! This opt-in guard binds a replay witness to its pipeline settings and logical feed grouping.
//! It excludes paths, build identity, chunking and output controls; it makes no identity,
//! authority, authenticity or completeness claim.

use oa_sha256::{Digest, Sha256};

use crate::cli::Input;
use crate::pipeline::{FeedKind, PipelineConfig};

const DOMAIN: &[u8] = b"OpenATCA/oa-replay/settings/v1\0";
const PIPELINE: &[u8] = b"PCFG1";
const STREAMS: &[u8] = b"STRM1";

fn put(hash: &mut Sha256, bytes: &[u8]) -> bool {
    hash.update(bytes).is_ok()
}

/// Digest all effective pipeline settings followed by ordered logical stream feed kinds.
///
/// Paths and file-versus-directory presentation are excluded: each command input contributes one
/// logical stream, and paths may move without changing its settings identity. Chunk size and
/// output controls are delivery/presentation details and are excluded as well. Config structs are
/// exhaustively destructured so adding an operational field requires updating this versioned
/// encoding before the crate compiles.
pub fn settings_fingerprint(cfg: &PipelineConfig, inputs: &[Input]) -> Option<Digest> {
    let PipelineConfig {
        table,
        checkpoint_ns,
        checkpoint_ring,
    } = *cfg;
    let crate::table::TableConfig {
        capacity,
        max_age_ns,
        active_age_ns,
        proposed_age_ns,
        terminal_age_ns,
        sweep_ns,
    } = table;
    let capacity = u64::try_from(capacity).ok()?;
    let checkpoint_ring = u64::try_from(checkpoint_ring).ok()?;
    let stream_count = u64::try_from(inputs.len()).ok()?;

    let mut hash = Sha256::new();
    if !put(&mut hash, DOMAIN) || !put(&mut hash, PIPELINE) {
        return None;
    }
    for bytes in [
        capacity.to_le_bytes(),
        max_age_ns.to_le_bytes(),
        active_age_ns.to_le_bytes(),
        proposed_age_ns.to_le_bytes(),
        terminal_age_ns.to_le_bytes(),
        sweep_ns.to_le_bytes(),
        checkpoint_ns.to_le_bytes(),
        checkpoint_ring.to_le_bytes(),
    ] {
        if !put(&mut hash, &bytes) {
            return None;
        }
    }
    if !put(&mut hash, STREAMS) || !put(&mut hash, &stream_count.to_le_bytes()) {
        return None;
    }
    for input in inputs {
        let feed = match input.feed {
            FeedKind::Sfdps => 0u8,
            FeedKind::Tbfm => 1,
            FeedKind::Tfms => 2,
            FeedKind::Fns => 3,
            FeedKind::Adsb => 4,
        };
        if !put(&mut hash, &[feed]) {
            return None;
        }
    }
    Some(hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::{settings_fingerprint, FeedKind, Input, PipelineConfig};
    use std::path::PathBuf;

    // REQ: OA-RPL-277
    #[test]
    fn ordered_logical_streams_are_bound_independently_of_paths_and_rotation() {
        let config = PipelineConfig::default();
        let input = |feed, name, dir| Input {
            feed,
            path: PathBuf::from(name),
            dir,
        };
        let expected = settings_fingerprint(
            &config,
            &[
                input(FeedKind::Sfdps, "original.oaf", false),
                input(FeedKind::Adsb, "raw", true),
            ],
        );
        assert!(expected.is_some());
        assert_eq!(
            expected,
            settings_fingerprint(
                &config,
                &[
                    input(FeedKind::Sfdps, "rotated", true),
                    input(FeedKind::Adsb, "concatenated.oaf", false),
                ],
            )
        );
        assert_ne!(
            expected,
            settings_fingerprint(
                &config,
                &[
                    input(FeedKind::Adsb, "raw", true),
                    input(FeedKind::Sfdps, "original.oaf", false),
                ],
            )
        );
        assert_ne!(
            expected,
            settings_fingerprint(
                &config,
                &[
                    input(FeedKind::Sfdps, "original.oaf", false),
                    input(FeedKind::Adsb, "raw", true),
                    input(FeedKind::Adsb, "another-stream", true),
                ],
            )
        );
    }
}
