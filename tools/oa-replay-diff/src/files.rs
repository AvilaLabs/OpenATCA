//! The shell of `oa-replay-diff` (B3): reads digest, expect and rows files within named limits
//! and writes the report with its errors handled. The checks themselves are in [`crate::diff`].
//!
//! | item | class |
//! |---|---|
//! | [`load_digest_text`], [`load_expect_text`] | shell: read a file, size checked before reading |
//! | [`open_rows`], [`RowsReader`] | shell: stream a rows file as groups of rows |
//! | [`write_out`] | shell: write text, an error (a closed pipe included) is returned |

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use oa_track::canon::{parse_rows_header, CanonRow, ROWS_HEADER_BYTES, ROW_BYTES};

use crate::diff::{
    grouper_finish, grouper_push, Group, Grouper, MAX_DIGEST_BYTES, MAX_EXPECT_BYTES,
};

/// Maximum size of the metadata-only replay witness specimen.
pub const MAX_WITNESS_BYTES: u64 = 4096;

/// Reads a text file of at most `max` bytes; the size is checked before the read and again on
/// the bytes read (a file that grows between the two).
fn read_limited(p: &Path, max: u64) -> Result<String, String> {
    let name = p.display();
    // The guard first: the path is checked before anything, metadata included, reads it.
    let f = oa_sealed::open(p).map_err(|e| format!("{name}: {e}"))?;
    let len = f.metadata().map_err(|e| format!("{name}: {e}"))?.len();
    read_limited_file(p, f, len, max)
}

fn read_limited_file(p: &Path, file: File, len: u64, max: u64) -> Result<String, String> {
    let name = p.display();
    if len > max {
        return Err(format!("{name}: {len} bytes, more than the limit of {max}"));
    }
    let mut s = String::new();
    file.take(max.saturating_add(1))
        .read_to_string(&mut s)
        .map_err(|e| format!("{name}: {e}"))?;
    if u64::try_from(s.len()).unwrap_or(u64::MAX) > max {
        return Err(format!("{name}: more than the limit of {max} bytes"));
    }
    Ok(s)
}

/// The text of a digest file, within [`MAX_DIGEST_BYTES`].
pub fn load_digest_text(p: &Path) -> Result<String, String> {
    read_limited(p, MAX_DIGEST_BYTES)
}

/// The text of an expect file, within [`MAX_EXPECT_BYTES`].
pub fn load_expect_text(p: &Path) -> Result<String, String> {
    read_limited(p, MAX_EXPECT_BYTES)
}

/// Reads a metadata-only replay witness specimen through the sealed path guard.
pub fn load_witness_text(p: &Path) -> Result<String, String> {
    let (file, len) =
        oa_sealed::open_regular(p).map_err(|error| format!("{}: {error}", p.display()))?;
    read_limited_file(p, file, len, MAX_WITNESS_BYTES)
}

/// Writes `text` and flushes; a failure (a closed pipe, a full disk) is an error, not a panic.
pub fn write_out(w: &mut dyn Write, text: &str) -> Result<(), String> {
    w.write_all(text.as_bytes())
        .and_then(|()| w.flush())
        .map_err(|e| format!("cannot write the report: {e}"))
}

/// Reads a rows file group by group (a group is the rows of one second). The header is checked
/// in full when it is opened, the row count it declares must be the count the digest gives, the
/// stream must hold exactly that many rows and nothing after them.
// A7.2: cold struct, size guard waived
#[derive(Debug)]
pub struct RowsReader<R: Read> {
    r: R,
    remaining: u64,
    grouper: Grouper,
    done: bool,
}

impl<R: Read> RowsReader<R> {
    /// Reads and checks the header; `digest_rows` is the total of the digest the file belongs to.
    pub fn new(mut r: R, digest_rows: u64) -> Result<RowsReader<R>, String> {
        let mut h = [0u8; ROWS_HEADER_BYTES];
        r.read_exact(&mut h)
            .map_err(|e| format!("rows file header: {e}"))?;
        let declared = parse_rows_header(&h)?;
        if declared != digest_rows {
            return Err(format!(
                "rows file declares {declared} rows, its digest has {digest_rows}"
            ));
        }
        Ok(RowsReader {
            r,
            remaining: declared,
            grouper: Grouper::default(),
            done: false,
        })
    }

    fn advance(&mut self) -> Result<Option<Group>, String> {
        loop {
            if self.remaining == 0 {
                // Exactly the declared rows: one more byte is an error.
                let mut extra = [0u8; 1];
                let n = self
                    .r
                    .read(&mut extra)
                    .map_err(|e| format!("rows file: {e}"))?;
                if n > 0 {
                    return Err("rows file has bytes after its last row".to_string());
                }
                return Ok(grouper_finish(std::mem::take(&mut self.grouper)));
            }
            let mut b: CanonRow = [0u8; ROW_BYTES];
            self.r
                .read_exact(&mut b)
                .map_err(|e| format!("rows file ends early: {e}"))?;
            self.remaining -= 1;
            if let Some(g) = grouper_push(&mut self.grouper, b)? {
                return Ok(Some(g));
            }
        }
    }
}

impl<R: Read> Iterator for RowsReader<R> {
    type Item = Result<Group, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.advance() {
            Ok(Some(g)) => Some(Ok(g)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// Opens a rows file for the digest whose total is `digest_rows`.
pub fn open_rows(p: &Path, digest_rows: u64) -> Result<RowsReader<BufReader<File>>, String> {
    let f = oa_sealed::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    RowsReader::new(BufReader::with_capacity(1 << 20, f), digest_rows)
        .map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oa_track::canon::{canon_row, rows_header, OFF_FLAGS};
    use oa_track::OutRow;

    const S: i64 = 1_000_000_000;

    fn row(t_ns: i64, addr: u32) -> CanonRow {
        canon_row(&OutRow {
            t_ns,
            addr,
            lat_e7: 1,
            lon_e7: 2,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            pos_cov_m2: [0.0; 3],
            vel_cov_m2s2: [0.0; 3],
            turn_rate_rps: 0.0,
            alt_ft: 0.0,
            alt_var_ft2: 0.0,
            vrate_fpm: 0.0,
            mu_h: [0.0; 2],
            mu_v: [0.0; 2],
            flags: 0,
        })
    }

    fn file(rows: &[CanonRow]) -> Vec<u8> {
        let mut b = rows_header(rows.len() as u64).to_vec();
        for r in rows {
            b.extend_from_slice(r);
        }
        b
    }

    // REQ: OA-RDF-005
    #[test]
    fn rows_files_are_checked_and_read_group_by_group() {
        let a = [row(10 * S, 1), row(10 * S, 2), row(12 * S, 1)];
        let f = file(&a);
        let g: Vec<Group> = RowsReader::new(&f[..], 3)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            g.iter().map(|x| (x.0, x.1.len())).collect::<Vec<_>>(),
            vec![(10, 2), (12, 1)]
        );
        // Bad magic, version, size.
        for (at, what) in [(0usize, "magic"), (8, "version"), (12, "row size")] {
            let mut bad = f.clone();
            bad[at] ^= 0x40;
            assert!(RowsReader::new(&bad[..], 3).unwrap_err().contains(what));
        }
        // Truncated: ends early, named; the iterator then ends.
        let cut = &f[..f.len() - 3];
        let mut rd = RowsReader::new(cut, 3).unwrap();
        assert!(rd.next().unwrap().unwrap_err().contains("ends early"));
        assert!(rd.next().is_none());
        // Not grouped by ascending second.
        let unsorted = file(&[row(12 * S, 1), row(10 * S, 1)]);
        let mut rd = RowsReader::new(&unsorted[..], 2).unwrap();
        assert!(rd.next().unwrap().unwrap_err().contains("ascending"));
    }

    // REQ: OA-RDF-013
    #[test]
    fn a_rows_file_with_trailing_bytes_or_the_wrong_count_is_refused() {
        let a = [row(10 * S, 1), row(11 * S, 1)];
        let mut f = file(&a);
        // The header count must be the digest's.
        assert!(RowsReader::new(&f[..], 3)
            .unwrap_err()
            .contains("declares 2"));
        // Trailing bytes after the last row, even one.
        f.push(0);
        let e: Result<Vec<Group>, String> = RowsReader::new(&f[..], 2).unwrap().collect();
        assert!(e.unwrap_err().contains("after its last row"));
        // A header claiming fewer rows than the file holds is the same case.
        let mut g = file(&a);
        g[16] = 1;
        let e: Result<Vec<Group>, String> = RowsReader::new(&g[..], 1).unwrap().collect();
        assert!(e.unwrap_err().contains("after its last row"));
        // A header claiming more rows than the file holds ends early.
        let mut h = file(&a);
        h[16] = 3;
        let e: Result<Vec<Group>, String> = RowsReader::new(&h[..], 3).unwrap().collect();
        assert!(e.unwrap_err().contains("ends early"));
        let _ = OFF_FLAGS;
    }

    // REQ: OA-RDF-008
    #[test]
    fn output_errors_are_returned_not_panicked() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let e = write_out(&mut Closed, "text").unwrap_err();
        assert!(e.contains("cannot write"), "{e}");
        let mut ok = Vec::new();
        write_out(&mut ok, "abc").unwrap();
        assert_eq!(ok, b"abc");
    }

    // REQ: OA-SEAL-013
    #[test]
    fn the_file_readers_refuse_a_sealed_path() {
        let p = Path::new("/x/scds/stdds-20261119-holdout.bak");
        let d = std::env::temp_dir().join(format!(
            "oa-rd-seal-{}/derived/zau-2026.11.21",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let e = read_limited(&f, 10).unwrap_err();
        assert!(e.contains("OA-SEAL-001"), "{e}");
        let e = open_rows(p, 0).err().unwrap();
        assert!(e.contains("OA-SEAL-001"), "{e}");
    }
}
