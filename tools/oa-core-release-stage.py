#!/usr/bin/env python3
"""Create a private source-and-test staging tree for a reviewed CORE 0.1 package selection.

This tool copies no build outputs, recordings, or datasets. It never publishes. The generated
manifests are package-name/dependency transformations of one clean, committed workspace revision;
tracked source, tests, approved fixtures, and API pages are byte-for-byte copies. Any unsupported
dependency or source layout stops staging instead of dropping that requirement.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib


ROOT_CANDIDATES = (
    "oa-ieee", "oa-sha256", "oa-tar", "oa-xml", "oa-json", "oa-csv",
    "oa-math", "oa-geo", "oa-deflate", "oa-zip", "oa-asterix", "oa-arinc424",
    "oa-nasr", "oa-notam", "oa-fixm", "oa-grib2",
)
API_DOCS = {
    "oa-ieee": "docs/core-api/foundations/math-ieee.md",
    "oa-math": "docs/core-api/foundations/math-ieee.md",
    "oa-geo": "docs/core-api/foundations/geo.md",
    "oa-time-window": "docs/core-api/foundations/time-window.md",
    "oa-sha256": "docs/core-api/foundations/integrity.md",
    "oa-deflate": "docs/core-api/foundations/containers.md",
    "oa-zip": "docs/core-api/foundations/containers.md",
    "oa-tar": "docs/core-api/foundations/containers.md",
    "oa-xml": "docs/core-api/foundations/text.md",
    "oa-json": "docs/core-api/foundations/text.md",
    "oa-csv": "docs/core-api/foundations/text.md",
    "oa-asterix": "docs/core-api/formats/asterix.md",
    "oa-arinc424": "docs/core-api/formats/arinc424.md",
    "oa-nasr": "docs/core-api/formats/nasr.md",
    "oa-notam": "docs/core-api/formats/aixm-notam.md",
    "oa-fixm": "docs/core-api/formats/fixm-sfdps.md",
    "oa-grib2": "docs/core-api/formats/grib2-winds.md",
    "oa-j2k": "docs/core-api/formats/grib2-winds.md",
    "oa-airspace": "docs/core-api/modules/airspace.md",
    "oa-adsb": "crates/oa-adsb/README.md",
}
PUBLIC_PREFIX = "openatca-"
BLOCKED_PACKAGES = {"oa-sealed"}
MAX_SOURCE_FILE = 32 * 1024 * 1024
MAX_SOURCE_TOTAL = 512 * 1024 * 1024
TEST_ASSETS = {
    "oa-sha256": {
        "tests/fixtures/SHA256LongMsg.rsp": "6fac36f37360bcf74ffcf4465c18e30d6d5a04cc90885b901fc3130c16060974",
        "tests/fixtures/SHA256Monte.rsp": "29ea30c6bb4b84e425fb8c1d731c6bb852dac935825f2bd1143e5d3c4f10bfb9",
        "tests/fixtures/SHA256ShortMsg.rsp": "75e1cb83994638481808e225b9eb0c1ebd0c232d952ac42b61abce6363be283c",
    },
    "oa-math": {
        "tests/data/acos.bin": "b0f336e00801637fd6093e2e9eb3835b835d269d758d6f29bd366c363be940a3",
        "tests/data/asin.bin": "275d5cdf9b5cf9b93dceb0213f1e78fa9336ef73a12b748c108b399717bd125d",
        "tests/data/atan.bin": "d9557f2484ecd6bfe85d0540767dac6dd23c4f350ba50335b85133afb787d5c4",
        "tests/data/atan2.bin": "68290cf27e1fcbac02cc46d69ac1e0f35d43a76d4812dca652a7e5aea420540f",
        "tests/data/cbrt.bin": "587a64c17e9b407fa0f1ebb5fe2c3ab8ed34911b1fdcdb3d03b8e9a9a4266cc1",
        "tests/data/cos.bin": "e41258a11a52c85a0abb3ee762a08984b958f34bbcadc5b9c8e46be3c93311ee",
        "tests/data/cw_hard.bin": "f46971dd4a4fd126d30e1ce979f34917a5ffb9b3ec741b943e27db66d8272a20",
        "tests/data/exp.bin": "39ab38fb5d1eadaf90fa20aff70835a56894c4067d8346b4391ec4aecd0651e3",
        "tests/data/hypot.bin": "50e0acd4ed8b702536012ad0f5f8100ce4f47740ec2890c09ac5a3b5455cf9d0",
        "tests/data/ln.bin": "454237f403bf7dbdd6786552b893d3d369256b51ec4cf5a0a1a6ddc697583c96",
        "tests/data/pow.bin": "5b870b5b8cc510c695bebd1e713a492b02bb0ae79b939c484682ca3422b10f0d",
        "tests/data/sin.bin": "fd572e03009412eb331934642096bf3faca815fd04057b83312529deadf6270b",
        "tests/data/tan.bin": "af3c7604aa66ee4fc58ef3fbbbcb604df6c6f8c6c0378755afd6ec8910479bd6",
    },
    "oa-geo": {
        "tests/fixtures/geodtest_sample.dat": "c4d444b6e92242c73de67d8f275ad10c0639cb75d5136ef171251b35eb1f16c2",
        "tests/fixtures/zau_low.csv": "aedda625b858f159ecdb4d65952d742198de5dd6865e3cb40a8c8c64f5a8617f",
    },
    'oa-arinc424': {'tests/fixtures/airports.txt': 'ffbfb10c660617481f3c814647870a6b0e792823e1599f726b7cd8eb8238df9c',
     'tests/fixtures/airways.txt': '7b8234df22015f6e3ce57b3c6d0af09476a0032478b697a2fd87b86bc96a84c6',
     'tests/fixtures/approach.txt': 'df0d8bcf12991a6fdf25ec1083b2723d2ac16a9a72a73697cfd33f9ee159c59d',
     'tests/fixtures/enroute_waypoints.txt': '9ca38fcd0ffba90a8db76e6c6f1503631609b686e4885e08acd38d365315195a',
     'tests/fixtures/header.txt': '8cce598ca90de3e1f51c8142eb154e36e56c3c23bfca11024a755e7d46ac702a',
     'tests/fixtures/localizers.txt': '224a610336da9bd257a894b1ac258983207211a56783913796474d63546b81f9',
     'tests/fixtures/mora.txt': 'bc2be02bbed0c555b8c74fe6ddf2bf9393b2b3b651ff03a1052a6f8a3262bb9e',
     'tests/fixtures/ndb.txt': '8f8aec32ab2937b19d07f75356e7e8489b04b0cc78e8e8ddacd86b37bb594491',
     'tests/fixtures/runways.txt': '50f4b3ef46626ed3b48b4a4e77ff26730629a8263b8604beeb8ec57f49c51b85',
     'tests/fixtures/sid.txt': 'c02d2387b3ceec11ff9faa6b57f0b7464febe26cca54f58b8949287b2f791bbe',
     'tests/fixtures/star.txt': 'ce8d64952a05aaae630ba1420d24fd09c69d8e7ab87395719e3b0882ce5a4130',
     'tests/fixtures/terminal_ndb.txt': '9ae603a579bca6d8271d115e798200d5c6d5fd7fdbc509e8f3ae4a89d8a70875',
     'tests/fixtures/terminal_waypoints.txt': 'b22d808249b6a3165c0671f405d0ff6da57afb8fcaa0e4d055fb30b73c147d86',
     'tests/fixtures/uc.txt': 'ec3d17523c038876ca0dced2fbc98bd2410cd88acc4eadf1e055e9507a10eb7d',
     'tests/fixtures/ur.txt': '7728b243aefa835e49af6078f5bf7a4b6c875312510afb5073a72f85393abd27',
     'tests/fixtures/vhf.txt': 'f91b2f712e36f2ee259ba0d1608912f77c938fb1686b7c2ea64baf82777ff42c'},
    'oa-nasr': {'tests/fixtures/APT_BASE.csv': '7c51791b1554080865a0cab21dc3f1829e641c2e75d751e3aebf788f69c78a80',
     'tests/fixtures/APT_CSV_DATA_STRUCTURE.csv': 'c026f1163bfcdd7c8136943139d74d9b1129d5e803905055b090617c243fe130',
     'tests/fixtures/APT_RWY.csv': 'f0b38a533a8bed36af7241d528ed2792cc33c4999424c7e7a22cf8f5316355a5',
     'tests/fixtures/APT_RWY_END.csv': '736795f53d791f3a4c6c9fb03c2f9e97b18da81eb79e791a934b141db0dae277',
     'tests/fixtures/ARB_BASE.csv': 'b66f7a83ef20ebbfa8d3b7dc7076031c74c674e29694b5eba1957acfab807516',
     'tests/fixtures/ARB_CSV_DATA_STRUCTURE.csv': 'b17148293ced74fccf95ea0bc1de14d5c76e16a20a294538aa9eaf9857fc43cd',
     'tests/fixtures/ARB_SEG.csv': '7441eb6a082ddda781d212c540bd60a8413df52894bfae4e0d5ee2644eac11bb',
     'tests/fixtures/AWY_BASE.csv': 'a527477588e58e5170d77a60e927b7b98b2f7f5d1d9603ed82280555e722c202',
     'tests/fixtures/AWY_CSV_DATA_STRUCTURE.csv': '42de02dca2833446d4783f901af76058d406788081bbcf7fd2f0f684703bae9f',
     'tests/fixtures/AWY_SEG_ALT.csv': '0b9c341af071fed1d93baaeca500d27580645cd4d01e45a1d350bf279b8c6571',
     'tests/fixtures/CLS_ARSP.csv': '608247ce0236006df2dcaad306151d301997f02b0343167f52d80963fe30c761',
     'tests/fixtures/CLS_ARSP_CSV_DATA_STRUCTURE.csv': 'ebaa9e3e8c665a8b10a531935eab4cbbc866503234dc2a6c00a249e8733aedd1',
     'tests/fixtures/DP_APT.csv': 'a55027d1c9539c47766c9ef892ff5184deb014508fee4edfe0627d4d4769cb7a',
     'tests/fixtures/DP_BASE.csv': '551a29d2de7d64fb2a52b3f3659cb4644391cb21e6712807ba45b42579716019',
     'tests/fixtures/DP_CSV_DATA_STRUCTURE.csv': '185c2862ce3079f999f29cb0bb22172c5f8d46f860916ea682d9c00087e1573d',
     'tests/fixtures/DP_RTE.csv': 'bb2a47a789d6827a195bf65c8071c0c29504a8824591e1e234b9df6b376d1c48',
     'tests/fixtures/FIX_BASE.csv': '4d287466a9a960c9c3b7c1cdbcf439795138d4932c7bfe5a8487c7a5459bd0e9',
     'tests/fixtures/FIX_CSV_DATA_STRUCTURE.csv': 'b48134bb79f1786f60e26b39099e79eaf89eaec82d7ae53dc427295abe5017e8',
     'tests/fixtures/FIX_NAV.csv': 'd9bbc347e69ec9c3a60c19debea8302ebc70921d9b93ad131df40fd3d521e3ae',
     'tests/fixtures/NAV_BASE.csv': 'b72e59e46e326dd5d347886910816bb8d96f84466e47ba6b729b8ed93ba175c3',
     'tests/fixtures/NAV_CSV_DATA_STRUCTURE.csv': '31a4abdca6e59b74965b3d1a85a3cf4a7b5df52864039d648e7a6deafabed2f7',
     'tests/fixtures/STAR_APT.csv': '8ecec4a92576b228f3321228b1b680a5249a869f54173faae30a6ce6a559fcea',
     'tests/fixtures/STAR_BASE.csv': '94f514e5d0de03e97b56573a7e44e15fb409487447a58f1752fb8fe111154c4e',
     'tests/fixtures/STAR_CSV_DATA_STRUCTURE.csv': '3c928d199a96538d9fb50146c448038c779134bf7e4467237243db874df98025',
     'tests/fixtures/STAR_RTE.csv': '1cdf1b99f004a66d2e9c8a887318cf55467b4c54773f2189e375a0c85d4bbef6'},
    'oa-zip': {'tests/fixtures/bomb.zip': '5df59d1513e6389763f6945bcb591ee98cb2c56af5ff29670a1b2d0ebe033deb',
     'tests/fixtures/deflate.zip': '8636f7907eebf3e284bb23fb661aa368edf7dd4288f7446341c038a0409888ce',
     'tests/fixtures/descriptor.zip': '4f728e692b6f59cc4fbf681fbfc486779999c0f4f5067196952aef08cbb4ecfe',
     'tests/fixtures/expected/a.txt': '91e9e42a1b3af80703c4b6e85a91b4c009854335283b613535107d65b99b3580',
     'tests/fixtures/expected/big64.txt': '2880ce1764916c2cc311a95af466f2f5143dafb75f6f20bea5a803c622e75be4',
     'tests/fixtures/expected/dynamic.txt': 'fa2f50d4e5c056499b9494b2da08e624175477e5f0178b9605f84890cb7b3289',
     'tests/fixtures/expected/empty.txt': 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
     'tests/fixtures/expected/far.bin': 'e070e9318a4427ef2d81197eea5195f36210bebf137ef0c6ac60e3e4752806fe',
     'tests/fixtures/expected/fixed.txt': '66dafcbddf1265a536c7cd4dc458b50de3c109cedc0896cdb77041616c98a003',
     'tests/fixtures/expected/mixed.bin': 'cf18c533d0d3dac76c26078e2a028bf91cf472740f9743bcff3cc09491e2a4f3',
     'tests/fixtures/expected/random.bin': '389abf870b29b7d8d1de0e2c455e265f1fab1484fb7acf94d437462b4d3932ad',
     'tests/fixtures/expected/run.bin': '6d1cf22d7cc09b085dfc25ee1a1f3ae0265804c607bc2074ad253bcc82fd81ee',
     'tests/fixtures/expected/storedblock.bin': '725c2e51e75ac19b4d6c17cd0fc3f8b150dc7ec98ddf3d34489092a53657c4bf',
     'tests/fixtures/expected/streamed.txt': 'b14a2a8362290ed5361372970cb735e04907bc6ac26d58d36ff357e8c9726403',
     'tests/fixtures/expected/streamed2.bin': '5da07a9c5f9fb026f57dc2e4d040e797e78eaa9927f66c72f2ddf77fba35ca6c',
     'tests/fixtures/make_fixtures.py': 'be91643c37958e5ae83d7927ba3e1e8f3460118a3b29eb84a1cdf16f5256f314',
     'tests/fixtures/many.zip': '9c2d8c2e654510866fcf90ea13340f9cfd010689e96009d502cb3ff552a9cc19',
     'tests/fixtures/stored.zip': '2aa1898e1c1da7ee31efb941e2948fdd82efae2e540932f8154c7ed2e672b824',
     'tests/fixtures/traversal.zip': 'b502a6fae10c45efb5742dad205454ecc20cbb97f84c25a6e50cc323d781d3d5',
     'tests/fixtures/zip64.zip': 'f324ca63ee782dbbab0907eef7b15a945a897bb0338295a6afbec144198b3ce5',
     'tests/fixtures/zip64_huge.zip': 'fe68088ef40725e209aeb043efee07e2dbfeff3b65ccf97b88326e317f822110'},
    'oa-grib2': {'tests/fixtures/points-flat-codestream.grib2': 'dded57ef8abf03496d2ad92f3f8463dbee6c97ca0818fe3f4e2ad13813e9b9a8',
     'tests/fixtures/points-without-data.grib2': 'e7a837a0edf24c5881cb13d7284f4009aa9ae268510038fe6caa3c974f743256',
     'tests/fixtures/width-overflow.grib2': 'e3fe590068864a8887465e4f042ba7aadd1d27d3f3e45ea23e36eb2bdaadaec5'},
    "oa-xml": {},
}
PACKAGE_TOOLS = {
    "oa-geo": {
        "tools/gen_coef.py": "63f6193529f1fe21e3f270d97028bc25bd50bc038003e7cb785cdd4c9d03d879",
    },
}
INPUT_PROVENANCE = {
    "oa-math": {
        "tests/data/*.bin": (
            "Generated mathematical reference vectors from the tracked oa-math reference generators "
            "(tools/oa-math-ref/gen_ref.py and gen_hard_cw.py); these are test inputs, not operational data."
        ),
    },
    "oa-geo": {
        "tests/fixtures/geodtest_sample.dat": (
            "The tracked geodesic test identifies this as a 31-line excerpt of GeographicLib's "
            "GeodTest set, DOI 10.5281/zenodo.32156, and describes the set as CC0. Confirm the "
            "record's Rights metadata before external publication; primary Zenodo metadata verified CC0 on 2026-11-26."
        ),
        "tests/fixtures/zau_low.csv": (
            "The tracked M0 design identifies committed test excerpts as public FAA NASR "
            "aeronautical data, and the polygon test identifies this as the ZAU LOW boundary. "
            "It is NASR boundary test data, not private SFDPS-derived sector data."
        ),
        "tools/gen_coef.py": (
            "Tracked exact-rational coefficient generator for oa-geo/src/coef.rs; copied from "
            "the selected source revision byte-for-byte."
        ),
    },
}
COMPILE_TIME_MACRO = re.compile(rb'\b(?:include_(?:bytes|str)|include)!\s*\(([^)]*)\)')
COMPILE_TIME_LITERAL = re.compile(rb'^\s*"([^\"]+)"\s*$')
PATH_ATTRIBUTE = re.compile(rb'#\[path\s*=\s*"([^\"]+)"\s*\]')


class StageError(Exception):
    """A source or package boundary that needs review before staging."""


def git(repo: Path, *args: str, binary: bool = False) -> bytes | str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], check=False, capture_output=True,
        text=not binary, encoding=None if binary else "utf-8",
    )
    if result.returncode != 0:
        error = result.stderr if isinstance(result.stderr, str) else result.stderr.decode("utf-8", "replace")
        raise StageError(f"git {' '.join(args)} failed: {error.strip()}")
    return result.stdout


def source_bytes(repo: Path, revision: str, relative: str) -> bytes:
    if relative.startswith("/") or ".." in PurePosixPath(relative).parts:
        raise StageError(f"unsafe repository path: {relative}")
    value = git(repo, "show", f"{revision}:{relative}", binary=True)
    if not isinstance(value, bytes):
        raise StageError(f"git did not return binary source for {relative}")
    if len(value) > MAX_SOURCE_FILE:
        raise StageError(f"source file exceeds {MAX_SOURCE_FILE} bytes: {relative}")
    return value


def public_name(internal_name: str) -> str:
    if not internal_name.startswith("oa-") or internal_name in BLOCKED_PACKAGES:
        raise StageError(f"no public package mapping for internal dependency {internal_name}")
    return PUBLIC_PREFIX + internal_name[3:]


def rust_crate_name(manifest: dict, internal_name: str) -> str:
    lib_target = manifest.get("lib")
    if lib_target is None:
        return internal_name.replace("-", "_")
    if not isinstance(lib_target, dict):
        raise StageError(f"unsupported library target declaration for {internal_name}")
    target_name = lib_target.get("name")
    if target_name is None:
        return internal_name.replace("-", "_")
    if not isinstance(target_name, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", target_name):
        raise StageError(f"unsupported Rust library import name for {internal_name}: {target_name!r}")
    return target_name


def dependency_tables(manifest: dict) -> list[tuple[str, str, object]]:
    found: list[tuple[str, str, object]] = []
    for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        section = manifest.get(section_name, {})
        if isinstance(section, dict):
            found.extend((section_name, alias, value) for alias, value in section.items())
    target = manifest.get("target", {})
    if isinstance(target, dict):
        for target_cfg in target.values():
            if not isinstance(target_cfg, dict):
                continue
            for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
                section = target_cfg.get(section_name, {})
                if isinstance(section, dict):
                    found.extend((f"target.{section_name}", alias, value) for alias, value in section.items())
    return found


def resolve_package(repo: Path, revision: str, relative: str, cache: dict[str, tuple[dict, bytes]]) -> tuple[dict, bytes]:
    manifest_path = f"{relative}/Cargo.toml"
    if manifest_path not in cache:
        raw = source_bytes(repo, revision, manifest_path)
        try:
            parsed = tomllib.loads(raw.decode("utf-8"))
        except (UnicodeError, tomllib.TOMLDecodeError) as exc:
            raise StageError(f"cannot parse {manifest_path}: {exc}") from exc
        cache[manifest_path] = (parsed, raw)
    return cache[manifest_path]


def dependency_graph(repo: Path, revision: str, root_candidates: tuple[str, ...]) -> tuple[
        dict[str, str], dict[str, set[str]], dict[str, tuple[dict, bytes]], list[str]]:
    cache: dict[str, tuple[dict, bytes]] = {}
    module_paths: dict[str, str] = {}
    graph: dict[str, set[str]] = {}
    blockers: list[str] = []
    if not root_candidates or len(set(root_candidates)) != len(root_candidates):
        raise StageError("package selection must be nonempty and contain no duplicates")
    unknown = sorted(set(root_candidates) - set(ROOT_CANDIDATES))
    if unknown:
        raise StageError(f"package selection contains unsupported roots: {', '.join(unknown)}")
    pending = list(root_candidates)
    while pending:
        module = pending.pop()
        if module in module_paths:
            continue
        if module in BLOCKED_PACKAGES:
            raise StageError(f"public closure contains prohibited internal package {module}")
        relative = f"crates/{module}"
        parsed, _ = resolve_package(repo, revision, relative, cache)
        package = parsed.get("package")
        if not isinstance(package, dict) or package.get("name") != module:
            raise StageError(f"package identity mismatch at {relative}/Cargo.toml")
        module_paths[module] = relative
        graph[module] = set()
        for section_name, alias, dep in dependency_tables(parsed):
            if not isinstance(dep, dict):
                blockers.append(f"{module}: dependency needs an explicit package table: {alias}")
                continue
            if dep.get("workspace") is True:
                blockers.append(f"{module}: workspace-inherited dependency needs explicit staging support: {alias}")
                continue
            path_value = dep.get("path")
            if path_value is None:
                blockers.append(f"{module}: unresolved or external dependency: {alias}")
                continue
            if not isinstance(path_value, str):
                blockers.append(f"{module}: unsupported dependency path: {alias}")
                continue
            resolved = PurePosixPath(relative).joinpath(path_value)
            normalized = PurePosixPath(os.path.normpath(str(resolved)))
            if normalized.is_absolute() or not normalized.parts or normalized.parts[0] != "crates":
                blockers.append(f"{module}: dependency escapes crate tree: {alias} -> {path_value}")
                continue
            target_relative = normalized.as_posix()
            try:
                target_manifest, _ = resolve_package(repo, revision, target_relative, cache)
            except StageError:
                blockers.append(f"{module}: dependency manifest missing: {alias} -> {target_relative}")
                continue
            target_package = target_manifest.get("package")
            target_name = target_package.get("name") if isinstance(target_package, dict) else None
            if not isinstance(target_name, str):
                blockers.append(f"{module}: dependency package name missing: {alias} -> {target_relative}")
                continue
            if target_name in BLOCKED_PACKAGES:
                blockers.append(f"{module}: prohibited private {section_name} dependency {alias} -> {target_name}")
                continue
            if not target_name.startswith("oa-"):
                blockers.append(f"{module}: unexpected non-OpenATCA path dependency: {alias} -> {target_name}")
                continue
            graph[module].add(target_name)
            if target_name not in module_paths:
                pending.append(target_name)
    return module_paths, graph, cache, blockers


def topological_order(graph: dict[str, set[str]]) -> list[str]:
    order: list[str] = []
    active: set[str] = set()
    done: set[str] = set()

    def visit(name: str) -> None:
        if name in done:
            return
        if name in active:
            raise StageError(f"cyclic package dependency involving {name}")
        active.add(name)
        for dependency in sorted(graph.get(name, ())):
            if dependency not in graph:
                raise StageError(f"dependency missing from generated closure: {name} -> {dependency}")
            visit(dependency)
        active.remove(name)
        done.add(name)
        order.append(name)

    for package in sorted(graph):
        visit(package)
    return order


def transform_manifest(raw: bytes, internal_name: str, dependency_paths: dict[str, str],
                       workspace_package: dict, workspace_lints: dict,
                       library_name: str) -> bytes:
    text = raw.decode("utf-8")
    replacements = {
        rf'(?m)^name\s*=\s*"{re.escape(internal_name)}"\s*$': f'name = "{public_name(internal_name)}"',
        r'(?m)^version\.workspace\s*=\s*true\s*$': 'version = "0.1.0"',
    }
    for pattern, replacement in replacements.items():
        text, count = re.subn(pattern, replacement, text, count=1)
        if count != 1:
            raise StageError(f"could not transform manifest identity for {internal_name}")
    inherited_fields = {
        "edition": workspace_package.get("edition"),
        "license": workspace_package.get("license"),
        "rust-version": workspace_package.get("rust-version"),
        "authors": workspace_package.get("authors"),
    }
    for field, value in inherited_fields.items():
        if value is None:
            raise StageError(f"workspace package metadata lacks {field}")
        encoded = json.dumps(value, separators=(",", ":"))
        text, count = re.subn(rf'(?m)^{re.escape(field)}\.workspace\s*=\s*true\s*$',
                              f"{field} = {encoded}", text, count=1)
        if count != 1:
            raise StageError(f"could not expand inherited {field} metadata for {internal_name}")
    if workspace_lints:
        text, lint_count = re.subn(r'(?m)^\[lints\]\s*\nworkspace\s*=\s*true\s*$',
                                   "[lints.rust]\nunsafe_code = \"forbid\"\n"
                                   "unexpected_cfgs = { level = \"warn\", check-cfg = [\"cfg(kani)\", \"cfg(kani_full_capacity)\", \"cfg(kani_late_replay_covers)\", \"cfg(creusot)\"] }\n\n"
                                   "[lints.clippy]\nunwrap_used = \"deny\"\nexpect_used = \"deny\"\npanic = \"deny\"\nindexing_slicing = \"deny\"",
                                   text, count=1)
        if "[lints]\nworkspace = true" in text or lint_count not in (0, 1):
            raise StageError(f"could not expand inherited lints for {internal_name}")
    text, readme_count = re.subn(r'(?m)^readme\s*=.*$', 'readme = "README.md"', text, count=1)
    if readme_count == 0:
        marker = "[package]\n"
        if marker not in text:
            raise StageError(f"cannot add package README field for {internal_name}")
        text = text.replace(marker, marker + 'readme = "README.md"\n', 1)
    exact_assets = [*TEST_ASSETS.get(internal_name, {}), *PACKAGE_TOOLS.get(internal_name, {})]
    include_paths = ["Cargo.toml", "src/*.rs", "src/**/*.rs", "build.rs", "examples/**/*.rs",
                     "tests/*.rs", "tests/**/*.rs", "README.md", "API.md", "LICENSE"]
    include_paths.extend(exact_assets)
    include_value = "include = " + json.dumps(include_paths, separators=(",", ":"))
    if re.search(r'(?m)^include\s*=', text):
        text, count = re.subn(r'(?m)^include\s*=.*$', include_value, text, count=1)
        if count != 1:
            raise StageError(f"cannot scope package contents for {internal_name}")
    else:
        text, count = re.subn(r'(?m)^readme\s*=\s*"README\.md"\s*$',
                              'readme = "README.md"\n' + include_value, text, count=1)
        if count != 1:
            raise StageError(f"cannot scope package contents for {internal_name}")

    path_pattern = re.compile(r'(?P<left>\bpath\s*=\s*)"(?P<path>[^"]+)"')

    def replace_path(match: re.Match[str]) -> str:
        old_path = match.group("path")
        target = dependency_paths.get(old_path)
        if target is None:
            raise StageError(f"manifest dependency path is absent from parsed graph: {internal_name}:{old_path}")
        target_name = Path(target).name
        return (f'{match.group("left")}"../{target_name}", '
                f'package = "{target_name}", version = "0.1.0"')

    path_count = len(re.findall(r'\bpath\s*=', text))
    text, converted = path_pattern.subn(replace_path, text)
    if path_count != converted:
        raise StageError(f"unsupported path dependency form in {internal_name}: {path_count} path(s), {converted} converted")
    lib_header = re.search(r'(?m)^\[lib\]\s*$', text)
    if lib_header is None:
        text = text.rstrip() + f'\n\n[lib]\nname = {json.dumps(library_name)}\n'
    else:
        section_start = lib_header.end()
        next_section = re.search(r'(?m)^\[', text[section_start:])
        section_end = section_start + next_section.start() if next_section else len(text)
        lib_body = text[section_start:section_end]
        name_match = re.search(r'(?m)^name\s*=\s*"[^"]*"\s*$', lib_body)
        if name_match is None:
            text = text[:section_start] + f'\nname = {json.dumps(library_name)}\n' + text[section_start:]
        else:
            text = (text[:section_start + name_match.start()]
                    + f'name = {json.dumps(library_name)}'
                    + text[section_start + name_match.end():])
    return (text.rstrip() + "\n").encode("utf-8")


def git_files(repo: Path, revision: str, crate_relative: str) -> tuple[list[tuple[str, str]], list[str]]:
    raw = git(repo, "ls-tree", "-r", "-z", "--full-tree", revision, "--", crate_relative, binary=True)
    if not isinstance(raw, bytes):
        raise StageError(f"git did not return binary tree metadata for {crate_relative}")
    records: list[tuple[str, str]] = []
    omitted: list[str] = []
    for entry in raw.split(b"\0"):
        if not entry:
            continue
        metadata, path_bytes = entry.split(b"\t", 1)
        mode, kind, _object_id = metadata.decode("ascii").split(" ")
        path = path_bytes.decode("utf-8")
        if kind != "blob" or mode not in {"100644", "100755"}:
            raise StageError(f"non-regular package input is not supported: {path}")
        if path == f"{crate_relative}/Cargo.toml" or path == f"{crate_relative}/build.rs":
            records.append((path, mode))
        elif path.startswith(f"{crate_relative}/src/"):
            if not path.endswith(".rs"):
                raise StageError(f"non-Rust production source needs explicit review: {path}")
            records.append((path, mode))
        elif path.startswith(f"{crate_relative}/examples/"):
            if not path.endswith(".rs"):
                raise StageError(f"non-Rust example needs explicit review: {path}")
            records.append((path, mode))
        elif path.startswith(f"{crate_relative}/tests/"):
            inside = path[len(crate_relative) + 1:]
            if path.endswith(".rs") or inside in TEST_ASSETS.get(Path(crate_relative).name, {}):
                records.append((path, mode))
            else:
                omitted.append(path)
        elif path.startswith(f"{crate_relative}/tools/"):
            inside = path[len(crate_relative) + 1:]
            if inside in PACKAGE_TOOLS.get(Path(crate_relative).name, {}):
                records.append((path, mode))
            else:
                omitted.append(path)
        elif path == f"{crate_relative}/README.md":
            records.append((path, mode))
        else:
            omitted.append(path)
    if not any(path == f"{crate_relative}/Cargo.toml" for path, _ in records):
        raise StageError(f"package manifest is not tracked at {crate_relative}/Cargo.toml")
    if not any(path.startswith(f"{crate_relative}/src/") for path, _ in records):
        raise StageError(f"package has no tracked Rust source under {crate_relative}/src")
    if not any(path == f"{crate_relative}/README.md" for path, _ in records):
        raise StageError(f"package README is not tracked at {crate_relative}/README.md")
    return sorted(records), sorted(omitted)


def write_private(path: Path, data: bytes) -> str:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with path.open("xb") as stream:
        os.chmod(path, 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    return hashlib.sha256(data).hexdigest()


def fsync_directories(root: Path) -> None:
    directories = [root, *(path for path in root.rglob("*") if path.is_dir())]
    for directory in sorted(directories, key=lambda item: len(item.parts), reverse=True):
        fd = os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def stage(repo_arg: Path, revision: str, output: Path,
          root_candidates: tuple[str, ...] = ROOT_CANDIDATES) -> dict:
    repo = Path(git(repo_arg, "rev-parse", "--show-toplevel").strip()).resolve()
    head = git(repo, "rev-parse", "HEAD").strip()
    commit = git(repo, "rev-parse", f"{revision}^{{commit}}").strip()
    if head != commit:
        raise StageError("staging requires HEAD to equal the requested full source commit")
    status = git(repo, "status", "--porcelain=v1", "--untracked-files=all")
    if status:
        raise StageError("staging requires a clean checkout; no files were copied")
    destination = output.absolute()
    if destination.exists() or destination.is_symlink():
        raise StageError("staging destination must be a new path")
    try:
        destination.relative_to(repo)
    except ValueError:
        pass
    else:
        raise StageError("staging destination must be outside the source repository")
    if not destination.parent.is_dir():
        raise StageError("staging destination parent must already exist and be a real directory")
    if destination.parent.is_symlink():
        raise StageError("staging destination parent path must not traverse symlink components")
    if destination.parent.resolve(strict=True) != destination.parent:
        raise StageError("staging destination parent path must not traverse symlink components")

    modules, graph, parsed_cache, dependency_blockers = dependency_graph(repo, commit, root_candidates)
    order = topological_order(graph)
    library_names = {
        module: rust_crate_name(parsed_cache[f"{modules[module]}/Cargo.toml"][0], module)
        for module in order
    }
    root_cargo = source_bytes(repo, commit, "Cargo.toml")
    try:
        root_manifest = tomllib.loads(root_cargo.decode("utf-8"))
        workspace = root_manifest["workspace"]
        workspace_package = workspace["package"]
        workspace_lints = workspace["lints"]
    except (UnicodeError, tomllib.TOMLDecodeError, KeyError, TypeError) as exc:
        raise StageError(f"source workspace metadata is incomplete: {exc}") from exc
    expected_lints = {
        "rust": {
            "unsafe_code": "forbid",
            "unexpected_cfgs": {
                "level": "warn",
                "check-cfg": ["cfg(kani)", "cfg(kani_full_capacity)", "cfg(kani_late_replay_covers)", "cfg(creusot)"],
            },
        },
        "clippy": {"unwrap_used": "deny", "expect_used": "deny", "panic": "deny", "indexing_slicing": "deny"},
    }
    if workspace_lints != expected_lints:
        raise StageError("workspace lint inheritance changed; update and review the staging transformation")

    file_plans: dict[str, list[tuple[str, str]]] = {}
    blockers: list[str] = list(dependency_blockers)
    for module in order:
        source_dir = modules[module]
        files, omitted = git_files(repo, commit, source_dir)
        file_plans[module] = files
        blockers.extend(f"{module}: tracked package input is not staged: {path}" for path in omitted)
        planned = {path[len(source_dir) + 1:] for path, _ in files}
        for relpath, expected_hash in (*TEST_ASSETS.get(module, {}).items(),
                                       *PACKAGE_TOOLS.get(module, {}).items()):
            if relpath not in planned:
                blockers.append(f"{module}: allowlisted input is missing: {relpath}")
                continue
            actual_hash = hashlib.sha256(source_bytes(repo, commit, f"{source_dir}/{relpath}")).hexdigest()
            if actual_hash != expected_hash:
                blockers.append(f"{module}: allowlisted input hash mismatch: {relpath}")
        available = {path for path, _ in files}
        for source_path, _mode in files:
            if not source_path.endswith(".rs"):
                continue
            payload = source_bytes(repo, commit, source_path)
            references: list[tuple[str, str]] = []
            for match in COMPILE_TIME_MACRO.finditer(payload):
                literal = COMPILE_TIME_LITERAL.fullmatch(match.group(1))
                if literal is None:
                    blockers.append(f"{module}: {source_path} has a nonliteral compile-time include requiring review")
                    continue
                references.append((literal.group(1).decode("utf-8", "strict"), "compile-time include"))
            references.extend((match.group(1).decode("utf-8", "strict"), "path attribute")
                              for match in PATH_ATTRIBUTE.finditer(payload))
            for include_name, reference_kind in references:
                if PurePosixPath(include_name).is_absolute():
                    blockers.append(f"{module}: {source_path} uses absolute {reference_kind} {include_name}")
                    continue
                referenced = PurePosixPath(source_path).parent.joinpath(include_name)
                normalized_path = PurePosixPath(os.path.normpath(str(referenced)))
                normalized = normalized_path.as_posix()
                if ".." in normalized_path.parts or not normalized.startswith(f"{source_dir}/"):
                    blockers.append(f"{module}: {source_path} {reference_kind} escapes package: {include_name}")
                    continue
                if normalized not in available:
                    blockers.append(f"{module}: {source_path} {reference_kind} references unstaged file {normalized}")
        api_path = API_DOCS.get(module)
        if api_path is None:
            blockers.append(f"{module}: no source-backed API document mapping")
        else:
            try:
                source_bytes(repo, commit, api_path)
            except StageError as exc:
                blockers.append(f"{module}: API document unavailable: {exc}")
    if blockers:
        preview = "\n".join(f"- {item}" for item in sorted(set(blockers)))
        raise StageError("release staging is incomplete; no output was created:\n" + preview)

    os.mkdir(destination, 0o700)
    source_inventory: dict[str, str] = {"Cargo.toml": hashlib.sha256(root_cargo).hexdigest()}
    output_inventory: dict[str, str] = {}
    total = len(root_cargo)
    try:
        members = [f"crates/{public_name(module)}" for module in order]
        workspace_text = (
            "[workspace]\nresolver = \"2\"\nmembers = "
            + json.dumps(members, separators=(",", ":")) + "\n\n[workspace.package]\n"
            + f'version = "0.1.0"\nedition = {json.dumps(workspace_package["edition"])}\n'
            + f'license = {json.dumps(workspace_package["license"])}\n'
            + f'rust-version = {json.dumps(workspace_package["rust-version"])}\n'
            + f'authors = {json.dumps(workspace_package["authors"], separators=(",", ":"))}\n\n'
            + "[workspace.lints.rust]\nunsafe_code = \"forbid\"\n"
            + "unexpected_cfgs = { level = \"warn\", check-cfg = [\"cfg(kani)\", \"cfg(kani_full_capacity)\", \"cfg(kani_late_replay_covers)\", \"cfg(creusot)\"] }\n\n"
            + "[workspace.lints.clippy]\nunwrap_used = \"deny\"\nexpect_used = \"deny\"\npanic = \"deny\"\nindexing_slicing = \"deny\"\n"
        ).encode("utf-8")
        output_inventory["Cargo.toml"] = write_private(destination / "Cargo.toml", workspace_text)
        total += len(workspace_text)

        for module in order:
            source_dir = modules[module]
            package_dir = f"crates/{public_name(module)}"
            parsed, original_manifest = parsed_cache[f"{source_dir}/Cargo.toml"]
            dependency_paths: dict[str, str] = {}
            for _section_name, _alias, dep in dependency_tables(parsed):
                old_path = dep.get("path")
                if not isinstance(old_path, str):
                    continue
                resolved = PurePosixPath(source_dir).joinpath(old_path)
                normalized = PurePosixPath(os.path.normpath(str(resolved))).as_posix()
                dep_package = parsed_cache.get(f"{normalized}/Cargo.toml")
                if dep_package is None:
                    dep_manifest, _ = resolve_package(repo, commit, normalized, parsed_cache)
                else:
                    dep_manifest = dep_package[0]
                dep_info = dep_manifest.get("package", {})
                dep_name = dep_info.get("name") if isinstance(dep_info, dict) else None
                if not isinstance(dep_name, str) or dep_name not in modules:
                    raise StageError(f"dependency outside release closure while transforming {module}: {old_path}")
                dependency_paths[old_path] = f"crates/{public_name(dep_name)}"

            files = file_plans[module]
            for source_path, mode in files:
                relative_inside = source_path[len(source_dir) + 1:]
                payload = source_bytes(repo, commit, source_path)
                total += len(payload)
                if total > MAX_SOURCE_TOTAL:
                    raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
                source_inventory[source_path] = hashlib.sha256(payload).hexdigest()
                if relative_inside == "Cargo.toml":
                    payload = transform_manifest(original_manifest, module, dependency_paths,
                                                 workspace_package, workspace_lints, library_names[module])
                target_path = Path(package_dir) / relative_inside
                digest = write_private(destination / target_path, payload)
                output_inventory[target_path.as_posix()] = digest
                if mode == "100755":
                    os.chmod(destination / target_path, 0o700)
            api_path = API_DOCS.get(module)
            if not api_path:
                raise StageError(f"no source-backed API document mapped for package {module}")
            api_source_data = source_bytes(repo, commit, api_path)
            api_mapping = (
                f"# Package and Rust import names\n\n"
                f"Cargo package: `{public_name(module)}`  \n"
                f"Rust crate import: `{library_names[module]}`\n\n"
                "The package name used by Cargo and the Rust crate name used in source imports "
                "are intentionally recorded separately for review.\n\n---\n\n"
            ).encode("utf-8")
            api_data = api_mapping + api_source_data
            total += len(api_data)
            if total > MAX_SOURCE_TOTAL:
                raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
            source_inventory[api_path] = hashlib.sha256(api_source_data).hexdigest()
            package_api = Path(package_dir) / "API.md"
            output_inventory[package_api.as_posix()] = write_private(destination / package_api, api_data)
            package_license = Path(package_dir) / "LICENSE"
            license_data = source_bytes(repo, commit, "LICENSE")
            total += len(license_data)
            if total > MAX_SOURCE_TOTAL:
                raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
            source_inventory["LICENSE"] = hashlib.sha256(license_data).hexdigest()
            output_inventory[package_license.as_posix()] = write_private(destination / package_license, license_data)

        if total > MAX_SOURCE_TOTAL:
            raise StageError(f"source inventory exceeds {MAX_SOURCE_TOTAL} bytes")
        manifest = {
            "schema": "openatca-core-release-stage-v1",
            "source_commit": commit,
            "package_version": "0.1.0",
            "package_order": [public_name(module) for module in order],
            "package_import_names": {
                public_name(module): library_names[module] for module in order
            },
            "dependency_graph": {public_name(module): sorted(public_name(dep) for dep in graph[module]) for module in order},
            "source_files_sha256": dict(sorted(source_inventory.items())),
            "staged_files_sha256": dict(sorted(output_inventory.items())),
            "input_provenance": INPUT_PROVENANCE,
            "content_scope": "tracked Rust source, test code, hash-allowlisted fixtures, the exact allowlisted generator, generated package manifests, license, and mapped API markdown; no recordings, full datasets, or build outputs",
            "test_scope": "Tracked tests and explicitly allowlisted compile-time fixtures are copied byte-for-byte; staging does not execute tests or claim qualification.",
            "package_completeness": "source-and-test package preview for the selected roots and dependency closure; publication and release qualification remain unapproved",
            "publication_state": "not-published; review and gated validation required",
        }
        manifest_data = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode("utf-8")
        write_private(destination / "release-manifest.json", manifest_data)
        fsync_directories(destination)
        return manifest
    except Exception:
        # A partial staging tree is retained for diagnosis, with restrictive permissions.
        fsync_directories(destination)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True, help="clean source worktree")
    parser.add_argument("--commit", required=True, help="full source commit to stage")
    parser.add_argument("--output", type=Path, required=True, help="new private staging directory outside the repository")
    parser.add_argument("--packages", help="comma-separated release roots from the reviewed candidate set; default: all roots")
    args = parser.parse_args()
    try:
        roots = tuple(item.strip() for item in args.packages.split(",")) if args.packages else ROOT_CANDIDATES
        manifest = stage(args.repo, args.commit, args.output, roots)
    except (StageError, OSError, ValueError, TypeError, KeyError) as exc:
        print(f"oa-core-release-stage: {exc}", file=sys.stderr)
        return 2
    print(f"prepared source-and-test preview for {len(manifest['package_order'])} packages from {manifest['source_commit']}")
    print("only tracked source, tests, approved fixtures, and API docs are copied; no recordings or datasets")
    print("this is a source package preview, not a qualification or publication claim")
    print(f"manifest: {args.output / 'release-manifest.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
