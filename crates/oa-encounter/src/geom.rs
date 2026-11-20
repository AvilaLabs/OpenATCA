//! Separation tests. Core (pure).

use crate::types::{EncounterConfig, CLASS_VOLUME_ONLY};

/// True if a pair `h_m` apart horizontally and `v_ft` vertically (NaN: unknown) is inside the
/// record volume: horizontal under the volume's and, when known, vertical under its (strict).
pub fn inside_volume(c: &EncounterConfig, h_m: f64, v_ft: f64) -> bool {
    h_m < c.volume_h_m && (v_ft.is_nan() || v_ft < c.volume_v_ft)
}

/// The tightest class of a pair with a known vertical separation, [`CLASS_VOLUME_ONLY`] if inside
/// the volume but in none, or `None` if outside the volume. Edges are strict: a separation equal
/// to a limit is outside it.
pub fn pair_class(c: &EncounterConfig, h_m: f64, v_ft: f64) -> Option<u8> {
    if v_ft.is_nan() || !inside_volume(c, h_m, v_ft) {
        return None;
    }
    let n = c.n_classes.min(c.classes.len());
    for (k, cl) in c.classes.iter().take(n).enumerate() {
        if h_m < cl.h_m && v_ft < cl.v_ft {
            return Some(k as u8);
        }
    }
    Some(CLASS_VOLUME_ONLY)
}
