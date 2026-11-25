# Math and IEEE scalar operations

**Current surface.** `oa-ieee` exports `sqrt`, `floor`, `ceil`, `trunc`, and `round` over `f64`.
They have no error return. `sqrt` is correctly rounded; negative input returns a NaN whose sign
and payload may vary by target/folding. The four integral-rounding operations are exact;
`round` breaks ties away from zero. `-0.0` is preserved by `sqrt`.

`oa-math` is `no_std` outside tests and exports `sin`, `cos`, `sin_cos`, `tan`, `atan`, `atan2`,
`asin`, `acos`, `exp`, `ln`, `pow`, `powi`, `hypot`, `cbrt`, `rem_euclid`, canonical NaN helpers,
and canonical float ordering/sign/min/max/clamp helpers. APIs use radians for trig angles and
`f64`/`f32` values in their mathematical domains. Functions return a float directly, so invalid
domains and overflow/underflow are represented by IEEE values rather than `Result`; callers must
check finiteness/domain where required. `powi` is documented as within one ulp, not faithfully
rounded. `rem_euclid` follows the crate's documented divisor/sign convention. The crate states
that its deterministic math avoids platform libm and fused multiply-add, canonicalizes NaNs it
produces, and aims for identical ordinary result bits; caller-created NaNs still need
`canon_nan[_f32]` before bit serialization.

**Limits/errors.** No dynamic work limits or typed errors. Exceptional input behavior is
function-specific; the crate docs and per-function docs are the contract. These are not
drop-in replacements for all `std::f64` methods. A stable API needs a reviewed domain/special-value
table for every export, especially NaN payload/sign, signed zero, overflow, subnormals, and
rounding.

**Tier/stability proposal.** L0 deterministic scalar math. `oa-ieee` is a five-operation adapter;
its `sqrt(-x)` NaN payload is explicitly target-dependent, so consumers must not call raw result
bits stable. `oa-math` has a pinned fingerprint and committed reference/identity/boundary tests,
and the README reports cross-target fingerprint checks for x86-64 and wasm32 but not aarch64.
That is useful evidence, not the full §8 Stable bundle. Verify reference provenance/licensing,
24-hour fuzz, mutation review, no-panic evidence, x86-64+aarch64 bit checks, runnable Rust and
Python examples, and publishable vectors before the tier becomes Stable.

There is no clock, RNG, I/O, environment, adaptation, or sealing policy in these APIs. They are
pure scalar computations; the caller supplies all values.
