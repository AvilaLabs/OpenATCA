"""Small stdlib reference readers for the versioned J2 evidence formats.

These readers use synthetic bytes only. They illustrate the public grammars and are not the
production acceptance path; the regional Debug payload is intentionally build-scoped evidence.
"""

from __future__ import annotations

import hashlib
import json
import re
import struct
import xml.etree.ElementTree as ET
from pathlib import Path


HERE = Path(__file__).resolve().parent
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
HEX40 = re.compile(r"[0-9a-f]{40}\Z")
MAX_OAG2ADPT_BYTES = 1024
MAX_CA_PRESET_BYTES = 65_536
MAX_PVER_CATALOG_BYTES = 16_384
MAX_SNAPSHOT_BYTES = 512 * 1024 * 1024
MAX_PORTABILITY_BYTES = 1_048_576
MAX_BUNDLE_BYTES = 2 * 1024 * 1024 * 1024
MAX_INVENTORY_ENTRIES = 32
REGIONAL_FRAME_FIELDS = (
    "sequence", "input", "surveillance", "flight_operations", "flights", "flight_report",
    "refused_legacy_route_events", "controller_views", "controller_dispatches",
    "controller_outcome", "controller_selection_kind", "controller_selected_address",
    "controller_selection", "console_text", "quarantine",
)


class FormatError(ValueError):
    """Synthetic reference reader rejected bytes outside the documented profile."""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_spec(filename: str) -> dict:
    with (HERE / filename).open(encoding="utf-8") as stream:
        return json.load(stream)


def _take(data: bytes, offset: int, width: int) -> tuple[bytes, int]:
    end = offset + width
    if end > len(data):
        raise FormatError("truncated field")
    return data[offset:end], end


def _integer(data: bytes, offset: int, width: int) -> tuple[int, int]:
    value, offset = _take(data, offset, width)
    return int.from_bytes(value, "little"), offset


def _valid_token(value: str, maximum: int) -> bool:
    try:
        raw = value.encode("ascii")
    except UnicodeEncodeError:
        return False
    if not raw or len(raw) > maximum:
        return False
    if any(not (48 <= byte <= 57 or 65 <= byte <= 90 or 97 <= byte <= 122 or byte in b"-_./") for byte in raw):
        return False
    return all(part not in ("", ".", "..") for part in value.split("/"))


def read_oag2adpt(
    manifest: bytes,
    snapshot: bytes,
    *,
    ca_preset: bytes | None = None,
    pver_catalog: bytes | None = None,
    expected_manifest_sha256: str | None = None,
) -> dict:
    """Read canonical OAG2ADPT v1-v4 bytes and bind the declared components."""
    if len(manifest) > MAX_OAG2ADPT_BYTES or len(manifest) < 58:
        raise FormatError("manifest byte length is outside 58..=1024")
    if manifest[:8] != b"OAG2ADPT":
        raise FormatError("bad magic")
    version, reader_min, reader_max = struct.unpack_from("<HHH", manifest, 8)
    if version not in (1, 2, 3, 4):
        raise FormatError("unsupported schema")
    release_len, facility_len, change_len, component_count = manifest[14:18]
    expected_counts = {1: (1,), 2: (1,), 3: (2,), 4: (2, 3)}
    if component_count not in expected_counts[version]:
        raise FormatError("component count does not match schema")
    snapshot_len = int.from_bytes(manifest[18:26], "little")
    snapshot_digest = manifest[26:58]
    if snapshot_len > MAX_SNAPSHOT_BYTES:
        raise FormatError("snapshot length exceeds its byte limit")
    offset = 58
    issue = None
    preset = None
    catalog = None
    if version >= 2:
        adaptation_version, offset = _integer(manifest, offset, 8)
        previous_digest, offset = _take(manifest, offset, 32)
        change_class, offset = _integer(manifest, offset, 1)
        issuer_len, offset = _integer(manifest, offset, 1)
        approver_len, offset = _integer(manifest, offset, 1)
        reason_len, offset = _integer(manifest, offset, 2)
        issue = (adaptation_version, previous_digest, change_class, issuer_len, approver_len, reason_len)
    if version == 3 or (version == 4 and component_count == 3):
        preset_len, offset = _integer(manifest, offset, 8)
        preset_digest, offset = _take(manifest, offset, 32)
        preset = (preset_len, preset_digest)
    if version == 4:
        catalog_len, offset = _integer(manifest, offset, 8)
        catalog_digest, offset = _take(manifest, offset, 32)
        catalog = (catalog_len, catalog_digest)
    lengths = [release_len, facility_len, change_len]
    if version >= 2:
        lengths.extend((issuer_len, approver_len, reason_len))
    names = ["release", "facility", "change_ref"]
    if version >= 2:
        names.extend(("issuer_principal_id", "approver_principal_id", "reason"))
    text_fields = {}
    for name, length in zip(names, lengths, strict=True):
        raw, offset = _take(manifest, offset, length)
        try:
            text_fields[name] = raw.decode("utf-8")
        except UnicodeDecodeError as error:
            raise FormatError(f"{name} is not UTF-8") from error
    if offset != len(manifest):
        raise FormatError("trailing bytes")
    if not (reader_min <= reader_max and reader_min <= 1 <= reader_max):
        raise FormatError("reader API 1 is outside the manifest range")
    if not _valid_token(text_fields["release"], 64):
        raise FormatError("invalid release token")
    if not _valid_token(text_fields["facility"], 64):
        raise FormatError("invalid facility token")
    if not _valid_token(text_fields["change_ref"], 128):
        raise FormatError("invalid change reference token")
    if snapshot_len != len(snapshot) or snapshot_digest != bytes.fromhex(sha256(snapshot)):
        raise FormatError("snapshot component binding mismatch")
    if expected_manifest_sha256 is not None and sha256(manifest) != expected_manifest_sha256:
        raise FormatError("separate manifest pin mismatch")
    issuer_bytes = approver_bytes = reason_bytes = b""
    if issue is not None:
        adaptation_version, previous_digest, change_class, issuer_len, approver_len, reason_len = issue
        issuer = text_fields["issuer_principal_id"]
        approver = text_fields["approver_principal_id"]
        reason = text_fields["reason"]
        try:
            issuer_bytes = issuer.encode("ascii")
            approver_bytes = approver.encode("ascii")
            reason_bytes = reason.encode("ascii")
        except UnicodeEncodeError as error:
            raise FormatError("issued principal IDs and reason must be ASCII") from error
        def principal_ok(item: str) -> bool:
            encoded = item.encode("ascii")
            return bool(encoded) and len(encoded) <= 64 and all(
                48 <= byte <= 57 or 65 <= byte <= 90 or 97 <= byte <= 122 or byte in b"-_."
                for byte in encoded
            )
        if adaptation_version == 0 or change_class not in (1, 2, 3):
            raise FormatError("invalid issued metadata")
        if (adaptation_version == 1) != (change_class == 1):
            raise FormatError("initial class/version mismatch")
        if not principal_ok(issuer) or not principal_ok(approver) or issuer == approver:
            raise FormatError("invalid issuer/approver IDs")
        if not reason_bytes or len(reason_bytes) > 256 or any(byte < 0x20 or byte > 0x7E for byte in reason_bytes):
            raise FormatError("invalid reason")
        if len(issuer_bytes) != issuer_len or len(approver_bytes) != approver_len or len(reason_bytes) != reason_len:
            raise FormatError("issued text length mismatch")
        text_fields["adaptation_version"] = adaptation_version
        text_fields["previous_manifest_sha256"] = previous_digest.hex()
        text_fields["change_class"] = change_class
    if preset is not None:
        preset_len, preset_digest = preset
        if preset_len == 0 or preset_len > MAX_CA_PRESET_BYTES:
            raise FormatError("CA preset length outside limit")
        if ca_preset is None or len(ca_preset) != preset_len or bytes.fromhex(sha256(ca_preset)) != preset_digest:
            raise FormatError("CA preset component binding mismatch")
        text_fields["ca_preset_len"] = preset_len
        text_fields["ca_preset_sha256"] = preset_digest.hex()
    elif ca_preset is not None:
        raise FormatError("unexpected CA preset component")
    if catalog is not None:
        catalog_len, catalog_digest = catalog
        if catalog_len == 0 or catalog_len > MAX_PVER_CATALOG_BYTES:
            raise FormatError("PVER catalog length outside limit")
        if pver_catalog is None or len(pver_catalog) != catalog_len or bytes.fromhex(sha256(pver_catalog)) != catalog_digest:
            raise FormatError("PVER catalog component binding mismatch")
        text_fields["pver_catalog_len"] = catalog_len
        text_fields["pver_catalog_sha256"] = catalog_digest.hex()
    elif pver_catalog is not None:
        raise FormatError("unexpected PVER catalog component")
    release_bytes = text_fields["release"].encode("utf-8")
    facility_bytes = text_fields["facility"].encode("utf-8")
    change_bytes = text_fields["change_ref"].encode("utf-8")
    canonical = bytearray(b"OAG2ADPT")
    canonical.extend(struct.pack("<HHH", version, reader_min, reader_max))
    canonical.extend(bytes((len(release_bytes), len(facility_bytes), len(change_bytes), component_count)))
    canonical.extend(struct.pack("<Q", snapshot_len))
    canonical.extend(snapshot_digest)
    if issue is not None:
        canonical.extend(struct.pack("<Q", adaptation_version))
        canonical.extend(previous_digest)
        canonical.extend(bytes((change_class, issuer_len, approver_len)))
        canonical.extend(struct.pack("<H", reason_len))
    if preset is not None:
        canonical.extend(struct.pack("<Q", preset[0]))
        canonical.extend(preset[1])
    if catalog is not None:
        canonical.extend(struct.pack("<Q", catalog[0]))
        canonical.extend(catalog[1])
    canonical.extend(release_bytes + facility_bytes + change_bytes)
    if issue is not None:
        canonical.extend(issuer_bytes + approver_bytes + reason_bytes)
    if bytes(canonical) != manifest:
        raise FormatError("manifest is not the canonical byte encoding")
    return {
        "schema_version": version,
        "reader_min": reader_min,
        "reader_max": reader_max,
        "component_count": component_count,
        "snapshot_len": snapshot_len,
        "snapshot_sha256": snapshot_digest.hex(),
        **text_fields,
    }


def read_oag2adpt_package(files: dict[str, bytes], *, expected_manifest_sha256: str) -> dict:
    """Read a synthetic package inventory with exact version-specific component names."""
    allowed = {"manifest.oag2", "airspace.snapshot", "ca-preset.oaca", "pver-catalog.oapv"}
    if not isinstance(files, dict) or not set(files) <= allowed or not {"manifest.oag2", "airspace.snapshot"} <= set(files):
        raise FormatError("package component inventory has missing or unexpected names")
    if any(not isinstance(value, bytes) for value in files.values()):
        raise FormatError("package components must be bytes")
    maximum_package_bytes = MAX_SNAPSHOT_BYTES + MAX_OAG2ADPT_BYTES + MAX_CA_PRESET_BYTES + MAX_PVER_CATALOG_BYTES
    if sum(map(len, files.values())) > maximum_package_bytes:
        raise FormatError("package byte limit exceeded")
    manifest = files["manifest.oag2"]
    if len(manifest) < 18:
        raise FormatError("manifest truncated before component inventory can be determined")
    version = int.from_bytes(manifest[8:10], "little")
    component_count = manifest[17]
    expected_names = {"manifest.oag2", "airspace.snapshot"}
    if version == 3 or (version == 4 and component_count == 3):
        expected_names.add("ca-preset.oaca")
    if version == 4:
        expected_names.add("pver-catalog.oapv")
    if set(files) != expected_names:
        raise FormatError("package file inventory does not match manifest version")
    decoded = read_oag2adpt(
        manifest,
        files["airspace.snapshot"],
        ca_preset=files.get("ca-preset.oaca"),
        pver_catalog=files.get("pver-catalog.oapv"),
        expected_manifest_sha256=expected_manifest_sha256,
    )
    return decoded


def read_tsv(data: bytes, header: str, *, max_bytes: int, max_rows: int) -> list[list[str]]:
    if len(data) > max_bytes or not data.endswith(b"\n") or b"\r" in data:
        raise FormatError("TSV byte/line-ending constraint failed")
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise FormatError("TSV is not UTF-8") from error
    lines = text[:-1].split("\n")
    if not lines or lines[0] != header or len(lines) > max_rows:
        raise FormatError("TSV header or row count failed")
    return [line.split("\t") for line in lines[1:] if line]


def read_settings(data: bytes) -> dict[str, str]:
    rows = read_tsv(data, "OA_REGIONAL_SETTINGS\t1", max_bytes=65536, max_rows=4096)
    fields = {}
    for row in rows:
        if len(row) != 2 or not row[0] or row[0] in fields:
            raise FormatError("duplicate or malformed setting")
        fields[row[0]] = row[1]
    return fields


def read_regional_profile(data: bytes) -> tuple[str, str | None]:
    """Read the replay profile using the capacity tuple written in the public spec."""
    fields = read_settings(data)
    profile = fields.get("profile")
    if profile is None:
        return "DEFAULT_V1", None
    spec = load_spec("oa-regional-j2-formats-v1.json")["regional_settings"]["recorded_profile"]
    if profile != spec["reader_profile_id"]:
        raise FormatError("unsupported profile")
    if any(fields.get(key) != value for key, value in spec["capacity_tuple"].items()):
        raise FormatError("recorded profile capacity tuple mismatch")
    return profile, "recorded-peak-rendered"


def read_regional_manifest(data: bytes) -> tuple[dict[str, str], list[dict[str, str]]]:
    rows = read_tsv(data, "OA_REGIONAL_MANIFEST\t1", max_bytes=64 * 1024 * 1024, max_rows=100000)
    known = {"input_sha256", "snapshot_sha256", "executable_sha256", "settings_sha256",
             "msaw_data_sha256", "performance_sha256"}
    fields: dict[str, str] = {}
    frames = []
    for row in rows:
        if row[0] == "frame":
            if len(row) != 6 or not row[1].isdecimal() or not row[2].isdecimal():
                raise FormatError("malformed frame manifest row")
            seq = int(row[1])
            if row[3] != f"frame-{seq:04}.txt" or not HEX64.fullmatch(row[4]) or not HEX64.fullmatch(row[5]):
                raise FormatError("frame row filename or digest mismatch")
            frames.append({"sequence": row[1], "receipt_ns": row[2], "evidence": row[3],
                           "evidence_sha256": row[4], "ppm_sha256": row[5]})
        elif len(row) == 2 and row[0] in known and row[0] not in fields and HEX64.fullmatch(row[1]):
            fields[row[0]] = row[1]
        else:
            raise FormatError("unknown, duplicate, or malformed manifest row")
    required = {"input_sha256", "snapshot_sha256", "executable_sha256", "settings_sha256"}
    if not required <= fields.keys() or not frames:
        raise FormatError("manifest missing required digest or frame")
    return fields, frames


def read_operated_manifest(data: bytes) -> tuple[dict[str, str], list[dict[str, str]]]:
    """Read the operated-session manifest shape using synthetic bytes only."""
    rows = read_tsv(data, "OA_OPERATED_MANIFEST\t1", max_bytes=64 * 1024 * 1024, max_rows=100000)
    singleton_order = ("mode", "input_sha256", "model_sha256", "executable_sha256",
                       "settings", "operator_journal", "script_sha256")
    fields: dict[str, str] = {}
    frames = []
    frame_rows_started = False
    for row in rows:
        if row[0] == "frame":
            frame_rows_started = True
            if (len(row) != 7 or not row[1].isdecimal() or not row[2].isdecimal()
                    or not HEX64.fullmatch(row[4]) or not HEX64.fullmatch(row[6])):
                raise FormatError("malformed operated frame row")
            sequence = int(row[1])
            if row[3] != f"frame-{sequence:04}.txt" or row[5] != f"frame-{sequence:04}.ppm":
                raise FormatError("operated frame filenames do not match sequence")
            frames.append({"sequence": row[1], "receipt_ns": row[2], "evidence_filename": row[3],
                           "evidence_sha256": row[4], "ppm_filename": row[5], "ppm_sha256": row[6]})
            continue
        if frame_rows_started:
            raise FormatError("operated singleton row follows frame rows")
        if row[0] == "mode":
            if len(row) != 2 or row[1] != "RECORDED_REPLAY" or row[0] in fields:
                raise FormatError("invalid operated mode row")
            fields[row[0]] = row[1]
        elif row[0] in ("settings", "operator_journal"):
            expected_name = "settings.tsv" if row[0] == "settings" else "operator.tsv"
            if len(row) != 3 or row[1] != expected_name or row[0] in fields or not HEX64.fullmatch(row[2]):
                raise FormatError("invalid operated file digest row")
            fields[row[0]] = f"{row[1]}:{row[2]}"
        elif row[0] in ("input_sha256", "model_sha256", "executable_sha256", "script_sha256"):
            if len(row) != 2 or row[0] in fields or not HEX64.fullmatch(row[1]):
                raise FormatError("invalid operated digest row")
            fields[row[0]] = row[1]
        else:
            raise FormatError("unknown operated manifest row")
    if tuple(fields) != singleton_order or not frames:
        raise FormatError("operated manifest rows are incomplete or out of writer order")
    return fields, frames


def read_recording_manifest(data: bytes) -> tuple[dict[str, str], list[dict[str, str]]]:
    rows = read_tsv(data, "OA_RECORDING_IMPORT_MANIFEST\t1", max_bytes=1024 * 1024, max_rows=128)
    field_names = (
        "centre", "normalized_bytes", "normalized_sha256", "provenance_bytes", "provenance_sha256",
        "quarantine_bytes", "quarantine_sha256", "report_bytes", "report_sha256", "executable_sha256",
        "physical_input_cap_bytes", "expanded_input_cap_bytes", "frame_cap", "stream_cap",
        "normalized_input_cap_bytes", "normalized_event_cap", "events_per_receipt_cap", "runtime_group_cap",
        "runtime_gap_cap_ns", "quarantine_record_cap", "quarantine_bytes_cap",
    )
    fields = {}
    streams = []
    for row in rows:
        if row[0] == "input_stream":
            if len(row) != 7 or not row[1].isdecimal() or row[2] not in ("ADSB", "SFDPS"):
                raise FormatError("malformed input stream row")
            if not HEX64.fullmatch(row[3]) or not row[4].isdecimal() or not row[5].isdecimal() or row[6] not in ("0", "1"):
                raise FormatError("invalid stream metadata")
            streams.append({"stream_index": row[1], "source": row[2], "sha256": row[3],
                            "compressed_bytes": row[4], "decoded_bytes": row[5], "gzip": row[6]})
            continue
        if len(row) != 2 or row[0] not in field_names or row[0] in fields:
            raise FormatError("unknown, duplicate, or malformed recording metadata")
        fields[row[0]] = row[1]
    if tuple(fields) != field_names or not streams:
        raise FormatError("recording metadata incomplete or out of writer order")
    if fields["centre"] != "ZAU1":
        raise FormatError("unsupported centre")
    for name, value in fields.items():
        if name == "centre":
            continue
        if name.endswith("sha256"):
            if not HEX64.fullmatch(value):
                raise FormatError(f"invalid digest in {name}")
        elif not value.isdecimal():
            raise FormatError(f"invalid decimal in {name}")
    if len({stream["stream_index"] for stream in streams}) != len(streams):
        raise FormatError("duplicate stream index")
    return fields, streams


def read_portability(manifest: bytes, complete: bytes, files: dict[str, bytes]) -> dict:
    if len(manifest) > MAX_PORTABILITY_BYTES or complete != (sha256(manifest) + "\n").encode("ascii"):
        raise FormatError("portability manifest size or COMPLETE mismatch")
    try:
        parsed = json.loads(manifest)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise FormatError("invalid portability JSON") from error
    if parsed.get("schema") != "openatca-j2-regional-portability-v1":
        raise FormatError("unsupported portability schema")
    if not HEX40.fullmatch(parsed.get("source_commit", "")):
        raise FormatError("invalid source commit")
    if not HEX64.fullmatch(parsed.get("recording_index_sha256", "")) or not HEX64.fullmatch(parsed.get("baseline_manifest_sha256", "")):
        raise FormatError("invalid bundle manifest digest")
    inventory = parsed.get("files")
    if not isinstance(inventory, dict) or not inventory or len(inventory) > MAX_INVENTORY_ENTRIES:
        raise FormatError("invalid inventory size")
    if set(files) != set(inventory):
        raise FormatError("file set differs from inventory")
    total = 0
    for name, record in inventory.items():
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or not isinstance(record, dict):
            raise FormatError("unsafe path or inventory entry")
        payload = files[name]
        if isinstance(record.get("bytes"), bool) or not isinstance(record.get("bytes"), int):
            raise FormatError("invalid inventory byte count")
        if record["bytes"] != len(payload) or record.get("sha256") != sha256(payload):
            raise FormatError("inventory file digest or length mismatch")
        total += len(payload)
    if total > MAX_BUNDLE_BYTES:
        raise FormatError("bundle byte limit exceeded")
    return parsed


def _split_debug_fields(body: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    start = 0
    depth = 0
    quote = None
    escaped = False
    parts = []
    for index, char in enumerate(body):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in ('"', "'"):
            quote = char
        elif char in "[{(":
            depth += 1
        elif char in "]})":
            depth -= 1
            if depth < 0:
                raise FormatError("unbalanced Debug value")
        elif char == "," and depth == 0:
            parts.append(body[start:index].strip())
            start = index + 1
    tail = body[start:].strip()
    if tail:
        parts.append(tail)
    if depth != 0 or quote is not None:
        raise FormatError("unclosed Debug value")
    for part in parts:
        if ":" not in part:
            raise FormatError("malformed Debug field")
        name, value = part.split(":", 1)
        name = name.strip()
        if not name or name in fields:
            raise FormatError("empty or duplicate Debug field")
        fields[name] = value.strip()
    return fields


def read_regional_frame(data: bytes, *, operated: bool = False) -> dict:
    if len(data) > 512 * 1024 or not data.endswith(b"\n"):
        raise FormatError("frame evidence size or newline invalid")
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise FormatError("frame evidence is not UTF-8") from error
    lines = text.splitlines(keepends=True)
    if operated:
        if len(lines) < 4 or lines[0] != "OA_REGIONAL_OPERATED_FRAME\t1\n":
            raise FormatError("operated frame tag missing")
        expected = ("pixels_ppm_sha256", "rendered_frame_sha256")
        hashes = {}
        for line, key in zip(lines[1:3], expected, strict=True):
            parts = line.removesuffix("\n").split("\t")
            if len(parts) != 2 or parts[0] != key or not HEX64.fullmatch(parts[1]):
                raise FormatError("operated frame hash row malformed")
            hashes[key] = parts[1]
        payload = "".join(lines[3:])
    else:
        if not lines or lines[0] != "OA_REGIONAL_FRAME\t1\n":
            raise FormatError("frame tag missing")
        hashes = {}
        payload = "".join(lines[1:])
    open_at = payload.find(" {\n")
    if open_at <= 0 or not payload.endswith("}\n"):
        raise FormatError("pretty Debug outer struct malformed")
    type_name = payload[:open_at]
    if type_name != "RegionalFrame":
        raise FormatError("unexpected frame Debug struct")
    body = payload[open_at + 3:-2]
    fields = _split_debug_fields(body)
    if tuple(fields) != REGIONAL_FRAME_FIELDS:
        raise FormatError("RegionalFrame field set/order differs from the documented producer")
    return {"tag": "OA_REGIONAL_OPERATED_FRAME" if operated else "OA_REGIONAL_FRAME",
            "hashes": hashes, "type": type_name, "fields": fields}


def read_iwxxm_envelope(document: bytes) -> dict:
    """Inspect only the documented XML root/child envelope, not full IWXXM semantics."""
    spec = load_spec("iwxxm-2025-2-metar-speci-profile.json")
    if len(document) > spec["limits"]["document_bytes"]:
        raise FormatError("IWXXM document exceeds profile byte limit")
    try:
        root = ET.fromstring(document)
    except ET.ParseError as error:
        raise FormatError("invalid XML") from error
    namespace = spec["namespaces"]["iwxxm"]
    gml = spec["namespaces"]["gml"]
    if root.tag not in {f"{{{namespace}}}{kind}" for kind in ("METAR", "SPECI", "TAF")}:
        raise FormatError("unsupported IWXXM root")
    kind = root.tag.rsplit("}", 1)[-1]
    if any(not child.tag.startswith(f"{{{namespace}}}") for child in root):
        raise FormatError("non-IWXXM child refused")
    attr_names = set(root.attrib)
    gml_id_name = f"{{{gml}}}id"
    if gml_id_name not in attr_names:
        raise FormatError("GML id required")
    if kind in ("METAR", "SPECI"):
        allowed = {gml_id_name, "reportStatus", "permissibleUsage", "automatedStation"}
        if not {"reportStatus", "permissibleUsage"} <= attr_names:
            raise FormatError("required METAR/SPECI root attribute missing")
        if "translationFailedTAC" in attr_names:
            raise FormatError("translationFailedTAC refused")
        if attr_names - allowed:
            raise FormatError("METAR/SPECI root attributes outside profile")
        if root.attrib.get("reportStatus") not in ("NORMAL", "AMENDMENT", "CORRECTION"):
            raise FormatError("invalid reportStatus")
        if root.attrib.get("permissibleUsage") not in ("OPERATIONAL", "NON-OPERATIONAL"):
            raise FormatError("invalid permissibleUsage")
        if root.attrib.get("automatedStation") not in (None, "true", "false", "1", "0"):
            raise FormatError("invalid automatedStation")
    else:
        if "translationFailedTAC" in attr_names:
            raise FormatError("translationFailedTAC refused")
        allowed = {gml_id_name, "reportStatus", "permissibleUsage", "isCancelReport"}
        if not {"reportStatus", "permissibleUsage"} <= attr_names:
            raise FormatError("required TAF root attribute missing")
        if attr_names - allowed:
            raise FormatError("TAF root attributes outside profile")
        if root.attrib.get("isCancelReport") not in (None, "false", "0"):
            raise FormatError("cancelled TAF refused")
    names = [child.tag.rsplit("}", 1)[-1] for child in root]
    if kind in ("METAR", "SPECI"):
        expected = ["issueTime", "aerodrome", "observationTime"]
        if names[:3] != expected or names[3:] and names[3] not in ("observation", "trendForecast"):
            raise FormatError("METAR/SPECI root child order invalid")
        if names[3:] and names[3] == "observation":
            names = names[:3] + names[4:]
        if names[3:] and (names[3] != "trendForecast" or len(names[3:]) > 2 or any(n != "trendForecast" for n in names[3:])):
            raise FormatError("METAR/SPECI trend sequence invalid")
    else:
        expected = ["issueTime", "aerodrome", "validPeriod", "baseForecast"]
        if names[:4] != expected or any(name != "changeForecast" for name in names[4:]) or len(names[4:]) > 5:
            raise FormatError("TAF root child order invalid")
    return {"kind": kind, "gml_id": root.attrib[f"{{{gml}}}id"], "children": names}
