//! Helpers shared by the integration tests: a deterministic generator, one-shot compression
//! helpers, a gzip member reader built on the oa-zip inflater, and system-tool plumbing.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_deflate::{Compressor, GzipWriter};
use oa_zip::{crc32, inflate};
use std::io::Write;
use std::process::{Command, Stdio};

/// xorshift64*: deterministic, no clock, no dependencies.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 32) as u8).collect()
    }
}

/// Compress `data` as raw DEFLATE, writing in `chunk`-sized pieces.
pub fn deflate_chunked(level: u8, data: &[u8], chunk: usize) -> Vec<u8> {
    let mut c = Compressor::new(level);
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| -> Result<(), ()> {
        out.extend_from_slice(b);
        Ok(())
    };
    for piece in data.chunks(chunk.max(1)) {
        c.write(piece, &mut sink).unwrap();
    }
    c.finish(&mut sink).unwrap();
    out
}

pub fn deflate(level: u8, data: &[u8]) -> Vec<u8> {
    deflate_chunked(level, data, usize::MAX / 2)
}

pub fn gzip_chunked(level: u8, data: &[u8], chunk: usize) -> Vec<u8> {
    let mut w = GzipWriter::new(level);
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| -> Result<(), ()> {
        out.extend_from_slice(b);
        Ok(())
    };
    for piece in data.chunks(chunk.max(1)) {
        w.write(piece, &mut sink).unwrap();
    }
    w.finish(&mut sink).unwrap();
    out
}

pub fn gzip(level: u8, data: &[u8]) -> Vec<u8> {
    gzip_chunked(level, data, usize::MAX / 2)
}

/// Inflate a raw stream and require it to equal `data`.
pub fn assert_roundtrip(stream: &[u8], data: &[u8], what: &str) {
    let got = inflate(stream, data.len(), data.len().max(1) + 16)
        .unwrap_or_else(|e| panic!("{what}: our inflater rejected the stream: {e}"));
    assert!(
        got == data,
        "{what}: round trip differs ({} vs {} bytes)",
        got.len(),
        data.len()
    );
}

/// Read one gzip member (header flags honoured, trailer checked) with the oa-zip inflater.
pub fn gunzip(member: &[u8]) -> Result<Vec<u8>, String> {
    if member.len() < 18 || member[0] != 0x1f || member[1] != 0x8b || member[2] != 8 {
        return Err("not a gzip member".into());
    }
    let flg = member[3];
    let mut p = 10usize;
    if flg & 4 != 0 {
        let xlen = usize::from(member[p]) | usize::from(member[p + 1]) << 8;
        p += 2 + xlen;
    }
    for bit in [8u8, 16] {
        if flg & bit != 0 {
            while *member.get(p).ok_or("truncated name")? != 0 {
                p += 1;
            }
            p += 1;
        }
    }
    if flg & 2 != 0 {
        p += 2;
    }
    let trailer = member.len() - 8;
    let want_crc = u32::from_le_bytes(member[trailer..trailer + 4].try_into().unwrap());
    let want_len = u32::from_le_bytes(member[trailer + 4..].try_into().unwrap());
    let body = member.get(p..trailer).ok_or("short member")?;
    let out = inflate(body, want_len as usize, 1 << 31).map_err(|e| e.to_string())?;
    if out.len() as u32 != want_len {
        return Err(format!("ISIZE {want_len} but inflated {}", out.len()));
    }
    if crc32(&out) != want_crc {
        return Err("CRC-32 mismatch".into());
    }
    Ok(out)
}

/// Run a system tool with `input` on stdin. `None` if the tool is not installed.
pub fn run_tool(cmd: &str, args: &[&str], input: &[u8]) -> Option<std::process::Output> {
    let mut child = match Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{cmd} is not installed: skipping its checks");
            return None;
        }
        Err(e) => panic!("cannot run {cmd}: {e}"),
    };
    let mut stdin = child.stdin.take().unwrap();
    std::thread::scope(|s| {
        s.spawn(move || {
            // A tool that stops reading early (for example after an error) closes the pipe.
            let _ = stdin.write_all(input);
        });
        Some(child.wait_with_output().unwrap())
    })
}

/// A realistic compressible text: pseudo-XML with repeating structure and varying numbers.
pub fn xmlish(n: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng(seed);
    let mut s = String::new();
    while s.len() < n {
        s.push_str(&format!(
            "<flight id=\"AAL{}\" lat=\"{}.{}\" lon=\"-{}.{}\"><alt>{}</alt><state>airborne</state></flight>\n",
            r.next() % 9000,
            r.next() % 90,
            r.next() % 10000,
            r.next() % 180,
            r.next() % 10000,
            r.next() % 40000
        ));
    }
    s.truncate(n);
    s.into_bytes()
}
