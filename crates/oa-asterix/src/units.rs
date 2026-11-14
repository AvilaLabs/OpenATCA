//! Unit LSBs of the items the typed layer decodes, as named constants. Each cites the
//! specification section. A decoded value is `raw * LSB`; the typed records keep the raw
//! integer so that encoding is exact (A9).

// CAT021 Ed 2.7

/// I021/071, 072, 073, 075, 077 (5.2.8 to 5.2.14): time since midnight UTC, 2^-7 s.
pub const CAT021_TIME_S: f64 = 1.0 / 128.0;
/// I021/074, 076 (5.2.11, 5.2.13): fraction of the second, 2^-30 s.
pub const CAT021_TIME_HP_S: f64 = 1.0 / 1_073_741_824.0;
/// I021/130 latitude and longitude (5.2.18): 180/2^23 degrees, about 2.145767e-5.
pub const CAT021_POS_DEG: f64 = 180.0 / 8_388_608.0;
/// Bits in the denominator of [`CAT021_POS_DEG`], for [`crate::raw_to_e7`].
pub const CAT021_POS_BITS: u32 = 23;
/// I021/131 latitude and longitude (5.2.19): 180/2^30 degrees, about 1.676381e-7.
pub const CAT021_POS_HR_DEG: f64 = 180.0 / 1_073_741_824.0;
/// Bits in the denominator of [`CAT021_POS_HR_DEG`].
pub const CAT021_POS_HR_BITS: u32 = 30;
/// I021/140 geometric height (5.2.21): 6.25 ft.
pub const CAT021_GEOM_HEIGHT_FT: f64 = 6.25;
/// I021/145 flight level (5.2.22): 1/4 FL (25 ft).
pub const CAT021_FL: f64 = 0.25;
/// I021/146 and 148 selected altitude (5.2.23, 5.2.24): 25 ft.
pub const CAT021_SELECTED_ALT_FT: f64 = 25.0;
/// I021/150 air speed as IAS (5.2.25): 2^-14 NM/s. As Mach the LSB is
/// [`CAT021_MACH`].
pub const CAT021_IAS_NM_S: f64 = 1.0 / 16_384.0;
/// I021/150 air speed as Mach (5.2.25): 0.001.
pub const CAT021_MACH: f64 = 0.001;
/// I021/151 true airspeed (5.2.26): 1 kt.
pub const CAT021_TAS_KT: f64 = 1.0;
/// I021/152 magnetic heading (5.2.27) and I021/160 track angle (5.2.30): 360/2^16 degrees.
pub const CAT021_ANGLE_DEG: f64 = 360.0 / 65_536.0;
/// I021/155 and 157 vertical rates (5.2.28, 5.2.29): 6.25 ft/min.
pub const CAT021_VRATE_FPM: f64 = 6.25;
/// I021/160 ground speed (5.2.30): 2^-14 NM/s, about 0.22 kt.
pub const CAT021_GS_NM_S: f64 = 1.0 / 16_384.0;
/// I021/165 track angle rate (5.2.32): 1/32 degree per second.
pub const CAT021_TAR_DEG_S: f64 = 1.0 / 32.0;
/// I021/230 roll angle (5.2.37): 0.01 degree.
pub const CAT021_ROLL_DEG: f64 = 0.01;

// CAT048 Ed 1.32

/// I048/140 time of day (5.2.17): 2^-7 s.
pub const CAT048_TIME_S: f64 = 1.0 / 128.0;
/// I048/040 range (5.2.4): 1/256 NM.
pub const CAT048_RHO_NM: f64 = 1.0 / 256.0;
/// I048/040 azimuth (5.2.4): 360/2^16 degrees, about 0.0055.
pub const CAT048_THETA_DEG: f64 = 360.0 / 65_536.0;
/// I048/042 X and Y (5.2.5): 1/128 NM.
pub const CAT048_XY_NM: f64 = 1.0 / 128.0;
/// I048/090 flight level (5.2.12): 1/4 FL.
pub const CAT048_FL: f64 = 0.25;
/// I048/130 subitems 1 and 4 plot runlength (5.2.16): 360/2^13 degrees, about 0.044.
pub const CAT048_RUNLENGTH_DEG: f64 = 360.0 / 8_192.0;
/// I048/130 subitems 3 and 5 amplitude (5.2.16): 1 dBm.
pub const CAT048_AMPLITUDE_DBM: f64 = 1.0;
/// I048/130 subitem 6 PSR minus SSR range (5.2.16): 1/256 NM.
pub const CAT048_RPD_NM: f64 = 1.0 / 256.0;
/// I048/130 subitem 7 PSR minus SSR azimuth (5.2.16): 360/2^14 degrees.
pub const CAT048_APD_DEG: f64 = 360.0 / 16_384.0;

// CAT062 Ed 1.21

/// I062/070 time of track information (5.2.5): 2^-7 s.
pub const CAT062_TIME_S: f64 = 1.0 / 128.0;
/// I062/105 latitude and longitude (5.2.8): 180/2^25 degrees, better than 0.6 m.
pub const CAT062_POS_DEG: f64 = 180.0 / 33_554_432.0;
/// Bits in the denominator of [`CAT062_POS_DEG`].
pub const CAT062_POS_BITS: u32 = 25;
/// I062/100 X and Y (5.2.7): 0.5 m.
pub const CAT062_CART_M: f64 = 0.5;
/// I062/185 Vx and Vy (5.2.14): 0.25 m/s.
pub const CAT062_VEL_M_S: f64 = 0.25;
/// I062/130 geometric altitude (5.2.11): 6.25 ft.
pub const CAT062_GEOM_ALT_FT: f64 = 6.25;
/// I062/135 barometric altitude and I062/136 measured flight level (5.2.12, 5.2.13): 1/4 FL.
pub const CAT062_FL: f64 = 0.25;
/// I062/220 rate of climb/descent (5.2.17): 6.25 ft/min.
pub const CAT062_ROC_FPM: f64 = 6.25;
/// I062/290 system track update ages (5.2.20): 1/4 s.
pub const CAT062_AGE_S: f64 = 0.25;
/// I062/380 subitem 6 selected altitude (5.2.24): 25 ft.
pub const CAT062_SELECTED_ALT_FT: f64 = 25.0;
