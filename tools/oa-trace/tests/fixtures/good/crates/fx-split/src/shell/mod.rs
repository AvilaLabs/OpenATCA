//! Shell of the split crate.
pub fn load(p: &str) -> Vec<u8> {
    let _ = fx_shell::read(p);
    std::fs::read(p).unwrap_or_default()
}
