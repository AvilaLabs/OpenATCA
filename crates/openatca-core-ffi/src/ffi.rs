//! Raw-pointer adaptation and exported C symbols. Product operations call existing safe `oa-*` APIs.
#![allow(unsafe_code)] // RULESET A0: the sole unsafe module of openatca-core-ffi.

use crate::formats::asterix::{self, OaAsterixCounts, OaAsterixError, OaAsterixRecord};
use crate::formats::zip::{self, OaZipEntry, ZipAdapterError};
use crate::text;
use oa_asterix::{AsterixError, ErrorKind as AsterixErrorKind, NO_ITEM, NO_OFFSET, NO_RECORD};
use oa_deflate::{Compressor, MAX_LEVEL, MIN_LEVEL};
use oa_geo::{wgs84, GeoError, LatLon, Polygon, RhumbInverse, Separation};
use oa_sha256::{sha256, to_hex_into, MAX_MESSAGE_BYTES};
use oa_tar::{Next as TarNext, TarError, TarReader, DEFAULT_LIMITS};
use oa_zip::ZipError;
use oa_zip::{InflateError, InflateLimits, Inflater, StreamError};
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub const ABI_VERSION_PROVISIONAL: u32 = 1;

pub const OA_STATUS_OK: u32 = 0;
pub const OA_STATUS_INVALID_ARGUMENT: u32 = 1;
pub const OA_STATUS_NON_FINITE: u32 = 2;
pub const OA_STATUS_LATITUDE_OUT_OF_RANGE: u32 = 3;
pub const OA_STATUS_LONGITUDE_OUT_OF_RANGE: u32 = 4;
pub const OA_STATUS_NO_CONVERGENCE: u32 = 5;
pub const OA_STATUS_BUFFER_TOO_SMALL: u32 = 6;
pub const OA_STATUS_INTERNAL_PANIC: u32 = 7;
pub const OA_STATUS_BAD_PARAMETER: u32 = 8;
pub const OA_STATUS_LENGTH_TOO_LARGE: u32 = 9;
pub const OA_STATUS_BAD_OPERATION: u32 = 10;
pub const OA_STATUS_ALLOCATION_FAILED: u32 = 11;
pub const OA_STATUS_PARSE_ERROR: u32 = 12;
pub const OA_STATUS_DEFLATE_ERROR: u32 = 13;
pub const OA_STATUS_DEFLATE_TRUNCATED: u32 = 14;
pub const OA_STATUS_DEFLATE_OUTPUT_LIMIT: u32 = 15;
pub const OA_STATUS_DEFLATE_RATIO_LIMIT: u32 = 16;
pub const OA_STATUS_DEFLATE_ENCODE_OUTPUT_LIMIT: u32 = 17;
pub const OA_STATUS_TAR_BAD_HEADER: u32 = 18;
pub const OA_STATUS_TAR_TRUNCATED: u32 = 19;
pub const OA_STATUS_TAR_LIMIT: u32 = 20;
pub const OA_STATUS_TAR_NAME_TOO_LONG: u32 = 21;
pub const OA_STATUS_TAR_READ_ERROR: u32 = 22;
pub const OA_STATUS_ZIP_FORMAT: u32 = 32;
pub const OA_STATUS_ZIP_TRUNCATED: u32 = 33;
pub const OA_STATUS_ZIP_UNSUPPORTED_METHOD: u32 = 34;
pub const OA_STATUS_ZIP_ENCRYPTED: u32 = 35;
pub const OA_STATUS_ZIP_UNSAFE_PATH: u32 = 36;
pub const OA_STATUS_ZIP_LIMIT: u32 = 37;
pub const OA_STATUS_ZIP_INFLATE_OR_SIZE: u32 = 38;
pub const OA_STATUS_ZIP_CRC: u32 = 39;
pub const OA_STATUS_ZIP_BAD_ENTRY_INDEX: u32 = 40;

/// Hard ceiling for one raw-DEFLATE input slice accepted by the provisional C ABI.
pub const MAX_DEFLATE_INPUT_BYTES: u64 = 64 * 1024 * 1024;
/// Hard ceiling for one inflated output, matching ZIP's per-entry output limit.
pub const MAX_DEFLATE_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
/// Hard ceiling for one raw-DEFLATE output buffer accepted by the provisional C ABI.
pub const MAX_DEFLATE_ENCODE_OUTPUT_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum archive byte slice parsed by the metadata-only tar adapter.
pub const MAX_TAR_INPUT_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum member records returned by one metadata-only tar parse.
pub const MAX_TAR_ENTRIES: usize = 4096;
/// Maximum UTF-8 name-pool bytes (entry cap times oa-tar's name bound).
pub const MAX_TAR_NAME_POOL_BYTES: usize = MAX_TAR_ENTRIES * oa_tar::MAX_NAME_BYTES;
/// Ratio cap and small-output exemption used by the ZIP entry inflater.
pub const DEFLATE_MAX_RATIO: u64 = oa_zip::MAX_RATIO;
pub const DEFLATE_RATIO_EXEMPT_BYTES: u64 = oa_zip::RATIO_EXEMPT_BYTES;
pub const MAX_ZIP_INPUT_BYTES: u64 = oa_zip::MAX_ARCHIVE_BYTES;
pub const MAX_ZIP_ENTRIES: usize = oa_zip::MAX_ENTRIES;
pub const MAX_ZIP_NAME_POOL_BYTES: usize = oa_zip::MAX_ENTRIES * oa_zip::MAX_NAME_BYTES;

pub const OA_AST_E_BUFFER_TOO_SMALL: u32 = 1001;
pub const OA_AST_E_INVALID_ARGUMENT: u32 = 1002;
pub const OA_AST_E_LENGTH_TOO_LARGE: u32 = 1003;
pub const OA_AST_E_ALLOCATION_FAILED: u32 = 1004;
pub const OA_AST_E_INTERNAL_PANIC: u32 = 1005;

pub const OA_LOCATION_NONE: u32 = 0;
pub const OA_LOCATION_ARGUMENT: u32 = 1;
pub const OA_LOCATION_ELEMENT: u32 = 2;

/// Opaque immutable polygon handle. Its layout is never exposed in the C header.
pub struct OaPolygon {
    inner: Polygon,
}

/// One member's metadata from a parsed tar archive; names reference the call's UTF-8 byte pool.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaTarEntry {
    pub name_offset: u64,
    pub name_length: u64,
    pub size: u64,
    pub mtime: u64,
    pub header_offset: u64,
    pub typeflag: u8,
    pub reserved: [u8; 7],
}

const _: () = assert!(core::mem::size_of::<OaTarEntry>() == 48);
const _: () = assert!(core::mem::align_of::<OaTarEntry>() == 8);

fn map_tar_error(error: TarError) -> Failure {
    match error {
        TarError::BadHeader { .. } | TarError::Failed => Failure::general(OA_STATUS_TAR_BAD_HEADER),
        TarError::EntryTooLarge { .. }
        | TarError::MetaTooLarge { .. }
        | TarError::TooManyMeta { .. } => Failure::general(OA_STATUS_TAR_LIMIT),
        TarError::NameTooLong { .. } => Failure::general(OA_STATUS_TAR_NAME_TOO_LONG),
        TarError::Io { .. } => Failure::general(OA_STATUS_TAR_READ_ERROR),
    }
}

fn map_zip_error(error: ZipError) -> Failure {
    let status = match error {
        ZipError::Truncated(_) | ZipError::NoEndOfCentralDirectory => OA_STATUS_ZIP_TRUNCATED,
        ZipError::UnsupportedMethod { .. } => OA_STATUS_ZIP_UNSUPPORTED_METHOD,
        ZipError::Encrypted(_) => OA_STATUS_ZIP_ENCRYPTED,
        ZipError::UnsafePath(_) => OA_STATUS_ZIP_UNSAFE_PATH,
        ZipError::Limit { .. } => OA_STATUS_ZIP_LIMIT,
        ZipError::SizeMismatch { .. } | ZipError::Inflate(_) => OA_STATUS_ZIP_INFLATE_OR_SIZE,
        ZipError::CrcMismatch { .. } => OA_STATUS_ZIP_CRC,
        ZipError::NameNotFound(_) => OA_STATUS_ZIP_BAD_ENTRY_INDEX,
        ZipError::BadSignature(_)
        | ZipError::MultiDisk
        | ZipError::InvalidNameEncoding
        | ZipError::Zip64(_)
        | ZipError::DuplicateName(_)
        | ZipError::Inconsistent(_)
        | ZipError::DataDescriptorMismatch(_)
        | ZipError::Io(_) => OA_STATUS_ZIP_FORMAT,
    };
    Failure::general(status)
}

fn map_zip_adapter_error(error: ZipAdapterError) -> Failure {
    match error {
        ZipAdapterError::Parse(error) => map_zip_error(error),
        ZipAdapterError::Allocation => Failure::general(OA_STATUS_ALLOCATION_FAILED),
    }
}

pub const MATH_U_SIN: u32 = 1;
pub const MATH_U_COS: u32 = 2;
pub const MATH_U_TAN: u32 = 3;
pub const MATH_U_ATAN: u32 = 4;
pub const MATH_U_ASIN: u32 = 5;
pub const MATH_U_ACOS: u32 = 6;
pub const MATH_U_EXP: u32 = 7;
pub const MATH_U_LN: u32 = 8;
pub const MATH_U_CBRT: u32 = 9;
pub const MATH_U_CANON_NAN: u32 = 10;
pub const MATH_U_SQRT: u32 = 11;
pub const MATH_U_FLOOR: u32 = 12;
pub const MATH_U_CEIL: u32 = 13;
pub const MATH_U_TRUNC: u32 = 14;
pub const MATH_U_ROUND: u32 = 15;
pub const MATH_U_SIGNUM_F64: u32 = 16;

pub const MATH_B_ATAN2: u32 = 1;
pub const MATH_B_POW: u32 = 2;
pub const MATH_B_HYPOT: u32 = 3;
pub const MATH_B_REM_EUCLID: u32 = 4;
pub const MATH_B_MIN_F64: u32 = 5;
pub const MATH_B_MAX_F64: u32 = 6;
pub const MATH_B_COPYSIGN_F64: u32 = 7;

pub const MATH_T_CLAMP_F64: u32 = 1;

pub const MATH_F32_U_CANON_NAN: u32 = 1;
pub const MATH_F32_U_SIGNUM: u32 = 2;
pub const MATH_F32_B_COPYSIGN: u32 = 1;
pub const MATH_F32_B_MIN: u32 = 2;
pub const MATH_F32_B_MAX: u32 = 3;
pub const MATH_F32_T_CLAMP: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OaErrorInfo {
    pub code: u32,
    pub location_kind: u32,
    pub location: u64,
}
// A7.2: cold ABI error record, size asserted to keep the provisional C layout explicit.
const _: () = assert!(core::mem::size_of::<OaErrorInfo>() == 16);
const _: () = assert!(core::mem::align_of::<OaErrorInfo>() == 8);

#[derive(Clone, Copy)]
struct Failure {
    status: u32,
    location_kind: u32,
    location: u64,
}

impl Failure {
    const fn argument(status: u32, index: u64) -> Failure {
        Failure {
            status,
            location_kind: OA_LOCATION_ARGUMENT,
            location: index,
        }
    }

    const fn general(status: u32) -> Failure {
        Failure {
            status,
            location_kind: OA_LOCATION_NONE,
            location: 0,
        }
    }

    const fn element(status: u32, index: u64) -> Failure {
        Failure {
            status,
            location_kind: OA_LOCATION_ELEMENT,
            location: index,
        }
    }
}

fn map_geo(error: GeoError, argument: u64) -> Failure {
    match error {
        GeoError::NonFinite(_) => Failure::argument(OA_STATUS_NON_FINITE, argument),
        GeoError::LatitudeOutOfRange(_) => {
            Failure::argument(OA_STATUS_LATITUDE_OUT_OF_RANGE, argument)
        }
        GeoError::LongitudeOutOfRange(_) => {
            Failure::argument(OA_STATUS_LONGITUDE_OUT_OF_RANGE, argument)
        }
        GeoError::NoConvergence(_) => Failure::general(OA_STATUS_NO_CONVERGENCE),
        GeoError::BadParameter(_) | GeoError::BadPolygon(_) | GeoError::RhumbPole => {
            Failure::argument(OA_STATUS_BAD_PARAMETER, argument)
        }
        _ => Failure::argument(OA_STATUS_INVALID_ARGUMENT, argument),
    }
}

fn map_geo_base(error: GeoError, base: u64) -> Failure {
    let argument = coordinate_argument(&error, base);
    map_geo(error, argument)
}

fn map_geo_motion(error: GeoError, course_arg: u64, distance_arg: u64) -> Failure {
    let argument = match &error {
        GeoError::NonFinite("course") | GeoError::NonFinite("azimuth") => course_arg,
        GeoError::NonFinite("distance") => distance_arg,
        _ => course_arg,
    };
    map_geo(error, argument)
}

fn map_geo_element(error: GeoError, element: u64) -> Failure {
    let status = match error {
        GeoError::NonFinite(_) => OA_STATUS_NON_FINITE,
        GeoError::LatitudeOutOfRange(_) => OA_STATUS_LATITUDE_OUT_OF_RANGE,
        GeoError::LongitudeOutOfRange(_) => OA_STATUS_LONGITUDE_OUT_OF_RANGE,
        GeoError::NoConvergence(_) => OA_STATUS_NO_CONVERGENCE,
        GeoError::BadParameter(_) | GeoError::BadPolygon(_) | GeoError::RhumbPole => {
            OA_STATUS_BAD_PARAMETER
        }
        _ => OA_STATUS_INVALID_ARGUMENT,
    };
    Failure::element(status, element)
}

fn write_error(pointer: *mut OaErrorInfo, value: OaErrorInfo) {
    // SAFETY: every caller of this private helper first checks non-null; the public C contract
    // requires a valid, aligned, writable OaErrorInfo for the duration of the call.
    unsafe { pointer.write(value) };
}

fn write_asterix_error(pointer: *mut OaAsterixError, value: OaAsterixError) {
    // SAFETY: callers check non-null; the C contract requires a valid, aligned, writable,
    // non-overlapping error record for this call.
    unsafe { pointer.write(value) };
}

fn write_asterix_counts(pointer: *mut OaAsterixCounts, value: OaAsterixCounts) {
    // SAFETY: callers check non-null; the C contract requires a valid, aligned, writable,
    // non-overlapping counts record for this call.
    unsafe { pointer.write(value) };
}

fn asterix_kind_code(kind: AsterixErrorKind) -> u32 {
    match kind {
        AsterixErrorKind::Truncated => 1,
        AsterixErrorKind::BlockLengthBelowHeader => 2,
        AsterixErrorKind::BlockLengthExceedsInput => 3,
        AsterixErrorKind::EmptyBlock => 4,
        AsterixErrorKind::TooManyRecords => 5,
        AsterixErrorKind::UnsupportedCategory => 6,
        AsterixErrorKind::EmptyFspec => 7,
        AsterixErrorKind::FspecTooLong => 8,
        AsterixErrorKind::FspecNotMinimal => 9,
        AsterixErrorKind::UnknownUapSlot { .. } => 10,
        AsterixErrorKind::MissingMandatory { .. } => 11,
        AsterixErrorKind::ExtensionUnterminated => 12,
        AsterixErrorKind::ZeroRepetition => 13,
        AsterixErrorKind::ExplicitLengthInvalid => 14,
        AsterixErrorKind::EmptyCompound => 15,
        AsterixErrorKind::UnknownSubitem => 16,
        AsterixErrorKind::CompoundNotMinimal => 17,
        AsterixErrorKind::SpareBitSet => 18,
        AsterixErrorKind::InvalidValue => 19,
        AsterixErrorKind::ValueOutOfRange => 20,
        AsterixErrorKind::DuplicateItem => 21,
        AsterixErrorKind::BlockTooLong => 22,
        AsterixErrorKind::UnknownFrn { .. } => 23,
    }
}

fn asterix_error_record(status: u32, kind: u32) -> OaAsterixError {
    OaAsterixError {
        status,
        kind,
        category: 0,
        subitem: 0,
        item: NO_ITEM,
        record: NO_RECORD,
        reserved_bytes: [0; 8],
        offset: u64::MAX,
    }
}

fn map_asterix_error(error: AsterixError) -> OaAsterixError {
    OaAsterixError {
        status: OA_STATUS_PARSE_ERROR,
        kind: asterix_kind_code(error.kind),
        category: error.cat,
        subitem: error.sub,
        item: error.item,
        record: error.record,
        reserved_bytes: [0; 8],
        offset: if error.offset == NO_OFFSET {
            u64::MAX
        } else {
            u64::try_from(error.offset).unwrap_or(u64::MAX)
        },
    }
}

fn dispatch<T>(
    error: *mut OaErrorInfo,
    operation: impl FnOnce() -> Result<T, Failure>,
) -> (u32, Option<T>) {
    if error.is_null() {
        return (OA_STATUS_INVALID_ARGUMENT, None);
    }
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => {
            write_error(
                error,
                OaErrorInfo {
                    code: OA_STATUS_OK,
                    location_kind: OA_LOCATION_NONE,
                    location: 0,
                },
            );
            (OA_STATUS_OK, Some(value))
        }
        Ok(Err(failure)) => {
            write_error(
                error,
                OaErrorInfo {
                    code: failure.status,
                    location_kind: failure.location_kind,
                    location: failure.location,
                },
            );
            (failure.status, None)
        }
        Err(_) => {
            write_error(
                error,
                OaErrorInfo {
                    code: OA_STATUS_INTERNAL_PANIC,
                    location_kind: OA_LOCATION_NONE,
                    location: 0,
                },
            );
            (OA_STATUS_INTERNAL_PANIC, None)
        }
    }
}

/// The caller must supply every output and `error` as valid, aligned, writable, pairwise
/// non-overlapping memory for this call. Null pointers are rejected where representable; the
/// library does not and cannot validate arbitrary C addresses.
///
/// # Safety
/// `distance_m`, both azimuth outputs and `error` must each point to valid, aligned, writable
/// storage for this call, with all four regions pairwise non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_wgs84_inverse(
    lat1_deg: f64,
    lon1_deg: f64,
    lat2_deg: f64,
    lon2_deg: f64,
    distance_m: *mut f64,
    azimuth1_deg: *mut f64,
    azimuth2_deg: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if distance_m.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        if azimuth1_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        if azimuth2_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 6));
        }
        let first = LatLon::new(lat1_deg, lon1_deg).map_err(|e| {
            let argument = coordinate_argument(&e, 0);
            map_geo(e, argument)
        })?;
        let second = LatLon::new(lat2_deg, lon2_deg).map_err(|e| {
            let argument = coordinate_argument(&e, 2);
            map_geo(e, argument)
        })?;
        let answer = wgs84().inverse(first, second).map_err(|e| map_geo(e, 0))?;
        Ok((answer.distance_m, answer.azimuth1_deg, answer.azimuth2_deg))
    });
    if let Some((distance, azimuth1, azimuth2)) = result {
        // SAFETY: non-null checks above passed; the C caller's precondition guarantees valid,
        // aligned, writable, non-overlapping output storage for these three f64 values.
        unsafe {
            distance_m.write(distance);
            azimuth1_deg.write(azimuth1);
            azimuth2_deg.write(azimuth2);
        }
    }
    status
}

/// The caller must supply every output and `error` as valid, aligned, writable, pairwise
/// non-overlapping memory for this call. Null pointers are rejected where representable; the
/// library does not and cannot validate arbitrary C addresses.
///
/// # Safety
/// `lat2_deg`, `lon2_deg`, `azimuth2_deg` and `error` must each point to valid, aligned, writable
/// storage for this call, with all four regions pairwise non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_wgs84_direct(
    lat1_deg: f64,
    lon1_deg: f64,
    azimuth1_deg: f64,
    distance_m: f64,
    lat2_deg: *mut f64,
    lon2_deg: *mut f64,
    azimuth2_deg: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if lat2_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        if lon2_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        if azimuth2_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 6));
        }
        let start = LatLon::new(lat1_deg, lon1_deg).map_err(|e| {
            let argument = coordinate_argument(&e, 0);
            map_geo(e, argument)
        })?;
        if !azimuth1_deg.is_finite() {
            return Err(Failure::argument(OA_STATUS_NON_FINITE, 2));
        }
        if !distance_m.is_finite() {
            return Err(Failure::argument(OA_STATUS_NON_FINITE, 3));
        }
        let answer = wgs84()
            .direct(start, azimuth1_deg, distance_m)
            .map_err(|e| map_geo(e, 0))?;
        Ok((
            answer.position.lat(),
            answer.position.lon(),
            answer.azimuth2_deg,
        ))
    });
    if let Some((latitude, longitude, azimuth)) = result {
        // SAFETY: non-null checks above passed; the C caller's precondition guarantees valid,
        // aligned, writable, non-overlapping output storage for these three f64 values.
        unsafe {
            lat2_deg.write(latitude);
            lon2_deg.write(longitude);
            azimuth2_deg.write(azimuth);
        }
    }
    status
}

/// Stateless scalar operation; domain/special-value behavior is the selected `oa-math` function.
///
/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_unary(
    operation: u32,
    value: f64,
    output: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        let result = match operation {
            MATH_U_SIN => oa_math::sin(value),
            MATH_U_COS => oa_math::cos(value),
            MATH_U_TAN => oa_math::tan(value),
            MATH_U_ATAN => oa_math::atan(value),
            MATH_U_ASIN => oa_math::asin(value),
            MATH_U_ACOS => oa_math::acos(value),
            MATH_U_EXP => oa_math::exp(value),
            MATH_U_LN => oa_math::ln(value),
            MATH_U_CBRT => oa_math::cbrt(value),
            MATH_U_CANON_NAN => oa_math::canon_nan(value),
            MATH_U_SQRT => oa_ieee::sqrt(value),
            MATH_U_FLOOR => oa_ieee::floor(value),
            MATH_U_CEIL => oa_ieee::ceil(value),
            MATH_U_TRUNC => oa_ieee::trunc(value),
            MATH_U_ROUND => oa_ieee::round(value),
            MATH_U_SIGNUM_F64 => oa_math::signum_canon_f64(value),
            _ => return Err(Failure::argument(OA_STATUS_BAD_OPERATION, 0)),
        };
        Ok(result)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_binary(
    operation: u32,
    left: f64,
    right: f64,
    output: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        let value = match operation {
            MATH_B_ATAN2 => oa_math::atan2(left, right),
            MATH_B_POW => oa_math::pow(left, right),
            MATH_B_HYPOT => oa_math::hypot(left, right),
            MATH_B_REM_EUCLID => oa_math::rem_euclid(left, right),
            MATH_B_MIN_F64 => oa_math::min_f64(left, right),
            MATH_B_MAX_F64 => oa_math::max_f64(left, right),
            MATH_B_COPYSIGN_F64 => oa_math::copysign_canon_f64(left, right),
            _ => return Err(Failure::argument(OA_STATUS_BAD_OPERATION, 0)),
        };
        Ok(value)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_ternary(
    operation: u32,
    value: f64,
    lower: f64,
    upper: f64,
    output: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        match operation {
            MATH_T_CLAMP_F64 => Ok(oa_math::clamp_f64(value, lower, upper)),
            _ => Err(Failure::argument(OA_STATUS_BAD_OPERATION, 0)),
        }
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_powi(
    value: f64,
    exponent: i32,
    output: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        Ok(oa_math::powi(value, exponent))
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `sine`, `cosine` and `error` must point to valid, aligned, writable pairwise non-overlapping
/// storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_sin_cos(
    value: f64,
    sine: *mut f64,
    cosine: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if sine.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 1));
        }
        if cosine.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        Ok(oa_math::sin_cos(value))
    });
    if let Some((sine_value, cosine_value)) = result {
        // SAFETY: pointers are non-null; caller guarantees writable, pairwise non-overlapping outputs.
        unsafe {
            sine.write(sine_value);
            cosine.write(cosine_value);
        }
    }
    status
}

/// # Safety
/// `ordering` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_compare(
    left: f64,
    right: f64,
    output: *mut i32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        let order = match oa_math::cmp_canon_f64(left, right) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        };
        Ok(order)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `result` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f64_is_sign_negative(
    value: f64,
    output: *mut u32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 1));
        }
        Ok(u32::from(oa_math::is_sign_negative_canon_f64(value)))
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f32_unary(
    operation: u32,
    value: f32,
    output: *mut f32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        let result = match operation {
            MATH_F32_U_CANON_NAN => oa_math::canon_nan_f32(value),
            MATH_F32_U_SIGNUM => oa_math::signum_canon_f32(value),
            _ => return Err(Failure::argument(OA_STATUS_BAD_OPERATION, 0)),
        };
        Ok(result)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f32_binary(
    operation: u32,
    left: f32,
    right: f32,
    output: *mut f32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        let result = match operation {
            MATH_F32_B_COPYSIGN => oa_math::copysign_canon_f32(left, right),
            MATH_F32_B_MIN => oa_math::min_f32(left, right),
            MATH_F32_B_MAX => oa_math::max_f32(left, right),
            _ => return Err(Failure::argument(OA_STATUS_BAD_OPERATION, 0)),
        };
        Ok(result)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `output` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f32_ternary(
    operation: u32,
    value: f32,
    lower: f32,
    upper: f32,
    output: *mut f32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        match operation {
            MATH_F32_T_CLAMP => Ok(oa_math::clamp_f32(value, lower, upper)),
            _ => Err(Failure::argument(OA_STATUS_BAD_OPERATION, 0)),
        }
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `ordering` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f32_compare(
    left: f32,
    right: f32,
    output: *mut i32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        let order = match oa_math::cmp_canon_f32(left, right) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        };
        Ok(order)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `result` and `error` must point to valid, aligned, writable, non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_math_f32_is_sign_negative(
    value: f32,
    output: *mut u32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 1));
        }
        Ok(u32::from(oa_math::is_sign_negative_canon_f32(value)))
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// For nonzero `length`, `input` must identify `length` readable bytes. `digest` and `error` must
/// be valid, aligned, writable storage, and all input/output/error regions must not overlap.
#[no_mangle]
pub unsafe extern "C" fn oa_sha256_digest(
    input: *const u8,
    length: u64,
    digest: *mut u8,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if digest.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        if length > MAX_MESSAGE_BYTES || length > isize::MAX as u64 {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1));
        }
        let len = usize::try_from(length)
            .map_err(|_| Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1))?;
        if input.is_null() && len != 0 {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        // SAFETY: null is allowed only for empty input. `from_raw_parts` still requires a non-null
        // aligned pointer at length zero, so empty input uses a static empty slice.
        let bytes = if len == 0 {
            &[]
        } else {
            // SAFETY: caller guarantees `length` readable bytes at a non-null pointer.
            unsafe { core::slice::from_raw_parts(input, len) }
        };
        Ok(sha256(bytes))
    });
    if let Some(value) = result {
        // SAFETY: caller guarantees a writable 32-byte digest buffer distinct from input/error.
        unsafe { core::ptr::copy_nonoverlapping(value.as_ptr(), digest, value.len()) };
    }
    status
}

/// # Safety
/// `digest` must identify 32 readable bytes; `output` must identify 64 writable bytes; `error`
/// must be valid, aligned, writable storage. The three regions must not overlap.
#[no_mangle]
pub unsafe extern "C" fn oa_sha256_hex(
    digest: *const u8,
    output: *mut u8,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if digest.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 1));
        }
        // SAFETY: caller guarantees a readable 32-byte digest distinct from output/error.
        let source: &[u8; 32] = unsafe { &*(digest.cast::<[u8; 32]>()) };
        let mut hex = [0u8; 64];
        to_hex_into(source, &mut hex);
        Ok(hex)
    });
    if let Some(value) = result {
        // SAFETY: caller guarantees 64 writable bytes distinct from digest/error.
        unsafe { core::ptr::copy_nonoverlapping(value.as_ptr(), output, value.len()) };
    }
    status
}

/// # Safety
/// `distance_m`, `course_deg` and `error` must point to valid, aligned, writable pairwise
/// non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_wgs84_rhumb_inverse(
    lat1_deg: f64,
    lon1_deg: f64,
    lat2_deg: f64,
    lon2_deg: f64,
    distance_m: *mut f64,
    course_deg: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if distance_m.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        if course_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        let p1 = LatLon::new(lat1_deg, lon1_deg).map_err(|e| map_geo_base(e, 0))?;
        let p2 = LatLon::new(lat2_deg, lon2_deg).map_err(|e| map_geo_base(e, 2))?;
        let answer: RhumbInverse = wgs84().rhumb_inverse(p1, p2).map_err(|e| map_geo(e, 0))?;
        Ok(answer)
    });
    if let Some(answer) = result {
        // SAFETY: non-null checks passed; caller guarantees valid, distinct outputs.
        unsafe {
            distance_m.write(answer.distance_m);
            course_deg.write(answer.course_deg);
        }
    }
    status
}

/// # Safety
/// `lat2_deg`, `lon2_deg` and `error` must point to valid, aligned, writable pairwise
/// non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_wgs84_rhumb_direct(
    lat1_deg: f64,
    lon1_deg: f64,
    course_deg: f64,
    distance_m: f64,
    lat2_deg: *mut f64,
    lon2_deg: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if lat2_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        if lon2_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        let start = LatLon::new(lat1_deg, lon1_deg).map_err(|e| map_geo_base(e, 0))?;
        let answer = wgs84()
            .rhumb_direct(start, course_deg, distance_m)
            .map_err(|e| map_geo_motion(e, 2, 3))?;
        Ok((answer.lat(), answer.lon()))
    });
    if let Some((lat, lon)) = result {
        // SAFETY: non-null checks passed; caller guarantees valid, distinct outputs.
        unsafe {
            lat2_deg.write(lat);
            lon2_deg.write(lon);
        }
    }
    status
}

/// # Safety
/// `lat_out_deg`, `lon_out_deg` and `error` must point to valid, aligned, writable pairwise
/// non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_wgs84_rhumb_point_at_fraction(
    lat1_deg: f64,
    lon1_deg: f64,
    lat2_deg: f64,
    lon2_deg: f64,
    fraction: f64,
    lat_out_deg: *mut f64,
    lon_out_deg: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if lat_out_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        if lon_out_deg.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 6));
        }
        let p1 = LatLon::new(lat1_deg, lon1_deg).map_err(|e| map_geo_base(e, 0))?;
        let p2 = LatLon::new(lat2_deg, lon2_deg).map_err(|e| map_geo_base(e, 2))?;
        let answer = wgs84()
            .rhumb_point_at_fraction(p1, p2, fraction)
            .map_err(|e| map_geo(e, 4))?;
        Ok((answer.lat(), answer.lon()))
    });
    if let Some((lat, lon)) = result {
        // SAFETY: non-null checks passed; caller guarantees valid, distinct outputs.
        unsafe {
            lat_out_deg.write(lat);
            lon_out_deg.write(lon);
        }
    }
    status
}

/// # Safety
/// All four result pointers and `error` must point to valid, aligned, writable pairwise
/// non-overlapping storage for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_wgs84_geodesic_rhumb_separation(
    lat1_deg: f64,
    lon1_deg: f64,
    lat2_deg: f64,
    lon2_deg: f64,
    samples: u32,
    max_m: *mut f64,
    at_fraction: *mut f64,
    geodesic_m: *mut f64,
    rhumb_m: *mut f64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        let pointers = [max_m, at_fraction, geodesic_m, rhumb_m];
        for (index, pointer) in pointers.iter().enumerate() {
            if pointer.is_null() {
                return Err(Failure::argument(
                    OA_STATUS_INVALID_ARGUMENT,
                    5 + index as u64,
                ));
            }
        }
        let p1 = LatLon::new(lat1_deg, lon1_deg).map_err(|e| map_geo_base(e, 0))?;
        let p2 = LatLon::new(lat2_deg, lon2_deg).map_err(|e| map_geo_base(e, 2))?;
        let count =
            usize::try_from(samples).map_err(|_| Failure::argument(OA_STATUS_BAD_PARAMETER, 4))?;
        let answer: Separation = wgs84()
            .geodesic_rhumb_separation(p1, p2, count)
            .map_err(|e| map_geo(e, 4))?;
        Ok(answer)
    });
    if let Some(answer) = result {
        // SAFETY: all outputs are non-null; caller guarantees valid pairwise-disjoint writable memory.
        unsafe {
            max_m.write(answer.max_m);
            at_fraction.write(answer.at_fraction);
            geodesic_m.write(answer.geodesic_m);
            rhumb_m.write(answer.rhumb_m);
        }
    }
    status
}

/// # Safety
/// For nonzero `count`, both coordinate pointers must identify `count` readable, aligned `f64`
/// values. `output` and `error` must be valid, aligned, writable, and non-overlapping with each
/// other and with the input arrays. Inputs are borrowed only for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_polygon_new_wgs84(
    latitudes: *const f64,
    longitudes: *const f64,
    count: u64,
    output: *mut *mut OaPolygon,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        if count > 100_001 {
            return Err(Failure::argument(OA_STATUS_BAD_PARAMETER, 2));
        }
        let len =
            usize::try_from(count).map_err(|_| Failure::argument(OA_STATUS_BAD_PARAMETER, 2))?;
        if len != 0 && (latitudes.is_null() || longitudes.is_null()) {
            return Err(Failure::argument(
                OA_STATUS_INVALID_ARGUMENT,
                if latitudes.is_null() { 0 } else { 1 },
            ));
        }
        let lats = if len == 0 {
            &[]
        } else {
            // SAFETY: this nonzero branch has a checked non-null pointer; caller guarantees `count`
            // readable values in the array.
            unsafe { core::slice::from_raw_parts(latitudes, len) }
        };
        let lons = if len == 0 {
            &[]
        } else {
            // SAFETY: this nonzero branch has a checked non-null pointer; caller guarantees `count`
            // readable values in the array.
            unsafe { core::slice::from_raw_parts(longitudes, len) }
        };
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(len)
            .map_err(|_| Failure::general(OA_STATUS_ALLOCATION_FAILED))?;
        for (index, (&lat, &lon)) in lats.iter().zip(lons).enumerate() {
            let element =
                u64::try_from(index).map_err(|_| Failure::argument(OA_STATUS_BAD_PARAMETER, 2))?;
            vertices.push(LatLon::new(lat, lon).map_err(|e| map_geo_element(e, element))?);
        }
        let polygon = Polygon::new(&wgs84(), &vertices).map_err(|e| map_geo(e, 2))?;
        Ok(Box::new(OaPolygon { inner: polygon }))
    });
    if let Some(polygon) = result {
        // SAFETY: output is non-null; caller guarantees writable pointer storage distinct from error and input.
        unsafe { output.write(Box::into_raw(polygon)) };
    }
    status
}

/// # Safety
/// `polygon` must be a live handle returned by `oa_geo_polygon_new_wgs84` and not concurrently
/// freed. `inside` and `error` must be valid, aligned, writable and non-overlapping with each
/// other or the handle for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_polygon_contains_wgs84(
    polygon: *const OaPolygon,
    latitude: f64,
    longitude: f64,
    inside: *mut u32,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if polygon.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if inside.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        let point = LatLon::new(latitude, longitude).map_err(|e| map_geo_base(e, 1))?;
        // SAFETY: pointer is non-null; caller guarantees a live handle returned by polygon_new and not concurrently freed.
        let value = unsafe { &*polygon }.inner.contains(point);
        Ok(u32::from(value))
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { inside.write(value) };
    }
    status
}

/// # Safety
/// `polygon` must be a live handle returned by `oa_geo_polygon_new_wgs84` and not concurrently
/// freed. `output` and `error` must be valid, aligned, writable and non-overlapping with each
/// other or the handle for this call.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_polygon_edge_count(
    polygon: *const OaPolygon,
    output: *mut u64,
    error: *mut OaErrorInfo,
) -> u32 {
    let (status, result) = dispatch(error, || {
        if polygon.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 1));
        }
        // SAFETY: pointer is non-null; caller guarantees a live handle returned by polygon_new and not concurrently freed.
        let count = unsafe { &*polygon }.inner.len();
        Ok(count as u64)
    });
    if let Some(value) = result {
        // SAFETY: non-null check passed; caller guarantees valid writable output distinct from error.
        unsafe { output.write(value) };
    }
    status
}

/// # Safety
/// `polygon` must be a live handle returned by `oa_geo_polygon_new_wgs84`, with no concurrent
/// call using it. This function consumes the handle; it must not be used or freed again.
#[no_mangle]
pub unsafe extern "C" fn oa_geo_polygon_free(polygon: *mut OaPolygon) {
    if !polygon.is_null() {
        // SAFETY: caller must pass a live handle returned by polygon_new exactly once and not use it afterward.
        unsafe { drop(Box::from_raw(polygon)) };
    }
}

fn map_text_failure(failure: text::AdapterFailure) -> Failure {
    match failure.kind {
        text::FailureKind::Parser => Failure::general(OA_STATUS_PARSE_ERROR),
        text::FailureKind::ResourceLimit => Failure::general(OA_STATUS_LENGTH_TOO_LARGE),
        text::FailureKind::Allocation => Failure::general(OA_STATUS_ALLOCATION_FAILED),
        text::FailureKind::Internal => Failure::general(OA_STATUS_INTERNAL_PANIC),
        text::FailureKind::InvalidArgument => Failure::general(OA_STATUS_INVALID_ARGUMENT),
    }
}

fn text_count(index: usize, counts: &[u64; 5]) -> u64 {
    counts.get(index).copied().unwrap_or(0)
}

/// Parses JSON into caller-owned preorder nodes and a UTF-8/lexical-value pool. All provided
/// input/output regions must be valid, aligned as required, and pairwise non-overlapping.
/// Capacities are measured in elements for arrays and bytes for the value pool. A zero-capacity
/// query returns required counts with `BUFFER_TOO_SMALL` and writes no arrays.
///
/// # Safety
/// Every non-null pointer must refer to valid storage for its claimed extent; the caller must keep
/// input immutable for this call and must not overlap any input/output or output/output regions.
#[no_mangle]
pub unsafe extern "C" fn oa_json_parse(
    input: *const u8,
    input_length: u64,
    nodes: *mut text::OaJsonNode,
    node_capacity: u64,
    value_pool: *mut u8,
    value_capacity: u64,
    counts: *mut text::OaJsonCounts,
    parse_error: *mut text::OaTextError,
    error: *mut OaErrorInfo,
) -> u32 {
    if error.is_null() {
        return OA_STATUS_INVALID_ARGUMENT;
    }
    dispatch(error, || {
        if counts.is_null()
            || parse_error.is_null()
            || (input.is_null() && input_length != 0)
            || (nodes.is_null() && node_capacity != 0)
            || (value_pool.is_null() && value_capacity != 0)
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        if input_length > isize::MAX as u64 {
            return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
        }
        let length = usize::try_from(input_length)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        // SAFETY: the C contract requires a readable `input_length` region; null is accepted only
        // for an empty slice, which is constructed without dereferencing the pointer.
        let bytes = unsafe {
            if length == 0 {
                &[]
            } else {
                core::slice::from_raw_parts(input, length)
            }
        };
        let required = match text::json_counts(bytes) {
            Ok(required) => required,
            Err(failure) => {
                let reported = text::OaJsonCounts {
                    node_count: text_count(0, &failure.counts),
                    value_bytes: text_count(1, &failure.counts),
                };
                // SAFETY: required output pointers were checked above; caller guarantees writable,
                // aligned, non-overlapping output records for the duration of this call.
                unsafe {
                    counts.write(reported);
                    parse_error.write(failure.parser);
                }
                return Err(map_text_failure(failure));
            }
        };
        // SAFETY: required output pointers were checked above; caller guarantees writable,
        // aligned, non-overlapping output records for the duration of this call.
        unsafe {
            counts.write(required);
            parse_error.write(text::OaTextError::default());
        }
        if node_capacity < required.node_count || value_capacity < required.value_bytes {
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        if required.node_count != 0 && nodes.is_null()
            || required.value_bytes != 0 && value_pool.is_null()
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        let flat = match text::parse_json(bytes) {
            Ok(flat) => flat,
            Err(failure) => {
                // SAFETY: output records were validated above; C memory regions must not overlap.
                unsafe {
                    parse_error.write(failure.parser);
                }
                return Err(map_text_failure(failure));
            }
        };
        // SAFETY: prior capacity and null checks establish valid destination extents by contract;
        // source vectors remain live, and the ABI requires all regions to be non-overlapping.
        unsafe {
            if !flat.nodes.is_empty() {
                core::ptr::copy_nonoverlapping(flat.nodes.as_ptr(), nodes, flat.nodes.len());
            }
            if !flat.pool.is_empty() {
                core::ptr::copy_nonoverlapping(flat.pool.as_ptr(), value_pool, flat.pool.len());
            }
        }
        Ok(())
    })
    .0
}

/// Parses one bounded XML document into flat element, attribute, text and namespace arrays.
/// Names, prefixes and raw values are spans into the immutable input; namespace URIs are copied
/// into their pool. All output regions must be pairwise non-overlapping. A zero-capacity query
/// returns required counts with `BUFFER_TOO_SMALL` and writes no arrays.
///
/// # Safety
/// Every non-null pointer must refer to valid storage for its claimed extent; input must remain
/// immutable for this call and no input/output or output/output regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn oa_xml_parse(
    input: *const u8,
    input_length: u64,
    elements: *mut text::OaXmlElement,
    element_capacity: u64,
    attributes: *mut text::OaXmlAttribute,
    attribute_capacity: u64,
    texts: *mut text::OaXmlText,
    text_capacity: u64,
    namespaces: *mut text::OaXmlNamespace,
    namespace_capacity: u64,
    namespace_uri_pool: *mut u8,
    namespace_uri_capacity: u64,
    counts: *mut text::OaXmlCounts,
    parse_error: *mut text::OaTextError,
    error: *mut OaErrorInfo,
) -> u32 {
    if error.is_null() {
        return OA_STATUS_INVALID_ARGUMENT;
    }
    dispatch(error, || {
        if counts.is_null()
            || parse_error.is_null()
            || (input.is_null() && input_length != 0)
            || (elements.is_null() && element_capacity != 0)
            || (attributes.is_null() && attribute_capacity != 0)
            || (texts.is_null() && text_capacity != 0)
            || (namespaces.is_null() && namespace_capacity != 0)
            || (namespace_uri_pool.is_null() && namespace_uri_capacity != 0)
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        if input_length > isize::MAX as u64 {
            return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
        }
        let length = usize::try_from(input_length)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        // SAFETY: the C contract requires a readable `input_length` region; null is accepted only
        // for an empty slice, which is constructed without dereferencing the pointer.
        let bytes = unsafe {
            if length == 0 {
                &[]
            } else {
                core::slice::from_raw_parts(input, length)
            }
        };
        let required = match text::xml_counts(bytes) {
            Ok(required) => required,
            Err(failure) => {
                let reported = text::OaXmlCounts {
                    elements: text_count(0, &failure.counts),
                    attributes: text_count(1, &failure.counts),
                    text_nodes: text_count(2, &failure.counts),
                    namespaces: text_count(3, &failure.counts),
                    namespace_uri_bytes: text_count(4, &failure.counts),
                };
                // SAFETY: required output pointers were checked above; caller guarantees writable,
                // aligned, non-overlapping output records for the duration of this call.
                unsafe {
                    counts.write(reported);
                    parse_error.write(failure.parser);
                }
                return Err(map_text_failure(failure));
            }
        };
        // SAFETY: required output pointers were checked above; caller guarantees writable,
        // aligned, non-overlapping output records for the duration of this call.
        unsafe {
            counts.write(required);
            parse_error.write(text::OaTextError::default());
        }
        if element_capacity < required.elements
            || attribute_capacity < required.attributes
            || text_capacity < required.text_nodes
            || namespace_capacity < required.namespaces
            || namespace_uri_capacity < required.namespace_uri_bytes
        {
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        if (required.elements != 0 && elements.is_null())
            || (required.attributes != 0 && attributes.is_null())
            || (required.text_nodes != 0 && texts.is_null())
            || (required.namespaces != 0 && namespaces.is_null())
            || (required.namespace_uri_bytes != 0 && namespace_uri_pool.is_null())
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        let flat = match text::parse_xml(bytes) {
            Ok(flat) => flat,
            Err(failure) => {
                // SAFETY: output records were validated above; C memory regions must not overlap.
                unsafe {
                    parse_error.write(failure.parser);
                }
                return Err(map_text_failure(failure));
            }
        };
        // SAFETY: capacities and null conditions were checked; vector allocations remain live;
        // caller guarantees valid aligned extents and pairwise non-overlap for all destinations.
        unsafe {
            if !flat.elements.is_empty() {
                core::ptr::copy_nonoverlapping(
                    flat.elements.as_ptr(),
                    elements,
                    flat.elements.len(),
                );
            }
            if !flat.attributes.is_empty() {
                core::ptr::copy_nonoverlapping(
                    flat.attributes.as_ptr(),
                    attributes,
                    flat.attributes.len(),
                );
            }
            if !flat.texts.is_empty() {
                core::ptr::copy_nonoverlapping(flat.texts.as_ptr(), texts, flat.texts.len());
            }
            if !flat.namespaces.is_empty() {
                core::ptr::copy_nonoverlapping(
                    flat.namespaces.as_ptr(),
                    namespaces,
                    flat.namespaces.len(),
                );
            }
            if !flat.namespace_uris.is_empty() {
                core::ptr::copy_nonoverlapping(
                    flat.namespace_uris.as_ptr(),
                    namespace_uri_pool,
                    flat.namespace_uris.len(),
                );
            }
        }
        Ok(())
    })
    .0
}

/// Decodes one XML text or attribute span into a caller-owned output buffer.
///
/// # Safety
/// The input/output regions and all scalar output pointers must be valid and non-overlapping for
/// this call. `output` may be null only when `output_capacity` is zero.
#[no_mangle]
pub unsafe extern "C" fn oa_xml_decode_span(
    input: *const u8,
    input_length: u64,
    start: u32,
    length: u32,
    span_kind: u32,
    output: *mut u8,
    output_capacity: u64,
    written: *mut u64,
    parse_error: *mut text::OaTextError,
    error: *mut OaErrorInfo,
) -> u32 {
    if error.is_null() {
        return OA_STATUS_INVALID_ARGUMENT;
    }
    dispatch(error, || {
        if written.is_null()
            || parse_error.is_null()
            || (input.is_null() && input_length != 0)
            || (output.is_null() && output_capacity != 0)
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        // SAFETY: scalar outputs were checked non-null; caller guarantees writable, aligned,
        // non-overlapping records; initialize even if conversion refuses the request.
        unsafe {
            written.write(0);
            parse_error.write(text::OaTextError::default());
        }
        if input_length > isize::MAX as u64 || output_capacity > isize::MAX as u64 {
            return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
        }
        let input_len = usize::try_from(input_length)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        let output_len = usize::try_from(output_capacity)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        // SAFETY: both regions are valid for their caller-supplied extents, null only accompanies
        // zero length, and the ABI requires input, output, written, parse_error and error not overlap.
        let (input_bytes, output_bytes) = unsafe {
            let input_bytes = if input_len == 0 {
                &[]
            } else {
                core::slice::from_raw_parts(input, input_len)
            };
            let output_bytes = if output_len == 0 {
                &mut []
            } else {
                core::slice::from_raw_parts_mut(output, output_len)
            };
            (input_bytes, output_bytes)
        };
        match text::decode_xml_span(input_bytes, start, length, span_kind, output_bytes) {
            Ok(size) => {
                // SAFETY: `written` was checked and does not overlap output per the ABI contract.
                unsafe {
                    written.write(size as u64);
                }
                Ok(())
            }
            Err(failure) => {
                // SAFETY: `parse_error` was checked and does not overlap any other output.
                unsafe {
                    parse_error.write(failure.parser);
                }
                Err(map_text_failure(failure))
            }
        }
    })
    .0
}

/// Parses bounded CSV bytes into row/field tables and one UTF-8 byte pool. Row zero is the
/// required header. The caller owns every output; all regions must be pairwise non-overlapping.
/// A zero-capacity query returns required counts with `BUFFER_TOO_SMALL` and writes no arrays.
///
/// # Safety
/// Every non-null pointer must refer to valid storage for its claimed extent; input must remain
/// immutable for this call and no input/output or output/output regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn oa_csv_parse(
    input: *const u8,
    input_length: u64,
    rows: *mut text::OaCsvRow,
    row_capacity: u64,
    fields: *mut text::OaCsvField,
    field_capacity: u64,
    value_pool: *mut u8,
    value_capacity: u64,
    counts: *mut text::OaCsvCounts,
    parse_error: *mut text::OaTextError,
    error: *mut OaErrorInfo,
) -> u32 {
    if error.is_null() {
        return OA_STATUS_INVALID_ARGUMENT;
    }
    dispatch(error, || {
        if counts.is_null()
            || parse_error.is_null()
            || (input.is_null() && input_length != 0)
            || (rows.is_null() && row_capacity != 0)
            || (fields.is_null() && field_capacity != 0)
            || (value_pool.is_null() && value_capacity != 0)
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        if input_length > isize::MAX as u64 {
            return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
        }
        let length = usize::try_from(input_length)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        // SAFETY: the C contract requires a readable `input_length` region; null is accepted only
        // for an empty slice, which is constructed without dereferencing the pointer.
        let bytes = unsafe {
            if length == 0 {
                &[]
            } else {
                core::slice::from_raw_parts(input, length)
            }
        };
        let required = match text::csv_counts_for_input(bytes) {
            Ok(required) => required,
            Err(failure) => {
                let reported = text::OaCsvCounts {
                    rows: text_count(0, &failure.counts),
                    fields: text_count(1, &failure.counts),
                    value_bytes: text_count(2, &failure.counts),
                };
                // SAFETY: required output pointers were checked above; caller guarantees writable,
                // aligned, non-overlapping output records for the duration of this call.
                unsafe {
                    counts.write(reported);
                    parse_error.write(failure.parser);
                }
                return Err(map_text_failure(failure));
            }
        };
        // SAFETY: required output pointers were checked above; caller guarantees writable,
        // aligned, non-overlapping output records for the duration of this call.
        unsafe {
            counts.write(required);
            parse_error.write(text::OaTextError::default());
        }
        if row_capacity < required.rows
            || field_capacity < required.fields
            || value_capacity < required.value_bytes
        {
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        if (required.rows != 0 && rows.is_null())
            || (required.fields != 0 && fields.is_null())
            || (required.value_bytes != 0 && value_pool.is_null())
        {
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        let flat = match text::parse_csv(bytes) {
            Ok(flat) => flat,
            Err(failure) => {
                // SAFETY: output records were validated above; C memory regions must not overlap.
                unsafe {
                    parse_error.write(failure.parser);
                }
                return Err(map_text_failure(failure));
            }
        };
        // SAFETY: capacities and null conditions were checked; vector allocations remain live;
        // caller guarantees valid aligned extents and pairwise non-overlap for all destinations.
        unsafe {
            if !flat.rows.is_empty() {
                core::ptr::copy_nonoverlapping(flat.rows.as_ptr(), rows, flat.rows.len());
            }
            if !flat.fields.is_empty() {
                core::ptr::copy_nonoverlapping(flat.fields.as_ptr(), fields, flat.fields.len());
            }
            if !flat.pool.is_empty() {
                core::ptr::copy_nonoverlapping(flat.pool.as_ptr(), value_pool, flat.pool.len());
            }
        }
        Ok(())
    })
    .0
}

fn map_inflate_error(error: InflateError) -> Failure {
    match error {
        InflateError::Truncated => Failure::general(OA_STATUS_DEFLATE_TRUNCATED),
        InflateError::OutputLimit { .. } => Failure::general(OA_STATUS_DEFLATE_OUTPUT_LIMIT),
        InflateError::RatioLimit { .. } => Failure::general(OA_STATUS_DEFLATE_RATIO_LIMIT),
        InflateError::ReservedBlockType
        | InflateError::StoredLengthMismatch
        | InflateError::BadHuffmanTable(_)
        | InflateError::InvalidCode
        | InflateError::InvalidSymbol
        | InflateError::DistanceTooFar => Failure::general(OA_STATUS_DEFLATE_ERROR),
    }
}

/// Inflates one bounded raw-DEFLATE stream transactionally. Bytes after the final block are not
/// consumed and are reported through `consumed`. On success, or `BUFFER_TOO_SMALL`, the exact
/// output length and consumed input length are written; all other failures leave output storage
/// and length metadata untouched. A zero output capacity with a null output pointer is a size
/// query. The output is copied only after the complete stream passes all checks.
///
/// # Safety
/// `input` must be readable for `input_length` bytes unless length is zero, in which case it may
/// be null. `output` must be writable for `output_capacity` bytes unless capacity is zero, when it
/// may be null. `output_length`, `consumed`, and `error` must be valid, aligned, writable and
/// pairwise non-overlapping. All input, output and metadata regions must be pairwise non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_deflate_raw_inflate(
    input: *const u8,
    input_length: u64,
    max_output: u64,
    output: *mut u8,
    output_capacity: u64,
    output_length: *mut u64,
    consumed: *mut u64,
    error: *mut OaErrorInfo,
) -> u32 {
    let mut required_if_small = 0u64;
    let mut consumed_if_small = 0u64;
    let (status, result) = dispatch(error, || {
        if output_length.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        if consumed.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 6));
        }
        if input_length > MAX_DEFLATE_INPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1));
        }
        if max_output > MAX_DEFLATE_OUTPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 2));
        }
        let input_length = usize::try_from(input_length)
            .map_err(|_| Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1))?;
        if input_length != 0 && input.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if output_capacity != 0 && output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        let input = if input_length == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees a readable, non-overlapping input region of this
            // checked length; the ABI cannot validate an arbitrary C address.
            unsafe { core::slice::from_raw_parts(input, input_length) }
        };
        let limits = InflateLimits {
            max_output,
            max_ratio: DEFLATE_MAX_RATIO,
            ratio_exempt_bytes: DEFLATE_RATIO_EXEMPT_BYTES,
        };
        let mut inflater = Inflater::new(limits);
        let mut decoded = Vec::new();
        let mut sink = |run: &[u8]| -> Result<(), ()> {
            decoded.try_reserve(run.len()).map_err(|_| ())?;
            decoded.extend_from_slice(run);
            Ok(())
        };
        let outcome = match inflater.push(input, &mut sink) {
            Ok(outcome) => outcome,
            Err(StreamError::Inflate(failure)) => return Err(map_inflate_error(failure)),
            Err(StreamError::Sink(())) => {
                return Err(Failure::general(OA_STATUS_ALLOCATION_FAILED));
            }
        };
        if !outcome.finished {
            return Err(Failure::general(OA_STATUS_DEFLATE_TRUNCATED));
        }
        let required = u64::try_from(decoded.len())
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        let consumed_bytes = u64::try_from(outcome.input)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        if required > output_capacity {
            required_if_small = required;
            consumed_if_small = consumed_bytes;
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        Ok((decoded, consumed_bytes))
    });
    if let Some((decoded, consumed_bytes)) = result {
        // SAFETY: validation above accepted the pointer/capacity; caller guarantees writable,
        // aligned, pairwise non-overlapping regions. Empty output performs no pointer access.
        unsafe {
            if !decoded.is_empty() {
                core::ptr::copy_nonoverlapping(decoded.as_ptr(), output, decoded.len());
            }
            output_length.write(decoded.len() as u64);
            consumed.write(consumed_bytes);
        }
    } else if status == OA_STATUS_BUFFER_TOO_SMALL {
        // SAFETY: non-null metadata pointers were validated and are pairwise non-overlapping by
        // the caller's contract. Buffer-too-small is also the documented size-query result.
        unsafe {
            output_length.write(required_if_small);
            consumed.write(consumed_if_small);
        }
    }
    status
}

/// Compresses one bounded input slice into a raw-DEFLATE stream transactionally. The requested
/// level follows `oa-deflate`'s documented clamp policy (0..=1 becomes 1; above 9 becomes 9);
/// `actual_level` reports the applied value. All input bytes are consumed on success or
/// `BUFFER_TOO_SMALL`; the exact output length is reported on those outcomes. Other failures
/// leave output storage and length metadata untouched.
///
/// # Safety
/// `input` must be readable for `input_length` bytes unless length is zero, in which case it may
/// be null. `output` must be writable for `output_capacity` bytes unless capacity is zero, when it
/// may be null. `output_length`, `consumed`, `actual_level`, and `error` must be valid, aligned,
/// writable and pairwise non-overlapping. All input, output and metadata regions must be pairwise
/// non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_deflate_raw_compress(
    input: *const u8,
    input_length: u64,
    requested_level: u32,
    max_output: u64,
    output: *mut u8,
    output_capacity: u64,
    output_length: *mut u64,
    consumed: *mut u64,
    actual_level: *mut u32,
    error: *mut OaErrorInfo,
) -> u32 {
    let mut output_length_if_small = 0u64;
    let mut actual_level_if_small = 0u32;
    let (status, result) = dispatch(error, || {
        if output_length.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 6));
        }
        if consumed.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 7));
        }
        if actual_level.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 8));
        }
        if input_length > MAX_DEFLATE_INPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1));
        }
        if max_output > MAX_DEFLATE_ENCODE_OUTPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 3));
        }
        let input_length = usize::try_from(input_length)
            .map_err(|_| Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1))?;
        if input_length != 0 && input.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if output_capacity != 0 && output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        let input = if input_length == 0 {
            &[]
        } else {
            // SAFETY: caller guarantees a readable, non-overlapping input region of this checked
            // length; the ABI cannot validate an arbitrary C address.
            unsafe { core::slice::from_raw_parts(input, input_length) }
        };
        let bounded_level = requested_level.clamp(u32::from(MIN_LEVEL), u32::from(MAX_LEVEL)) as u8;
        let mut compressor = Compressor::new(bounded_level);
        let mut encoded = Vec::new();
        let mut sink = |chunk: &[u8]| -> Result<(), u32> {
            let Some(required) = encoded.len().checked_add(chunk.len()) else {
                return Err(OA_STATUS_DEFLATE_ENCODE_OUTPUT_LIMIT);
            };
            if u64::try_from(required).unwrap_or(u64::MAX) > max_output {
                return Err(OA_STATUS_DEFLATE_ENCODE_OUTPUT_LIMIT);
            }
            encoded
                .try_reserve(chunk.len())
                .map_err(|_| OA_STATUS_ALLOCATION_FAILED)?;
            encoded.extend_from_slice(chunk);
            Ok(())
        };
        if let Err(code) = compressor.write(input, &mut sink) {
            return Err(Failure::general(code));
        }
        if let Err(code) = compressor.finish(&mut sink) {
            return Err(Failure::general(code));
        }
        let required = u64::try_from(encoded.len())
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        if required > output_capacity {
            output_length_if_small = required;
            actual_level_if_small = u32::from(bounded_level);
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        Ok((
            encoded,
            u64::try_from(input_length).unwrap_or(u64::MAX),
            bounded_level,
        ))
    });
    if let Some((encoded, consumed_bytes, level)) = result {
        // SAFETY: validation accepted output pointer/capacity and caller guarantees disjoint
        // writable regions. Empty encoded output performs no pointer access.
        unsafe {
            if !encoded.is_empty() {
                core::ptr::copy_nonoverlapping(encoded.as_ptr(), output, encoded.len());
            }
            output_length.write(encoded.len() as u64);
            consumed.write(consumed_bytes);
            actual_level.write(u32::from(level));
        }
    } else if status == OA_STATUS_BUFFER_TOO_SMALL {
        // SAFETY: non-null metadata pointers were validated and caller guarantees they are
        // pairwise disjoint. Buffer-too-small is also the documented size-query result.
        unsafe {
            output_length.write(output_length_if_small);
            consumed.write(input_length);
            actual_level.write(actual_level_if_small);
        }
    }
    status
}

/// Parses a bounded tar byte slice into ordered member metadata and a UTF-8 name byte pool.
/// Payload bytes are never returned or extracted. Member names, typeflags and effective sizes are
/// preserved from `oa-tar`; offsets identify the first extension header when present, otherwise
/// the member header.
/// A capacity query validates the entire archive before returning exact sizes. Other failures
/// leave output arrays and size metadata untouched.
///
/// # Safety
/// `input` must be readable for `input_length` bytes unless length is zero, in which case it may
/// be null. `entries` and `name_pool` must be writable for their declared capacities unless the
/// corresponding capacity is zero, when each may be null. `entry_count`, `name_pool_length`, and
/// `error` must be valid, aligned, writable, and pairwise non-overlapping. All input, output, and
/// metadata regions must be pairwise non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_tar_parse(
    input: *const u8,
    input_length: u64,
    max_entries: u32,
    entries: *mut OaTarEntry,
    entry_capacity: u64,
    entry_count: *mut u64,
    name_pool: *mut u8,
    name_pool_capacity: u64,
    name_pool_length: *mut u64,
    error: *mut OaErrorInfo,
) -> u32 {
    let mut required_entries = 0u64;
    let mut required_names = 0u64;
    let (status, result) = dispatch(error, || {
        if entry_count.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        if name_pool_length.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 7));
        }
        if input_length > MAX_TAR_INPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1));
        }
        if max_entries as usize > MAX_TAR_ENTRIES {
            return Err(Failure::argument(OA_STATUS_BAD_PARAMETER, 2));
        }
        let input_length = usize::try_from(input_length)
            .map_err(|_| Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1))?;
        if input_length != 0 && input.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if entry_capacity != 0 && entries.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        if name_pool_capacity != 0 && name_pool.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 6));
        }
        let input = if input_length == 0 {
            &[]
        } else {
            // SAFETY: caller guarantees a readable, disjoint region of this checked length.
            unsafe { core::slice::from_raw_parts(input, input_length) }
        };
        let mut reader = TarReader::new(vec![Cursor::new(input)], DEFAULT_LIMITS);
        let mut parsed_entries = Vec::new();
        let mut parsed_names = Vec::new();
        loop {
            let header = match reader.next_entry() {
                Ok(TarNext::Entry(header)) => header,
                Ok(TarNext::End) => break,
                Ok(TarNext::Truncated(_)) => {
                    return Err(Failure::general(OA_STATUS_TAR_TRUNCATED));
                }
                Err(failure) => return Err(map_tar_error(failure)),
            };
            if parsed_entries.len() >= max_entries as usize {
                return Err(Failure::general(OA_STATUS_TAR_LIMIT));
            }
            let name = header.name.as_bytes();
            let next_name_len = parsed_names
                .len()
                .checked_add(name.len())
                .ok_or_else(|| Failure::general(OA_STATUS_TAR_LIMIT))?;
            if next_name_len > MAX_TAR_NAME_POOL_BYTES {
                return Err(Failure::general(OA_STATUS_TAR_LIMIT));
            }
            parsed_entries
                .try_reserve(1)
                .map_err(|_| Failure::general(OA_STATUS_ALLOCATION_FAILED))?;
            parsed_names
                .try_reserve(name.len())
                .map_err(|_| Failure::general(OA_STATUS_ALLOCATION_FAILED))?;
            let name_offset = u64::try_from(parsed_names.len())
                .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
            let name_length = u64::try_from(name.len())
                .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
            parsed_entries.push(OaTarEntry {
                name_offset,
                name_length,
                size: header.size,
                mtime: header.mtime,
                header_offset: header.offset,
                typeflag: header.typeflag,
                reserved: [0; 7],
            });
            parsed_names.extend_from_slice(name);
        }
        required_entries = u64::try_from(parsed_entries.len())
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        required_names = u64::try_from(parsed_names.len())
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        if entry_capacity < required_entries || name_pool_capacity < required_names {
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        Ok((parsed_entries, parsed_names))
    });
    if let Some((parsed_entries, parsed_names)) = result {
        // SAFETY: validated capacities cover both complete output slices; caller guarantees all
        // pointers are aligned, writable and disjoint. Empty outputs perform no pointer access.
        unsafe {
            if !parsed_entries.is_empty() {
                core::ptr::copy_nonoverlapping(
                    parsed_entries.as_ptr(),
                    entries,
                    parsed_entries.len(),
                );
            }
            if !parsed_names.is_empty() {
                core::ptr::copy_nonoverlapping(
                    parsed_names.as_ptr(),
                    name_pool,
                    parsed_names.len(),
                );
            }
            entry_count.write(required_entries);
            name_pool_length.write(required_names);
        }
    } else if status == OA_STATUS_BUFFER_TOO_SMALL {
        // SAFETY: non-null size metadata pointers were validated and are caller-disjoint.
        unsafe {
            entry_count.write(required_entries);
            name_pool_length.write(required_names);
        }
    }
    status
}

/// Parses one borrowed ZIP byte slice and returns all validated member metadata plus a UTF-8
/// name pool. A short-capacity query reports exact sizes without writing output arrays.
///
/// # Safety
/// `input` must be readable for `input_length` bytes unless empty. Nonzero output capacities
/// require writable aligned buffers. Count, pool-length and error pointers must be valid and
/// writable. All regions must be pairwise non-overlapping as required by the header contract.
#[no_mangle]
pub unsafe extern "C" fn oa_zip_list(
    input: *const u8,
    input_length: u64,
    entries: *mut OaZipEntry,
    entry_capacity: u64,
    entry_count: *mut u64,
    name_pool: *mut u8,
    name_pool_capacity: u64,
    name_pool_length: *mut u64,
    error: *mut OaErrorInfo,
) -> u32 {
    let mut required_entries = 0u64;
    let mut required_names = 0u64;
    let (status, result) = dispatch(error, || {
        if entry_count.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 4));
        }
        if name_pool_length.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 7));
        }
        if input_length > MAX_ZIP_INPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1));
        }
        if entry_capacity > MAX_ZIP_ENTRIES as u64 {
            return Err(Failure::argument(OA_STATUS_BAD_PARAMETER, 3));
        }
        if name_pool_capacity > MAX_ZIP_NAME_POOL_BYTES as u64 {
            return Err(Failure::argument(OA_STATUS_BAD_PARAMETER, 6));
        }
        let input_length = usize::try_from(input_length)
            .map_err(|_| Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1))?;
        if input_length != 0 && input.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if entry_capacity != 0 && entries.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 2));
        }
        if name_pool_capacity != 0 && name_pool.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        let input = if input_length == 0 {
            &[]
        } else {
            // SAFETY: caller supplies the checked readable slice and disjoint output regions.
            unsafe { core::slice::from_raw_parts(input, input_length) }
        };
        let metadata = zip::parse_metadata(input).map_err(map_zip_adapter_error)?;
        required_entries = u64::try_from(metadata.entries.len())
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        required_names = u64::try_from(metadata.names.len())
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        if entry_capacity < required_entries || name_pool_capacity < required_names {
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        Ok(metadata)
    });
    if let Some(metadata) = result {
        // SAFETY: capacities cover complete output slices; caller promises aligned, writable,
        // disjoint regions. Empty slices do not dereference their pointers.
        unsafe {
            if !metadata.entries.is_empty() {
                core::ptr::copy_nonoverlapping(
                    metadata.entries.as_ptr(),
                    entries,
                    metadata.entries.len(),
                );
            }
            if !metadata.names.is_empty() {
                core::ptr::copy_nonoverlapping(
                    metadata.names.as_ptr(),
                    name_pool,
                    metadata.names.len(),
                );
            }
            entry_count.write(required_entries);
            name_pool_length.write(required_names);
        }
    } else if status == OA_STATUS_BUFFER_TOO_SMALL {
        // SAFETY: size metadata pointers were checked and caller promises writable storage.
        unsafe {
            entry_count.write(required_entries);
            name_pool_length.write(required_names);
        }
    }
    status
}

/// Extracts one selected member into caller memory. A zero-capacity query returns the declared
/// output size after archive-directory validation; full extraction later verifies data and CRC.
/// Output bytes and `output_length` are unchanged on every failure other than BUFFER_TOO_SMALL.
///
/// # Safety
/// `input` must be readable for `input_length` bytes unless empty. `output` must be writable for
/// `output_capacity` bytes unless capacity is zero. `output_length` and `error` must be valid,
/// aligned, writable, and all regions pairwise non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_zip_extract_entry(
    input: *const u8,
    input_length: u64,
    entry_index: u64,
    output: *mut u8,
    output_capacity: u64,
    output_length: *mut u64,
    error: *mut OaErrorInfo,
) -> u32 {
    let mut required = 0u64;
    let (status, result) = dispatch(error, || {
        if output_length.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 5));
        }
        if input_length > MAX_ZIP_INPUT_BYTES {
            return Err(Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1));
        }
        if output_capacity > oa_zip::MAX_ENTRY_BYTES {
            return Err(Failure::argument(OA_STATUS_BAD_PARAMETER, 4));
        }
        let input_length = usize::try_from(input_length)
            .map_err(|_| Failure::argument(OA_STATUS_LENGTH_TOO_LARGE, 1))?;
        if input_length != 0 && input.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 0));
        }
        if output_capacity != 0 && output.is_null() {
            return Err(Failure::argument(OA_STATUS_INVALID_ARGUMENT, 3));
        }
        let input = if input_length == 0 {
            &[]
        } else {
            // SAFETY: caller supplies the checked readable slice and disjoint output region.
            unsafe { core::slice::from_raw_parts(input, input_length) }
        };
        let metadata = zip::parse_metadata(input).map_err(map_zip_adapter_error)?;
        let index = usize::try_from(entry_index)
            .map_err(|_| Failure::argument(OA_STATUS_ZIP_BAD_ENTRY_INDEX, 2))?;
        let selected = metadata
            .entries
            .get(index)
            .ok_or_else(|| Failure::argument(OA_STATUS_ZIP_BAD_ENTRY_INDEX, 2))?;
        required = selected.uncompressed_size;
        if output_capacity < required {
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        let extracted = zip::extract_entry(input, index).map_err(map_zip_error)?;
        if extracted.len() as u64 != required {
            return Err(Failure::general(OA_STATUS_ZIP_INFLATE_OR_SIZE));
        }
        Ok(extracted)
    });
    if let Some(extracted) = result {
        // SAFETY: the complete validated entry fits the caller's declared output capacity.
        unsafe {
            if !extracted.is_empty() {
                core::ptr::copy_nonoverlapping(extracted.as_ptr(), output, extracted.len());
            }
            output_length.write(required);
        }
    } else if status == OA_STATUS_BUFFER_TOO_SMALL {
        // SAFETY: output_length was checked and caller promises writable storage.
        unsafe { output_length.write(required) };
    }
    status
}

fn coordinate_argument(error: &GeoError, base: u64) -> u64 {
    match error {
        GeoError::NonFinite("longitude") | GeoError::LongitudeOutOfRange(_) => base + 1,
        _ => base,
    }
}

fn message(code: u32) -> &'static [u8] {
    match code {
        OA_STATUS_OK => b"success\0",
        OA_STATUS_INVALID_ARGUMENT => b"invalid argument\0",
        OA_STATUS_NON_FINITE => b"argument must be finite\0",
        OA_STATUS_LATITUDE_OUT_OF_RANGE => b"latitude is outside [-90, 90] degrees\0",
        OA_STATUS_LONGITUDE_OUT_OF_RANGE => b"longitude is outside [-180, 180] degrees\0",
        OA_STATUS_NO_CONVERGENCE => b"geodesic computation did not converge\0",
        OA_STATUS_BUFFER_TOO_SMALL => b"message buffer is too small\0",
        OA_STATUS_INTERNAL_PANIC => b"internal panic was contained at the C boundary\0",
        OA_STATUS_BAD_PARAMETER => b"parameter is outside its supported range\0",
        OA_STATUS_LENGTH_TOO_LARGE => b"input length exceeds the supported limit\0",
        OA_STATUS_BAD_OPERATION => b"operation code is not supported\0",
        OA_STATUS_ALLOCATION_FAILED => b"bounded allocation could not be reserved\0",
        OA_STATUS_PARSE_ERROR => b"bounded text parser refused input\0",
        OA_STATUS_DEFLATE_ERROR => b"raw DEFLATE stream is malformed\0",
        OA_STATUS_DEFLATE_TRUNCATED => b"raw DEFLATE stream is truncated\0",
        OA_STATUS_DEFLATE_OUTPUT_LIMIT => b"raw DEFLATE output exceeded its limit\0",
        OA_STATUS_DEFLATE_RATIO_LIMIT => b"raw DEFLATE output exceeded its ratio limit\0",
        OA_STATUS_DEFLATE_ENCODE_OUTPUT_LIMIT => {
            b"raw DEFLATE encoding exceeded its output limit\0"
        }
        OA_STATUS_TAR_BAD_HEADER => b"tar header failed validation\0",
        OA_STATUS_TAR_TRUNCATED => b"tar archive is truncated\0",
        OA_STATUS_TAR_LIMIT => b"tar archive exceeds a metadata limit\0",
        OA_STATUS_TAR_NAME_TOO_LONG => b"tar entry name exceeds its limit\0",
        OA_STATUS_TAR_READ_ERROR => b"tar input reader failed\0",
        OA_STATUS_ZIP_FORMAT => b"ZIP structure or member-name encoding is invalid\0",
        OA_STATUS_ZIP_TRUNCATED => b"ZIP archive or selected member is truncated\0",
        OA_STATUS_ZIP_UNSUPPORTED_METHOD => b"ZIP compression method is unsupported\0",
        OA_STATUS_ZIP_ENCRYPTED => b"encrypted ZIP entries are unsupported\0",
        OA_STATUS_ZIP_UNSAFE_PATH => b"ZIP member path is unsafe to extract\0",
        OA_STATUS_ZIP_LIMIT => b"ZIP archive exceeds an existing parser limit\0",
        OA_STATUS_ZIP_INFLATE_OR_SIZE => b"ZIP member decompression or size validation failed\0",
        OA_STATUS_ZIP_CRC => b"ZIP member CRC-32 validation failed\0",
        OA_STATUS_ZIP_BAD_ENTRY_INDEX => b"ZIP member index is outside the archive\0",
        _ => b"unknown status\0",
    }
}

#[no_mangle]
pub extern "C" fn oa_core_abi_version() -> u32 {
    ABI_VERSION_PROVISIONAL
}

/// Copies one deterministic NUL-terminated status message. `required` must be valid, aligned,
/// writable and non-overlapping with `buffer`. A null buffer is allowed only for a zero-capacity
/// size query. The library cannot validate arbitrary C addresses or the caller's capacity claim.
///
/// # Safety
/// `required` must point to valid, aligned, writable `u64` storage. If `capacity` is nonzero,
/// `buffer` must point to at least that many writable bytes. `required` and `buffer` must not overlap.
#[no_mangle]
pub unsafe extern "C" fn oa_core_error_message(
    code: u32,
    buffer: *mut u8,
    capacity: u64,
    required: *mut u64,
) -> u32 {
    if required.is_null() || (buffer.is_null() && capacity != 0) {
        return OA_STATUS_INVALID_ARGUMENT;
    }
    let text = message(code);
    let length = text.len() as u64;
    // SAFETY: required is non-null; its valid, aligned, writable and non-overlapping storage is
    // an explicit caller precondition for this function.
    unsafe { required.write(length) };
    if capacity < length {
        return OA_STATUS_BUFFER_TOO_SMALL;
    }
    // SAFETY: capacity is at least the static message length and buffer is non-null; the caller
    // guarantees `capacity` writable bytes, alignment is irrelevant for u8, and source/destination
    // are non-overlapping per the ABI contract.
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len()) };
    OA_STATUS_OK
}

/// Decodes one ASTERIX block to caller-owned fixed-layout records using the existing `oa-asterix`
/// core. CAT021 requires selector 24 or 27; CAT048/CAT062 require selector 0. Input is capped at
/// 65,535 bytes. A short-capacity query writes required counts and no records; other failures
/// preserve counts and record output. Caller-owned regions must be valid, aligned and disjoint.
///
/// # Safety
/// Every non-null pointer must refer to valid storage for its claimed extent. Input must remain
/// immutable for the call. Input, record, counts, detailed error and generic error regions must
/// be pairwise non-overlapping.
#[no_mangle]
pub unsafe extern "C" fn oa_asterix_decode_block(
    input: *const u8,
    input_length: u64,
    cat021_edition: u8,
    records: *mut OaAsterixRecord,
    record_capacity: u64,
    counts: *mut OaAsterixCounts,
    parse_error: *mut OaAsterixError,
    error: *mut OaErrorInfo,
) -> u32 {
    if error.is_null() {
        return OA_STATUS_INVALID_ARGUMENT;
    }
    let status = dispatch(error, || {
        if counts.is_null()
            || parse_error.is_null()
            || (input.is_null() && input_length != 0)
            || (records.is_null() && record_capacity != 0)
        {
            if !parse_error.is_null() {
                write_asterix_error(
                    parse_error,
                    asterix_error_record(OA_STATUS_INVALID_ARGUMENT, OA_AST_E_INVALID_ARGUMENT),
                );
            }
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        if input_length > asterix::MAX_INPUT_BYTES as u64
            || record_capacity > asterix::MAX_RECORDS as u64
        {
            write_asterix_error(
                parse_error,
                asterix_error_record(OA_STATUS_LENGTH_TOO_LARGE, OA_AST_E_LENGTH_TOO_LARGE),
            );
            return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
        }
        let length = usize::try_from(input_length)
            .map_err(|_| Failure::general(OA_STATUS_LENGTH_TOO_LARGE))?;
        // SAFETY: the C contract guarantees a readable `input_length` extent; null is accepted
        // only for an empty slice, which is constructed without dereferencing the pointer.
        let bytes = unsafe {
            if length == 0 {
                &[]
            } else {
                core::slice::from_raw_parts(input, length)
            }
        };
        let category = bytes.first().copied().unwrap_or(0);
        let edition = match (category, cat021_edition) {
            (21, 24) => oa_asterix::Cat021Edition::Ed24,
            (21, 27) => oa_asterix::Cat021Edition::Ed27,
            (category, 0) if category != 21 => oa_asterix::Cat021Edition::Ed27,
            _ => {
                let mut detail =
                    asterix_error_record(OA_STATUS_INVALID_ARGUMENT, OA_AST_E_INVALID_ARGUMENT);
                detail.category = category;
                write_asterix_error(parse_error, detail);
                return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
            }
        };
        let flat = match asterix::decode(bytes, edition) {
            Ok(flat) => flat,
            Err(asterix::ProjectionFailure::Decode(decode_error)) => {
                write_asterix_error(parse_error, map_asterix_error(decode_error));
                return Err(Failure::general(OA_STATUS_PARSE_ERROR));
            }
            Err(asterix::ProjectionFailure::Allocation) => {
                write_asterix_error(
                    parse_error,
                    asterix_error_record(OA_STATUS_ALLOCATION_FAILED, OA_AST_E_ALLOCATION_FAILED),
                );
                return Err(Failure::general(OA_STATUS_ALLOCATION_FAILED));
            }
        };
        if flat.records.len() > asterix::MAX_RECORDS {
            write_asterix_error(
                parse_error,
                asterix_error_record(OA_STATUS_LENGTH_TOO_LARGE, OA_AST_E_LENGTH_TOO_LARGE),
            );
            return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
        }
        let required = match u64::try_from(flat.records.len()) {
            Ok(value) => value,
            Err(_) => {
                write_asterix_error(
                    parse_error,
                    asterix_error_record(OA_STATUS_LENGTH_TOO_LARGE, OA_AST_E_LENGTH_TOO_LARGE),
                );
                return Err(Failure::general(OA_STATUS_LENGTH_TOO_LARGE));
            }
        };
        let mut reported = flat.counts;
        reported.required_record_count = required;
        reported.written_record_count = 0;
        if record_capacity < required {
            write_asterix_counts(counts, reported);
            let mut detail =
                asterix_error_record(OA_STATUS_BUFFER_TOO_SMALL, OA_AST_E_BUFFER_TOO_SMALL);
            detail.category = reported.category;
            write_asterix_error(parse_error, detail);
            return Err(Failure::general(OA_STATUS_BUFFER_TOO_SMALL));
        }
        if required != 0 && records.is_null() {
            let mut detail =
                asterix_error_record(OA_STATUS_INVALID_ARGUMENT, OA_AST_E_INVALID_ARGUMENT);
            detail.category = reported.category;
            write_asterix_error(parse_error, detail);
            return Err(Failure::general(OA_STATUS_INVALID_ARGUMENT));
        }
        // SAFETY: capacity and null checks establish the exact writable row extent; source rows
        // remain live, and the C contract requires non-overlapping input/output regions.
        unsafe {
            if !flat.records.is_empty() {
                core::ptr::copy_nonoverlapping(flat.records.as_ptr(), records, flat.records.len());
            }
        }
        reported.written_record_count = required;
        write_asterix_counts(counts, reported);
        Ok(())
    })
    .0;
    if status == OA_STATUS_INTERNAL_PANIC && !parse_error.is_null() {
        write_asterix_error(
            parse_error,
            asterix_error_record(OA_STATUS_INTERNAL_PANIC, OA_AST_E_INTERNAL_PANIC),
        );
    }
    status
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::*;

    const VECTORS: &str = include_str!("../tests/fixtures/wgs84.csv");
    const L0_VECTORS: &str = include_str!("../tests/fixtures/l0.csv");
    const POLYGON_VECTORS: &str = include_str!("../tests/fixtures/polygon.csv");

    fn number(field: &str) -> Option<f64> {
        if field.is_empty() {
            None
        } else {
            field.parse::<f64>().ok()
        }
    }

    fn vector_numbers(field: &str) -> Vec<f64> {
        if field.is_empty() {
            Vec::new()
        } else {
            field
                .split(';')
                .map(|part| part.parse::<f64>().expect("number"))
                .collect()
        }
    }

    // Exercise the same synthetic L0 vectors as the C and Python runners.
    // REQ: OA-FFI-002, OA-FFI-003
    #[test]
    fn ffi_l0_matches_the_shared_synthetic_vectors() {
        for (line_number, line) in L0_VECTORS.lines().enumerate().skip(1) {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            assert_eq!(fields.len(), 7, "L0 vector line {}", line_number + 1);
            if fields[1] == "sha256" {
                let input: Vec<u8> = fields[2]
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| {
                        let text = core::str::from_utf8(pair).expect("hex");
                        u8::from_str_radix(text, 16).expect("hex byte")
                    })
                    .collect();
                let mut digest = [0_u8; 32];
                let mut digest_hex = [0_u8; 64];
                let mut error = OaErrorInfo {
                    code: 99,
                    location_kind: 99,
                    location: 99,
                };
                let pointer = if input.is_empty() {
                    core::ptr::null()
                } else {
                    input.as_ptr()
                };
                // SAFETY: input bytes and fixed-size digest/error outputs are live, valid and distinct.
                let digest_status = unsafe {
                    oa_sha256_digest(pointer, input.len() as u64, digest.as_mut_ptr(), &mut error)
                };
                assert_eq!(digest_status, OA_STATUS_OK, "{}", fields[0]);
                let hex_status =
                    // SAFETY: digest and output/error arrays are live, valid and non-overlapping.
                    unsafe { oa_sha256_hex(digest.as_ptr(), digest_hex.as_mut_ptr(), &mut error) };
                assert_eq!(hex_status, OA_STATUS_OK, "{}", fields[0]);
                assert_eq!(
                    core::str::from_utf8(&digest_hex).expect("hex"),
                    fields[6],
                    "{}",
                    fields[0]
                );
                assert_eq!(error.code, OA_STATUS_OK, "{}", fields[0]);
                continue;
            }
            let args = vector_numbers(fields[2]);
            let mut actual = [0.0_f64; 4];
            let mut expected = vector_numbers(fields[3]);
            if !fields[4].is_empty() {
                expected.push(fields[4].parse::<f64>().expect("out2"));
            }
            let tolerance = fields[5].parse::<f64>().expect("tolerance");
            let mut error = OaErrorInfo {
                code: 99,
                location_kind: 99,
                location: 99,
            };
            // SAFETY: the synthetic test passes live local output/error storage per each signature.
            let status = unsafe {
                match fields[1] {
                    "math_unary" => {
                        oa_math_f64_unary(args[0] as u32, args[1], &mut actual[0], &mut error)
                    }
                    "math_binary" => oa_math_f64_binary(
                        args[0] as u32,
                        args[1],
                        args[2],
                        &mut actual[0],
                        &mut error,
                    ),
                    "math_ternary" => oa_math_f64_ternary(
                        args[0] as u32,
                        args[1],
                        args[2],
                        args[3],
                        &mut actual[0],
                        &mut error,
                    ),
                    "math_powi" => {
                        oa_math_f64_powi(args[0], args[1] as i32, &mut actual[0], &mut error)
                    }
                    "math_sin_cos" => {
                        oa_math_f64_sin_cos(args[0], &mut actual[0], &mut actual[1], &mut error)
                    }
                    "math_compare" => {
                        let mut value = 0_i32;
                        let status = oa_math_f64_compare(args[0], args[1], &mut value, &mut error);
                        actual[0] = value as f64;
                        status
                    }
                    "math_sign_negative" => {
                        let mut value = 0_u32;
                        let status = oa_math_f64_is_sign_negative(args[0], &mut value, &mut error);
                        actual[0] = value as f64;
                        status
                    }
                    "math_f32_unary" => {
                        let mut value = 0_f32;
                        let status = oa_math_f32_unary(
                            args[0] as u32,
                            args[1] as f32,
                            &mut value,
                            &mut error,
                        );
                        actual[0] = value as f64;
                        status
                    }
                    "math_f32_binary" => {
                        let mut value = 0_f32;
                        let status = oa_math_f32_binary(
                            args[0] as u32,
                            args[1] as f32,
                            args[2] as f32,
                            &mut value,
                            &mut error,
                        );
                        actual[0] = value as f64;
                        status
                    }
                    "math_f32_ternary" => {
                        let mut value = 0_f32;
                        let status = oa_math_f32_ternary(
                            args[0] as u32,
                            args[1] as f32,
                            args[2] as f32,
                            args[3] as f32,
                            &mut value,
                            &mut error,
                        );
                        actual[0] = value as f64;
                        status
                    }
                    "math_f32_compare" => {
                        let mut value = 0_i32;
                        let status = oa_math_f32_compare(
                            args[0] as f32,
                            args[1] as f32,
                            &mut value,
                            &mut error,
                        );
                        actual[0] = value as f64;
                        status
                    }
                    "math_f32_sign_negative" => {
                        let mut value = 0_u32;
                        let status =
                            oa_math_f32_is_sign_negative(args[0] as f32, &mut value, &mut error);
                        actual[0] = value as f64;
                        status
                    }
                    "rhumb_inverse" => oa_geo_wgs84_rhumb_inverse(
                        args[0],
                        args[1],
                        args[2],
                        args[3],
                        &mut actual[0],
                        &mut actual[1],
                        &mut error,
                    ),
                    "rhumb_direct" => oa_geo_wgs84_rhumb_direct(
                        args[0],
                        args[1],
                        args[2],
                        args[3],
                        &mut actual[0],
                        &mut actual[1],
                        &mut error,
                    ),
                    "rhumb_fraction" => oa_geo_wgs84_rhumb_point_at_fraction(
                        args[0],
                        args[1],
                        args[2],
                        args[3],
                        args[4],
                        &mut actual[0],
                        &mut actual[1],
                        &mut error,
                    ),
                    "separation" => oa_geo_wgs84_geodesic_rhumb_separation(
                        args[0],
                        args[1],
                        args[2],
                        args[3],
                        args[4] as u32,
                        &mut actual[0],
                        &mut actual[1],
                        &mut actual[2],
                        &mut actual[3],
                        &mut error,
                    ),
                    operation => panic!("unknown L0 vector operation {operation}"),
                }
            };
            assert_eq!(status, OA_STATUS_OK, "{}", fields[0]);
            assert_eq!(error.code, OA_STATUS_OK, "{}", fields[0]);
            assert_eq!(error.location_kind, OA_LOCATION_NONE, "{}", fields[0]);
            if fields[0] == "equator-zero-separation" {
                assert!(actual[1].is_finite() && (0.0..=1.0).contains(&actual[1]));
                let tied_actual = [actual[0], actual[2], actual[3]];
                assert_eq!(expected.len(), tied_actual.len(), "{}", fields[0]);
                for (got, want) in tied_actual.iter().zip(&expected) {
                    assert!(got.is_finite(), "{}", fields[0]);
                    assert!(
                        (got - want).abs() <= tolerance,
                        "{}: {} vs {}",
                        fields[0],
                        got,
                        want
                    );
                }
            } else {
                for (index, want) in expected.iter().enumerate() {
                    assert!(actual[index].is_finite(), "{}", fields[0]);
                    assert!(
                        (actual[index] - want).abs() <= tolerance,
                        "{}: {} vs {}",
                        fields[0],
                        actual[index],
                        want
                    );
                }
            }
        }
    }

    // The polygon handle uses the common synthetic C/Python fixtures.
    // REQ: OA-FFI-003
    #[test]
    fn ffi_polygon_matches_the_shared_synthetic_vectors() {
        for (line_number, line) in POLYGON_VECTORS.lines().enumerate().skip(1) {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            assert_eq!(fields.len(), 5, "polygon vector line {}", line_number + 1);
            let points: Vec<(f64, f64)> = fields[1]
                .split(';')
                .map(|pair| {
                    let mut values = pair.split('|');
                    (
                        values.next().expect("lat").parse().expect("lat"),
                        values.next().expect("lon").parse().expect("lon"),
                    )
                })
                .collect();
            let latitudes: Vec<f64> = points.iter().map(|point| point.0).collect();
            let longitudes: Vec<f64> = points.iter().map(|point| point.1).collect();
            let mut polygon = core::ptr::null_mut();
            let mut error = OaErrorInfo {
                code: 99,
                location_kind: 99,
                location: 99,
            };
            // SAFETY: coordinate slices and output/error locals are live, aligned and distinct.
            let status = unsafe {
                oa_geo_polygon_new_wgs84(
                    latitudes.as_ptr(),
                    longitudes.as_ptr(),
                    points.len() as u64,
                    &mut polygon,
                    &mut error,
                )
            };
            assert_eq!(status, OA_STATUS_OK, "{}", fields[0]);
            let mut edges = 0_u64;
            let mut inside = u32::MAX;
            let status_edges =
                // SAFETY: polygon is the live constructor result; `edges` and `error` are distinct locals.
                unsafe { oa_geo_polygon_edge_count(polygon, &mut edges, &mut error) };
            // SAFETY: polygon is live; `inside` and `error` are distinct locals and remain writable.
            let status_contains = unsafe {
                oa_geo_polygon_contains_wgs84(
                    polygon,
                    fields[2].parse().expect("lat"),
                    fields[3].parse().expect("lon"),
                    &mut inside,
                    &mut error,
                )
            };
            // SAFETY: this is the live handle returned above, freed once after the two queries.
            unsafe { oa_geo_polygon_free(polygon) };
            assert_eq!(status_edges, OA_STATUS_OK, "{}", fields[0]);
            assert_eq!(status_contains, OA_STATUS_OK, "{}", fields[0]);
            assert_eq!(edges, points.len() as u64, "{}", fields[0]);
            assert_eq!(
                inside,
                fields[4].parse::<u32>().expect("inside"),
                "{}",
                fields[0]
            );
        }
    }

    // REQ: OA-FFI-002
    #[test]
    fn ffi_matches_the_shared_synthetic_geodesy_vectors() {
        assert_eq!(oa_core_abi_version(), ABI_VERSION_PROVISIONAL);
        for (line_number, line) in VECTORS.lines().enumerate().skip(1) {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            assert_eq!(fields.len(), 12, "vector line {}", line_number + 1);
            let a = fields[1].parse::<f64>().expect("arg1");
            let b = fields[2].parse::<f64>().expect("arg2");
            let c = fields[3].parse::<f64>().expect("arg3");
            let d = fields[4].parse::<f64>().expect("arg4");
            let expected_status = fields[5].parse::<u32>().expect("status");
            let expected_location = fields[6].parse::<u64>().expect("location");
            let (mut out1, mut out2, mut out3) = (1234.0, 1234.0, 1234.0);
            let mut error = OaErrorInfo {
                code: 99,
                location_kind: 99,
                location: 99,
            };
            // SAFETY: the three local outputs and error record are valid, aligned, writable,
            // distinct objects with lifetimes covering this call; scalar inputs need no pointers.
            let status = unsafe {
                if fields[0] == "inverse" {
                    oa_geo_wgs84_inverse(a, b, c, d, &mut out1, &mut out2, &mut out3, &mut error)
                } else {
                    oa_geo_wgs84_direct(a, b, c, d, &mut out1, &mut out2, &mut out3, &mut error)
                }
            };
            assert_eq!(status, expected_status, "vector line {}", line_number + 1);
            assert_eq!(
                error.code,
                expected_status,
                "vector line {}",
                line_number + 1
            );
            if status == OA_STATUS_OK {
                assert_eq!(error.location_kind, OA_LOCATION_NONE);
                let outputs = [out1, out2, out3];
                let tolerance = fields[10].parse::<f64>().expect("tolerance");
                for (index, expected) in fields[7..10].iter().enumerate() {
                    if let Some(want) = number(expected) {
                        assert!(
                            (outputs[index] - want).abs() <= tolerance,
                            "vector {} output {}: got {} expected {} ± {}",
                            fields[11],
                            index,
                            outputs[index],
                            want,
                            tolerance
                        );
                    } else {
                        assert!(outputs[index].is_finite(), "vector {}", fields[11]);
                    }
                }
            } else {
                assert_eq!(error.location_kind, OA_LOCATION_ARGUMENT, "{}", fields[11]);
                assert_eq!(error.location, expected_location, "{}", fields[11]);
                assert_eq!((out1, out2, out3), (1234.0, 1234.0, 1234.0));
            }
        }
    }

    // REQ: OA-FFI-001
    #[test]
    fn null_output_is_refused_and_message_copy_is_bounded() {
        let (mut out2, mut out3) = (1234.0, 1234.0);
        let mut error = OaErrorInfo {
            code: 99,
            location_kind: 99,
            location: 99,
        };
        // SAFETY: error and the two non-null outputs are valid distinct local objects; the null
        // first output is deliberately passed to exercise the documented null refusal path.
        let status = unsafe {
            oa_geo_wgs84_inverse(
                0.0,
                0.0,
                0.0,
                1.0,
                core::ptr::null_mut(),
                &mut out2,
                &mut out3,
                &mut error,
            )
        };
        assert_eq!(status, OA_STATUS_INVALID_ARGUMENT);
        assert_eq!(error.location_kind, OA_LOCATION_ARGUMENT);
        assert_eq!(error.location, 4);
        assert_eq!((out2, out3), (1234.0, 1234.0));

        let mut required = 0_u64;
        let needed = message(OA_STATUS_INVALID_ARGUMENT).len() as u64;
        // SAFETY: `required` is a valid writable local; null buffer with capacity zero is the
        // documented size-query form and does not dereference a buffer pointer.
        let query = unsafe {
            oa_core_error_message(
                OA_STATUS_INVALID_ARGUMENT,
                core::ptr::null_mut(),
                0,
                &mut required,
            )
        };
        assert_eq!(query, OA_STATUS_BUFFER_TOO_SMALL);
        assert_eq!(required, needed);
    }

    // Exercise query, refusal and exact-capacity message-copy semantics.
    // REQ: OA-FFI-001
    #[test]
    fn error_message_capacity_is_all_or_nothing_and_includes_nul() {
        let text = message(OA_STATUS_INVALID_ARGUMENT);
        let needed = text.len() as u64;
        assert_eq!(text.last(), Some(&0));
        let mut buffer = [0xA5_u8; 64];
        let mut required = u64::MAX;

        // SAFETY: required is a valid aligned writable local; null buffer at zero capacity is the
        // documented size query and does not dereference a destination buffer.
        let query = unsafe {
            oa_core_error_message(
                OA_STATUS_INVALID_ARGUMENT,
                core::ptr::null_mut(),
                0,
                &mut required,
            )
        };
        assert_eq!(query, OA_STATUS_BUFFER_TOO_SMALL);
        assert_eq!(required, needed);

        required = u64::MAX;
        // SAFETY: buffer is valid, aligned and writable; required is a separate valid local.
        // The null buffer with nonzero capacity is deliberately refused before either write.
        let null_refusal = unsafe {
            oa_core_error_message(
                OA_STATUS_INVALID_ARGUMENT,
                core::ptr::null_mut(),
                1,
                &mut required,
            )
        };
        assert_eq!(null_refusal, OA_STATUS_INVALID_ARGUMENT);
        assert_eq!(required, u64::MAX);
        assert_eq!(buffer, [0xA5; 64]);

        required = 0;
        // SAFETY: buffer and required are valid, aligned, writable and non-overlapping; the
        // deliberately short capacity is within buffer but shorter than the static message.
        let short = unsafe {
            oa_core_error_message(
                OA_STATUS_INVALID_ARGUMENT,
                buffer.as_mut_ptr(),
                needed - 1,
                &mut required,
            )
        };
        assert_eq!(short, OA_STATUS_BUFFER_TOO_SMALL);
        assert_eq!(required, needed);
        assert_eq!(buffer, [0xA5; 64]);

        required = 0;
        // SAFETY: buffer has more than `needed` writable bytes; only the exact declared capacity
        // is passed and required is a distinct valid local.
        let exact = unsafe {
            oa_core_error_message(
                OA_STATUS_INVALID_ARGUMENT,
                buffer.as_mut_ptr(),
                needed,
                &mut required,
            )
        };
        assert_eq!(exact, OA_STATUS_OK);
        assert_eq!(required, needed);
        assert_eq!(buffer.get(..text.len()), Some(text));
        assert!(buffer.iter().skip(text.len()).all(|byte| *byte == 0xA5));
    }
}
