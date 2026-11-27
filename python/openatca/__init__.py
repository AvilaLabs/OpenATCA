"""Provisional ctypes surface for OpenATCA's L0 C ABI.

PROVISIONAL / UNVALIDATED: this wrapper targets the experimental Rust FFI shell. Importing this
module loads nothing. Call :func:`load` with an explicit path to a separately built library; the
ABI names and layout are not stable or published.
"""

from __future__ import annotations

import ctypes
import array
import os
import threading
from dataclasses import dataclass, make_dataclass
from pathlib import Path
from typing import Final, Iterable, Sequence, TypeAlias, Union

ABI_VERSION_PROVISIONAL: Final[int] = 1
_MAX_MESSAGE_BYTES: Final[int] = 4096
_MAX_POLYGON_VERTICES: Final[int] = 100_001
_MAX_TEXT_OUTPUT_BYTES: Final[int] = 256 * 1024 * 1024
_MAX_DEFLATE_INPUT_BYTES: Final[int] = 64 * 1024 * 1024
_MAX_DEFLATE_OUTPUT_BYTES: Final[int] = 256 * 1024 * 1024
_MAX_DEFLATE_ENCODE_OUTPUT_BYTES: Final[int] = 128 * 1024 * 1024
_MAX_TAR_INPUT_BYTES: Final[int] = 64 * 1024 * 1024
_MAX_TAR_ENTRIES: Final[int] = 4096
_MAX_TAR_NAME_POOL_BYTES: Final[int] = _MAX_TAR_ENTRIES * 4096
_MAX_ASTERIX_BLOCK_BYTES: Final[int] = 65_535
_MAX_ASTERIX_RECORDS: Final[int] = 21_844
_MAX_ZIP_INPUT_BYTES: Final[int] = 1024 * 1024 * 1024
_MAX_ZIP_ENTRIES: Final[int] = 10_000
_MAX_ZIP_ENTRY_BYTES: Final[int] = 256 * 1024 * 1024
_MAX_ZIP_NAME_POOL_BYTES: Final[int] = _MAX_ZIP_ENTRIES * 4096
_MAX_GRIB2_INPUT_BYTES: Final[int] = 256 * 1024 * 1024
_MAX_GRIB2_FIELDS: Final[int] = 16
_MAX_GRIB2_VALUES: Final[int] = 64 * 1024 * 1024
_MAX_NOTAM_INPUT_BYTES: Final[int] = 1 << 20
_MAX_NOTAM_WORKING_BYTES: Final[int] = 16 * 1024 * 1024
_MAX_NOTAM_TEXT_BYTES: Final[int] = 8192
_MAX_NOTAM_POLYGON_VERTICES: Final[int] = 4096
_MAX_NOTAM_UNKNOWN_PATHS: Final[int] = 128

_MAX_CIFP_INPUT_BYTES: Final[int] = 128 * 1024 * 1024
_MAX_CIFP_OUTPUT_BYTES: Final[int] = 768 * 1024 * 1024

class _ErrorInfo(ctypes.Structure):
    _fields_ = [
        ("code", ctypes.c_uint32),
        ("location_kind", ctypes.c_uint32),
        ("location", ctypes.c_uint64),
    ]


class _Grib2Time(ctypes.Structure):
    _fields_ = [("year", ctypes.c_uint16), ("month", ctypes.c_uint8),
                ("day", ctypes.c_uint8), ("hour", ctypes.c_uint8),
                ("minute", ctypes.c_uint8), ("second", ctypes.c_uint8),
                ("reserved", ctypes.c_uint8)]


class _Grib2Field(ctypes.Structure):
    _fields_ = [("discipline", ctypes.c_uint8), ("category", ctypes.c_uint8),
        ("number", ctypes.c_uint8), ("named_parameter", ctypes.c_uint8),
        ("grid_template", ctypes.c_uint16), ("product_template", ctypes.c_uint16),
        ("packing_template", ctypes.c_uint16), ("level_kind", ctypes.c_uint8),
        ("forecast_time_unit", ctypes.c_uint8), ("reference_time", _Grib2Time),
        ("forecast_time", ctypes.c_uint32), ("level_value", ctypes.c_float),
        ("interval_present", ctypes.c_uint8), ("interval_ranges", ctypes.c_uint8),
        ("interval_statistic", ctypes.c_uint8), ("reserved0", ctypes.c_uint8),
        ("interval_end", _Grib2Time), ("nx", ctypes.c_uint32), ("ny", ctypes.c_uint32),
        ("grid_flags", ctypes.c_uint8), ("scan_mode", ctypes.c_uint8),
        ("latitude_row_direction", ctypes.c_int8), ("global_longitude", ctypes.c_uint8),
        ("first_latitude_deg", ctypes.c_double), ("first_longitude_deg", ctypes.c_double),
        ("i_increment_deg", ctypes.c_double), ("j_increment_deg", ctypes.c_double),
        ("lambert_lad_deg", ctypes.c_double), ("lambert_lov_deg", ctypes.c_double),
        ("lambert_dx_m", ctypes.c_double), ("lambert_dy_m", ctypes.c_double),
        ("lambert_latin1_deg", ctypes.c_double), ("lambert_latin2_deg", ctypes.c_double),
        ("earth_radius_m", ctypes.c_double), ("value_offset", ctypes.c_uint64),
        ("value_count", ctypes.c_uint64), ("missing_count", ctypes.c_uint64)]


assert ctypes.sizeof(_Grib2Time) == 8 and ctypes.alignment(_Grib2Time) == 2
assert ctypes.sizeof(_Grib2Field) == 168 and ctypes.alignment(_Grib2Field) == 8
assert _Grib2Field.reference_time.offset == 12 and _Grib2Field.value_offset.offset == 144


class _AsterixPosition(ctypes.Structure):
    _fields_ = [("item", ctypes.c_uint16), ("kind", ctypes.c_uint8),
                ("scale", ctypes.c_uint8), ("a_raw", ctypes.c_int32),
                ("b_raw", ctypes.c_int32)]


class _AsterixVertical(ctypes.Structure):
    _fields_ = [("item", ctypes.c_uint16), ("subitem", ctypes.c_uint8),
                ("kind", ctypes.c_uint8), ("scale", ctypes.c_uint8),
                ("flags", ctypes.c_uint8), ("auxiliary", ctypes.c_uint8),
                ("reserved", ctypes.c_uint8), ("raw", ctypes.c_int32)]


class _AsterixRecord(ctypes.Structure):
    _fields_ = [("category", ctypes.c_uint8), ("cat021_edition", ctypes.c_uint8),
                ("source_sac", ctypes.c_uint8), ("source_sic", ctypes.c_uint8),
                ("reserved0", ctypes.c_uint32), ("present_frn_mask", ctypes.c_uint64),
                ("raw_untyped_frn_mask", ctypes.c_uint64),
                ("typed_unprojected_frn_mask", ctypes.c_uint64),
                ("present_field_mask", ctypes.c_uint64), ("target_address", ctypes.c_uint32),
                ("target_ident", ctypes.c_uint8 * 8), ("track_number", ctypes.c_uint16),
                ("time_item", ctypes.c_uint16), ("time_raw", ctypes.c_uint32),
                ("position_count", ctypes.c_uint8), ("velocity_kind", ctypes.c_uint8),
                ("velocity_flags", ctypes.c_uint8), ("mode3a_flags", ctypes.c_uint8),
                ("positions", _AsterixPosition * 2), ("velocity_a_raw", ctypes.c_int32),
                ("velocity_b_raw", ctypes.c_int32), ("mode3a_code", ctypes.c_uint16),
                ("reserved3", ctypes.c_uint16), ("raw_untyped_subitem_mask", ctypes.c_uint32),
                ("reserved4", ctypes.c_uint32), ("reserved5", ctypes.c_uint32),
                ("vertical_count", ctypes.c_uint8), ("reserved6", ctypes.c_uint8 * 7),
                ("vertical", _AsterixVertical * 6)]


class _AsterixCounts(ctypes.Structure):
    _fields_ = [("category", ctypes.c_uint8), ("edition", ctypes.c_uint8),
                ("reserved", ctypes.c_uint16), ("reserved_bytes", ctypes.c_uint8 * 4),
                ("consumed_bytes", ctypes.c_uint64),
                ("required_record_count", ctypes.c_uint64),
                ("written_record_count", ctypes.c_uint64)]


class _AsterixError(ctypes.Structure):
    _fields_ = [("status", ctypes.c_uint32), ("kind", ctypes.c_uint32),
                ("category", ctypes.c_uint8), ("subitem", ctypes.c_uint8),
                ("item", ctypes.c_uint16), ("record", ctypes.c_uint32),
                ("reserved_bytes", ctypes.c_uint8 * 8), ("offset", ctypes.c_uint64)]


class _NotamRow(ctypes.Structure):
    _fields_ = [
        ("present", ctypes.c_uint64), ("errors", ctypes.c_uint64),
        ("issued_ms", ctypes.c_int64), ("start_ms", ctypes.c_int64),
        ("end_ms", ctypes.c_int64), ("canceled_ms", ctypes.c_int64),
        ("flags", ctypes.c_uint32), ("number", ctypes.c_uint32),
        ("ref_number", ctypes.c_uint32), ("sched_off", ctypes.c_uint32),
        ("sched_len", ctypes.c_uint32), ("text_off", ctypes.c_uint32),
        ("text_len", ctypes.c_uint32), ("radius_m", ctypes.c_uint32),
        ("poly_first", ctypes.c_uint32), ("poly_count", ctypes.c_uint32),
        ("feature_first", ctypes.c_uint32), ("feature_count", ctypes.c_uint32),
        ("feat_mask", ctypes.c_uint32), ("lat_e7", ctypes.c_int32),
        ("lon_e7", ctypes.c_int32), ("floor_ft", ctypes.c_int32),
        ("ceil_ft", ctypes.c_int32), ("year", ctypes.c_uint16),
        ("ref_year", ctypes.c_uint16), ("scenario", ctypes.c_uint16),
        ("ntype", ctypes.c_uint8), ("class_code", ctypes.c_uint8),
        ("traffic", ctypes.c_uint8), ("purpose", ctypes.c_uint8),
        ("scope", ctypes.c_uint8), ("floor_ref", ctypes.c_uint8),
        ("ceil_ref", ctypes.c_uint8), ("feat_kind", ctypes.c_uint8),
        ("interpretation", ctypes.c_uint8), ("slice_meta", ctypes.c_uint8),
        ("slice_errors", ctypes.c_uint8), ("sequence_number", ctypes.c_uint32),
        ("correction_number", ctypes.c_uint32), ("event_id", ctypes.c_uint8 * 16),
        ("series", ctypes.c_uint8 * 12), ("ref_series", ctypes.c_uint8 * 12),
        ("xover", ctypes.c_uint8 * 16), ("feat_desig", ctypes.c_uint8 * 12),
        ("runway", ctypes.c_uint8 * 12), ("airport", ctypes.c_uint8 * 8),
        ("navaid", ctypes.c_uint8 * 8), ("unit", ctypes.c_uint8 * 8),
        ("selection", ctypes.c_uint8 * 5), ("location", ctypes.c_uint8 * 4),
        ("fir", ctypes.c_uint8 * 4), ("icao_location", ctypes.c_uint8 * 4),
        ("airport_icao", ctypes.c_uint8 * 4),
    ]


class _NotamFeature(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_uint8), ("time_kind", ctypes.c_uint8),
                ("interpretation", ctypes.c_uint8), ("meta", ctypes.c_uint8),
                ("time_meta", ctypes.c_uint8), ("errors", ctypes.c_uint8),
                ("reserved", ctypes.c_uint8 * 2), ("begin_ms", ctypes.c_int64),
                ("end_ms", ctypes.c_int64), ("sequence_number", ctypes.c_uint32),
                ("correction_number", ctypes.c_uint32), ("designator", ctypes.c_uint8 * 12),
                ("designator_len", ctypes.c_uint8), ("icao", ctypes.c_uint8 * 4),
                ("icao_len", ctypes.c_uint8)]


class _NotamPolygon(ctypes.Structure):
    _fields_ = [("first_vertex", ctypes.c_uint32), ("vertex_count", ctypes.c_uint32),
                ("feature_index", ctypes.c_uint32), ("kind", ctypes.c_uint8),
                ("reserved", ctypes.c_uint8 * 3)]


class _NotamVertex(ctypes.Structure):
    _fields_ = [("lat_e7", ctypes.c_int32), ("lon_e7", ctypes.c_int32)]


class _NotamUnknownPath(ctypes.Structure):
    _fields_ = [("hash", ctypes.c_uint64), ("count", ctypes.c_uint64),
                ("is_attribute", ctypes.c_uint8), ("text_len", ctypes.c_uint8),
                ("reserved", ctypes.c_uint8 * 6), ("text", ctypes.c_uint8 * 160)]


class _NotamSummary(ctypes.Structure):
    _fields_ = [("status", ctypes.c_uint32), ("refusal_kind", ctypes.c_uint32),
                ("source_offset", ctypes.c_uint64), ("row_count", ctypes.c_uint64),
                ("feature_count", ctypes.c_uint64), ("polygon_count", ctypes.c_uint64),
                ("vertex_count", ctypes.c_uint64), ("string_bytes", ctypes.c_uint64),
                ("unknown_path_count", ctypes.c_uint64), ("unknown_elements", ctypes.c_uint64),
                ("unknown_attributes", ctypes.c_uint64), ("unknown_overflow", ctypes.c_uint64),
                ("sealed_dropped", ctypes.c_uint64), ("field_errors", ctypes.c_uint64 * 31),
                ("rejects", ctypes.c_uint64 * 9), ("features", ctypes.c_uint64 * 12)]


class _NotamBuffers(ctypes.Structure):
    _fields_ = [("row", ctypes.POINTER(_NotamRow)), ("row_capacity", ctypes.c_uint64),
                ("features", ctypes.POINTER(_NotamFeature)), ("feature_capacity", ctypes.c_uint64),
                ("polygons", ctypes.POINTER(_NotamPolygon)), ("polygon_capacity", ctypes.c_uint64),
                ("vertices", ctypes.POINTER(_NotamVertex)), ("vertex_capacity", ctypes.c_uint64),
                ("strings", ctypes.POINTER(ctypes.c_uint8)), ("string_capacity", ctypes.c_uint64),
                ("unknown_paths", ctypes.POINTER(_NotamUnknownPath)),
                ("unknown_path_capacity", ctypes.c_uint64)]

_CIFP_CTYPES = {
    "uint8_t": ctypes.c_uint8,
    "int8_t": ctypes.c_int8,
    "uint16_t": ctypes.c_uint16,
    "int16_t": ctypes.c_int16,
    "uint32_t": ctypes.c_uint32,
    "int32_t": ctypes.c_int32,
    "uint64_t": ctypes.c_uint64,
    "int64_t": ctypes.c_int64,
    "double": ctypes.c_double,
}

def _cifp_type(name: str):
    if name in _CIFP_CTYPES:
        return _CIFP_CTYPES[name]
    return globals()["_" + name]

class _OaCifpSpan(ctypes.Structure):
    _fields_ = [
        ("offset", _cifp_type("uint64_t")),
        ("length", _cifp_type("uint64_t")),
    ]

class _OaCifpPosition(ctypes.Structure):
    _fields_ = [
        ("lat", _cifp_type("double")),
        ("lon", _cifp_type("double")),
    ]

class _OaCifpMeta(ctypes.Structure):
    _fields_ = [
        ("line", _cifp_type("uint64_t")),
        ("cycle_year", _cifp_type("uint8_t")),
        ("cycle_release", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 6),
        ("customer_area", _cifp_type("OaCifpSpan")),
        ("file_record_number", _cifp_type("OaCifpSpan")),
    ]

class _OaCifpAltitude(ctypes.Structure):
    _fields_ = [
        ("kind", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 3),
        ("value", _cifp_type("int32_t")),
    ]

class _OaCifpFixRef(ctypes.Structure):
    _fields_ = [
        ("ident", _cifp_type("OaCifpSpan")),
        ("region", _cifp_type("OaCifpSpan")),
        ("section", _cifp_type("uint8_t")),
        ("subsection", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 6),
    ]

class _OaCifpAirport(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("icao_region", _cifp_type("OaCifpSpan")),
        ("iata", _cifp_type("OaCifpSpan")),
        ("datum", _cifp_type("OaCifpSpan")),
        ("name", _cifp_type("OaCifpSpan")),
        ("position", _cifp_type("OaCifpPosition")),
        ("magnetic_variation", _cifp_type("double")),
        ("elevation_ft", _cifp_type("int32_t")),
        ("longest_runway_ft", _cifp_type("uint32_t")),
        ("transition_altitude", _cifp_type("OaCifpAltitude")),
        ("transition_level", _cifp_type("OaCifpAltitude")),
        ("ifr_capable", _cifp_type("uint8_t")),
        ("longest_runway_surface", _cifp_type("uint8_t")),
        ("public_military", _cifp_type("uint8_t")),
        ("north_reference", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 4),
    ]

class _OaCifpRunway(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("airport_ident", _cifp_type("OaCifpSpan")),
        ("airport_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("localizer_ident", _cifp_type("OaCifpSpan")),
        ("threshold", _cifp_type("OaCifpPosition")),
        ("length_ft", _cifp_type("uint32_t")),
        ("displaced_threshold_ft", _cifp_type("uint32_t")),
        ("magnetic_bearing", _cifp_type("double")),
        ("ellipsoid_height_m", _cifp_type("double")),
        ("threshold_elevation_ft", _cifp_type("int32_t")),
        ("threshold_crossing_height_ft", _cifp_type("uint32_t")),
        ("width_ft", _cifp_type("uint32_t")),
        ("tch_indicator", _cifp_type("uint8_t")),
        ("localizer_category", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 6),
        ("reserved2", _cifp_type("uint8_t") * 4),
    ]

class _OaCifpLocalizer(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("airport_ident", _cifp_type("OaCifpSpan")),
        ("airport_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("runway_ident", _cifp_type("OaCifpSpan")),
        ("localizer_position", _cifp_type("OaCifpPosition")),
        ("glide_slope_position", _cifp_type("OaCifpPosition")),
        ("category", _cifp_type("uint8_t")),
        ("reserved0", _cifp_type("uint8_t") * 7),
        ("frequency_mhz", _cifp_type("double")),
        ("bearing", _cifp_type("double")),
        ("localizer_width_deg", _cifp_type("double")),
        ("glide_slope_angle_deg", _cifp_type("double")),
        ("station_declination", _cifp_type("double")),
        ("glide_slope_tch_ft", _cifp_type("uint32_t")),
        ("glide_slope_elevation_ft", _cifp_type("int32_t")),
    ]

class _OaCifpVhfNavaid(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("airport_ident", _cifp_type("OaCifpSpan")),
        ("airport_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("icao_region", _cifp_type("OaCifpSpan")),
        ("dme_ident", _cifp_type("OaCifpSpan")),
        ("datum", _cifp_type("OaCifpSpan")),
        ("name", _cifp_type("OaCifpSpan")),
        ("vor_position", _cifp_type("OaCifpPosition")),
        ("dme_position", _cifp_type("OaCifpPosition")),
        ("frequency_mhz", _cifp_type("double")),
        ("station_declination", _cifp_type("double")),
        ("dme_elevation_ft", _cifp_type("int32_t")),
        ("class", _cifp_type("uint8_t") * 5),
        ("figure_of_merit", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 2),
        ("reserved2", _cifp_type("uint8_t") * 4),
    ]

class _OaCifpNdb(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("airport_ident", _cifp_type("OaCifpSpan")),
        ("airport_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("icao_region", _cifp_type("OaCifpSpan")),
        ("class", _cifp_type("OaCifpSpan")),
        ("datum", _cifp_type("OaCifpSpan")),
        ("name", _cifp_type("OaCifpSpan")),
        ("position", _cifp_type("OaCifpPosition")),
        ("frequency_khz", _cifp_type("double")),
        ("magnetic_variation", _cifp_type("double")),
        ("terminal", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpWaypoint(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("area", _cifp_type("OaCifpSpan")),
        ("area_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("icao_region", _cifp_type("OaCifpSpan")),
        ("waypoint_type", _cifp_type("OaCifpSpan")),
        ("usage", _cifp_type("OaCifpSpan")),
        ("datum", _cifp_type("OaCifpSpan")),
        ("name", _cifp_type("OaCifpSpan")),
        ("position", _cifp_type("OaCifpPosition")),
        ("magnetic_variation", _cifp_type("double")),
        ("kind", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpAirway(ctypes.Structure):
    _fields_ = [
        ("customer_area", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("first_line", _cifp_type("uint64_t")),
        ("fix_start", _cifp_type("uint64_t")),
        ("fix_count", _cifp_type("uint64_t")),
        ("route_type", _cifp_type("uint8_t")),
        ("level", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 6),
    ]

class _OaCifpAirwayFix(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("route_ident", _cifp_type("OaCifpSpan")),
        ("fix", _cifp_type("OaCifpFixRef")),
        ("sequence", _cifp_type("uint32_t")),
        ("description", _cifp_type("uint8_t") * 4),
        ("route_type", _cifp_type("uint8_t")),
        ("level", _cifp_type("uint8_t")),
        ("reserved0", _cifp_type("uint8_t") * 6),
        ("present", _cifp_type("uint64_t")),
        ("outbound_course", _cifp_type("double")),
        ("distance_nm", _cifp_type("double")),
        ("inbound_course", _cifp_type("double")),
        ("min_altitude", _cifp_type("OaCifpAltitude")),
        ("min_altitude_2", _cifp_type("OaCifpAltitude")),
        ("max_altitude", _cifp_type("OaCifpAltitude")),
    ]

class _OaCifpProcedure(ctypes.Structure):
    _fields_ = [
        ("customer_area", _cifp_type("OaCifpSpan")),
        ("airport_ident", _cifp_type("OaCifpSpan")),
        ("airport_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("first_line", _cifp_type("uint64_t")),
        ("route_start", _cifp_type("uint64_t")),
        ("route_count", _cifp_type("uint64_t")),
        ("kind", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpRoute(ctypes.Structure):
    _fields_ = [
        ("transition", _cifp_type("OaCifpSpan")),
        ("first_line", _cifp_type("uint64_t")),
        ("leg_start", _cifp_type("uint64_t")),
        ("leg_count", _cifp_type("uint64_t")),
        ("route_type", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpLeg(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("airport_ident", _cifp_type("OaCifpSpan")),
        ("airport_region", _cifp_type("OaCifpSpan")),
        ("ident", _cifp_type("OaCifpSpan")),
        ("transition", _cifp_type("OaCifpSpan")),
        ("rnp_code", _cifp_type("OaCifpSpan")),
        ("fix", _cifp_type("OaCifpFixRef")),
        ("recommended_navaid", _cifp_type("OaCifpFixRef")),
        ("center_fix", _cifp_type("OaCifpFixRef")),
        ("sequence", _cifp_type("uint32_t")),
        ("description", _cifp_type("uint8_t") * 4),
        ("kind", _cifp_type("uint8_t")),
        ("route_type", _cifp_type("uint8_t")),
        ("turn_direction", _cifp_type("uint8_t")),
        ("path_terminator", _cifp_type("uint8_t")),
        ("altitude_description", _cifp_type("uint8_t")),
        ("gnss_fms_indication", _cifp_type("uint8_t")),
        ("speed_limit_description", _cifp_type("uint8_t")),
        ("reserved0", _cifp_type("uint8_t")),
        ("arc_radius_nm", _cifp_type("double")),
        ("theta_deg", _cifp_type("double")),
        ("rho_nm", _cifp_type("double")),
        ("course_deg", _cifp_type("double")),
        ("distance_or_time_value", _cifp_type("double")),
        ("distance_or_time_kind", _cifp_type("uint8_t")),
        ("reserved1", _cifp_type("uint8_t") * 7),
        ("altitude1", _cifp_type("OaCifpAltitude")),
        ("altitude2", _cifp_type("OaCifpAltitude")),
        ("transition_altitude", _cifp_type("OaCifpAltitude")),
        ("speed_limit_kt", _cifp_type("uint32_t")),
        ("reserved2", _cifp_type("uint8_t") * 4),
        ("vertical_angle_deg", _cifp_type("double")),
    ]

class _OaCifpContinuation(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("owner_index", _cifp_type("uint64_t")),
        ("text", _cifp_type("OaCifpSpan")),
        ("number", _cifp_type("uint8_t")),
        ("application_type", _cifp_type("uint8_t")),
        ("owner_kind", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 5),
    ]

class _OaCifpLevelOfService(ctypes.Structure):
    _fields_ = [
        ("continuation_index", _cifp_type("uint64_t")),
        ("name", _cifp_type("OaCifpSpan")),
        ("authorized", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpAirspace(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("present", _cifp_type("uint64_t")),
        ("icao_region", _cifp_type("OaCifpSpan")),
        ("designation", _cifp_type("OaCifpSpan")),
        ("name", _cifp_type("OaCifpSpan")),
        ("boundary_start", _cifp_type("uint64_t")),
        ("boundary_count", _cifp_type("uint64_t")),
        ("continuation_start", _cifp_type("uint64_t")),
        ("continuation_count", _cifp_type("uint64_t")),
        ("kind", _cifp_type("uint8_t")),
        ("airspace_type", _cifp_type("uint8_t")),
        ("classification", _cifp_type("uint8_t")),
        ("multiple_code", _cifp_type("uint8_t")),
        ("level", _cifp_type("uint8_t")),
        ("time_code", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 2),
        ("lower", _cifp_type("OaCifpAltitude")),
        ("lower_unit", _cifp_type("uint8_t")),
        ("upper_unit", _cifp_type("uint8_t")),
        ("reserved2", _cifp_type("uint8_t") * 6),
        ("upper", _cifp_type("OaCifpAltitude")),
    ]

class _OaCifpBoundary(ctypes.Structure):
    _fields_ = [
        ("position", _cifp_type("OaCifpPosition")),
        ("arc_origin", _cifp_type("OaCifpPosition")),
        ("sequence", _cifp_type("uint32_t")),
        ("via", _cifp_type("uint8_t")),
        ("end", _cifp_type("uint8_t")),
        ("present", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t")),
        ("arc_distance_nm", _cifp_type("double")),
        ("arc_bearing_deg", _cifp_type("double")),
        ("source_line", _cifp_type("uint64_t")),
    ]

class _OaCifpMora(ctypes.Structure):
    _fields_ = [
        ("meta", _cifp_type("OaCifpMeta")),
        ("start_lat", _cifp_type("int16_t")),
        ("start_lon", _cifp_type("int16_t")),
        ("present_mask", _cifp_type("uint32_t")),
        ("cells_ft", _cifp_type("uint32_t") * 30),
    ]

class _OaCifpHeader(ctypes.Structure):
    _fields_ = [
        ("file_name", _cifp_type("OaCifpSpan")),
        ("version", _cifp_type("OaCifpSpan")),
        ("creation_date", _cifp_type("OaCifpSpan")),
        ("creation_time", _cifp_type("OaCifpSpan")),
        ("data_supplier", _cifp_type("OaCifpSpan")),
        ("crc", _cifp_type("OaCifpSpan")),
        ("record_length", _cifp_type("uint32_t")),
        ("reserved0", _cifp_type("uint8_t") * 4),
        ("record_count", _cifp_type("uint64_t")),
        ("cycle_year", _cifp_type("uint8_t")),
        ("cycle_release", _cifp_type("uint8_t")),
        ("production_flag", _cifp_type("uint8_t")),
        ("crc_verified", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 4),
    ]

class _OaCifpHeaderText(ctypes.Structure):
    _fields_ = [
        ("text", _cifp_type("OaCifpSpan")),
    ]

class _OaCifpSectionCount(ctypes.Structure):
    _fields_ = [
        ("section", _cifp_type("OaCifpSpan")),
        ("count", _cifp_type("uint64_t")),
        ("supported", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpCycleCount(ctypes.Structure):
    _fields_ = [
        ("count", _cifp_type("uint64_t")),
        ("year", _cifp_type("uint8_t")),
        ("release", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 6),
    ]

class _OaCifpError(ctypes.Structure):
    _fields_ = [
        ("status", _cifp_type("uint32_t")),
        ("kind", _cifp_type("uint32_t")),
        ("line", _cifp_type("uint64_t")),
        ("first_column", _cifp_type("uint32_t")),
        ("last_column", _cifp_type("uint32_t")),
        ("limit", _cifp_type("uint64_t")),
        ("actual", _cifp_type("uint64_t")),
        ("previous", _cifp_type("uint64_t")),
        ("found", _cifp_type("uint64_t")),
        ("code", _cifp_type("uint32_t")),
        ("flags", _cifp_type("uint32_t")),
        ("field", _cifp_type("uint8_t") * 32),
        ("value", _cifp_type("uint8_t") * 132),
        ("reason", _cifp_type("uint8_t") * 64),
        ("reserved", _cifp_type("uint8_t") * 4),
    ]

class _OaCifpCounts(ctypes.Structure):
    _fields_ = [
        ("airports", _cifp_type("uint64_t")),
        ("runways", _cifp_type("uint64_t")),
        ("localizers", _cifp_type("uint64_t")),
        ("vhf_navaids", _cifp_type("uint64_t")),
        ("ndbs", _cifp_type("uint64_t")),
        ("waypoints", _cifp_type("uint64_t")),
        ("moras", _cifp_type("uint64_t")),
        ("airspaces", _cifp_type("uint64_t")),
        ("airways", _cifp_type("uint64_t")),
        ("airway_fixes", _cifp_type("uint64_t")),
        ("procedures", _cifp_type("uint64_t")),
        ("routes", _cifp_type("uint64_t")),
        ("legs", _cifp_type("uint64_t")),
        ("continuations", _cifp_type("uint64_t")),
        ("levels_of_service", _cifp_type("uint64_t")),
        ("boundaries", _cifp_type("uint64_t")),
        ("header_records", _cifp_type("uint64_t")),
        ("sections", _cifp_type("uint64_t")),
        ("cycles", _cifp_type("uint64_t")),
        ("errors", _cifp_type("uint64_t")),
        ("errors_dropped", _cifp_type("uint64_t")),
        ("strings_bytes", _cifp_type("uint64_t")),
        ("output_bytes", _cifp_type("uint64_t")),
        ("is_clean", _cifp_type("uint8_t")),
        ("reserved", _cifp_type("uint8_t") * 7),
    ]

class _OaCifpBuffers(ctypes.Structure):
    _fields_ = [
        ("header", ctypes.POINTER(_cifp_type("OaCifpHeader"))),
        ("header_capacity", _cifp_type("uint64_t")),
        ("header_text", ctypes.POINTER(_cifp_type("OaCifpHeaderText"))),
        ("header_text_capacity", _cifp_type("uint64_t")),
        ("airports", ctypes.POINTER(_cifp_type("OaCifpAirport"))),
        ("airports_capacity", _cifp_type("uint64_t")),
        ("runways", ctypes.POINTER(_cifp_type("OaCifpRunway"))),
        ("runways_capacity", _cifp_type("uint64_t")),
        ("localizers", ctypes.POINTER(_cifp_type("OaCifpLocalizer"))),
        ("localizers_capacity", _cifp_type("uint64_t")),
        ("vhf_navaids", ctypes.POINTER(_cifp_type("OaCifpVhfNavaid"))),
        ("vhf_navaids_capacity", _cifp_type("uint64_t")),
        ("ndbs", ctypes.POINTER(_cifp_type("OaCifpNdb"))),
        ("ndbs_capacity", _cifp_type("uint64_t")),
        ("waypoints", ctypes.POINTER(_cifp_type("OaCifpWaypoint"))),
        ("waypoints_capacity", _cifp_type("uint64_t")),
        ("moras", ctypes.POINTER(_cifp_type("OaCifpMora"))),
        ("moras_capacity", _cifp_type("uint64_t")),
        ("airspaces", ctypes.POINTER(_cifp_type("OaCifpAirspace"))),
        ("airspaces_capacity", _cifp_type("uint64_t")),
        ("airways", ctypes.POINTER(_cifp_type("OaCifpAirway"))),
        ("airways_capacity", _cifp_type("uint64_t")),
        ("airway_fixes", ctypes.POINTER(_cifp_type("OaCifpAirwayFix"))),
        ("airway_fixes_capacity", _cifp_type("uint64_t")),
        ("procedures", ctypes.POINTER(_cifp_type("OaCifpProcedure"))),
        ("procedures_capacity", _cifp_type("uint64_t")),
        ("routes", ctypes.POINTER(_cifp_type("OaCifpRoute"))),
        ("routes_capacity", _cifp_type("uint64_t")),
        ("legs", ctypes.POINTER(_cifp_type("OaCifpLeg"))),
        ("legs_capacity", _cifp_type("uint64_t")),
        ("continuations", ctypes.POINTER(_cifp_type("OaCifpContinuation"))),
        ("continuations_capacity", _cifp_type("uint64_t")),
        ("levels_of_service", ctypes.POINTER(_cifp_type("OaCifpLevelOfService"))),
        ("levels_of_service_capacity", _cifp_type("uint64_t")),
        ("boundaries", ctypes.POINTER(_cifp_type("OaCifpBoundary"))),
        ("boundaries_capacity", _cifp_type("uint64_t")),
        ("sections", ctypes.POINTER(_cifp_type("OaCifpSectionCount"))),
        ("sections_capacity", _cifp_type("uint64_t")),
        ("cycles", ctypes.POINTER(_cifp_type("OaCifpCycleCount"))),
        ("cycles_capacity", _cifp_type("uint64_t")),
        ("errors", ctypes.POINTER(_cifp_type("OaCifpError"))),
        ("errors_capacity", _cifp_type("uint64_t")),
        ("strings", ctypes.POINTER(_cifp_type("uint8_t"))),
        ("strings_capacity", _cifp_type("uint64_t")),
    ]

_CifpCounts = _OaCifpCounts
_CifpError = _OaCifpError
_CifpBuffers = _OaCifpBuffers
_CIFP_COUNT_NAMES: Final[tuple[str, ...]] = tuple(name for name, _ in _CifpCounts._fields_ if name not in ("is_clean", "reserved"))
_CIFP_ROW_TYPES: Final[tuple[tuple[str, type[ctypes.Structure]], ...]] = (
    ("header", _OaCifpHeader),
    ("header_text", _OaCifpHeaderText),
    ("airports", _OaCifpAirport),
    ("runways", _OaCifpRunway),
    ("localizers", _OaCifpLocalizer),
    ("vhf_navaids", _OaCifpVhfNavaid),
    ("ndbs", _OaCifpNdb),
    ("waypoints", _OaCifpWaypoint),
    ("moras", _OaCifpMora),
    ("airspaces", _OaCifpAirspace),
    ("airways", _OaCifpAirway),
    ("airway_fixes", _OaCifpAirwayFix),
    ("procedures", _OaCifpProcedure),
    ("routes", _OaCifpRoute),
    ("legs", _OaCifpLeg),
    ("continuations", _OaCifpContinuation),
    ("levels_of_service", _OaCifpLevelOfService),
    ("boundaries", _OaCifpBoundary),
    ("sections", _OaCifpSectionCount),
    ("cycles", _OaCifpCycleCount),
    ("errors", _OaCifpError),)

assert ctypes.sizeof(_OaCifpSpan) == 16 and ctypes.alignment(_OaCifpSpan) == 8
assert ctypes.sizeof(_OaCifpPosition) == 16 and ctypes.alignment(_OaCifpPosition) == 8
assert ctypes.sizeof(_OaCifpMeta) == 48 and ctypes.alignment(_OaCifpMeta) == 8
assert ctypes.sizeof(_OaCifpAltitude) == 8 and ctypes.alignment(_OaCifpAltitude) == 4
assert ctypes.sizeof(_OaCifpFixRef) == 40 and ctypes.alignment(_OaCifpFixRef) == 8
assert ctypes.sizeof(_OaCifpAirport) == 192 and ctypes.alignment(_OaCifpAirport) == 8
assert ctypes.sizeof(_OaCifpRunway) == 184 and ctypes.alignment(_OaCifpRunway) == 8
assert ctypes.sizeof(_OaCifpLocalizer) == 208 and ctypes.alignment(_OaCifpLocalizer) == 8
assert ctypes.sizeof(_OaCifpVhfNavaid) == 232 and ctypes.alignment(_OaCifpVhfNavaid) == 8
assert ctypes.sizeof(_OaCifpNdb) == 208 and ctypes.alignment(_OaCifpNdb) == 8
assert ctypes.sizeof(_OaCifpWaypoint) == 216 and ctypes.alignment(_OaCifpWaypoint) == 8
assert ctypes.sizeof(_OaCifpAirway) == 64 and ctypes.alignment(_OaCifpAirway) == 8
assert ctypes.sizeof(_OaCifpAirwayFix) == 176 and ctypes.alignment(_OaCifpAirwayFix) == 8
assert ctypes.sizeof(_OaCifpProcedure) == 96 and ctypes.alignment(_OaCifpProcedure) == 8
assert ctypes.sizeof(_OaCifpRoute) == 48 and ctypes.alignment(_OaCifpRoute) == 8
assert ctypes.sizeof(_OaCifpLeg) == 360 and ctypes.alignment(_OaCifpLeg) == 8
assert ctypes.sizeof(_OaCifpContinuation) == 80 and ctypes.alignment(_OaCifpContinuation) == 8
assert ctypes.sizeof(_OaCifpLevelOfService) == 32 and ctypes.alignment(_OaCifpLevelOfService) == 8
assert ctypes.sizeof(_OaCifpAirspace) == 168 and ctypes.alignment(_OaCifpAirspace) == 8
assert ctypes.sizeof(_OaCifpBoundary) == 64 and ctypes.alignment(_OaCifpBoundary) == 8
assert ctypes.sizeof(_OaCifpMora) == 176 and ctypes.alignment(_OaCifpMora) == 8
assert ctypes.sizeof(_OaCifpHeader) == 120 and ctypes.alignment(_OaCifpHeader) == 8
assert ctypes.sizeof(_OaCifpHeaderText) == 16 and ctypes.alignment(_OaCifpHeaderText) == 8
assert ctypes.sizeof(_OaCifpSectionCount) == 32 and ctypes.alignment(_OaCifpSectionCount) == 8
assert ctypes.sizeof(_OaCifpCycleCount) == 16 and ctypes.alignment(_OaCifpCycleCount) == 8
assert ctypes.sizeof(_OaCifpError) == 296 and ctypes.alignment(_OaCifpError) == 8
assert ctypes.sizeof(_OaCifpCounts) == 192 and ctypes.alignment(_OaCifpCounts) == 8
assert ctypes.sizeof(_OaCifpBuffers) == 352 and ctypes.alignment(_OaCifpBuffers) == 8
assert getattr(_OaCifpSpan, "offset").offset == 0
assert getattr(_OaCifpSpan, "length").offset == 8
assert getattr(_OaCifpPosition, "lat").offset == 0
assert getattr(_OaCifpPosition, "lon").offset == 8
assert getattr(_OaCifpMeta, "line").offset == 0
assert getattr(_OaCifpMeta, "cycle_year").offset == 8
assert getattr(_OaCifpMeta, "cycle_release").offset == 9
assert getattr(_OaCifpMeta, "reserved").offset == 10
assert getattr(_OaCifpMeta, "customer_area").offset == 16
assert getattr(_OaCifpMeta, "file_record_number").offset == 32
assert getattr(_OaCifpAltitude, "kind").offset == 0
assert getattr(_OaCifpAltitude, "reserved").offset == 1
assert getattr(_OaCifpAltitude, "value").offset == 4
assert getattr(_OaCifpFixRef, "ident").offset == 0
assert getattr(_OaCifpFixRef, "region").offset == 16
assert getattr(_OaCifpFixRef, "section").offset == 32
assert getattr(_OaCifpFixRef, "subsection").offset == 33
assert getattr(_OaCifpFixRef, "reserved").offset == 34
assert getattr(_OaCifpAirport, "meta").offset == 0
assert getattr(_OaCifpAirport, "present").offset == 48
assert getattr(_OaCifpAirport, "ident").offset == 56
assert getattr(_OaCifpAirport, "icao_region").offset == 72
assert getattr(_OaCifpAirport, "iata").offset == 88
assert getattr(_OaCifpAirport, "datum").offset == 104
assert getattr(_OaCifpAirport, "name").offset == 120
assert getattr(_OaCifpAirport, "position").offset == 136
assert getattr(_OaCifpAirport, "magnetic_variation").offset == 152
assert getattr(_OaCifpAirport, "elevation_ft").offset == 160
assert getattr(_OaCifpAirport, "longest_runway_ft").offset == 164
assert getattr(_OaCifpAirport, "transition_altitude").offset == 168
assert getattr(_OaCifpAirport, "transition_level").offset == 176
assert getattr(_OaCifpAirport, "ifr_capable").offset == 184
assert getattr(_OaCifpAirport, "longest_runway_surface").offset == 185
assert getattr(_OaCifpAirport, "public_military").offset == 186
assert getattr(_OaCifpAirport, "north_reference").offset == 187
assert getattr(_OaCifpAirport, "reserved").offset == 188
assert getattr(_OaCifpRunway, "meta").offset == 0
assert getattr(_OaCifpRunway, "present").offset == 48
assert getattr(_OaCifpRunway, "airport_ident").offset == 56
assert getattr(_OaCifpRunway, "airport_region").offset == 72
assert getattr(_OaCifpRunway, "ident").offset == 88
assert getattr(_OaCifpRunway, "localizer_ident").offset == 104
assert getattr(_OaCifpRunway, "threshold").offset == 120
assert getattr(_OaCifpRunway, "length_ft").offset == 136
assert getattr(_OaCifpRunway, "displaced_threshold_ft").offset == 140
assert getattr(_OaCifpRunway, "magnetic_bearing").offset == 144
assert getattr(_OaCifpRunway, "ellipsoid_height_m").offset == 152
assert getattr(_OaCifpRunway, "threshold_elevation_ft").offset == 160
assert getattr(_OaCifpRunway, "threshold_crossing_height_ft").offset == 164
assert getattr(_OaCifpRunway, "width_ft").offset == 168
assert getattr(_OaCifpRunway, "tch_indicator").offset == 172
assert getattr(_OaCifpRunway, "localizer_category").offset == 173
assert getattr(_OaCifpRunway, "reserved").offset == 174
assert getattr(_OaCifpRunway, "reserved2").offset == 180
assert getattr(_OaCifpLocalizer, "meta").offset == 0
assert getattr(_OaCifpLocalizer, "present").offset == 48
assert getattr(_OaCifpLocalizer, "airport_ident").offset == 56
assert getattr(_OaCifpLocalizer, "airport_region").offset == 72
assert getattr(_OaCifpLocalizer, "ident").offset == 88
assert getattr(_OaCifpLocalizer, "runway_ident").offset == 104
assert getattr(_OaCifpLocalizer, "localizer_position").offset == 120
assert getattr(_OaCifpLocalizer, "glide_slope_position").offset == 136
assert getattr(_OaCifpLocalizer, "category").offset == 152
assert getattr(_OaCifpLocalizer, "reserved0").offset == 153
assert getattr(_OaCifpLocalizer, "frequency_mhz").offset == 160
assert getattr(_OaCifpLocalizer, "bearing").offset == 168
assert getattr(_OaCifpLocalizer, "localizer_width_deg").offset == 176
assert getattr(_OaCifpLocalizer, "glide_slope_angle_deg").offset == 184
assert getattr(_OaCifpLocalizer, "station_declination").offset == 192
assert getattr(_OaCifpLocalizer, "glide_slope_tch_ft").offset == 200
assert getattr(_OaCifpLocalizer, "glide_slope_elevation_ft").offset == 204
assert getattr(_OaCifpVhfNavaid, "meta").offset == 0
assert getattr(_OaCifpVhfNavaid, "present").offset == 48
assert getattr(_OaCifpVhfNavaid, "airport_ident").offset == 56
assert getattr(_OaCifpVhfNavaid, "airport_region").offset == 72
assert getattr(_OaCifpVhfNavaid, "ident").offset == 88
assert getattr(_OaCifpVhfNavaid, "icao_region").offset == 104
assert getattr(_OaCifpVhfNavaid, "dme_ident").offset == 120
assert getattr(_OaCifpVhfNavaid, "datum").offset == 136
assert getattr(_OaCifpVhfNavaid, "name").offset == 152
assert getattr(_OaCifpVhfNavaid, "vor_position").offset == 168
assert getattr(_OaCifpVhfNavaid, "dme_position").offset == 184
assert getattr(_OaCifpVhfNavaid, "frequency_mhz").offset == 200
assert getattr(_OaCifpVhfNavaid, "station_declination").offset == 208
assert getattr(_OaCifpVhfNavaid, "dme_elevation_ft").offset == 216
assert getattr(_OaCifpVhfNavaid, "class").offset == 220
assert getattr(_OaCifpVhfNavaid, "figure_of_merit").offset == 225
assert getattr(_OaCifpVhfNavaid, "reserved").offset == 226
assert getattr(_OaCifpVhfNavaid, "reserved2").offset == 228
assert getattr(_OaCifpNdb, "meta").offset == 0
assert getattr(_OaCifpNdb, "present").offset == 48
assert getattr(_OaCifpNdb, "airport_ident").offset == 56
assert getattr(_OaCifpNdb, "airport_region").offset == 72
assert getattr(_OaCifpNdb, "ident").offset == 88
assert getattr(_OaCifpNdb, "icao_region").offset == 104
assert getattr(_OaCifpNdb, "class").offset == 120
assert getattr(_OaCifpNdb, "datum").offset == 136
assert getattr(_OaCifpNdb, "name").offset == 152
assert getattr(_OaCifpNdb, "position").offset == 168
assert getattr(_OaCifpNdb, "frequency_khz").offset == 184
assert getattr(_OaCifpNdb, "magnetic_variation").offset == 192
assert getattr(_OaCifpNdb, "terminal").offset == 200
assert getattr(_OaCifpNdb, "reserved").offset == 201
assert getattr(_OaCifpWaypoint, "meta").offset == 0
assert getattr(_OaCifpWaypoint, "present").offset == 48
assert getattr(_OaCifpWaypoint, "area").offset == 56
assert getattr(_OaCifpWaypoint, "area_region").offset == 72
assert getattr(_OaCifpWaypoint, "ident").offset == 88
assert getattr(_OaCifpWaypoint, "icao_region").offset == 104
assert getattr(_OaCifpWaypoint, "waypoint_type").offset == 120
assert getattr(_OaCifpWaypoint, "usage").offset == 136
assert getattr(_OaCifpWaypoint, "datum").offset == 152
assert getattr(_OaCifpWaypoint, "name").offset == 168
assert getattr(_OaCifpWaypoint, "position").offset == 184
assert getattr(_OaCifpWaypoint, "magnetic_variation").offset == 200
assert getattr(_OaCifpWaypoint, "kind").offset == 208
assert getattr(_OaCifpWaypoint, "reserved").offset == 209
assert getattr(_OaCifpAirway, "customer_area").offset == 0
assert getattr(_OaCifpAirway, "ident").offset == 16
assert getattr(_OaCifpAirway, "first_line").offset == 32
assert getattr(_OaCifpAirway, "fix_start").offset == 40
assert getattr(_OaCifpAirway, "fix_count").offset == 48
assert getattr(_OaCifpAirway, "route_type").offset == 56
assert getattr(_OaCifpAirway, "level").offset == 57
assert getattr(_OaCifpAirway, "reserved").offset == 58
assert getattr(_OaCifpAirwayFix, "meta").offset == 0
assert getattr(_OaCifpAirwayFix, "route_ident").offset == 48
assert getattr(_OaCifpAirwayFix, "fix").offset == 64
assert getattr(_OaCifpAirwayFix, "sequence").offset == 104
assert getattr(_OaCifpAirwayFix, "description").offset == 108
assert getattr(_OaCifpAirwayFix, "route_type").offset == 112
assert getattr(_OaCifpAirwayFix, "level").offset == 113
assert getattr(_OaCifpAirwayFix, "reserved0").offset == 114
assert getattr(_OaCifpAirwayFix, "present").offset == 120
assert getattr(_OaCifpAirwayFix, "outbound_course").offset == 128
assert getattr(_OaCifpAirwayFix, "distance_nm").offset == 136
assert getattr(_OaCifpAirwayFix, "inbound_course").offset == 144
assert getattr(_OaCifpAirwayFix, "min_altitude").offset == 152
assert getattr(_OaCifpAirwayFix, "min_altitude_2").offset == 160
assert getattr(_OaCifpAirwayFix, "max_altitude").offset == 168
assert getattr(_OaCifpProcedure, "customer_area").offset == 0
assert getattr(_OaCifpProcedure, "airport_ident").offset == 16
assert getattr(_OaCifpProcedure, "airport_region").offset == 32
assert getattr(_OaCifpProcedure, "ident").offset == 48
assert getattr(_OaCifpProcedure, "first_line").offset == 64
assert getattr(_OaCifpProcedure, "route_start").offset == 72
assert getattr(_OaCifpProcedure, "route_count").offset == 80
assert getattr(_OaCifpProcedure, "kind").offset == 88
assert getattr(_OaCifpProcedure, "reserved").offset == 89
assert getattr(_OaCifpRoute, "transition").offset == 0
assert getattr(_OaCifpRoute, "first_line").offset == 16
assert getattr(_OaCifpRoute, "leg_start").offset == 24
assert getattr(_OaCifpRoute, "leg_count").offset == 32
assert getattr(_OaCifpRoute, "route_type").offset == 40
assert getattr(_OaCifpRoute, "reserved").offset == 41
assert getattr(_OaCifpLeg, "meta").offset == 0
assert getattr(_OaCifpLeg, "present").offset == 48
assert getattr(_OaCifpLeg, "airport_ident").offset == 56
assert getattr(_OaCifpLeg, "airport_region").offset == 72
assert getattr(_OaCifpLeg, "ident").offset == 88
assert getattr(_OaCifpLeg, "transition").offset == 104
assert getattr(_OaCifpLeg, "rnp_code").offset == 120
assert getattr(_OaCifpLeg, "fix").offset == 136
assert getattr(_OaCifpLeg, "recommended_navaid").offset == 176
assert getattr(_OaCifpLeg, "center_fix").offset == 216
assert getattr(_OaCifpLeg, "sequence").offset == 256
assert getattr(_OaCifpLeg, "description").offset == 260
assert getattr(_OaCifpLeg, "kind").offset == 264
assert getattr(_OaCifpLeg, "route_type").offset == 265
assert getattr(_OaCifpLeg, "turn_direction").offset == 266
assert getattr(_OaCifpLeg, "path_terminator").offset == 267
assert getattr(_OaCifpLeg, "altitude_description").offset == 268
assert getattr(_OaCifpLeg, "gnss_fms_indication").offset == 269
assert getattr(_OaCifpLeg, "speed_limit_description").offset == 270
assert getattr(_OaCifpLeg, "reserved0").offset == 271
assert getattr(_OaCifpLeg, "arc_radius_nm").offset == 272
assert getattr(_OaCifpLeg, "theta_deg").offset == 280
assert getattr(_OaCifpLeg, "rho_nm").offset == 288
assert getattr(_OaCifpLeg, "course_deg").offset == 296
assert getattr(_OaCifpLeg, "distance_or_time_value").offset == 304
assert getattr(_OaCifpLeg, "distance_or_time_kind").offset == 312
assert getattr(_OaCifpLeg, "reserved1").offset == 313
assert getattr(_OaCifpLeg, "altitude1").offset == 320
assert getattr(_OaCifpLeg, "altitude2").offset == 328
assert getattr(_OaCifpLeg, "transition_altitude").offset == 336
assert getattr(_OaCifpLeg, "speed_limit_kt").offset == 344
assert getattr(_OaCifpLeg, "reserved2").offset == 348
assert getattr(_OaCifpLeg, "vertical_angle_deg").offset == 352
assert getattr(_OaCifpContinuation, "meta").offset == 0
assert getattr(_OaCifpContinuation, "owner_index").offset == 48
assert getattr(_OaCifpContinuation, "text").offset == 56
assert getattr(_OaCifpContinuation, "number").offset == 72
assert getattr(_OaCifpContinuation, "application_type").offset == 73
assert getattr(_OaCifpContinuation, "owner_kind").offset == 74
assert getattr(_OaCifpContinuation, "reserved").offset == 75
assert getattr(_OaCifpLevelOfService, "continuation_index").offset == 0
assert getattr(_OaCifpLevelOfService, "name").offset == 8
assert getattr(_OaCifpLevelOfService, "authorized").offset == 24
assert getattr(_OaCifpLevelOfService, "reserved").offset == 25
assert getattr(_OaCifpAirspace, "meta").offset == 0
assert getattr(_OaCifpAirspace, "present").offset == 48
assert getattr(_OaCifpAirspace, "icao_region").offset == 56
assert getattr(_OaCifpAirspace, "designation").offset == 72
assert getattr(_OaCifpAirspace, "name").offset == 88
assert getattr(_OaCifpAirspace, "boundary_start").offset == 104
assert getattr(_OaCifpAirspace, "boundary_count").offset == 112
assert getattr(_OaCifpAirspace, "continuation_start").offset == 120
assert getattr(_OaCifpAirspace, "continuation_count").offset == 128
assert getattr(_OaCifpAirspace, "kind").offset == 136
assert getattr(_OaCifpAirspace, "airspace_type").offset == 137
assert getattr(_OaCifpAirspace, "classification").offset == 138
assert getattr(_OaCifpAirspace, "multiple_code").offset == 139
assert getattr(_OaCifpAirspace, "level").offset == 140
assert getattr(_OaCifpAirspace, "time_code").offset == 141
assert getattr(_OaCifpAirspace, "reserved").offset == 142
assert getattr(_OaCifpAirspace, "lower").offset == 144
assert getattr(_OaCifpAirspace, "lower_unit").offset == 152
assert getattr(_OaCifpAirspace, "upper_unit").offset == 153
assert getattr(_OaCifpAirspace, "reserved2").offset == 154
assert getattr(_OaCifpAirspace, "upper").offset == 160
assert getattr(_OaCifpBoundary, "position").offset == 0
assert getattr(_OaCifpBoundary, "arc_origin").offset == 16
assert getattr(_OaCifpBoundary, "sequence").offset == 32
assert getattr(_OaCifpBoundary, "via").offset == 36
assert getattr(_OaCifpBoundary, "end").offset == 37
assert getattr(_OaCifpBoundary, "present").offset == 38
assert getattr(_OaCifpBoundary, "reserved").offset == 39
assert getattr(_OaCifpBoundary, "arc_distance_nm").offset == 40
assert getattr(_OaCifpBoundary, "arc_bearing_deg").offset == 48
assert getattr(_OaCifpBoundary, "source_line").offset == 56
assert getattr(_OaCifpMora, "meta").offset == 0
assert getattr(_OaCifpMora, "start_lat").offset == 48
assert getattr(_OaCifpMora, "start_lon").offset == 50
assert getattr(_OaCifpMora, "present_mask").offset == 52
assert getattr(_OaCifpMora, "cells_ft").offset == 56
assert getattr(_OaCifpHeader, "file_name").offset == 0
assert getattr(_OaCifpHeader, "version").offset == 16
assert getattr(_OaCifpHeader, "creation_date").offset == 32
assert getattr(_OaCifpHeader, "creation_time").offset == 48
assert getattr(_OaCifpHeader, "data_supplier").offset == 64
assert getattr(_OaCifpHeader, "crc").offset == 80
assert getattr(_OaCifpHeader, "record_length").offset == 96
assert getattr(_OaCifpHeader, "reserved0").offset == 100
assert getattr(_OaCifpHeader, "record_count").offset == 104
assert getattr(_OaCifpHeader, "cycle_year").offset == 112
assert getattr(_OaCifpHeader, "cycle_release").offset == 113
assert getattr(_OaCifpHeader, "production_flag").offset == 114
assert getattr(_OaCifpHeader, "crc_verified").offset == 115
assert getattr(_OaCifpHeader, "reserved").offset == 116
assert getattr(_OaCifpHeaderText, "text").offset == 0
assert getattr(_OaCifpSectionCount, "section").offset == 0
assert getattr(_OaCifpSectionCount, "count").offset == 16
assert getattr(_OaCifpSectionCount, "supported").offset == 24
assert getattr(_OaCifpSectionCount, "reserved").offset == 25
assert getattr(_OaCifpCycleCount, "count").offset == 0
assert getattr(_OaCifpCycleCount, "year").offset == 8
assert getattr(_OaCifpCycleCount, "release").offset == 9
assert getattr(_OaCifpCycleCount, "reserved").offset == 10
assert getattr(_OaCifpError, "status").offset == 0
assert getattr(_OaCifpError, "kind").offset == 4
assert getattr(_OaCifpError, "line").offset == 8
assert getattr(_OaCifpError, "first_column").offset == 16
assert getattr(_OaCifpError, "last_column").offset == 20
assert getattr(_OaCifpError, "limit").offset == 24
assert getattr(_OaCifpError, "actual").offset == 32
assert getattr(_OaCifpError, "previous").offset == 40
assert getattr(_OaCifpError, "found").offset == 48
assert getattr(_OaCifpError, "code").offset == 56
assert getattr(_OaCifpError, "flags").offset == 60
assert getattr(_OaCifpError, "field").offset == 64
assert getattr(_OaCifpError, "value").offset == 96
assert getattr(_OaCifpError, "reason").offset == 228
assert getattr(_OaCifpError, "reserved").offset == 292
assert getattr(_OaCifpCounts, "airports").offset == 0
assert getattr(_OaCifpCounts, "runways").offset == 8
assert getattr(_OaCifpCounts, "localizers").offset == 16
assert getattr(_OaCifpCounts, "vhf_navaids").offset == 24
assert getattr(_OaCifpCounts, "ndbs").offset == 32
assert getattr(_OaCifpCounts, "waypoints").offset == 40
assert getattr(_OaCifpCounts, "moras").offset == 48
assert getattr(_OaCifpCounts, "airspaces").offset == 56
assert getattr(_OaCifpCounts, "airways").offset == 64
assert getattr(_OaCifpCounts, "airway_fixes").offset == 72
assert getattr(_OaCifpCounts, "procedures").offset == 80
assert getattr(_OaCifpCounts, "routes").offset == 88
assert getattr(_OaCifpCounts, "legs").offset == 96
assert getattr(_OaCifpCounts, "continuations").offset == 104
assert getattr(_OaCifpCounts, "levels_of_service").offset == 112
assert getattr(_OaCifpCounts, "boundaries").offset == 120
assert getattr(_OaCifpCounts, "header_records").offset == 128
assert getattr(_OaCifpCounts, "sections").offset == 136
assert getattr(_OaCifpCounts, "cycles").offset == 144
assert getattr(_OaCifpCounts, "errors").offset == 152
assert getattr(_OaCifpCounts, "errors_dropped").offset == 160
assert getattr(_OaCifpCounts, "strings_bytes").offset == 168
assert getattr(_OaCifpCounts, "output_bytes").offset == 176
assert getattr(_OaCifpCounts, "is_clean").offset == 184
assert getattr(_OaCifpCounts, "reserved").offset == 185
assert getattr(_OaCifpBuffers, "header").offset == 0
assert getattr(_OaCifpBuffers, "header_capacity").offset == 8
assert getattr(_OaCifpBuffers, "header_text").offset == 16
assert getattr(_OaCifpBuffers, "header_text_capacity").offset == 24
assert getattr(_OaCifpBuffers, "airports").offset == 32
assert getattr(_OaCifpBuffers, "airports_capacity").offset == 40
assert getattr(_OaCifpBuffers, "runways").offset == 48
assert getattr(_OaCifpBuffers, "runways_capacity").offset == 56
assert getattr(_OaCifpBuffers, "localizers").offset == 64
assert getattr(_OaCifpBuffers, "localizers_capacity").offset == 72
assert getattr(_OaCifpBuffers, "vhf_navaids").offset == 80
assert getattr(_OaCifpBuffers, "vhf_navaids_capacity").offset == 88
assert getattr(_OaCifpBuffers, "ndbs").offset == 96
assert getattr(_OaCifpBuffers, "ndbs_capacity").offset == 104
assert getattr(_OaCifpBuffers, "waypoints").offset == 112
assert getattr(_OaCifpBuffers, "waypoints_capacity").offset == 120
assert getattr(_OaCifpBuffers, "moras").offset == 128
assert getattr(_OaCifpBuffers, "moras_capacity").offset == 136
assert getattr(_OaCifpBuffers, "airspaces").offset == 144
assert getattr(_OaCifpBuffers, "airspaces_capacity").offset == 152
assert getattr(_OaCifpBuffers, "airways").offset == 160
assert getattr(_OaCifpBuffers, "airways_capacity").offset == 168
assert getattr(_OaCifpBuffers, "airway_fixes").offset == 176
assert getattr(_OaCifpBuffers, "airway_fixes_capacity").offset == 184
assert getattr(_OaCifpBuffers, "procedures").offset == 192
assert getattr(_OaCifpBuffers, "procedures_capacity").offset == 200
assert getattr(_OaCifpBuffers, "routes").offset == 208
assert getattr(_OaCifpBuffers, "routes_capacity").offset == 216
assert getattr(_OaCifpBuffers, "legs").offset == 224
assert getattr(_OaCifpBuffers, "legs_capacity").offset == 232
assert getattr(_OaCifpBuffers, "continuations").offset == 240
assert getattr(_OaCifpBuffers, "continuations_capacity").offset == 248
assert getattr(_OaCifpBuffers, "levels_of_service").offset == 256
assert getattr(_OaCifpBuffers, "levels_of_service_capacity").offset == 264
assert getattr(_OaCifpBuffers, "boundaries").offset == 272
assert getattr(_OaCifpBuffers, "boundaries_capacity").offset == 280
assert getattr(_OaCifpBuffers, "sections").offset == 288
assert getattr(_OaCifpBuffers, "sections_capacity").offset == 296
assert getattr(_OaCifpBuffers, "cycles").offset == 304
assert getattr(_OaCifpBuffers, "cycles_capacity").offset == 312
assert getattr(_OaCifpBuffers, "errors").offset == 320
assert getattr(_OaCifpBuffers, "errors_capacity").offset == 328
assert getattr(_OaCifpBuffers, "strings").offset == 336
assert getattr(_OaCifpBuffers, "strings_capacity").offset == 344

assert ctypes.sizeof(_AsterixPosition) == 12 and ctypes.alignment(_AsterixPosition) == 4
assert ctypes.sizeof(_AsterixVertical) == 12 and ctypes.alignment(_AsterixVertical) == 4
assert ctypes.sizeof(_AsterixRecord) == 192 and ctypes.alignment(_AsterixRecord) == 8
assert _AsterixRecord.present_frn_mask.offset == 8 and _AsterixRecord.positions.offset == 64
assert _AsterixRecord.vertical.offset == 120
assert ctypes.sizeof(_AsterixCounts) == 32 and _AsterixCounts.consumed_bytes.offset == 8
assert ctypes.sizeof(_AsterixError) == 32 and _AsterixError.offset.offset == 24
assert ctypes.sizeof(_NotamRow) == 272 and ctypes.alignment(_NotamRow) == 8
assert ctypes.sizeof(_NotamFeature) == 56 and ctypes.alignment(_NotamFeature) == 8
assert ctypes.sizeof(_NotamPolygon) == 16 and ctypes.alignment(_NotamPolygon) == 4
assert ctypes.sizeof(_NotamVertex) == 8 and ctypes.sizeof(_NotamUnknownPath) == 184
assert ctypes.sizeof(_NotamSummary) == 512 and ctypes.sizeof(_NotamBuffers) == 96


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
    18: "TAR_BAD_HEADER",
    19: "TAR_TRUNCATED",
    20: "TAR_LIMIT",
    21: "TAR_NAME_TOO_LONG",
    22: "TAR_READ_ERROR",
    32: "ZIP_FORMAT",
    33: "ZIP_TRUNCATED",
    34: "ZIP_UNSUPPORTED_METHOD",
    35: "ZIP_ENCRYPTED",
    36: "ZIP_UNSAFE_PATH",
    37: "ZIP_LIMIT",
    38: "ZIP_INFLATE_OR_SIZE",
    39: "ZIP_CRC",
    40: "ZIP_BAD_ENTRY_INDEX",
    80: "GRIB2_FORMAT",
    81: "GRIB2_UNSUPPORTED",
    82: "GRIB2_LIMIT",
    42: "NOTAM_INPUT_LIMIT",
    43: "NOTAM_XML_REFUSAL",
    44: "NOTAM_DOCUMENT_REFUSAL",
    45: "NOTAM_ELEMENT_LIMIT",
    46: "NOTAM_CAPACITY",
    47: "NOTAM_SEALED_DROP",
    48: "NOTAM_MEMORY_BUDGET",
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
class TarEntry:
    name: str
    size: int
    typeflag: int
    mtime: int
    header_offset: int


@dataclass(frozen=True, slots=True)
class TarResult:
    entries: tuple[TarEntry, ...]


@dataclass(frozen=True, slots=True)
class AsterixPosition:
    item: int
    kind: int
    scale: int
    a_raw: int
    b_raw: int


@dataclass(frozen=True, slots=True)
class AsterixVertical:
    item: int
    subitem: int
    kind: int
    scale: int
    flags: int
    auxiliary: int
    raw: int


@dataclass(frozen=True, slots=True)
class AsterixRecord:
    category: int
    cat021_edition: int
    source_sac: int
    source_sic: int
    present_frn_mask: int
    raw_untyped_frn_mask: int
    typed_unprojected_frn_mask: int
    present_field_mask: int
    target_address: int
    target_ident: bytes
    track_number: int
    time_item: int
    time_raw: int
    positions: tuple[AsterixPosition, ...]
    velocity_kind: int
    velocity_flags: int
    velocity_a_raw: int
    velocity_b_raw: int
    mode3a_code: int
    mode3a_flags: int
    vertical: tuple[AsterixVertical, ...]
    raw_untyped_subitem_mask: int


@dataclass(frozen=True, slots=True)
class AsterixResult:
    records: tuple[AsterixRecord, ...]
    category: int
    edition: int
    consumed_bytes: int


class AsterixError(OpenAtcaError):
    """Structured ASTERIX block refusal from the existing Rust decoder."""

    def __init__(self, status: int, kind: int, category: int, record: int,
                 item: int, subitem: int, offset: int) -> None:
        self.asterix_kind = kind
        self.category = category
        self.record = record
        self.item = item
        self.subitem = subitem
        self.offset = offset
        super().__init__(status, message=f"ASTERIX error kind {kind}")


class NotamError(OpenAtcaError):
    """Whole-document or binding refusal from the FAA FNS AIXM 5.1 profile decoder."""

    def __init__(self, summary: NotamSummary, code: int) -> None:
        self.summary = summary
        self.refusal_kind = summary.refusal_kind
        self.source_offset = summary.source_offset
        super().__init__(code, message=f"NOTAM refusal kind {self.refusal_kind}")


@dataclass(frozen=True, slots=True)
class Grib2Field:
    discipline: int
    category: int
    number: int
    named_parameter: int
    grid_template: int
    product_template: int
    packing_template: int
    level_kind: int
    forecast_time_unit: int
    reference_time: tuple[int, int, int, int, int, int]
    forecast_time: int
    level_value: float
    interval_present: bool
    interval_ranges: int
    interval_statistic: int
    interval_end: tuple[int, int, int, int, int, int]
    nx: int
    ny: int
    grid_flags: int
    scan_mode: int
    latitude_row_direction: int
    global_longitude: bool
    grid_parameters: tuple[float, ...]
    value_offset: int
    value_count: int
    missing_count: int


@dataclass(frozen=True, slots=True)
class Grib2Message:
    fields: tuple[Grib2Field, ...]
    values: array.array


@dataclass(frozen=True, slots=True)
class ZipEntry:
    """One validated ZIP central-directory record; flags and method are source values."""

    name: str
    compressed_size: int
    uncompressed_size: int
    local_offset: int
    crc32: int
    flags: int
    method: int
    zip64: bool
    utf8_name_flag: bool


@dataclass(frozen=True, slots=True)
class ZipResult:
    entries: tuple[ZipEntry, ...]


@dataclass(frozen=True, slots=True)
class NotamRow:
    """One decoded FNS event row; time is Unix milliseconds and geometry is signed E7."""

    present: int
    errors: int
    issued_ms: int
    start_ms: int
    end_ms: int
    canceled_ms: int
    flags: int
    number: int
    ref_number: int
    sched_off: int
    sched_len: int
    text_off: int
    text_len: int
    radius_m: int
    poly_first: int
    poly_count: int
    feature_first: int
    feature_count: int
    feat_mask: int
    lat_e7: int
    lon_e7: int
    floor_ft: int
    ceil_ft: int
    year: int
    ref_year: int
    scenario: int
    ntype: int
    class_code: int
    traffic: int
    purpose: int
    scope: int
    floor_ref: int
    ceil_ref: int
    feat_kind: int
    interpretation: int
    slice_meta: int
    slice_errors: int
    sequence_number: int
    correction_number: int
    event_id: bytes
    series: bytes
    ref_series: bytes
    xover: bytes
    feat_desig: bytes
    runway: bytes
    airport: bytes
    navaid: bytes
    unit: bytes
    selection: bytes
    location: bytes
    fir: bytes
    icao_location: bytes
    airport_icao: bytes


@dataclass(frozen=True, slots=True)
class NotamFeature:
    kind: int
    time_kind: int
    interpretation: int
    meta: int
    time_meta: int
    errors: int
    begin_ms: int
    end_ms: int
    sequence_number: int
    correction_number: int
    designator: bytes
    designator_len: int
    icao: bytes
    icao_len: int


@dataclass(frozen=True, slots=True)
class NotamPolygon:
    first_vertex: int
    vertex_count: int
    feature_index: int
    kind: int


@dataclass(frozen=True, slots=True)
class NotamVertex:
    lat_e7: int
    lon_e7: int


@dataclass(frozen=True, slots=True)
class NotamUnknownPath:
    hash: int
    count: int
    is_attribute: bool
    text: bytes


@dataclass(frozen=True, slots=True)
class NotamSummary:
    status: int
    refusal_kind: int
    source_offset: int | None
    unknown_elements: int
    unknown_attributes: int
    unknown_overflow: int
    sealed_dropped: int
    field_errors: tuple[int, ...]
    rejects: tuple[int, ...]
    features: tuple[int, ...]


@dataclass(frozen=True, slots=True)
class NotamResult:
    row: NotamRow | None
    features: tuple[NotamFeature, ...]
    polygons: tuple[NotamPolygon, ...]
    vertices: tuple[NotamVertex, ...]
    strings: bytes
    unknown_paths: tuple[NotamUnknownPath, ...]
    summary: NotamSummary


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


class _TarEntry(ctypes.Structure):
    _fields_ = [
        ("name_offset", ctypes.c_uint64),
        ("name_length", ctypes.c_uint64),
        ("size", ctypes.c_uint64),
        ("mtime", ctypes.c_uint64),
        ("header_offset", ctypes.c_uint64),
        ("typeflag", ctypes.c_uint8),
        ("reserved", ctypes.c_uint8 * 7),
    ]


class _ZipEntry(ctypes.Structure):
    _fields_ = [
        ("name_offset", ctypes.c_uint64),
        ("name_length", ctypes.c_uint64),
        ("compressed_size", ctypes.c_uint64),
        ("uncompressed_size", ctypes.c_uint64),
        ("local_offset", ctypes.c_uint64),
        ("crc32", ctypes.c_uint32),
        ("flags", ctypes.c_uint16),
        ("method", ctypes.c_uint16),
        ("zip64", ctypes.c_uint8),
        ("utf8_name_flag", ctypes.c_uint8),
        ("reserved", ctypes.c_uint8 * 6),
    ]


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


class AsterixBindings:
    """One-block ASTERIX decoder using the explicit CAT021 edition selector."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        byte_p = ctypes.POINTER(ctypes.c_uint8)
        lib = library
        lib.oa_asterix_decode_block.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_uint8,
            ctypes.POINTER(_AsterixRecord), ctypes.c_uint64, ctypes.POINTER(_AsterixCounts),
            ctypes.POINTER(_AsterixError), ctypes.POINTER(_ErrorInfo)]
        lib.oa_asterix_decode_block.restype = ctypes.c_uint32

    def decode_block(self, data: bytes, *, cat021_edition: int = 0) -> AsterixResult:
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_ASTERIX_BLOCK_BYTES:
            raise ValueError("data exceeds the ASTERIX block limit")
        if isinstance(cat021_edition, bool) or cat021_edition not in (0, 24, 27):
            raise ValueError("cat021_edition must be 0, 24, or 27")
        source = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        counts, detail, generic = _AsterixCounts(), _AsterixError(), _ErrorInfo()
        status = int(self._library.oa_asterix_decode_block(source, len(data), cat021_edition,
            None, 0, ctypes.byref(counts), ctypes.byref(detail), ctypes.byref(generic)))
        if status not in (0, 6):
            raise AsterixError(detail.status, detail.kind, detail.category, detail.record,
                               detail.item, detail.subitem, detail.offset)
        count = int(counts.required_record_count)
        if count > _MAX_ASTERIX_RECORDS:
            raise RuntimeError("native ASTERIX record count exceeded its documented bound")
        if not count:
            return AsterixResult((), counts.category, counts.edition, counts.consumed_bytes)
        rows = (_AsterixRecord * count)()
        status = int(self._library.oa_asterix_decode_block(source, len(data), cat021_edition,
            rows, count, ctypes.byref(counts), ctypes.byref(detail), ctypes.byref(generic)))
        if status:
            raise AsterixError(detail.status, detail.kind, detail.category, detail.record,
                               detail.item, detail.subitem, detail.offset)
        converted = []
        for row in rows:
            positions = tuple(AsterixPosition(p.item, p.kind, p.scale, p.a_raw, p.b_raw)
                              for p in row.positions[:row.position_count])
            vertical = tuple(AsterixVertical(v.item, v.subitem, v.kind, v.scale, v.flags,
                                             v.auxiliary, v.raw)
                             for v in row.vertical[:row.vertical_count])
            converted.append(AsterixRecord(row.category, row.cat021_edition, row.source_sac,
                row.source_sic, row.present_frn_mask, row.raw_untyped_frn_mask,
                row.typed_unprojected_frn_mask, row.present_field_mask, row.target_address,
                bytes(row.target_ident), row.track_number, row.time_item, row.time_raw,
                positions, row.velocity_kind, row.velocity_flags, row.velocity_a_raw,
                row.velocity_b_raw, row.mode3a_code, row.mode3a_flags, vertical,
                row.raw_untyped_subitem_mask))
        return AsterixResult(tuple(converted), counts.category, counts.edition,
                             counts.consumed_bytes)


class NotamBindings:
    """Bounded one-document FAA FNS AIXM 5.1 profile decoder."""

_CIFP_ROW_SIZES: Final[tuple[tuple[str, int], ...]] = (
    ("header", 120), ("header_text", 16), ("airports", 192), ("runways", 184),
    ("localizers", 208), ("vhf_navaids", 232), ("ndbs", 208), ("waypoints", 216),
    ("moras", 176), ("airspaces", 168), ("airways", 64), ("airway_fixes", 176),
    ("procedures", 96), ("routes", 48), ("legs", 360), ("continuations", 80),
    ("levels_of_service", 32), ("boundaries", 64), ("sections", 32), ("cycles", 16),
    ("errors", 296),
)



@dataclass(frozen=True, slots=True)
class CifpCounts:
    """Required fixed-table and string-arena sizes from one CIFP parse."""

    airports: int
    runways: int
    localizers: int
    vhf_navaids: int
    ndbs: int
    waypoints: int
    moras: int
    airspaces: int
    airways: int
    airway_fixes: int
    procedures: int
    routes: int
    legs: int
    continuations: int
    levels_of_service: int
    boundaries: int
    header_records: int
    sections: int
    cycles: int
    errors: int
    errors_dropped: int
    strings_bytes: int
    output_bytes: int
    is_clean: bool


@dataclass(frozen=True, slots=True)
class CifpText:
    """UTF-8 field value and its original offset in the call's immutable string arena."""

    text: str
    offset: int
    length: int

    def __str__(self) -> str:
        return self.text


_CIFP_PUBLIC_RECORD_TYPES: dict[type[ctypes.Structure], type] = {}
_nested_ctypes = (
    _OaCifpMeta, _OaCifpAltitude, _OaCifpFixRef, _OaCifpPosition,
    *tuple(cls for _, cls in _CIFP_ROW_TYPES),
)
_CIFP_PUBLIC_NAMES = {
    _ctype: ("CifpDiagnostic" if _ctype is _OaCifpError else
             _ctype.__name__.removeprefix("_Oa"))
    for _ctype in _nested_ctypes
}


def _cifp_public_annotation(ctype: type) -> str:
    if ctype is _OaCifpSpan:
        return "CifpText | None"
    if ctype in _CIFP_PUBLIC_NAMES:
        return _CIFP_PUBLIC_NAMES[ctype]
    if issubclass(ctype, ctypes.Array):
        if ctype._type_ is ctypes.c_uint8:
            return "bytes"
        return f"tuple[{_cifp_public_annotation(ctype._type_)}, ...]"
    if ctype is ctypes.c_double:
        return "float"
    if ctype in _CIFP_CTYPES.values():
        return "int"
    raise TypeError(f"unsupported CIFP public field type: {ctype!r}")


for _ctype in _nested_ctypes:
    _public_fields = []
    for _field, _field_type in _ctype._fields_:
        if _field.startswith("reserved") or _field == "reserved":
            continue
        _public_fields.append((
            "class_codes" if _field == "class" else _field,
            _cifp_public_annotation(_field_type),
        ))
    _public_class = make_dataclass(
        _CIFP_PUBLIC_NAMES[_ctype], _public_fields, frozen=True, slots=True,
        namespace={"__module__": __name__},
    )
    _CIFP_PUBLIC_RECORD_TYPES[_ctype] = _public_class
    globals()[_CIFP_PUBLIC_NAMES[_ctype]] = _public_class

_CIFP_ROW_CLASS_TYPES = tuple(
    _CIFP_PUBLIC_RECORD_TYPES[ctype] for _, ctype in _CIFP_ROW_TYPES
)
CifpRow: TypeAlias = Union[_CIFP_ROW_CLASS_TYPES]


def _cifp_value(value: object, strings: bytes) -> object:
    if isinstance(value, _OaCifpSpan):
        offset, length = int(value.offset), int(value.length)
        if offset == (1 << 64) - 1:
            if length != 0:
                raise RuntimeError("native CIFP output used a missing span with nonzero length")
            return None
        end = offset + length
        if end < offset or end > len(strings):
            raise RuntimeError("native CIFP output span exceeds its string arena")
        try:
            text = strings[offset:end].decode("utf-8", errors="strict")
        except UnicodeDecodeError as exc:
            raise RuntimeError("native CIFP output contains invalid UTF-8") from exc
        return CifpText(text, offset, length)
    if isinstance(value, ctypes.Structure):
        cls = _CIFP_PUBLIC_RECORD_TYPES.get(type(value))
        if cls is None:
            raise RuntimeError(f"no immutable Python projection for {type(value).__name__}")
        fields = {}
        for name, _ in value._fields_:
            if name.startswith("reserved") or name == "reserved":
                continue
            fields["class_codes" if name == "class" else name] = _cifp_value(getattr(value, name), strings)
        return cls(**fields)
    if isinstance(value, ctypes.Array):
        if value._type_ is ctypes.c_uint8:
            return bytes(value)
        return tuple(_cifp_value(item, strings) for item in value)
    if isinstance(value, (int, float)):
        return value
    if isinstance(value, (ctypes._SimpleCData,)):
        return value.value
    raise RuntimeError(f"unsupported CIFP ctypes value {type(value).__name__}")


@dataclass(frozen=True, slots=True)
class CifpResult:
    """Decoded immutable record families; all string fields retain value and source span."""

    counts: CifpCounts
    clean: bool
    strings: bytes
    header: tuple[CifpHeader, ...]
    header_text: tuple[CifpHeaderText, ...]
    airports: tuple[CifpAirport, ...]
    runways: tuple[CifpRunway, ...]
    localizers: tuple[CifpLocalizer, ...]
    vhf_navaids: tuple[CifpVhfNavaid, ...]
    ndbs: tuple[CifpNdb, ...]
    waypoints: tuple[CifpWaypoint, ...]
    moras: tuple[CifpMora, ...]
    airspaces: tuple[CifpAirspace, ...]
    airways: tuple[CifpAirway, ...]
    airway_fixes: tuple[CifpAirwayFix, ...]
    procedures: tuple[CifpProcedure, ...]
    routes: tuple[CifpRoute, ...]
    legs: tuple[CifpLeg, ...]
    continuations: tuple[CifpContinuation, ...]
    levels_of_service: tuple[CifpLevelOfService, ...]
    boundaries: tuple[CifpBoundary, ...]
    sections: tuple[CifpSectionCount, ...]
    cycles: tuple[CifpCycleCount, ...]
    errors: tuple[CifpDiagnostic, ...]

    @property
    def tables(self) -> tuple[tuple[str, tuple[CifpRow, ...]], ...]:
        return tuple((name, getattr(self, name)) for name, _ in _CIFP_ROW_TYPES)

    def table(self, name: str) -> tuple[CifpRow, ...]:
        if name not in {key for key, _ in _CIFP_ROW_TYPES}:
            raise KeyError(name)
        value = getattr(self, name)
        if not isinstance(value, tuple):
            raise KeyError(name)
        return value


class CifpError(OpenAtcaError):
    """Structured refusal; ``code`` remains the generic status and diagnostic_code the CIFP code."""

    def __init__(self, detail: _CifpError) -> None:
        self.kind = int(detail.kind)
        self.status = int(detail.status)
        self.line = int(detail.line)
        self.first_column = int(detail.first_column)
        self.last_column = int(detail.last_column)
        self.limit = int(detail.limit)
        self.actual = int(detail.actual)
        self.previous = int(detail.previous)
        self.found = int(detail.found)
        self.diagnostic_code = int(detail.code)
        self.flags = int(detail.flags)
        self.field = bytes(detail.field).split(b"\0", 1)[0]
        self.value = bytes(detail.value).split(b"\0", 1)[0]
        self.reason = bytes(detail.reason).split(b"\0", 1)[0]
        super().__init__(int(detail.status), message=self.reason.decode("utf-8", "replace"))


class CifpBindings:
    """Bounded binding for the existing FAA CIFP decoder and its approved profiles."""
    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        byte_p = ctypes.POINTER(ctypes.c_uint8)
        library.oa_notam_fns_decode.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_int64,
            ctypes.c_int64, ctypes.POINTER(_NotamBuffers), ctypes.POINTER(_NotamSummary),
            ctypes.POINTER(_ErrorInfo)]
        library.oa_notam_fns_decode.restype = ctypes.c_uint32

    @staticmethod
    def _summary(value: _NotamSummary) -> NotamSummary:
        offset = int(value.source_offset)
        return NotamSummary(
            status=int(value.status), refusal_kind=int(value.refusal_kind),
            source_offset=None if offset == (1 << 64) - 1 else offset,
            unknown_elements=int(value.unknown_elements),
            unknown_attributes=int(value.unknown_attributes),
            unknown_overflow=int(value.unknown_overflow),
            sealed_dropped=int(value.sealed_dropped),
            field_errors=tuple(int(v) for v in value.field_errors),
            rejects=tuple(int(v) for v in value.rejects),
            features=tuple(int(v) for v in value.features),
        )

    @staticmethod
    def _row(value: _NotamRow) -> NotamRow:
        byte_fields = {"event_id": 16, "series": 12, "ref_series": 12, "xover": 16,
                       "feat_desig": 12, "runway": 12, "airport": 8, "navaid": 8,
                       "unit": 8, "selection": 5, "location": 4, "fir": 4,
                       "icao_location": 4, "airport_icao": 4}
        values: dict[str, int | bytes] = {}
        for name, _field_type in _NotamRow._fields_:
            raw = getattr(value, name)
            values[name] = bytes(raw) if name in byte_fields else int(raw)
        return NotamRow(**values)  # type: ignore[arg-type]

    def decode(self, data: bytes, *, drop_window_ms: tuple[int, int]) -> NotamResult:
        """Decode bytes with an explicit half-open issue/cancellation filter in Unix ms.

        Equal endpoints deliberately select an empty interval. Omitting the filter is an error;
        tools that open protected files must still apply their normal source-path guards.
        """
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_NOTAM_INPUT_BYTES:
            raise ValueError("data exceeds the NOTAM profile input limit")
        if (not isinstance(drop_window_ms, tuple) or len(drop_window_ms) != 2 or
                any(isinstance(value, bool) or not isinstance(value, int)
                    for value in drop_window_ms)):
            raise TypeError("drop_window_ms must be a pair of integer Unix-millisecond endpoints")
        drop_from, drop_to = drop_window_ms
        if drop_from < -(1 << 63) or drop_from >= (1 << 63) or drop_to < -(1 << 63) or drop_to >= (1 << 63):
            raise ValueError("drop-window endpoints must fit signed 64-bit milliseconds")
        source = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        summary_raw, generic = _NotamSummary(), _ErrorInfo()
        call = self._library.oa_notam_fns_decode
        status = int(call(source, len(data), drop_from, drop_to, None,
                          ctypes.byref(summary_raw), ctypes.byref(generic)))
        summary = self._summary(summary_raw)
        if status == 47:
            return NotamResult(None, (), (), (), b"", (), summary)
        if status != 6:
            raise NotamError(summary, status)
        counts = (int(summary_raw.row_count), int(summary_raw.feature_count),
                  int(summary_raw.polygon_count), int(summary_raw.vertex_count),
                  int(summary_raw.string_bytes), int(summary_raw.unknown_path_count))
        if (counts[0] > 1 or counts[1] > 50_000 or counts[2] > 50_000 or
                counts[3] > len(data) // 3 or counts[4] > 2 * _MAX_NOTAM_TEXT_BYTES or
                counts[5] > _MAX_NOTAM_UNKNOWN_PATHS):
            raise RuntimeError("native NOTAM counts exceeded the documented bounded profile")
        rows = (_NotamRow * counts[0])() if counts[0] else None
        features = (_NotamFeature * counts[1])() if counts[1] else None
        polygons = (_NotamPolygon * counts[2])() if counts[2] else None
        vertices = (_NotamVertex * counts[3])() if counts[3] else None
        strings = (ctypes.c_uint8 * counts[4])() if counts[4] else None
        unknown = (_NotamUnknownPath * counts[5])() if counts[5] else None
        buffers = _NotamBuffers(
            rows, counts[0], features, counts[1], polygons, counts[2], vertices, counts[3],
            strings, counts[4], unknown, counts[5])
        status = int(call(source, len(data), drop_from, drop_to, ctypes.byref(buffers),
                          ctypes.byref(summary_raw), ctypes.byref(generic)))
        summary = self._summary(summary_raw)
        if status != 0:
            raise NotamError(summary, status)
        row = self._row(rows[0]) if rows else None
        feature_rows = tuple(NotamFeature(
            int(value.kind), int(value.time_kind), int(value.interpretation), int(value.meta),
            int(value.time_meta), int(value.errors), int(value.begin_ms), int(value.end_ms),
            int(value.sequence_number), int(value.correction_number), bytes(value.designator),
            int(value.designator_len), bytes(value.icao), int(value.icao_len)) for value in features or ())
        polygon_rows = tuple(NotamPolygon(int(value.first_vertex), int(value.vertex_count),
            int(value.feature_index), int(value.kind)) for value in polygons or ())
        vertex_rows = tuple(NotamVertex(int(value.lat_e7), int(value.lon_e7)) for value in vertices or ())
        string_bytes = bytes(strings) if strings is not None else b""
        unknown_rows = tuple(NotamUnknownPath(int(value.hash), int(value.count),
            bool(value.is_attribute), bytes(value.text[:value.text_len])) for value in unknown or ())
        return NotamResult(row, feature_rows, polygon_rows, vertex_rows, string_bytes,
                           unknown_rows, summary)

        library.oa_cifp_parse.argtypes = [byte_p, ctypes.c_uint64, byte_p, ctypes.c_uint64,
            ctypes.POINTER(_CifpBuffers), ctypes.POINTER(_CifpCounts),
            ctypes.POINTER(_CifpError), ctypes.POINTER(_ErrorInfo)]
        library.oa_cifp_parse.restype = ctypes.c_uint32

    def parse(self, data: bytes, *, source: str = "<bytes>") -> CifpResult:
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_CIFP_INPUT_BYTES:
            raise ValueError("data exceeds the CIFP input bound")
        if not isinstance(source, str):
            raise TypeError("source must be str")
        source_bytes = source.encode("utf-8", errors="strict")
        if len(source_bytes) > 256:
            raise ValueError("source exceeds the CIFP source-label bound")
        input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        source_buffer = ((ctypes.c_uint8 * len(source_bytes)).from_buffer_copy(source_bytes)
                         if source_bytes else None)
        counts, detail, generic = _CifpCounts(), _CifpError(), _ErrorInfo()
        query_buffers = _CifpBuffers()
        status = int(self._library.oa_cifp_parse(input_buffer, len(data), source_buffer,
            len(source_bytes), ctypes.byref(query_buffers), ctypes.byref(counts),
            ctypes.byref(detail), ctypes.byref(generic)))
        if status not in (0, 6):
            raise CifpError(detail)
        count_values = {name: int(getattr(counts, name)) for name in _CIFP_COUNT_NAMES}
        if int(counts.output_bytes) > _MAX_CIFP_OUTPUT_BYTES:
            raise MemoryError("CIFP output exceeds the documented 768 MiB bound")
        owners: list[object] = []
        table_buffers: list[tuple[str, type[ctypes.Structure], int, object | None]] = []
        buffers = _CifpBuffers()
        for name, row_type in _CIFP_ROW_TYPES:
            count = (1 if name == "header" else
                     int(counts.header_records) if name == "header_text" else
                     int(getattr(counts, name)))
            storage = (row_type * count)() if count else None
            owners.append(storage)
            setattr(buffers, name, storage)
            setattr(buffers, f"{name}_capacity", count)
            table_buffers.append((name, row_type, count, storage))
        string_storage = (ctypes.c_uint8 * int(counts.strings_bytes))() if counts.strings_bytes else None
        owners.append(string_storage)
        buffers.strings = string_storage
        buffers.strings_capacity = int(counts.strings_bytes)
        status = int(self._library.oa_cifp_parse(input_buffer, len(data), source_buffer,
            len(source_bytes), ctypes.byref(buffers), ctypes.byref(counts),
            ctypes.byref(detail), ctypes.byref(generic)))
        if status:
            raise CifpError(detail)
        strings = ctypes.string_at(string_storage, int(counts.strings_bytes)) if string_storage else b""
        tables = {}
        for name, row_type, count, storage in table_buffers:
            rows = tuple(_cifp_value(storage[index], strings) for index in range(count)) if storage else ()
            tables[name] = rows
        typed_counts = CifpCounts(**count_values, is_clean=bool(counts.is_clean))
        return CifpResult(
            counts=typed_counts,
            clean=bool(counts.is_clean),
            strings=strings,
            **tables,
        )

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


class Grib2Bindings:
    """Decode one complete message from the existing bounded GRIB2 profile."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library
        byte_p = ctypes.POINTER(ctypes.c_uint8)
        u64_p = ctypes.POINTER(ctypes.c_uint64)
        error_p = ctypes.POINTER(_ErrorInfo)
        library.oa_grib2_inspect_message.argtypes = [byte_p, ctypes.c_uint64, u64_p, u64_p, error_p]
        library.oa_grib2_inspect_message.restype = ctypes.c_uint32
        library.oa_grib2_decode_message.argtypes = [byte_p, ctypes.c_uint64,
            ctypes.POINTER(_Grib2Field), ctypes.c_uint64, ctypes.POINTER(ctypes.c_float),
            ctypes.c_uint64, u64_p, u64_p, error_p]
        library.oa_grib2_decode_message.restype = ctypes.c_uint32

    @staticmethod
    def _time(value: _Grib2Time) -> tuple[int, int, int, int, int, int]:
        return (int(value.year), int(value.month), int(value.day), int(value.hour),
                int(value.minute), int(value.second))

    def inspect_message(self, data: bytes) -> tuple[int, int]:
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_GRIB2_INPUT_BYTES:
            raise ValueError("data exceeds the native GRIB2 message limit")
        source = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        fields, values, error = ctypes.c_uint64(), ctypes.c_uint64(), _ErrorInfo()
        status = int(self._library.oa_grib2_inspect_message(source, len(data),
            ctypes.byref(fields), ctypes.byref(values), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        if fields.value > _MAX_GRIB2_FIELDS or values.value > _MAX_GRIB2_VALUES:
            raise RuntimeError("native GRIB2 inspector exceeded its declared bounds")
        return int(fields.value), int(values.value)

    def decode_message(self, data: bytes) -> Grib2Message:
        field_count, value_count = self.inspect_message(data)
        source = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        fields = (_Grib2Field * field_count)() if field_count else None
        values = array.array("f", [0.0]) * value_count
        if values.itemsize != ctypes.sizeof(ctypes.c_float):
            raise RuntimeError("platform f32 array does not match the native ABI")
        value_storage = (ctypes.c_float * value_count).from_buffer(values) if value_count else None
        value_pointer = ctypes.cast(value_storage, ctypes.POINTER(ctypes.c_float)) if value_storage else None
        required_fields, required_values, error = ctypes.c_uint64(), ctypes.c_uint64(), _ErrorInfo()
        status = int(self._library.oa_grib2_decode_message(source, len(data), fields, field_count,
            value_pointer, value_count, ctypes.byref(required_fields), ctypes.byref(required_values),
            ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        if required_fields.value != field_count or required_values.value != value_count:
            raise RuntimeError("native GRIB2 sizes changed between inspection and decode")
        projected = tuple(Grib2Field(int(f.discipline), int(f.category), int(f.number),
            int(f.named_parameter), int(f.grid_template), int(f.product_template),
            int(f.packing_template), int(f.level_kind), int(f.forecast_time_unit),
            self._time(f.reference_time), int(f.forecast_time), float(f.level_value),
            bool(f.interval_present), int(f.interval_ranges), int(f.interval_statistic),
            self._time(f.interval_end), int(f.nx), int(f.ny), int(f.grid_flags),
            int(f.scan_mode), int(f.latitude_row_direction), bool(f.global_longitude),
            (float(f.first_latitude_deg), float(f.first_longitude_deg),
             float(f.i_increment_deg), float(f.j_increment_deg), float(f.lambert_lad_deg),
             float(f.lambert_lov_deg), float(f.lambert_dx_m), float(f.lambert_dy_m),
             float(f.lambert_latin1_deg), float(f.lambert_latin2_deg), float(f.earth_radius_m)),
            int(f.value_offset), int(f.value_count), int(f.missing_count)) for f in fields or ())
        return Grib2Message(projected, values)


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
        self.archives = Archives(library)
        self.asterix = AsterixBindings(library)
        self.grib2 = Grib2Bindings(library)
        self.notams = NotamBindings(library)

        self.cifp = CifpBindings(library)
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
        lib.oa_tar_parse.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_uint32,
            ctypes.POINTER(_TarEntry), ctypes.c_uint64, u64_p,
            byte_p, ctypes.c_uint64, u64_p, error_p]
        lib.oa_tar_parse.restype = ctypes.c_uint32
        lib.oa_zip_list.argtypes = [byte_p, ctypes.c_uint64,
            ctypes.POINTER(_ZipEntry), ctypes.c_uint64, u64_p,
            byte_p, ctypes.c_uint64, u64_p, error_p]
        lib.oa_zip_list.restype = ctypes.c_uint32
        lib.oa_zip_extract_entry.argtypes = [byte_p, ctypes.c_uint64, ctypes.c_uint64,
            byte_p, ctypes.c_uint64, u64_p, error_p]
        lib.oa_zip_extract_entry.restype = ctypes.c_uint32
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


class Archives:
    """Metadata-only archive readers; this surface never extracts files or writes paths."""

    def __init__(self, library: ctypes.CDLL) -> None:
        self._library = library

    def parse_tar(self, data: bytes, *, max_entries: int = _MAX_TAR_ENTRIES) -> TarResult:
        """Validate one bounded tar archive and return ordered member metadata only."""
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_TAR_INPUT_BYTES:
            raise ValueError("data exceeds the native tar input limit")
        if isinstance(max_entries, bool) or not isinstance(max_entries, int):
            raise TypeError("max_entries must be an integer")
        if max_entries < 0 or max_entries > _MAX_TAR_ENTRIES:
            raise ValueError("max_entries is outside the native tar limit")
        input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        count, names_length, error = ctypes.c_uint64(), ctypes.c_uint64(), _ErrorInfo()
        status = int(self._library.oa_tar_parse(
            input_buffer, len(data), max_entries, None, 0, ctypes.byref(count),
            None, 0, ctypes.byref(names_length), ctypes.byref(error)))
        if status not in (0, 6):
            _raise_if_error(self._library, status, error)
        if status == 0:
            if count.value != 0 or names_length.value != 0:
                raise RuntimeError("native tar parser skipped required output on an empty success")
            return TarResult(())
        if count.value > _MAX_TAR_ENTRIES or names_length.value > _MAX_TAR_NAME_POOL_BYTES:
            raise RuntimeError("native tar parser returned sizes beyond its declared bound")
        entries = (_TarEntry * count.value)() if count.value else None
        name_pool = (ctypes.c_uint8 * names_length.value)() if names_length.value else None
        status = int(self._library.oa_tar_parse(
            input_buffer, len(data), max_entries, entries, count.value, ctypes.byref(count),
            name_pool, names_length.value, ctypes.byref(names_length), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        if count.value != (len(entries) if entries is not None else 0):
            raise RuntimeError("native tar parser changed its entry count between passes")
        pool = bytes(name_pool) if name_pool is not None else b""
        if names_length.value != len(pool):
            raise RuntimeError("native tar parser changed its name-pool length between passes")
        output: list[TarEntry] = []
        for item in entries or ():
            start, end = item.name_offset, item.name_offset + item.name_length
            if end > len(pool):
                raise RuntimeError("native tar parser returned an invalid name-pool span")
            output.append(TarEntry(
                name=pool[start:end].decode("utf-8"),
                size=item.size,
                typeflag=item.typeflag,
                mtime=item.mtime,
                header_offset=item.header_offset,
            ))
        return TarResult(tuple(output))

    def parse_zip(self, data: bytes) -> ZipResult:
        """Validate bounded ZIP metadata; names follow oa-zip's UTF-8/ASCII profile."""
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_ZIP_INPUT_BYTES:
            raise ValueError("data exceeds the native ZIP input limit")
        input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        count, names_length, error = ctypes.c_uint64(), ctypes.c_uint64(), _ErrorInfo()
        status = int(self._library.oa_zip_list(
            input_buffer, len(data), None, 0, ctypes.byref(count), None, 0,
            ctypes.byref(names_length), ctypes.byref(error)))
        if status not in (0, 6):
            _raise_if_error(self._library, status, error)
        if status == 0:
            if count.value != 0 or names_length.value != 0:
                raise RuntimeError("native ZIP parser skipped required output on empty success")
            return ZipResult(())
        if count.value > _MAX_ZIP_ENTRIES or names_length.value > _MAX_ZIP_NAME_POOL_BYTES:
            raise RuntimeError("native ZIP parser returned sizes beyond its declared bound")
        entries = (_ZipEntry * count.value)() if count.value else None
        name_pool = (ctypes.c_uint8 * names_length.value)() if names_length.value else None
        status = int(self._library.oa_zip_list(
            input_buffer, len(data), entries, count.value, ctypes.byref(count),
            name_pool, names_length.value, ctypes.byref(names_length), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        if count.value != (len(entries) if entries is not None else 0):
            raise RuntimeError("native ZIP parser changed its entry count between passes")
        pool = bytes(name_pool) if name_pool is not None else b""
        if names_length.value != len(pool):
            raise RuntimeError("native ZIP parser changed its name-pool length between passes")
        output: list[ZipEntry] = []
        for item in entries or ():
            start, end = item.name_offset, item.name_offset + item.name_length
            if end > len(pool) or bytes(item.reserved) != bytes(6):
                raise RuntimeError("native ZIP parser returned invalid metadata spans/padding")
            output.append(ZipEntry(
                name=pool[start:end].decode("utf-8"),
                compressed_size=int(item.compressed_size),
                uncompressed_size=int(item.uncompressed_size),
                local_offset=int(item.local_offset),
                crc32=int(item.crc32),
                flags=int(item.flags),
                method=int(item.method),
                zip64=bool(item.zip64),
                utf8_name_flag=bool(item.utf8_name_flag),
            ))
        return ZipResult(tuple(output))

    def extract_zip_entry(self, data: bytes, index: int) -> bytes:
        """Extract one ZIP member to memory after full size, stream, and CRC validation."""
        if not isinstance(data, bytes):
            raise TypeError("data must be bytes")
        if len(data) > _MAX_ZIP_INPUT_BYTES:
            raise ValueError("data exceeds the native ZIP input limit")
        if isinstance(index, bool) or not isinstance(index, int):
            raise TypeError("index must be an integer")
        if index < 0 or index >= _MAX_ZIP_ENTRIES:
            raise ValueError("index is outside the native ZIP entry limit")
        input_buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
        output_length, error = ctypes.c_uint64(), _ErrorInfo()
        status = int(self._library.oa_zip_extract_entry(
            input_buffer, len(data), index, None, 0, ctypes.byref(output_length), ctypes.byref(error)))
        if status == 0:
            if output_length.value != 0:
                raise RuntimeError("native ZIP extractor omitted a required output buffer")
            return b""
        if status != 6:
            _raise_if_error(self._library, status, error)
        if output_length.value > _MAX_ZIP_ENTRY_BYTES:
            raise RuntimeError("native ZIP extractor returned a size beyond its declared bound")
        output = (ctypes.c_uint8 * output_length.value)() if output_length.value else None
        status = int(self._library.oa_zip_extract_entry(
            input_buffer, len(data), index, output, output_length.value,
            ctypes.byref(output_length), ctypes.byref(error)))
        _raise_if_error(self._library, status, error)
        if output_length.value != (len(output) if output is not None else 0):
            raise RuntimeError("native ZIP extractor changed its output length between passes")
        return bytes(output) if output is not None else b""


def load(library_path: os.PathLike[str] | str) -> Geodesy:
    """Explicit-path loader; this package does not bundle or build the native library."""
    return Geodesy.load(library_path)


__all__ = [
    "Archives",
    "AsterixBindings",
    "AsterixError",
    "AsterixPosition",
    "AsterixRecord",
    "AsterixResult",
    "AsterixVertical",
    "CifpBindings",
    "CifpCounts",
    "CifpError",
    "CifpResult",
    "CifpRow",
    "CifpText",
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
    "Grib2Bindings",
    "Grib2Field",
    "Grib2Message",
    "InverseResult",
    "OpenAtcaError",
    "RhumbInverseResult",
    "RawInflateResult",
    "RawDeflateResult",
    "TarEntry",
    "TarResult",
    "ZipEntry",
    "ZipResult",
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

__all__.extend(_CIFP_PUBLIC_NAMES.values())
