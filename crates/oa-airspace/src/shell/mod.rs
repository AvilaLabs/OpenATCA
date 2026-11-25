//! Shell (B1, B7): code that uses a hash map or coordinates parsing work. `build` is shell
//! because its working maps are `HashMap`s (looked up only; every output list is sorted, so no
//! iteration order reaches the output). Callers own file access; private commands live in
//! `oa-adapt`.

pub mod build;
pub mod load;
pub mod route_parse;
pub mod sources;
