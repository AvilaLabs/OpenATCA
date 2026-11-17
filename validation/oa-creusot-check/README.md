# Creusot check for `oa-replay` timestamp split

This standalone package imports `crates/oa-replay/src/mono_split.rs` directly by `#[path]`.
That is the implementation production `clock::classify` calls; there is no second Rust model.
The package is its own Cargo workspace and is excluded from the product dependency graph. Its
only dependency, `creusot-std = 0.14.0`, is proof-only. RULESET F1 justification: Connor explicitly
authorized this Creusot experiment in the 2026-11-17 task. No product runtime dependency or lockfile
entry is added.

Use Creusot release v0.14.0, which pins Rust `nightly-2026-08-03`; its opam and prover pins are in
the official release source. Keep the matching Creusot checkout/toolchain and solver versions
controlled. Do not update the root workspace toolchain or add this crate to the product workspace.

The ordinary boundary/parity tests compile the shared helper through the production crate with
the stable project toolchain, without fetching the proof-only dependency. From the product root:

```sh
cargo +1.89.0 test --locked --offline -p oa-replay mono_split
```

Creusot itself translates only library/binary targets, not Rust tests. After installing the pinned
Creusot toolchain and dependencies, run its library proof command from this directory:

```sh
cargo creusot -p oa-creusot-check -- --locked
```

The package-local `Cargo.lock` pins `creusot-std` and its transitive resolution. The matching
`why3find.json` is the official v0.14.0 prover configuration. Generated `verif/` artifacts belong
in the run evidence archive and are regenerated for each source revision. Do not generate or
modify the product workspace lockfile for this assurance package.

The exact shared helper has no `requires` clauses. Its `ensures` contract states the full
eligibility/representability split and exact mathematical values for every returned field using
Creusot integer views. It does not assume representability or mark any helper code trusted. A pinned v0.14.0 run on
2026-11-17 proved all eleven generated obligations, including the total helper contract. An isolated
wrong-step negative control translated successfully and failed that contract. See
[the assurance record](../../docs/RUST_ASSURANCE.md) for source binding and tool limitations.

The assurance claim is limited to this helper's refusal cases and checked
timestamp arithmetic. It does not verify clock policy, state transitions, stream ordering, stable
Rust 1.89 code generation, or the Creusot/Why3/SMT trusted base. Those assumptions and the relevant
`creusot-std` contracts must be reviewed before relying on a result.
