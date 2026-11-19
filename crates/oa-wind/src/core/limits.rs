//! The bounds of oa-wind (RULESET E5). Core (pure, B7): constants only.

/// Limit: most analysis hours held in memory at once. Three covers a query between two hours
/// plus the next one a time-ordered stream needs (about 270 MB for RAP 13 km).
pub const MAX_HOURS_HELD: usize = 3;
/// Limit: most isobaric levels in one hour. RAP has 37 (1013.2 to 50 hPa at 25 hPa steps is 40);
/// 64 is headroom, and bounds the arrays.
pub const MAX_LEVELS: usize = 64;
/// Limit: most values in one hour's arrays (`levels x points x 4`), 128 Mi (512 MiB of f32).
pub const MAX_HOUR_VALUES: u64 = 128 * 1024 * 1024;
/// Limit: largest file read whole, 1 GiB. A full RAP pressure-level file is about 100 MB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// Limit: most hour files in one archive.
pub const MAX_ARCHIVE_FILES: usize = 1024;
/// Limit: largest `.idx` sidecar read, 1 MiB. The largest real one (HRRR, 7 messages per level
/// of a subset) is 32 596 bytes; a full RAP file's is 16 497.
pub const MAX_IDX_BYTES: u64 = 1024 * 1024;
