# THE RULESET — OpenATCA

Non-negotiable design law. Adapted 2026-11-13 from Connor's Zig ruleset (the zat4 project, which
took the Zig compiler as its model of data-oriented design), translated to Rust and extended for
a safety-critical real-time system.

> **The load-bearing laws. Read before writing or reviewing any code.**
> 1. **Plain data in arrays.** Records are fields only. Hot collections are struct-of-arrays. (A1, A3)
> 2. **Indexes, not pointers.** Internal references are `u32` indexes and never leave their owning module. (A4, A5)
> 3. **The size guard.** Every hot struct carries a compile-time exact-size assertion. (A7)
> 4. **Pure core, thin shell.** All I/O, including the clock, lives in the shell. The core is a pure function of its inputs, which is what makes bit-exact replay possible. (B1–B5)
> 5. **Bounded, visible memory.** The real-time core allocates only at startup; every collection has a stated capacity. (C1–C3)
> 6. **No hidden cost, no casual dependency.** (F1–F3)
>
> When in doubt, obey the stricter reading.

Severity: **[MUST]** inviolable, breaking it fails review. **[MUST NOT]** forbidden. **[DEFAULT]**
the rule unless an exception is recorded at the site as a comment naming the rule and the reason.

---

## A0. Unsafe code

- **A0 [MUST]** `#![forbid(unsafe_code)]` in every crate except the single display-presentation
  crate, which needs raw OS calls for KMS/DRM (ioctl, mmap). There, every `unsafe` block is as
  small as possible, carries a `// SAFETY:` comment stating the invariant it relies on, and is
  reviewed line by line. The standalone `validation/oa-alloc-check` C5 harness is the only other
  exception: its single allocator module may implement `GlobalAlloc` to count allocations in the
  real-time tracker/fusion tests. It is excluded from the product workspace, has no product use,
  and must not be linked into a product target. Every unsafe operation is confined to that
  module, carries a `// SAFETY:` comment, and is reviewed line by line. No other crate may gain an
  exception without amending this rule. The gate checks it (amended 2026-11-19): every core and
  split crate declares `[lints] workspace = true` (the workspace forbids `unsafe_code`) and has
  `#![forbid(unsafe_code)]` as an inner attribute at the top of `src/lib.rs`, `oa-ieee` included, and
  core code contains none of the tokens `unsafe`, `extern`, `#[link`, `no_mangle`, `export_name`,
  `link_section`.

## A. Data layout

- **A1 [MUST]** Data is plain. A record type has fields and nothing else: no methods carrying
  behaviour, no trait objects, no invariants enforced by attached code. Behaviour lives in free
  functions that take data and return data. Derives (`Clone`, `Copy`, `Debug`, `PartialEq`) and
  `const` constructors that only assemble fields are allowed.
- **A1.1 [DEFAULT]** One exception (amended 2026-11-14): a *validated value type* (a single
  physical value or coordinate with a range invariant, such as `LatLon`) may keep private fields,
  a checked constructor that returns `Result`, and plain field accessors, and nothing else. This
  makes an invalid value unrepresentable, which is worth more here than the purity of A1.
  Subsystem types that hold precomputed state (an ellipsoid's series tables, a prepared polygon)
  are subsystems under D5, not records, and keep their interface methods.
- **A2 [MUST]** Design for the many. Anything that can occur more than once (tracks, flight plans,
  fixes, alerts) is stored and processed as a collection. There is no single-track code path the
  bulk path does not also serve.
- **A3 [DEFAULT]** Hot collections are struct-of-arrays: one `Vec` (or fixed array) per field,
  sharing an index. Exception requires a recorded reason (provably tiny, never hot).
- **A4 [MUST]** Cross-record references inside a subsystem are indexes (`u32`, `u16`) wrapped in a
  newtype per collection (`TrackIx(u32)`, `FixIx(u32)`), so an index into one array can never be
  used on another. No references or `Rc`/`Arc` between records.
- **A5 [MUST]** An index never leaves the module that owns its array. Across a boundary pass a
  value or a stable identifier (track number, flight ID, fix identifier plus region).
- **A6 [DEFAULT]** Booleans and sparse flags are stored out of band (bitsets or separate arrays),
  not as `bool` fields padding the hot struct.
- **A7 [MUST] The size guard.** Every hot struct (held in quantity, processed in a loop) asserts
  its exact size at compile time, in its own definition:
  ```rust
  #[derive(Clone, Copy)]
  #[repr(C)]
  pub struct TrackState {
      pub lat_e7: i32,
      pub lon_e7: i32,
      // ...
  }
  // Budget: processed every surveillance update for every track.
  // Raising this number requires a recorded justification (A7.1).
  const _: () = assert!(core::mem::size_of::<TrackState>() == 24);
  ```
  The check is exact (`==`), so it catches growth and silent padding alike. The budget is the
  theoretical packed size; if real size exceeds it, repack first (field order, smaller integers,
  flags out of band, indexes instead of pointers). Hot structs are `#[repr(C)]` so the layout is
  defined, not left to the compiler. A hot struct without a guard is incomplete.
- **A7.1 [MUST]** Raising a budget is a recorded act: the comment states why.
- **A7.2 [DEFAULT]** Cold structs (configuration, one-shot loading) are exempt, claimed in a comment
  `// A7.2: cold struct, size guard waived`. Ambiguous means hot.
- **A8 [MUST]** Content-identified data is keyed by its SHA-256. An adaptation snapshot, a
  recorded input stream and a build of the airspace model are identified by hash: same hash, same
  bytes, never rebuilt or re-parsed.
- **A9 [DEFAULT]** Fixed-point integers for positions, altitudes and times in the hot core
  (for example latitude and longitude in 1e-7 degree units, altitude in feet, time in
  microseconds). Floating point is used inside a calculation, not stored as state, so results are
  reproducible across machines and compilers. Exceptions are recorded.

## B. Purity and the core/shell split

- **B1 [MUST]** Every unit of code is classified core (pure) or shell (impure), stated by where it
  lives (B7) and in the module's doc comment. There is no third category.
- **B2 [MUST]** Core functions are pure: same input, same output; no I/O; no mutation of external
  state; no clock, randomness or globals. Tracking, correlation, prediction, conflict detection
  and display-list construction are all core.
- **B3 [MUST]** All I/O (network, disk, screen, clock, randomness) is shell, and thin.
- **B4 [MUST NOT]** The core never imports or knows about the network, the renderer or a clock.
  **Time is an input.** Every core step receives the current time as a value.
- **B5 [MUST]** Data crosses the core/shell boundary as plain values. No callbacks from the core
  into the shell. The gate lists every core `pub fn` (`pub(crate)` included) whose signature names
  `Fn`, `FnMut`, `FnOnce` or `dyn`, takes `impl Trait` as a parameter or has a bound (`<T: Bound>`
  or `where`), because each is a way for the shell to hand behaviour to the core, and fails on any
  that is not in the reviewed allowlist `tools/oa-trace/core-callbacks-allowlist.txt` (entries
  `crate file.rs::fn reason`; an entry that matches no signature fails too). A listed entry states
  why the caller cannot inject platform behaviour, or is marked OPEN where the type system does not
  stop it and the callers were only read. Also listed: a `pub fn` inside an `impl` whose generics or
  `where` clause carry a bound, a bare `fn(..)` pointer type in a signature, and a `pub struct`,
  `enum`, `union` or `type` that holds an `Fn*`, `dyn` or `fn(..)` type. Private functions are not
  boundaries and are not listed.
- **B6 [MUST]** Because of B2–B5, any run can be replayed bit for bit from its recorded inputs. A
  change that breaks replay determinism is a defect.
- **B7 [MUST]** Where code lives states its class (amended 2026-11-19, after a review found
  classifications in doc comments that had drifted from the code). Every crate's Cargo.toml
  declares `[package.metadata.openatca] class = "core"`, `"shell"` or `"split"`. In a split
  crate, core code lives under `src/core/` and shell code under `src/shell/`, and `lib.rs`
  declares exactly those two modules plus re-exports, doc comments and the attributes
  `#![forbid(unsafe_code)]` and `doc`: any other attribute, on `mod shell`, `mod core` or the crate
  (`#[macro_use]`, `#[allow]`, `cfg_attr`), could carry shell code into the core and fails. Shell
  code does not export a macro (`#[macro_export]`) and does not `impl` a type or trait that the
  crate's core module defines, whether named, written as `crate::core::..`, renamed with `as` or
  aliased with `type`: behaviour added to core data from `src/shell/` is behaviour the core scan
  cannot read. (Impls generated by a shell macro through a `$` fragment are not seen; the checker
  reads the macro body as written.) Binaries, tests, examples and benches are shell. A pure
  decoder that allocates per message is shell under C6, not core.
- **B8 [MUST]** The gate checks B7 mechanically (`oa-trace`), by allowlist (amended 2026-11-19 after
  a review showed a blocklist could be bypassed by aliases, globs and attributes). Outside
  comments and string literals, core code:
  - names from `std`, `core` and `alloc` only these modules: `fmt`, `ops`, `cmp`, `mem`, `num`,
    `convert`, `iter`, `slice`, `array`, `str`, `char`, `ascii`, `option`, `result`, `marker`,
    `default`, `clone`, `borrow`, `hint`, `error`, `f32`, `f64`, the primitive integer modules,
    `vec`, `boxed`, `string`, `format` (`alloc::format!`), `collections` (`BTreeMap`, `BTreeSet`, `VecDeque` and `BinaryHeap`
    only) and `core::time` (`Duration` only). Every other module (among them `fs`, `io`, `net`,
    `env`, `process`, `thread`, `sync`, `cell`, `rc`, `hash`, `os`, `path`, `ffi`, `panic`,
    `alloc::alloc`, `std::time` and the hash-map family, whose default hasher is seeded from OS
    randomness) fails. `core::hash` stays out because a `Hasher` is implemented by the caller, so
    passing one in is a callback from the shell (B5); `cell` and `rc` hide mutation and aliasing
    from the scan;
  - uses no `as` rename and no `*` glob on a `std`, `core`, `alloc` or workspace-crate path, no
    `extern crate` other than `extern crate alloc;`, no `#[path]`, no `include!`,
    `include_str!`, `include_bytes!`, `env!`, `option_env!` or `file!`, no `static mut`, no
    `thread_local!`, and no print or `dbg!` macro;
  - calls no floating-point method that `core` lacks on the pinned toolchain (among them `sqrt`,
    `floor`, `ceil`, `trunc`, `round`, `powi`, `powf`, `exp`, `exp_m1`, `ln`, `ln_1p`, `log`,
    `log2`, `log10`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`,
    `tanh`, `hypot`, `cbrt`, `mul_add`, `rem_euclid`, `div_euclid` and `fract`; the checker
    holds the complete list for the pinned toolchain), whether called as a method on any receiver
    (a turbofish between the name and the parentheses changes nothing) or through a
    `Name::method` path. A method that user code defines under one of these names fails too, so a
    user method may not reuse a float-method name. The five
    exactly specified operations come from `oa-ieee` and the rest from `oa-math`. The standard
    library's versions call the platform maths library, whose results differ between machines in
    the last bit, and the compiler does not refuse them in a `no_std` crate once `std` is linked
    below it;
  - names `crate::` paths only under `crate::core::`, and another workspace crate only if that
    crate is core, or through its `core` module if it is split.

  - defines no `macro_rules!` macro and uses no raw identifier (`r#name`), so every name the
    scan reads is the name the compiler resolves;
  - has no target-dependent code: no `cfg!(target_…)`, no `#[cfg(target_…)]` and no
    `cfg_attr` on a target, no raw pointer type, and no `{:p}` formatting, so core behaves the
    same on every machine (B6);
  - keeps no address or platform fact (amended 2026-11-19): no `as_ptr`, `as_mut_ptr`, `.addr(`,
    `expose_provenance`, `ptr::eq`, `ptr_eq`, no `usize`/`isize` `MAX`, `MIN` or `BITS`, no cast to a
    fn pointer, and `size_of`/`align_of` only inside a `const _: () = assert!(..)` size guard (a
    report that prints a size states the number as a constant and asserts it beside it); no
    `concat!`, `stringify!` or `format_args!`, which could build a `{:p}` format string the scan
    does not see; no `&raw`, no `Pointer` (`fmt::Pointer`) and no alias of `usize` or `isize`
    (`Alias::MAX` would hide the width); no `cfg`, `cfg!` or `cfg_attr` on `debug_assertions`
    (`debug_assert!` stays: it can only panic). `format!` and `alloc::format` are allowed: the
    `{:p}` ban covers addresses;
  - does not silence the type-name lint (`non_camel_case_types`, `nonstandard_style`, `bad_style`)
    and defines no type alias that is not CamelCase, so a lowercase `type fl = f64;` cannot hide
    a float method; a `Name::<float method>` path is allowed only when the qualifier is `oa_math`,
    `oa_ieee`, a core workspace crate, `self`/`super`/`crate`, a module the crate declares or (for
    `rem_euclid` and `div_euclid`) an integer type, and fails on any other qualifier;
  - takes the bits of a float (`to_bits`, `f64::to_bits(x)`, `to_le_bytes`, `to_be_bytes` and
    `to_ne_bytes` on a float) only from `oa_math::canon_nan(x)` or `canon_nan_f32(x)` (`oa-math`
    itself excepted), and those names are `oa_math`'s: imported from it or called through it, never
    defined, shadowed or imported from anywhere else. IEEE-754 fixes that an invalid operation gives a NaN, not which one, and its sign
    and payload differ between a compile-time fold, x86-64 and aarch64, so a float that feeds a
    hash, a digest or a written output is made canonical first (B6).

  Known lexical limits (the checker reads tokens; it resolves no names and infers no types):
  - `{:?}` or `{}` of a fn item or fn pointer, and `f as usize` of a fn item, print or cast an
    address and look like other uses; the fn-pointer cast ban, the `{:p}` ban and the module list
    narrow the ways to reach them, review catches the rest;
  - `to_le_bytes`, `to_be_bytes` and `to_ne_bytes` on a float are recognised only by a float literal,
    an `as f32`/`as f64` cast, the `f32::`/`f64::` path form, or a receiver name that the same `fn`
    declares `f32`/`f64` (parameters and typed `let`s, closure parameters included), indexed or not,
    or a struct field the crate declares so; a float that reaches the call through an inferred type
    (a `for` loop over a float slice, a function's return value) is not seen;
  - a shell `impl` that a macro generates through a `$` fragment is not seen, and a shell `impl` of a
    trait or type that another core crate defines is not seen (an `impl` of this crate's own core
    types and traits is);
  - test-only code is judged by its attribute (`cfg(test)`, `cfg(kani)`), not by what it does.

  A core or split crate has no `build.rs` and no custom `[lib]` or `[[bin]]` path. A core crate
  has no dependency, in any dependency table and however renamed, on anything but core
  workspace crates (F6), and its `src/lib.rs` declares no module named `main` or `bin`. A split
  crate may depend on shell crates, because its shell module needs them; its core code may not
  name them, which the scan above checks. The checker fails on anything it cannot read or parse,
  on a dependency it cannot resolve, on symlinks, and on a core scan that finds no files.
- **B10 [MUST]** Every crate of class core is `#![cfg_attr(not(test), no_std)]`, so the compiler
  refuses every `std` path in it (amended 2026-11-19). The one exception is `oa-ieee`, the only
  standard-library code beneath the core, whose public API is exactly `sqrt`, `floor`, `ceil`,
  `trunc` and `round`: the IEEE-754 operations that are exactly specified, give the same bits on
  every conforming machine for every result that is not a NaN, and that `core` does not provide on
  the pinned toolchain. (A NaN result, such as the square root of a negative number, is required
  to be a NaN but not which one: its sign and payload differ between machines. B8 makes any float
  that is hashed or written pass through `oa_math::canon_nan`.) The repository root pins the
  toolchain: `rust-toolchain.toml` holds 1.89.0 with `clippy` and `rustfmt`. What CI installs, by
  job: `check` installs 1.89.0 with `clippy` and `rustfmt`; the test jobs install 1.89.0 with no
  components; `fuzz-smoke` installs `nightly-2026-08-21` with `rust-src`; the proof workflow
  installs 1.89.0, `nightly-2026-08-21` (with `llvm-tools`, `rustc-dev`, `rust-src`, `rustfmt`, for
  the Kani installer) and, for Miri, `nightly-2026-08-21` with `miri` and `rust-src`. Every `cargo`
  or `rustc` step in a workflow names its toolchain with `+toolchain`, so the root file does not
  choose for it; `fuzz/` pins the fuzz nightly and `validation/oa-creusot-check/` the Creusot
  nightly (`nightly-2026-08-03`, Creusot v0.14.0's own pin) in their own files. A split
  crate cannot be `no_std` as a whole, so its core module relies on the B8 scan; core code that
  grows large is moved into its own core crate.
- **B9 [MUST]** A crate not yet migrated to B7 declares `class = "unsplit"`. The gate lists every
  unsplit crate and fails if one appears that is not in the fixed list of the twelve crates
  unsplit on 2026-11-19, held in the checker's source. This class exists only until the
  migration ends, and this rule is then removed. Until then the platform float-method calls in the
  non-test code of each unsplit crate are counted and recorded in
  `tools/oa-trace/unsplit-float-budget.txt`; the gate prints the counts and fails if one rises
  (a ratchet: it falls as code moves to `oa_math` and `oa_ieee`, or into a split crate).

## C. Memory

Rust has no stable allocator parameter, so the Zig rules C1–C2 are met by stronger means.

- **C1 [MUST]** The real-time core allocates only during initialization. After start, it runs in
  memory it already owns: fixed-capacity pools and preallocated struct-of-arrays.
- **C2 [MUST]** Every collection has a stated capacity, a named constant, chosen from an analysis
  written beside it (for example, peak airborne traffic in the airspace times a margin). Reaching
  capacity is a defined, reported state with a defined degradation, never a reallocation, panic or
  silent drop. This is how overload stays designed rather than discovered.
- **C3 [DEFAULT]** Per-update scratch space is a preallocated buffer cleared and reused each cycle.
- **C4 [MUST]** Every subsystem owns its memory. No subsystem mutates another's.
- **C5 [MUST]** Tests of the real-time core run under a counting allocator that fails the test if
  anything allocates after initialization.
- **C6 [DEFAULT]** Shell and offline tools (parsers, adaptation build, reports) may allocate
  normally, bounded per OA-GEN-003.
- **C7 [MUST]** Work is bounded, not only storage. Any loop whose length comes from input (records,
  vertices, tiles, segments, candidates, tracks visited) counts its iterations against a named
  budget, and the budget is checked before the work is done, from the sizes the input declares, or
  inside the loop. A limit that caps a buffer while the loop that fills it runs on is not a bound.
  Every search that can return the same item twice either deduplicates as it goes or is sized for
  the duplicates.

## D. Module boundaries

- **D1 [MUST]** Modules are drawn around decisions likely to change. The sealed decisions:
  input formats (each adapter: ADS-B, ASTERIX, SWIM, NASR, CIFP), adaptation source, rendering
  backend, alerting parameters, recording format. Each is its own deep module.
- **D2 [MUST]** Modules are deep: a small interface over a substantial implementation.
- **D3 [MUST]** Wire formats, file layouts and renderer primitives appear in no other module's
  signatures. The core sees only the normalized data model.
- **D4 [MUST]** Strong coupling (layout, packing, index meaning, ordering) stays inside one module.
- **D5 [MUST]** Encapsulate subsystems, not records.
- **D6 [MUST]** Features are vertical slices. A change that touches several layers for a trivial
  feature means the boundaries are wrong; fix them first.

## E. Failure and isolation

- **E1 [MUST]** Subsystems communicate by plain-data messages through bounded queues. No shared
  mutable state across a boundary.
- **E2 [MUST]** A subsystem can fail without corrupting or crashing its neighbours. A failed
  adapter degrades its own input, visibly, and nothing else.
- **E3 [MUST]** Errors are explicit in signatures (`Result`). `unwrap`, `expect`, unchecked
  indexing and any other panic path are forbidden outside tests. Overflow-prone arithmetic uses
  checked or saturating operations, chosen deliberately.
- **E4 [DEFAULT]** Define errors out of existence where honest: common cases are ordinary results.
  Never at the cost of hiding a real fault; in this system a fault that should be seen must be
  seen.
- **E5 [MUST]** Data that fails validation is refused and flagged, never passed on. Inconsistent
  data reaching a controller is worse than a visible gap.
- **E6 [MUST]** No silent substitution or omission. A value that cannot be computed is never
  replaced by a plausible one (NaN by zero, a missing field by a default, a failed projection by a
  coordinate). Every row, sample or query that is skipped, refused or excluded is counted under a
  named reason, and every reported rate states its denominator. Where several inputs describe the
  same thing, the choice between them is a stated rule that does not depend on input order, and
  a genuine conflict is reported as a conflict, not resolved by sort order.
- **E7 [MUST]** Identity needs positive evidence. Two things are the same only when an identifier
  says so in full: an XML name is its (namespace URI, local name) after the URI's references are
  expanded; an airport is its identifier and region; an aircraft is its address. Nearness in
  position, a matching local name, or matching text is not identity. Where identity cannot be
  settled, both are kept and the ambiguity is visible, because hiding an aircraft is worse than
  showing it twice.
- **E8 [MUST]** Arithmetic on a value that came from input or configuration is checked before it
  can overflow, wrap or truncate: widths, counts, offsets, times, epochs, durations and casts from
  float to integer. The check comes before the operation, not after. Calendar fields are checked
  against the calendar.

## F. Dependencies and simplicity

- **F1 [MUST]** No dependency, including dev-dependencies, without a written justification at the
  import site and Connor's approval. The default is no. Exceptions only for proven cryptography or
  entropy, and the minimal platform layer for windowing and sockets.
- **F2 [MUST]** Prefer `const` evaluation and the standard library over a crate.
- **F3 [MUST NOT]** No hidden control flow or cost: no operator overloading that hides work, no
  `Deref` tricks, no implicit conversions that allocate, no macros that hide control flow.
- **F4 [DEFAULT]** Do not abstract early. No generic or trait until repeated real use shows the cut.
- **F6 [MUST]** The core has zero dependencies beyond the Rust standard library. Any outside code
  that is unavoidable (TLS, a vendor messaging client such as SWIM's Solace transport, a radio
  driver) runs in its own adapter process and talks to the core through our own plain message
  format over a bounded socket. It is never linked into the core. If it fails, only its input
  degrades, visibly (E2).
- **F5 [MUST]** Get the data model right early; apply layout optimization (struct-of-arrays,
  packing, size guards) at hardening, once usage is known. A slice is not done until A3 and A7
  are met.

## G. Performance

- **G1 [MUST]** Performance claims are backed by measurement: bytes per record, latency per update,
  throughput at full NAS load.
- **G2 [MUST]** Profile before tuning.
- **G3 [MUST]** Stop rule: when a path's cost is already trivial against its deadline, stop tuning
  and spend the effort on clarity and assurance.
- **G4 [MUST]** No optimization may break a rule in A–F.
- **G5 [MUST]** Every real-time path has a stated deadline, and its worst case (not average) is
  measured against it.
- **G6 [MUST]** A measurement cannot flatter its subject. A run that loses work (refuses, caps,
  overflows, drops) does not pass, however fast it was. A test generator is checked against its
  own description over the whole run (coverage, load, timing), and a metric's definition is
  written before its result is read.

## H. Process

- **H1 [MUST]** Every change is reviewed against this ruleset by section. A MUST violation blocks it.
- **H2 [MUST]** A DEFAULT is overridden only by a comment at the site naming the rule and reason.
- **H3 [MUST]** When a rule obstructs its own purpose, amend this document deliberately. Rules
  change by edit, not drift.
- **H4 [MUST]** This document, SCOPE.md and REQUIREMENTS.md are the source of truth. Where code
  disagrees, the code is wrong until the documents are amended.

## Application to existing work

The M0 parsers (oa-csv, oa-nasr, oa-arinc424) are shell/offline code run once per 28-day cycle:
cold under A7.2 and C6. They still obey A1, A4, E3 and F1. The airspace model produced by M0 is
the first core data structure and must meet A3, A4, A7 and A9 before M0 is called done. The
tracker (M3) is the first fully real-time core and is held to every rule from its first commit.

## Checklist

- [ ] Records are plain data; behaviour in free functions (A1)
- [ ] Hot collections struct-of-arrays, or exception recorded (A3)
- [ ] Indexes are typed newtypes; none crosses a module boundary (A4, A5)
- [ ] Every hot struct has an exact compile-time size assertion (A7), increases justified (A7.1)
- [ ] Hot state in fixed-point units (A9)
- [ ] Module classified core or shell; core pure; time is an input (B1–B4)
- [ ] Crate class declared; core code under `core/` and passing the gate's scan (B7–B9)
- [ ] Replay stays bit-exact (B6)
- [ ] No allocation after init in the real-time core; capacities stated and analysed (C1, C2)
- [ ] Counting-allocator test for the core (C5)
- [ ] Deep modules around changing decisions (D1–D3)
- [ ] Bounded queues between subsystems; failures contained; invalid data refused (E1, E2, E5)
- [ ] No panic paths outside tests (E3)
- [ ] Work bounded by a budget checked before or during the work, not only storage (C7)
- [ ] Nothing substituted or dropped silently; every exclusion counted by reason (E6)
- [ ] Identity from full identifiers only; ambiguity kept visible (E7)
- [ ] Arithmetic on input and configuration checked before it can overflow or truncate (E8)
- [ ] Measurements fail on lost work; generators checked against their description (G6)
- [ ] No new dependency without justification and approval (F1)
- [ ] Measurements behind performance claims; worst case against deadline (G1, G5)
