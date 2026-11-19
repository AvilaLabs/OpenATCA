//! Proof obligations compile the exact production helper through a source path import.
#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../../../crates/oa-replay/src/core/mono_split.rs"]
mod mono_split;
