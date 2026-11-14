//! The ICAO standard atmosphere (ISO 2533), pressure altitude to pressure and back. Core (pure).
//!
//! Pressure altitude is defined as the altitude, in the standard atmosphere, at which the
//! pressure equals the measured static pressure. Converting it to pressure with the standard
//! atmosphere is therefore exact whatever the real temperature is; the real atmosphere enters
//! only in the model's own height and temperature at that pressure.
//!
//! Two layers cover -5000 m to 20 000 m geopotential: the troposphere (lapse 6.5 K/km, to
//! 11 000 m) and the isothermal layer (216.65 K, 11 000 to 20 000 m).

use crate::error::Refusal;

/// Sea-level standard pressure, hPa.
pub const P0_HPA: f64 = 1013.25;
const T0_K: f64 = 288.15;
const LAPSE_K_PER_M: f64 = 0.0065;
const G0: f64 = 9.80665;
/// Specific gas constant of dry air, J/(kg K), as ISO 2533 fixes it.
const R_AIR: f64 = 287.05287;
const TROPOPAUSE_M: f64 = 11_000.0;
const T_TROPOPAUSE_K: f64 = 216.65;
const FT_TO_M: f64 = 0.3048;
/// Lowest and highest geopotential altitude covered, metres.
pub const MIN_ALT_M: f64 = -5000.0;
pub const MAX_ALT_M: f64 = 20_000.0;

fn exponent() -> f64 {
    G0 / (R_AIR * LAPSE_K_PER_M)
}

fn p_tropopause_hpa() -> f64 {
    P0_HPA * (1.0 - LAPSE_K_PER_M * TROPOPAUSE_M / T0_K).powf(exponent())
}

/// ISA pressure in hPa at a pressure altitude in feet. Refuses non-finite input and altitudes
/// outside -5000 m to 20 000 m.
pub fn pressure_hpa_from_altitude_ft(alt_ft: f64) -> Result<f64, Refusal> {
    if !alt_ft.is_finite() {
        return Err(Refusal::BadInput("altitude is not finite"));
    }
    let h = alt_ft * FT_TO_M;
    if !(MIN_ALT_M..=MAX_ALT_M).contains(&h) {
        return Err(Refusal::AltitudeOutsideIsa { alt_m: h });
    }
    if h <= TROPOPAUSE_M {
        Ok(P0_HPA * (1.0 - LAPSE_K_PER_M * h / T0_K).powf(exponent()))
    } else {
        let scale = R_AIR * T_TROPOPAUSE_K / G0;
        Ok(p_tropopause_hpa() * (-(h - TROPOPAUSE_M) / scale).exp())
    }
}

/// Pressure altitude in feet of an ISA pressure in hPa (the inverse). Refuses non-positive or
/// non-finite pressure and pressures outside the range of [`pressure_hpa_from_altitude_ft`].
pub fn altitude_ft_from_pressure_hpa(p_hpa: f64) -> Result<f64, Refusal> {
    if !p_hpa.is_finite() || p_hpa <= 0.0 {
        return Err(Refusal::BadInput("pressure is not positive"));
    }
    let pt = p_tropopause_hpa();
    let h = if p_hpa >= pt {
        T0_K / LAPSE_K_PER_M * (1.0 - (p_hpa / P0_HPA).powf(1.0 / exponent()))
    } else {
        let scale = R_AIR * T_TROPOPAUSE_K / G0;
        TROPOPAUSE_M - scale * (p_hpa / pt).ln()
    };
    if !(MIN_ALT_M..=MAX_ALT_M).contains(&h) {
        return Err(Refusal::AltitudeOutsideIsa { alt_m: h });
    }
    Ok(h / FT_TO_M)
}
