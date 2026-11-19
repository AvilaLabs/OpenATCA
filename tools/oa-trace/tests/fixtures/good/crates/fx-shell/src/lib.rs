//! Shell: free to use the system.
use std::collections::HashMap;

pub fn read(p: &str) -> HashMap<u8, Vec<u8>> {
    println!("{p}");
    let _ = std::fs::read(p);
    HashMap::new()
}
