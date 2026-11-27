//! Canonical tracker-output serialization used by offline comparison and spill code. Hashing is
//! a shell concern: the pure engine returns fixed-width rows and does not depend on the split
//! SHA package.

use crate::types::{TrackOutput, O_HAS_ALT};
use oa_sha256::Sha256;

/// Tag and version of the canonical tracker-output serialization.
pub const OUTPUT_HASH_TAG: &[u8; 5] = b"OATK2";

fn put(h: &mut Sha256, b: &[u8]) {
    // Hashing a few megabytes cannot exceed the hasher's length limit; a failure would only
    // change the digest, which the determinism checks would then show.
    let _ = h.update(b);
}

/// Writes the canonical serialization of output rows to the hasher: tag, count, overflow, then
/// each column in a fixed order with integers little-endian and floats as IEEE bit patterns.
pub fn output_hash_into(h: &mut Sha256, o: &TrackOutput) {
    put(h, OUTPUT_HASH_TAG);
    put(h, &(o.t_ns.len() as u64).to_le_bytes());
    put(h, &o.overflow.to_le_bytes());
    for v in &o.t_ns {
        put(h, &v.to_le_bytes());
    }
    for v in &o.addr {
        put(h, &v.to_le_bytes());
    }
    for v in o.lat_e7.iter().chain(o.lon_e7.iter()) {
        put(h, &v.to_le_bytes());
    }
    for v in o
        .vel_east_mps
        .iter()
        .chain(o.vel_north_mps.iter())
        .chain(o.turn_rate_rps.iter())
    {
        put(h, &oa_math::canon_nan_f32(*v).to_bits().to_le_bytes());
    }
    // An absent altitude is hashed as zero, matching earlier digest versions; the flags below
    // distinguish absence and preserve compatibility with recorded digests.
    for col in [&o.alt_ft, &o.alt_var_ft2, &o.vrate_fpm] {
        for (i, v) in col.iter().enumerate() {
            let has = o.flags.get(i).is_some_and(|f| f & O_HAS_ALT != 0);
            put(
                h,
                &oa_math::canon_nan_f32(if has { *v } else { 0.0 })
                    .to_bits()
                    .to_le_bytes(),
            );
        }
    }
    for c in o.pos_cov_m2.iter().chain(o.vel_cov_m2s2.iter()) {
        for v in c {
            put(h, &oa_math::canon_nan_f32(*v).to_bits().to_le_bytes());
        }
    }
    for c in o.mu_h.iter().chain(o.mu_v.iter()) {
        for v in c {
            put(h, &oa_math::canon_nan_f32(*v).to_bits().to_le_bytes());
        }
    }
    put(h, &o.flags);
    for v in &o.ident {
        put(h, &v.to_le_bytes());
    }
    for v in &o.ident_ns {
        put(h, &v.to_le_bytes());
    }
    put(h, &o.ident_conflict);
}
