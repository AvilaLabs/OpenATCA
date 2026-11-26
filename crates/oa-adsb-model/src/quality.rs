//! Fixed-width quality bounds shared by the truth selector and the tracker adapter.

/// 95% horizontal position radius in metres for NACp 0 through 11; 0 is unknown.
const NACP_EPU_M: [f64; 12] = [
    f64::INFINITY,
    18_520.0,
    7_408.0,
    3_704.0,
    1_852.0,
    926.0,
    555.6,
    185.2,
    92.6,
    30.0,
    10.0,
    3.0,
];

/// 95% horizontal velocity error bound in m/s for NACv 0 through 4; 0 is unknown.
const NACV_EPU_MPS: [f64; 5] = [f64::INFINITY, 10.0, 3.0, 1.0, 0.3];

/// Returns the 95% horizontal position radius for NACp; unknown or out-of-range values are unbounded.
pub fn nacp_epu_m(nacp: u8) -> f64 {
    NACP_EPU_M
        .get(usize::from(nacp))
        .copied()
        .unwrap_or(f64::INFINITY)
}

/// Returns the 95% horizontal velocity bound for NACv; unknown or out-of-range values are unbounded.
pub fn nacv_epu_mps(nacv: u8) -> f64 {
    NACV_EPU_MPS
        .get(usize::from(nacv))
        .copied()
        .unwrap_or(f64::INFINITY)
}
