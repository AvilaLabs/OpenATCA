//! Bounded audit log verifier shell: checks the supplied trusted record count and tail.
use oa_audit_log::{shell, Checkpoint};
use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-audit-verify: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 6
        || args.iter().any(|argument| argument.len() > 4096)
        || args.first().map(String::as_str) != Some("--log")
        || args.get(2).map(String::as_str) != Some("--count")
        || args.get(4).map(String::as_str) != Some("--tail")
    {
        return Err(
            "usage: oa-audit-verify --log <file> --count <n> --tail <64 lowercase hex>".into(),
        );
    }
    let path = args.get(1).ok_or_else(|| "missing log path".to_owned())?;
    let count = args
        .get(3)
        .ok_or_else(|| "missing record count".to_owned())?
        .parse::<u64>()
        .map_err(|_| "record count must be an unsigned integer".to_owned())?;
    let tail_text = args
        .get(5)
        .ok_or_else(|| "missing tail digest".to_owned())?;
    let tail = parse_digest(tail_text)?;
    let checkpoint = Checkpoint { count, tail };
    let path = Path::new(path);
    oa_sealed::Sealed::production()
        .check(path)
        .map_err(|refusal| refusal.to_string())?;
    match shell::verify(path, checkpoint) {
        Ok(actual) => {
            println!(
                "valid\t{}\t{}",
                actual.count,
                oa_sha256::to_hex(&actual.tail)
            );
            Ok(())
        }
        Err(shell::LogError::BrokenRecord { sequence, error }) => {
            Err(format!("first broken record\t{sequence}\t{error:?}"))
        }
        Err(shell::LogError::Truncated { sequence }) => {
            Err(format!("first broken record\t{sequence}\ttruncated"))
        }
        Err(error) => Err(format!("verification failed\t{error:?}")),
    }
}

fn parse_digest(text: &str) -> Result<[u8; 32], String> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return Err("tail digest must contain exactly 64 lowercase hex digits".into());
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let offset = index
            .checked_mul(2)
            .ok_or_else(|| "digest offset overflow".to_owned())?;
        let high = bytes
            .get(offset)
            .copied()
            .ok_or_else(|| "short digest".to_owned())?;
        let low = bytes
            .get(offset + 1)
            .copied()
            .ok_or_else(|| "short digest".to_owned())?;
        *byte = (nibble(high)? << 4) | nibble(low)?;
    }
    Ok(digest)
}

fn nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("tail digest must use lowercase hexadecimal".into()),
    }
}
