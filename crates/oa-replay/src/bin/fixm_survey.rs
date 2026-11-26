#![forbid(unsafe_code)]
//! Shell entrypoint for the existing guarded offline survey. Keeping it a Cargo binary lets
//! integration tests use CARGO_BIN_EXE rather than guessing that an example was built.
#[path = "../../examples/fixm_survey.rs"]
mod survey;
fn main() -> std::process::ExitCode {
    survey::main()
}
