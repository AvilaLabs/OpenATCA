//! CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320) for the frame check. Core (pure, B1,
//! B7). The same function as `oa_zip::crc32`, held here because core may not name `oa-zip`
//! (a shell crate, B8); a shell test checks the two agree.

const POLY: u32 = 0xEDB8_8320;

/// Slice-by-8 tables, built at compile time (F2).
const TABLES: [[u32; 256]; 8] = build_tables();

// E3 / H2: this runs only at compile time, where an out-of-range index is a build error and not
// a runtime panic; `i` stays below 256 and `s` below 8.
#[allow(clippy::indexing_slicing)]
const fn build_tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut s = 1;
        while s < 8 {
            let prev = t[s - 1][i];
            t[s][i] = (prev >> 8) ^ t[0][(prev & 0xff) as usize];
            s += 1;
        }
        i += 1;
    }
    t
}

/// CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// Continue a CRC-32 over more bytes; `crc32_update(0, x)` equals `crc32(x)`.
// E3 / H2: every table index is a byte value (masked to 0..=255 or a shifted u32 byte) into a
// 256-entry table, and the table number is a constant below 8, so no index can be out of range.
#[allow(clippy::indexing_slicing)]
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    let (blocks, rest) = data.as_chunks::<8>();
    for b in blocks {
        let [b0, b1, b2, b3, b4, b5, b6, b7] = *b;
        let lo = u32::from_le_bytes([b0, b1, b2, b3]) ^ c;
        let hi = u32::from_le_bytes([b4, b5, b6, b7]);
        // Every index below is a byte or a constant, so it is in range for the 256-entry tables.
        c = TABLES[7][(lo & 0xff) as usize]
            ^ TABLES[6][((lo >> 8) & 0xff) as usize]
            ^ TABLES[5][((lo >> 16) & 0xff) as usize]
            ^ TABLES[4][(lo >> 24) as usize]
            ^ TABLES[3][(hi & 0xff) as usize]
            ^ TABLES[2][((hi >> 8) & 0xff) as usize]
            ^ TABLES[1][((hi >> 16) & 0xff) as usize]
            ^ TABLES[0][(hi >> 24) as usize];
    }
    for &b in rest {
        c = (c >> 8) ^ TABLES[0][((c ^ b as u32) & 0xff) as usize];
    }
    !c
}
