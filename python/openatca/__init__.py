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
_MAX_TEXT_OUTPUT_BYTES: Final[int] = 256 * 1024 * 1024
_MAX_DEFLATE_INPUT_BYTES: Final[int] = 64 * 1024 * 1024
_MAX_DEFLATE_OUTPUT_BYTES: Final[int] = 256 * 1024 * 1024
_MAX_DEFLATE_ENCODE_OUTPUT_BYTES: Final[int] = 128 * 1024 * 1024


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
    12: "PARSE_ERROR",
    13: "DEFLATE_ERROR",
    14: "DEFLATE_TRUNCATED",
    15: "DEFLATE_OUTPUT_LIMIT",
    16: "DEFLATE_RATIO_LIMIT",
    17: "DEFLATE_ENCODE_OUTPUT_LIMIT",
}


class OpenAtcaError(Exception):
    """A nonzero status returned by the provisional native ABI."""

    def __init__(self, code: int, location_kind: int = 0, location: int = 0,
                 message: str | None = None, *, parser_module: int | None = None,
                 parser_kind: int | None = None, parser_position: int | None = None,
                 parser_detail_a: int = 0, parser_detail_b: int = 0) -> None:
        self.code = int(code)
        self.status = _STATUS_NAMES.get(self.code, "UNKNOWN_STATUS")
        self.location_kind = int(location_kind)
        self.location = int(location)
        self.native_message = message
        self.parser_module = parser_module
        self.parser_kind = parser_kind
        self.parser_position = parser_position
        self.parser_detail_a = parser_detail_a
        self.parser_detail_b = parser_detail_b
        detail = f"OpenATCA status {self.status} ({self.code})"
        if self.location_kind == 1:
            detail += f" at argument {self.location}"
        elif self.location_kind == 2:
            detail += f" at element {self.location}"
        if message:
            detail += f": {message}"
        if parser_module is not None:
            detail += (f" (parser module {parser_module}, kind {parser_kind}, "
                       f"position {parser_position})")
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


@dataclass(frozen=True, slots=True)
class RawInflateResult:
    output: bytes
    consumed: int


@dataclass(frozen=True, slots=True)
class RawDeflateResult:
    output: bytes
    consumed: int
    actual_level: int


@dataclass(frozen=True, slots=True)
class JsonNode:
    kind: int
    first_child: int | None
    child_count: int
    subtree_end: int
    value_offset: int
    value_length: int


@dataclass(frozen=True, slots=True)
class JsonResult:
    nodes: tuple[JsonNode, ...]
    value_pool: bytes


@dataclass(frozen=True, slots=True)
class XmlSpan:
    start: int
    length: int


@dataclass(frozen=True, slots=True)
class XmlElement:
    name: XmlSpan
    prefix: XmlSpan
    parent: int
    first_attribute: int
    attribute_count: int
    first_child: int
    next_sibling: int
    namespace_head: int


@dataclass(frozen=True, slots=True)
class XmlAttribute:
    name: XmlSpan
    prefix: XmlSpan
    raw_value: XmlSpan
    owner_element: int
    flags: int


@dataclass(frozen=True, slots=True)
class XmlText:
    raw_value: XmlSpan
    owner_element: int
    flags: int


@dataclass(frozen=True, slots=True)
class XmlNamespace:
    owner_element: int
    prefix: XmlSpan
    uri_offset: int
    uri_length: int
    next_for_element: int


@dataclass(frozen=True, slots=True)
class XmlResult:
    elements: tuple[XmlElement, ...]
    attributes: tuple[XmlAttribute, ...]
    texts: tuple[XmlText, ...]
    namespaces: tuple[XmlNamespace, ...]
    namespace_uri_pool: bytes


@dataclass(frozen=True, slots=True)
class CsvRow:
    physical_line: int
    first_field: int
    field_count: int


@dataclass(frozen=True, slots=True)
class CsvField:
    value_offset: int
    value_length: int


@dataclass(frozen=True, slots=True)
class CsvResult:
    rows: tuple[CsvRow, ...]
    fields: tuple[CsvField, ...]
    value_pool: bytes


class _TextError(ctypes.Structure):
    _fields_ = [("module", ctypes.c_uint32), ("kind", ctypes.c_uint32),
                ("position", ctypes.c_uint64), ("detail_a", ctypes.c_uint64),
                ("detail_b", ctypes.c_uint64)]


class _JsonNode(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_uint32), ("first_child", ctypes.c_uint32),
                ("child_count", ctypes.c_uint32), ("subtree_end", ctypes.c_uint32),
                ("value_offset", ctypes.c_uint32), ("value_length", ctypes.c_uint32)]


class _JsonCounts(ctypes.Structure):
    _fields_ = [("node_count", ctypes.c_uint64), ("value_bytes", ctypes.c_uint64)]


class _XmlSpan(ctypes.Structure):
    _fields_ = [("start", ctypes.c_uint32), ("length", ctypes.c_uint32)]


class _XmlElement(ctypes.Structure):
    _fields_ = [("name", _XmlSpan), ("prefix", _XmlSpan), ("parent", ctypes.c_uint32),
                ("first_attribute", ctypes.c_uint32), ("attribute_count", ctypes.c_uint32),
                ("first_child", ctypes.c_uint32), ("next_sibling", ctypes.c_uint32),
                ("namespace_head", ctypes.c_uint32)]


class _XmlAttribute(ctypes.Structure):
    _fields_ = [("name", _XmlSpan), ("prefix", _XmlSpan), ("raw_value", _XmlSpan),
                ("owner_element", ctypes.c_uint32), ("flags", ctypes.c_uint32)]


class _XmlText(ctypes.Structure):
    _fields_ = [("raw_value", _XmlSpan), ("owner_element", ctypes.c_uint32),
                ("flags", ctypes.c_uint32)]


class _XmlNamespace(ctypes.Structure):
    _fields_ = [("owner_element", ctypes.c_uint32), ("prefix", _XmlSpan),
                ("uri_offset", ctypes.c_uint32), ("uri_length", ctypes.c_uint32),
                ("next_for_element", ctypes.c_uint32)]


class _XmlCounts(ctypes.Structure):
    _fields_ = [("elements", ctypes.c_uint64), ("attributes", ctypes.c_uint64),
                ("text_nodes", ctypes.c_uint64), ("namespaces", ctypes.c_uint64),
                ("namespace_uri_bytes", ctypes.c_uint64)]


class _CsvRow(ctypes.Structure):
    _fields_ = [("physical_line", ctypes.c_uint64), ("first_field", ctypes.c_uint32),
                ("field_count", ctypes.c_uint32)]


class _CsvField(ctypes.Structure):
    _fields_ = [("value_offset", ctypes.c_uint32), ("value_length", ctypes.c_uint32)]


class _CsvCounts(ctypes.Structure):
    _fields_ = [("rows", ctypes.c_uint64), ("fields", ctypes.c_uint64),
                ("value_bytes", ctypes.c_uint64)]


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


class TextBindings:
    """Bounded one-shot XML, JSON, and CSV parses through the native parsers."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        error_p = ctypes.POINTER(_ErrorInfo)
        text_error_p = ctypes.POINTER(_TextError)
        byte_p = ctypes.POINTER(ctypes.c_uint8)
        library.oa_json_parse.argtypes = [byte_p, ctypes.c_uint64,
            ctypes.POINTER(_JsonNode), ctypes.c_uint64, byte_p, ctypes.c_uint64,
            ctypes.POINTER(_JsonCounts), text_error_p, error_p]
        library.oa_json_parse.restype = ctypes.c_uint32
        library.oa_xml_parse.argtypes = [byte_p, ctypes.c_uint64,
            ctypes.POINTER(_XmlElement), ctypes.c_uint64,
            ctypes.POINTER(_XmlAttribute), ctypes.c_uint64,
            ctypes.POINTER(_XmlText), ctypes.c_uint64,
            ctypes.POINTER(_XmlNamespace), ctypes.c_uint64, byte_p, ctypes.c_uint64,
            ctypes.POINTER(_XmlCounts), text_error_p, error_p]
        library.oa_xml_parse.restype = ctypes.c_uint32
        library.oa_xml_decode_span.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_uint32,
            ctypes.c_uint32, ctypes.c_uint32, byte_p, ctypes.c_uint64,
            ctypes.POINTER(ctypes.c_uint64), text_error_p, error_p]
        library.oa_xml_decode_span.restype = ctypes.c_uint32
        library.oa_csv_parse.argtypes = [byte_p, ctypes.c_uint64,
            ctypes.POINTER(_CsvRow), ctypes.c_uint64,
            ctypes.POINTER(_CsvField), ctypes.c_uint64, byte_p, ctypes.c_uint64,
            ctypes.POINTER(_CsvCounts), text_error_p, error_p]
        library.oa_csv_parse.restype = ctypes.c_uint32

    def _input(self, data: bytes) -> tuple[object | None, int]:
        if not isinstance(data, bytes):
            raise TypeError("text parser input must be immutable bytes")
        source = ctypes.c_char_p(data) if data else None
        return source, len(data)

    def _raise_text(self, status: int, error: _ErrorInfo, parser: _TextError) -> None:
        if status == 0:
            return
        if int(error.code) != status:
            raise RuntimeError("native ABI violated its provisional status/error-code contract")
        message = _native_message(self._library, status)
        if status == 12:
            raise OpenAtcaError(status, error.location_kind, int(error.location), message,
                parser_module=int(parser.module), parser_kind=int(parser.kind),
                parser_position=int(parser.position), parser_detail_a=int(parser.detail_a),
                parser_detail_b=int(parser.detail_b))
        raise OpenAtcaError(status, error.location_kind, int(error.location), message)

    def parse_json(self, data: bytes) -> JsonResult:
        source, length = self._input(data)
        pointer = ctypes.cast(source, ctypes.POINTER(ctypes.c_uint8)) if source is not None else None
        counts, parser, error = _JsonCounts(), _TextError(), _ErrorInfo()
        status = int(self._library.oa_json_parse(pointer, length, None, 0, None, 0,
            ctypes.byref(counts), ctypes.byref(parser), ctypes.byref(error)))
        if status not in (0, 6):
            self._raise_text(status, error, parser)
        node_bytes = int(counts.node_count) * ctypes.sizeof(_JsonNode)
        total = node_bytes + int(counts.value_bytes)
        if total > _MAX_TEXT_OUTPUT_BYTES:
            raise MemoryError("JSON output exceeds the Python wrapper's 256 MiB bound")
        nodes = (_JsonNode * int(counts.node_count))()
        pool = (ctypes.c_uint8 * int(counts.value_bytes))()
        status = int(self._library.oa_json_parse(pointer, length, nodes if counts.node_count else None,
            counts.node_count, pool if counts.value_bytes else None, counts.value_bytes,
            ctypes.byref(counts), ctypes.byref(parser), ctypes.byref(error)))
        self._raise_text(status, error, parser)
        result_nodes = tuple(JsonNode(int(node.kind),
            None if node.first_child == 0xFFFFFFFF else int(node.first_child),
            int(node.child_count), int(node.subtree_end), int(node.value_offset),
            int(node.value_length)) for node in nodes)
        return JsonResult(result_nodes, bytes(pool))

    def parse_xml(self, data: bytes) -> XmlResult:
        source, length = self._input(data)
        pointer = ctypes.cast(source, ctypes.POINTER(ctypes.c_uint8)) if source is not None else None
        counts, parser, error = _XmlCounts(), _TextError(), _ErrorInfo()
        status = int(self._library.oa_xml_parse(pointer, length, None, 0, None, 0, None, 0,
            None, 0, None, 0, ctypes.byref(counts), ctypes.byref(parser), ctypes.byref(error)))
        if status not in (0, 6):
            self._raise_text(status, error, parser)
        sizes = ((int(counts.elements), ctypes.sizeof(_XmlElement)),
                 (int(counts.attributes), ctypes.sizeof(_XmlAttribute)),
                 (int(counts.text_nodes), ctypes.sizeof(_XmlText)),
                 (int(counts.namespaces), ctypes.sizeof(_XmlNamespace)))
        total = sum(count * width for count, width in sizes) + int(counts.namespace_uri_bytes)
        if total > _MAX_TEXT_OUTPUT_BYTES:
            raise MemoryError("XML output exceeds the Python wrapper's 256 MiB bound")
        elements = (_XmlElement * counts.elements)()
        attributes = (_XmlAttribute * counts.attributes)()
        texts = (_XmlText * counts.text_nodes)()
        namespaces = (_XmlNamespace * counts.namespaces)()
        uri_pool = (ctypes.c_uint8 * counts.namespace_uri_bytes)()
        status = int(self._library.oa_xml_parse(pointer, length,
            elements if counts.elements else None, counts.elements,
            attributes if counts.attributes else None, counts.attributes,
            texts if counts.text_nodes else None, counts.text_nodes,
            namespaces if counts.namespaces else None, counts.namespaces,
            uri_pool if counts.namespace_uri_bytes else None, counts.namespace_uri_bytes,
            ctypes.byref(counts), ctypes.byref(parser), ctypes.byref(error)))
        self._raise_text(status, error, parser)
        span = lambda value: XmlSpan(int(value.start), int(value.length))
        return XmlResult(
            tuple(XmlElement(span(row.name), span(row.prefix), int(row.parent),
                int(row.first_attribute), int(row.attribute_count), int(row.first_child),
                int(row.next_sibling), int(row.namespace_head)) for row in elements),
            tuple(XmlAttribute(span(row.name), span(row.prefix), span(row.raw_value),
                int(row.owner_element), int(row.flags)) for row in attributes),
            tuple(XmlText(span(row.raw_value), int(row.owner_element), int(row.flags)) for row in texts),
            tuple(XmlNamespace(int(row.owner_element), span(row.prefix), int(row.uri_offset),
                int(row.uri_length), int(row.next_for_element)) for row in namespaces),
            bytes(uri_pool))

    def decode_xml_span(self, data: bytes, start: int, length: int, kind: int) -> bytes:
        source, input_length = self._input(data)
        for value, name in ((start, "start"), (length, "length"), (kind, "kind")):
            if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value < (1 << 32):
                raise ValueError(f"{name} must fit an unsigned 32-bit integer")
        if length > _MAX_TEXT_OUTPUT_BYTES:
            raise MemoryError("XML span output exceeds the Python wrapper's 256 MiB bound")
        output = (ctypes.c_uint8 * length)()
        written, parser, error = ctypes.c_uint64(), _TextError(), _ErrorInfo()
        pointer = ctypes.cast(source, ctypes.POINTER(ctypes.c_uint8)) if source is not None else None
        status = int(self._library.oa_xml_decode_span(pointer, input_length, start, length, kind,
            output if length else None, length, ctypes.byref(written), ctypes.byref(parser), ctypes.byref(error)))
        self._raise_text(status, error, parser)
        if written.value > length:
            raise RuntimeError("native XML span decoder exceeded its caller buffer")
        return bytes(output[:written.value])

    def parse_csv(self, data: bytes) -> CsvResult:
        source, length = self._input(data)
        pointer = ctypes.cast(source, ctypes.POINTER(ctypes.c_uint8)) if source is not None else None
        counts, parser, error = _CsvCounts(), _TextError(), _ErrorInfo()
        status = int(self._library.oa_csv_parse(pointer, length, None, 0, None, 0, None, 0,
            ctypes.byref(counts), ctypes.byref(parser), ctypes.byref(error)))
        if status not in (0, 6):
            self._raise_text(status, error, parser)
        total = (int(counts.rows) * ctypes.sizeof(_CsvRow) +
                 int(counts.fields) * ctypes.sizeof(_CsvField) + int(counts.value_bytes))
        if total > _MAX_TEXT_OUTPUT_BYTES:
            raise MemoryError("CSV output exceeds the Python wrapper's 256 MiB bound")
        rows = (_CsvRow * counts.rows)()
        fields = (_CsvField * counts.fields)()
        pool = (ctypes.c_uint8 * counts.value_bytes)()
        status = int(self._library.oa_csv_parse(pointer, length,
            rows if counts.rows else None, counts.rows,
            fields if counts.fields else None, counts.fields,
            pool if counts.value_bytes else None, counts.value_bytes,
            ctypes.byref(counts), ctypes.byref(parser), ctypes.byref(error)))
        self._raise_text(status, error, parser)
        return CsvResult(tuple(CsvRow(int(row.physical_line), int(row.first_field), int(row.field_count))
            for row in rows), tuple(CsvField(int(field.value_offset), int(field.value_length))
            for field in fields), bytes(pool))


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
        self.text = TextBindings(library)
        self.compression = Compression(library)

    def _bind(self) -> None:
        lib = self._library
        lib.oa_core_abi_version.argtypes = []
        lib.oa_core_abi_version.restype = ctypes.c_uint32

        double_p = ctypes.POINTER(ctypes.c_double)
        error_p = ctypes.POINTER(_ErrorInfo)
        byte_p = ctypes.POINTER(ctypes.c_uint8)
        u64_p = ctypes.POINTER(ctypes.c_uint64)
        lib.oa_deflate_raw_inflate.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_uint64,
            byte_p, ctypes.c_uint64, u64_p, u64_p, error_p]
        lib.oa_deflate_raw_inflate.restype = ctypes.c_uint32
        lib.oa_deflate_raw_compress.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_uint32,
            ctypes.c_uint64, byte_p, ctypes.c_uint64, u64_p, u64_p,
            ctypes.POINTER(ctypes.c_uint32), error_p]
        lib.oa_deflate_raw_compress.restype = ctypes.c_uint32
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


class Compression:
    """Bounded raw-DEFLATE decoding through the existing oa-zip implementation."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library

    def raw_deflate(self, data: bytes, *, level: int = 6,
                    max_output: int = _MAX_DEFLATE_ENCODE_OUTPUT_BYTES) -> RawDeflateResult:
        """Compress bytes as one raw-DEFLATE stream; levels clamp to the native 1..=9 policy."""
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_DEFLATE_INPUT_BYTES:
            raise ValueError("data exceeds the native input limit")
        if isinstance(level, bool) or not isinstance(level, int):
            raise TypeError("level must be an integer")
        if level < 0 or level >= (1 << 32):
            raise ValueError("level must fit unsigned 32-bit range")
        if isinstance(max_output, bool) or not isinstance(max_output, int):
            raise TypeError("max_output must be an integer")
        if max_output < 0 or max_output > _MAX_DEFLATE_ENCODE_OUTPUT_BYTES:
            raise ValueError("max_output is outside the native encoded-output limit")
        input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        output_length, consumed = ctypes.c_uint64(), ctypes.c_uint64()
        actual_level, error = ctypes.c_uint32(), _ErrorInfo()
        status = int(self._library.oa_deflate_raw_compress(
            input_buffer, len(data), level, max_output, None, 0,
            ctypes.byref(output_length), ctypes.byref(consumed),
            ctypes.byref(actual_level), ctypes.byref(error)))
        if status not in (0, 6):
            _raise_if_error(self._library, status, error)
        if status == 0:
            return RawDeflateResult(b"", consumed.value, actual_level.value)
        output_buffer = (ctypes.c_uint8 * output_length.value)() if output_length.value else None
        status = int(self._library.oa_deflate_raw_compress(
            input_buffer, len(data), level, max_output, output_buffer, output_length.value,
            ctypes.byref(output_length), ctypes.byref(consumed),
            ctypes.byref(actual_level), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return RawDeflateResult(bytes(output_buffer) if output_buffer is not None else b"",
                                consumed.value, actual_level.value)

    def raw_inflate(self, data: bytes, *, max_output: int = _MAX_DEFLATE_OUTPUT_BYTES) -> RawInflateResult:
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_DEFLATE_INPUT_BYTES:
            raise ValueError("data exceeds the native input limit")
        if isinstance(max_output, bool) or not isinstance(max_output, int):
            raise TypeError("max_output must be an integer")
        if max_output < 0 or max_output > _MAX_DEFLATE_OUTPUT_BYTES:
            raise ValueError("max_output is outside the native output limit")
        input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        output_length, consumed, error = ctypes.c_uint64(), ctypes.c_uint64(), _ErrorInfo()
        status = int(self._library.oa_deflate_raw_inflate(
            input_buffer, len(data), max_output, None, 0,
            ctypes.byref(output_length), ctypes.byref(consumed), ctypes.byref(error)))
        if status not in (0, 6):
            _raise_if_error(self._library, status, error)
        if status == 0:
            return RawInflateResult(b"", consumed.value)
        output_buffer = (ctypes.c_uint8 * output_length.value)() if output_length.value else None
        status = int(self._library.oa_deflate_raw_inflate(
            input_buffer, len(data), max_output, output_buffer, output_length.value,
            ctypes.byref(output_length), ctypes.byref(consumed), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        return RawInflateResult(bytes(output_buffer) if output_buffer is not None else b"", consumed.value)


def load(library_path: os.PathLike[str] | str) -> Geodesy:
    """Explicit-path loader; this package does not bundle or build the native library."""
    return Geodesy.load(library_path)


__all__ = [
    "CsvField",
    "Compression",
    "CsvResult",
    "CsvRow",
    "DirectResult",
    "Integrity",
    "JsonNode",
    "JsonResult",
    "Polygon",
    "Geodesy",
    "InverseResult",
    "OpenAtcaError",
    "RhumbInverseResult",
    "RawInflateResult",
    "RawDeflateResult",
    "ScalarMath",
    "SeparationResult",
    "TextBindings",
    "XmlAttribute",
    "XmlElement",
    "XmlNamespace",
    "XmlResult",
    "XmlSpan",
    "XmlText",
    "load",
]
