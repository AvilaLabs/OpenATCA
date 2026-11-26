# OpenATCA Core FFI for Python

This distribution contains the provisional `openatca` ctypes wrapper only. It covers selected
scalar math, SHA-256 integrity digest, WGS-84 geodesy, and bounded XML, JSON, and CSV parsing
operations from `openatca-core-ffi`. It does not provide the full 0.1 format decoder set or a
Python binding for the ASTERIX, CIFP, NASR, AIXM, FIXM, or GRIB2 crates.

The wheel is pure Python and does not contain or build a native library. Build the native C ABI
separately from `crates/openatca-core-ffi`, then pass its explicit path to `openatca.load(path)`.
Importing `openatca` does not search for or load a library automatically. The provisional ABI
names and layouts are not stable; this distribution is not a Stable API claim.

```python
import openatca

core = openatca.load("/path/to/libopenatca_core_ffi.so")
distance = core.inverse(40.0, -75.0, 41.0, -74.0).distance_m
digest = core.integrity.sha256(b"sample")
```

The binding tests and their synthetic CSV vectors remain in the source tree:

- `crates/openatca-core-ffi/tests/test_ctypes_l0.py`
- `crates/openatca-core-ffi/tests/test_ctypes_vectors.py`
- `crates/openatca-core-ffi/tests/test_ctypes_text.py` with `tests/fixtures/text-v1.tsv`
- `crates/openatca-core-ffi/tests/fixtures/l0.csv`
- `crates/openatca-core-ffi/tests/fixtures/wgs84.csv`
- `crates/openatca-core-ffi/tests/fixtures/polygon.csv`
- `crates/openatca-core-ffi/tests/fixtures/text-v1.tsv`

Those fixtures are not copied into this wheel. Their licensing and provenance must be reviewed
before they are distributed separately.
