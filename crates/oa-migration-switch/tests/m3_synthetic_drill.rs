// Run the example's embedded acceptance test as a normal `cargo test` target so API changes
// cannot leave the 100-cycle/64-cycle M3 drill merely compiling without executing.
#[allow(dead_code)]
#[path = "../examples/m3_synthetic_drill.rs"]
mod drill;
