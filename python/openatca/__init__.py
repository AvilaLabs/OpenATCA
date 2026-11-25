"""Provisional ctypes surface for OpenATCA's L0 C ABI.

PROVISIONAL / UNVALIDATED: this wrapper targets the experimental Rust FFI shell. Importing this
module loads nothing. Call :func:`load` with an explicit path to a separately built library; the
ABI names and layout are not stable or published.
"""

from __future__ import annotations

import ctypes
import os
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Final, Iterable, Sequence

ABI_VERSION_PROVISIONAL: Final[int] = 1
_MAX_MESSAGE_BYTES: Final[int] = 4096
_MAX_POLYGON_VERTICES: Final[int] = 100_001


class _ErrorInfo(ctypes.Structure):
    _fields_ = [
        ("code", ctypes.c_uint32),
        ("location_kind", ctypes.c_uint32),
        ("location", ctypes.c_uint64),
    ]


_STATUS_NAMES: Final[dict[int, str]] = {
    0: "OK",
    1: "INVALID_ARGUMENT",
    2: "NON_FINITE",
    3: "LATITUDE_OUT_OF_RANGE",
    4: "LONGITUDE_OUT_OF_RANGE",
    5: "NO_CONVERGENCE",
    6: "BUFFER_TOO_SMALL",
    7: "INTERNAL_PANIC",
    8: "BAD_PARAMETER",
    9: "LENGTH_TOO_LARGE",
    10: "BAD_OPERATION",
    11: "ALLOCATION_FAILED",
}


class OpenAtcaError(Exception):
    """A nonzero status returned by the provisional native ABI."""

    def __init__(self, code: int, location_kind: int = 0, location: int = 0,
                 message: str | None = None) -> None:
        self.code = int(code)
        self.status = _STATUS_NAMES.get(self.code, "UNKNOWN_STATUS")
        self.location_kind = int(location_kind)
        self.location = int(location)
        self.native_message = message
        detail = f"OpenATCA status {self.status} ({self.code})"
        if self.location_kind == 1:
            detail += f" at argument {self.location}"
        elif self.location_kind == 2:
            detail += f" at element {self.location}"
        if message:
            detail += f": {message}"
        super().__init__(detail)


@dataclass(frozen=True, slots=True)
class InverseResult:
    """WGS-84 inverse result; distance is metres and azimuths are degrees."""

    distance_m: float
    azimuth1_deg: float
    azimuth2_deg: float


@dataclass(frozen=True, slots=True)
class DirectResult:
    """WGS-84 direct result; coordinates and terminal azimuth are degrees."""

    lat2_deg: float
    lon2_deg: float
    azimuth2_deg: float


@dataclass(frozen=True, slots=True)
class RhumbInverseResult:
    """WGS-84 rhumb distance in metres and constant course in degrees."""

    distance_m: float
    course_deg: float


@dataclass(frozen=True, slots=True)
class SeparationResult:
    """Maximum rhumb/geodesic separation and the corresponding fraction and path lengths."""

    max_m: float
    at_fraction: float
    geodesic_m: float
    rhumb_m: float


class _PolygonOpaque(ctypes.Structure):
    pass


def _as_double(value: object, name: str) -> float:
    if isinstance(value, bool):
        raise TypeError(f"{name} must be a real number, not bool")
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError, OverflowError) as exc:
        raise TypeError(f"{name} must be convertible to a C double") from exc


class ScalarMath:
    """Python scalar façade over the source-defined provisional `oa-math` operation groups."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        self._bind()

    def _bind(self) -> None:
        lib = self._library
        error_p = ctypes.POINTER(_ErrorInfo)
        d_p = ctypes.POINTER(ctypes.c_double)
        f_p = ctypes.POINTER(ctypes.c_float)
        i_p = ctypes.POINTER(ctypes.c_int32)
        u_p = ctypes.POINTER(ctypes.c_uint32)
        for name, signature in {
            "oa_math_f64_unary": [ctypes.c_uint32, ctypes.c_double, d_p, error_p],
            "oa_math_f64_binary": [ctypes.c_uint32, ctypes.c_double, ctypes.c_double, d_p, error_p],
            "oa_math_f64_ternary": [ctypes.c_uint32, ctypes.c_double, ctypes.c_double, ctypes.c_double, d_p, error_p],
            "oa_math_f64_powi": [ctypes.c_double, ctypes.c_int32, d_p, error_p],
            "oa_math_f64_sin_cos": [ctypes.c_double, d_p, d_p, error_p],
            "oa_math_f64_compare": [ctypes.c_double, ctypes.c_double, i_p, error_p],
            "oa_math_f64_is_sign_negative": [ctypes.c_double, u_p, error_p],
            "oa_math_f32_unary": [ctypes.c_uint32, ctypes.c_float, f_p, error_p],
            "oa_math_f32_binary": [ctypes.c_uint32, ctypes.c_float, ctypes.c_float, f_p, error_p],
            "oa_math_f32_ternary": [ctypes.c_uint32, ctypes.c_float, ctypes.c_float, ctypes.c_float, f_p, error_p],
            "oa_math_f32_compare": [ctypes.c_float, ctypes.c_float, i_p, error_p],
            "oa_math_f32_is_sign_negative": [ctypes.c_float, u_p, error_p],
        }.items():
            function = getattr(lib, name)
            function.argtypes = signature
            function.restype = ctypes.c_uint32

    def _unary(self, operation: int, value: object) -> float:
        output, error = ctypes.c_double(), _ErrorInfo()
        status = int(self._library.oa_math_f64_unary(operation, _as_double(value, "value"), ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return output.value

    def _binary(self, operation: int, left: object, right: object) -> float:
        output, error = ctypes.c_double(), _ErrorInfo()
        status = int(self._library.oa_math_f64_binary(operation, _as_double(left, "left"), _as_double(right, "right"), ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return output.value

    def sin(self, x: object) -> float: return self._unary(1, x)
    def cos(self, x: object) -> float: return self._unary(2, x)
    def tan(self, x: object) -> float: return self._unary(3, x)
    def atan(self, x: object) -> float: return self._unary(4, x)
    def asin(self, x: object) -> float: return self._unary(5, x)
    def acos(self, x: object) -> float: return self._unary(6, x)
    def exp(self, x: object) -> float: return self._unary(7, x)
    def ln(self, x: object) -> float: return self._unary(8, x)
    def cbrt(self, x: object) -> float: return self._unary(9, x)
    def canonical_nan(self, x: object) -> float: return self._unary(10, x)
    def sqrt(self, x: object) -> float: return self._unary(11, x)
    def floor(self, x: object) -> float: return self._unary(12, x)
    def ceil(self, x: object) -> float: return self._unary(13, x)
    def trunc(self, x: object) -> float: return self._unary(14, x)
    def round(self, x: object) -> float: return self._unary(15, x)
    def signum(self, x: object) -> float: return self._unary(16, x)

    def atan2(self, y: object, x: object) -> float: return self._binary(1, y, x)
    def pow(self, x: object, y: object) -> float: return self._binary(2, x, y)
    def hypot(self, x: object, y: object) -> float: return self._binary(3, x, y)
    def rem_euclid(self, x: object, y: object) -> float: return self._binary(4, x, y)
    def min(self, x: object, y: object) -> float: return self._binary(5, x, y)
    def max(self, x: object, y: object) -> float: return self._binary(6, x, y)
    def copysign(self, x: object, sign: object) -> float: return self._binary(7, x, sign)

    def powi(self, x: object, exponent: int) -> float:
        if isinstance(exponent, bool) or not isinstance(exponent, int):
            raise TypeError("exponent must be an integer")
        if exponent < -(1 << 31) or exponent >= (1 << 31):
            raise OverflowError("exponent does not fit signed 32-bit range")
        output, error = ctypes.c_double(), _ErrorInfo()
        status = int(self._library.oa_math_f64_powi(_as_double(x, "x"), exponent, ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return output.value

    def sin_cos(self, x: object) -> tuple[float, float]:
        sine, cosine, error = ctypes.c_double(), ctypes.c_double(), _ErrorInfo()
        status = int(self._library.oa_math_f64_sin_cos(_as_double(x, "x"), ctypes.byref(sine), ctypes.byref(cosine), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return sine.value, cosine.value

    def compare(self, left: object, right: object) -> int:
        order, error = ctypes.c_int32(), _ErrorInfo()
        status = int(self._library.oa_math_f64_compare(_as_double(left, "left"), _as_double(right, "right"), ctypes.byref(order), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return order.value

    def is_sign_negative(self, value: object) -> bool:
        result, error = ctypes.c_uint32(), _ErrorInfo()
        status = int(self._library.oa_math_f64_is_sign_negative(_as_double(value, "value"), ctypes.byref(result), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return bool(result.value)

    def clamp(self, value: object, lower: object, upper: object) -> float:
        output, error = ctypes.c_double(), _ErrorInfo()
        args = tuple(_as_double(x, name) for x, name in ((value, "value"), (lower, "lower"), (upper, "upper")))
        status = int(self._library.oa_math_f64_ternary(1, *args, ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return output.value

    def _f32_unary(self, operation: int, value: object) -> float:
        output, error = ctypes.c_float(), _ErrorInfo()
        status = int(self._library.oa_math_f32_unary(operation, ctypes.c_float(_as_double(value, "value")), ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return float(output.value)

    def _f32_binary(self, operation: int, left: object, right: object) -> float:
        output, error = ctypes.c_float(), _ErrorInfo()
        status = int(self._library.oa_math_f32_binary(operation, ctypes.c_float(_as_double(left, "left")), ctypes.c_float(_as_double(right, "right")), ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return float(output.value)

    def canonical_nan_f32(self, value: object) -> float: return self._f32_unary(1, value)
    def signum_f32(self, value: object) -> float: return self._f32_unary(2, value)
    def copysign_f32(self, value: object, sign: object) -> float: return self._f32_binary(1, value, sign)
    def min_f32(self, left: object, right: object) -> float: return self._f32_binary(2, left, right)
    def max_f32(self, left: object, right: object) -> float: return self._f32_binary(3, left, right)

    def compare_f32(self, left: object, right: object) -> int:
        output, error = ctypes.c_int32(), _ErrorInfo()
        status = int(self._library.oa_math_f32_compare(ctypes.c_float(_as_double(left, "left")), ctypes.c_float(_as_double(right, "right")), ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return output.value

    def is_sign_negative_f32(self, value: object) -> bool:
        output, error = ctypes.c_uint32(), _ErrorInfo()
        status = int(self._library.oa_math_f32_is_sign_negative(ctypes.c_float(_as_double(value, "value")), ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return bool(output.value)

    def clamp_f32(self, value: object, lower: object, upper: object) -> float:
        output, error = ctypes.c_float(), _ErrorInfo()
        args = tuple(ctypes.c_float(_as_double(x, name)) for x, name in ((value, "value"), (lower, "lower"), (upper, "upper")))
        status = int(self._library.oa_math_f32_ternary(1, *args, ctypes.byref(output), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return float(output.value)


class Integrity:
    """One-shot SHA-256 digest and lowercase hexadecimal integrity helpers."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        library.oa_sha256_digest.argtypes = [ctypes.POINTER(ctypes.c_uint8), ctypes.c_uint64, ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(_ErrorInfo)]
        library.oa_sha256_digest.restype = ctypes.c_uint32
        library.oa_sha256_hex.argtypes = [ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(_ErrorInfo)]
        library.oa_sha256_hex.restype = ctypes.c_uint32

    def sha256(self, data: bytes | bytearray | memoryview) -> bytes:
        raw = bytes(data)
        source = (ctypes.c_uint8 * len(raw)).from_buffer_copy(raw) if raw else None
        digest, error = (ctypes.c_uint8 * 32)(), _ErrorInfo()
        status = int(self._library.oa_sha256_digest(source, len(raw), digest, ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return bytes(digest)

    def sha256_hex(self, digest: bytes | bytearray | memoryview) -> str:
        raw = bytes(digest)
        if len(raw) != 32:
            raise ValueError("SHA-256 digest must contain exactly 32 bytes")
        source = (ctypes.c_uint8 * 32).from_buffer_copy(raw)
        output, error = (ctypes.c_uint8 * 64)(), _ErrorInfo()
        status = int(self._library.oa_sha256_hex(source, output, ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return bytes(output).decode("ascii", errors="strict")


class Polygon:
    """Immutable prepared WGS-84 geodesic-edge polygon with explicit native handle lifetime."""

    def __init__(self, library: ctypes.CDLL, vertices: Iterable[tuple[object, object]]) -> None:
        self._library = library
        points_list: list[tuple[float, float]] = []
        for index, (latitude, longitude) in enumerate(vertices):
            if index >= _MAX_POLYGON_VERTICES:
                raise ValueError(f"polygon supports at most {_MAX_POLYGON_VERTICES} vertices")
            points_list.append((_as_double(latitude, "latitude"), _as_double(longitude, "longitude")))
        points = tuple(points_list)
        latitudes = (ctypes.c_double * len(points))(*(point[0] for point in points))
        longitudes = (ctypes.c_double * len(points))(*(point[1] for point in points))
        handle = ctypes.c_void_p()
        error = _ErrorInfo()
        status = int(library.oa_geo_polygon_new_wgs84(latitudes, longitudes, len(points), ctypes.byref(handle), ctypes.byref(error)))
        _raise_if_error(library, status, error)
        self._handle = handle
        self._lock = threading.Lock()

    def _require_open(self) -> ctypes.c_void_p:
        """Return the handle while the caller holds `_lock`."""
        if not self._handle:
            raise RuntimeError("polygon handle is closed")
        return self._handle

    @property
    def edge_count(self) -> int:
        count, error = ctypes.c_uint64(), _ErrorInfo()
        with self._lock:
            handle = self._require_open()
            status = int(self._library.oa_geo_polygon_edge_count(handle, ctypes.byref(count), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return int(count.value)

    def contains(self, latitude: object, longitude: object) -> bool:
        inside, error = ctypes.c_uint32(), _ErrorInfo()
        lat = _as_double(latitude, "latitude")
        lon = _as_double(longitude, "longitude")
        with self._lock:
            handle = self._require_open()
            status = int(self._library.oa_geo_polygon_contains_wgs84(handle, lat, lon, ctypes.byref(inside), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return bool(inside.value)

    def close(self) -> None:
        with self._lock:
            if self._handle:
                handle = self._handle
                self._library.oa_geo_polygon_free(handle)
                self._handle = ctypes.c_void_p()

    def __enter__(self) -> "Polygon":
        with self._lock:
            self._require_open()
        return self

    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> None:
        self.close()


def _raise_if_error(library: ctypes.CDLL, status: int, error: _ErrorInfo) -> None:
    if status == 0:
        return
    if int(error.code) != status:
        raise RuntimeError("native ABI violated its provisional status/error-code contract")
    message = _native_message(library, status)
    raise OpenAtcaError(status, error.location_kind, int(error.location), message)


def _native_message(library: ctypes.CDLL, status: int) -> str | None:
    required = ctypes.c_uint64()
    query = int(library.oa_core_error_message(status, None, 0, ctypes.byref(required)))
    if query not in (0, 6) or required.value == 0 or required.value > _MAX_MESSAGE_BYTES:
        return None
    buffer = (ctypes.c_uint8 * int(required.value))()
    copied = int(library.oa_core_error_message(status, buffer, required.value, ctypes.byref(required)))
    if copied != 0:
        return None
    raw = bytes(buffer)
    if not raw.endswith(b"\0"):
        return None
    try:
        return raw[:-1].decode("utf-8", errors="strict")
    except UnicodeDecodeError:
        return None


class Geodesy:
    """Calls the provisional stateless WGS-84 symbols in an explicitly loaded library."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        self._bind()
        version = int(self._library.oa_core_abi_version())
        if version != ABI_VERSION_PROVISIONAL:
            raise RuntimeError(
                f"unsupported provisional OpenATCA ABI version {version}; "
                f"expected {ABI_VERSION_PROVISIONAL}"
            )
        self.math = ScalarMath(library)
        self.integrity = Integrity(library)

    def _bind(self) -> None:
        lib = self._library
        lib.oa_core_abi_version.argtypes = []
        lib.oa_core_abi_version.restype = ctypes.c_uint32

        double_p = ctypes.POINTER(ctypes.c_double)
        error_p = ctypes.POINTER(_ErrorInfo)
        lib.oa_geo_wgs84_inverse.argtypes = [
            ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double,
            double_p, double_p, double_p, error_p,
        ]
        lib.oa_geo_wgs84_inverse.restype = ctypes.c_uint32
        lib.oa_geo_wgs84_direct.argtypes = [
            ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double,
            double_p, double_p, double_p, error_p,
        ]
        lib.oa_geo_wgs84_direct.restype = ctypes.c_uint32
        error_p = ctypes.POINTER(_ErrorInfo)
        d_p = ctypes.POINTER(ctypes.c_double)
        u64_p = ctypes.POINTER(ctypes.c_uint64)
        u32_p = ctypes.POINTER(ctypes.c_uint32)
        polygon_p = ctypes.c_void_p
        lib.oa_geo_wgs84_rhumb_inverse.argtypes = [ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double, d_p, d_p, error_p]
        lib.oa_geo_wgs84_rhumb_inverse.restype = ctypes.c_uint32
        lib.oa_geo_wgs84_rhumb_direct.argtypes = [ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double, d_p, d_p, error_p]
        lib.oa_geo_wgs84_rhumb_direct.restype = ctypes.c_uint32
        lib.oa_geo_wgs84_rhumb_point_at_fraction.argtypes = [ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double, d_p, d_p, error_p]
        lib.oa_geo_wgs84_rhumb_point_at_fraction.restype = ctypes.c_uint32
        lib.oa_geo_wgs84_geodesic_rhumb_separation.argtypes = [ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_double, ctypes.c_uint32, d_p, d_p, d_p, d_p, error_p]
        lib.oa_geo_wgs84_geodesic_rhumb_separation.restype = ctypes.c_uint32
        lib.oa_geo_polygon_new_wgs84.argtypes = [d_p, d_p, ctypes.c_uint64, ctypes.POINTER(polygon_p), error_p]
        lib.oa_geo_polygon_new_wgs84.restype = ctypes.c_uint32
        lib.oa_geo_polygon_contains_wgs84.argtypes = [polygon_p, ctypes.c_double, ctypes.c_double, u32_p, error_p]
        lib.oa_geo_polygon_contains_wgs84.restype = ctypes.c_uint32
        lib.oa_geo_polygon_edge_count.argtypes = [polygon_p, u64_p, error_p]
        lib.oa_geo_polygon_edge_count.restype = ctypes.c_uint32
        lib.oa_geo_polygon_free.argtypes = [polygon_p]
        lib.oa_geo_polygon_free.restype = None
        lib.oa_core_error_message.argtypes = [
            ctypes.c_uint32,
            ctypes.POINTER(ctypes.c_uint8),
            ctypes.c_uint64,
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.oa_core_error_message.restype = ctypes.c_uint32

    @classmethod
    def load(cls, library_path: os.PathLike[str] | str) -> "Geodesy":
        """Load only the caller-selected native library; never builds or searches for one."""
        path = Path(library_path)
        if not path.is_absolute():
            raise ValueError("library_path must be absolute; implicit search is disabled")
        if not path.is_file():
            raise FileNotFoundError(path)
        return cls(ctypes.CDLL(str(path)))

    def inverse(self, lat1_deg: object, lon1_deg: object,
                lat2_deg: object, lon2_deg: object) -> InverseResult:
        """Solve WGS-84 inverse; azimuth is clockwise from north in (-180, 180]."""
        a = _as_double(lat1_deg, "lat1_deg")
        b = _as_double(lon1_deg, "lon1_deg")
        c = _as_double(lat2_deg, "lat2_deg")
        d = _as_double(lon2_deg, "lon2_deg")
        distance, azi1, azi2 = ctypes.c_double(), ctypes.c_double(), ctypes.c_double()
        error = _ErrorInfo()
        status = int(self._library.oa_geo_wgs84_inverse(
            a, b, c, d, ctypes.byref(distance), ctypes.byref(azi1),
            ctypes.byref(azi2), ctypes.byref(error),
        ))
        self._raise_if_error(status, error)
        return InverseResult(distance.value, azi1.value, azi2.value)

    def direct(self, lat1_deg: object, lon1_deg: object,
               azimuth1_deg: object, distance_m: object) -> DirectResult:
        """Solve WGS-84 direct; negative finite distance follows the native geodesy contract."""
        a = _as_double(lat1_deg, "lat1_deg")
        b = _as_double(lon1_deg, "lon1_deg")
        c = _as_double(azimuth1_deg, "azimuth1_deg")
        d = _as_double(distance_m, "distance_m")
        lat2, lon2, azi2 = ctypes.c_double(), ctypes.c_double(), ctypes.c_double()
        error = _ErrorInfo()
        status = int(self._library.oa_geo_wgs84_direct(
            a, b, c, d, ctypes.byref(lat2), ctypes.byref(lon2),
            ctypes.byref(azi2), ctypes.byref(error),
        ))
        self._raise_if_error(status, error)
        return DirectResult(lat2.value, lon2.value, azi2.value)

    def rhumb_inverse(self, lat1_deg: object, lon1_deg: object, lat2_deg: object, lon2_deg: object) -> RhumbInverseResult:
        distance, course, error = ctypes.c_double(), ctypes.c_double(), _ErrorInfo()
        values = tuple(_as_double(value, name) for value, name in ((lat1_deg, "lat1_deg"), (lon1_deg, "lon1_deg"), (lat2_deg, "lat2_deg"), (lon2_deg, "lon2_deg")))
        status = int(self._library.oa_geo_wgs84_rhumb_inverse(*values, ctypes.byref(distance), ctypes.byref(course), ctypes.byref(error)))
        self._raise_if_error(status, error)
        return RhumbInverseResult(distance.value, course.value)

    def rhumb_direct(self, lat1_deg: object, lon1_deg: object, course_deg: object, distance_m: object) -> tuple[float, float]:
        latitude, longitude, error = ctypes.c_double(), ctypes.c_double(), _ErrorInfo()
        values = tuple(_as_double(value, name) for value, name in ((lat1_deg, "lat1_deg"), (lon1_deg, "lon1_deg"), (course_deg, "course_deg"), (distance_m, "distance_m")))
        status = int(self._library.oa_geo_wgs84_rhumb_direct(*values, ctypes.byref(latitude), ctypes.byref(longitude), ctypes.byref(error)))
        self._raise_if_error(status, error)
        return latitude.value, longitude.value

    def rhumb_point_at_fraction(self, lat1_deg: object, lon1_deg: object, lat2_deg: object, lon2_deg: object, fraction: object) -> tuple[float, float]:
        latitude, longitude, error = ctypes.c_double(), ctypes.c_double(), _ErrorInfo()
        values = tuple(_as_double(value, name) for value, name in ((lat1_deg, "lat1_deg"), (lon1_deg, "lon1_deg"), (lat2_deg, "lat2_deg"), (lon2_deg, "lon2_deg"), (fraction, "fraction")))
        status = int(self._library.oa_geo_wgs84_rhumb_point_at_fraction(*values, ctypes.byref(latitude), ctypes.byref(longitude), ctypes.byref(error)))
        self._raise_if_error(status, error)
        return latitude.value, longitude.value

    def geodesic_rhumb_separation(self, lat1_deg: object, lon1_deg: object, lat2_deg: object, lon2_deg: object, samples: int) -> SeparationResult:
        if isinstance(samples, bool) or not isinstance(samples, int):
            raise TypeError("samples must be an integer")
        if samples < 0 or samples >= (1 << 32):
            raise OverflowError("samples does not fit unsigned 32-bit range")
        outputs = [ctypes.c_double() for _ in range(4)]
        error = _ErrorInfo()
        values = tuple(_as_double(value, name) for value, name in ((lat1_deg, "lat1_deg"), (lon1_deg, "lon1_deg"), (lat2_deg, "lat2_deg"), (lon2_deg, "lon2_deg")))
        status = int(self._library.oa_geo_wgs84_geodesic_rhumb_separation(*values, samples, *(ctypes.byref(output) for output in outputs), ctypes.byref(error)))
        self._raise_if_error(status, error)
        return SeparationResult(*(output.value for output in outputs))

    def polygon(self, vertices: Sequence[tuple[object, object]]) -> Polygon:
        return Polygon(self._library, vertices)

    def _raise_if_error(self, status: int, error: _ErrorInfo) -> None:
        _raise_if_error(self._library, status, error)

    def _message(self, status: int) -> str | None:
        return _native_message(self._library, status)


def load(library_path: os.PathLike[str] | str) -> Geodesy:
    """Explicit-path loader; this package does not bundle or build the native library."""
    return Geodesy.load(library_path)


__all__ = [
    "DirectResult",
    "Integrity",
    "Polygon",
    "Geodesy",
    "InverseResult",
    "OpenAtcaError",
    "RhumbInverseResult",
    "ScalarMath",
    "SeparationResult",
    "load",
]
