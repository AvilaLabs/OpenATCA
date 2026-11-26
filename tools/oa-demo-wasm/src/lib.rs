//! Host shell for the fixed-width F2 demo ABI. This crate supports native 64-bit parity tools and
//! the wasm32 browser target; `f2_frame` checks address conversion rather than truncating it.

#![deny(unsafe_code)]

#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!("oa-demo-wasm supports only 64-bit native hosts and wasm32-unknown-unknown");

// The native workspace builds the shell package without pretending to provide browser
// exports or embedding an arbitrary asset. The scalar ABI exists only for the exact
// browser target; its build script pins and embeds the real OASD bundle there.
#[cfg(all(
    target_arch = "wasm32",
    target_pointer_width = "32",
    target_os = "unknown",
    target_vendor = "unknown"
))]
mod abi;
