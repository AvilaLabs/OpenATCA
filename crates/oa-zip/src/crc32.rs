//! CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320) as used by ZIP. Core (pure).

const POLY: u32 = 0xEDB8_8320;

/// Slice-by-8 tables, built at compile time (F2).
const TABLES: [[u32; 256]; 8] = build_tables();

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
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    let (blocks, rest) = data.as_chunks::<8>();
    for b in blocks {
        let lo = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) ^ c;
        let hi = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
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

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-ZIP-002
    #[test]
    fn known_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    // REQ: OA-ZIP-002
    #[test]
    fn incremental_matches_one_shot_at_every_split() {
        let d: Vec<u8> = (0..300u32).map(|i| (i * 31 + 7) as u8).collect();
        let whole = crc32(&d);
        for s in 0..=d.len() {
            assert_eq!(crc32_update(crc32(&d[..s]), &d[s..]), whole);
        }
    }
}
