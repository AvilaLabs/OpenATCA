//! Registry-authorized, bounded shell for appending one audit intent.
use oa_audit_core::{authorize_principal, details, target, Action, Checkpoint, Event};
use oa_audit_log::shell;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-audit-action: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 18
        || args.iter().any(|argument| argument.len() > 4096)
        || args.first().map(String::as_str) != Some("--log")
        || args.get(2).map(String::as_str) != Some("--count")
        || args.get(4).map(String::as_str) != Some("--tail")
        || args.get(6).map(String::as_str) != Some("--registry")
        || args.get(8).map(String::as_str) != Some("--registry-sha256")
        || args.get(10).map(String::as_str) != Some("--principal")
        || args.get(12).map(String::as_str) != Some("--action")
        || args.get(14).map(String::as_str) != Some("--target")
        || args.get(16).map(String::as_str) != Some("--details")
    {
        return Err("usage: oa-audit-action --log <file> --count <n> --tail <hex> --registry <file> --registry-sha256 <hex> --principal <id> --action vulnerability-intake|vulnerability-disposition|runway-assignment --target <ascii> --details <ascii>".into());
    }
    let get = |index: usize, name: &str| {
        args.get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {name}"))
    };
    let count = get(3, "count")?
        .parse::<u64>()
        .map_err(|_| "count must be an unsigned integer".to_owned())?;
    let tail = parse_digest(get(5, "tail")?)?;
    let registry_pin = parse_digest(get(9, "registry pin")?)?;
    let principal = get(11, "principal")?
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "principal must be a positive unsigned ID".to_owned())?;
    let action = match get(13, "action")? {
        "vulnerability-intake" => Action::VulnerabilityIntake,
        "vulnerability-disposition" => Action::VulnerabilityDisposition,
        "runway-assignment" => Action::RunwayAssignment,
        _ => return Err("unsupported action".into()),
    };
    let registry =
        shell::load_principal_registry(Path::new(get(7, "registry path")?), registry_pin)
            .map_err(|error| format!("principal registry refused: {error:?}"))?;
    let role = authorize_principal(&registry, principal, action)
        .map_err(|error| format!("principal registry refused action: {error:?}"))?;
    let (target, target_len) = target(get(15, "target")?.as_bytes())
        .map_err(|error| format!("invalid target: {error:?}"))?;
    let (details, details_len) = details(get(17, "details")?.as_bytes())
        .map_err(|error| format!("invalid details: {error:?}"))?;
    let time_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock precedes Unix epoch".to_owned())?
        .as_nanos()
        .try_into()
        .map_err(|_| "nanosecond timestamp overflow".to_owned())?;
    let event = Event {
        principal,
        role,
        action,
        target,
        target_len,
        time_ns,
        details,
        details_len,
    };
    let expected = Checkpoint { count, tail };
    let (actual, receipt) = shell::append_receipt(Path::new(get(1, "log path")?), expected, event)
        .map_err(|error| format!("audit append failed: {error:?}"))?;
    println!(
        "appended\t{}\t{}\tprincipal_claim={}\tregistry_role={role:?}",
        actual.count,
        oa_sha256::to_hex(&actual.tail),
        receipt.principal()
    );
    Ok(())
}

fn parse_digest(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 {
        return Err("digest must contain 64 lowercase hexadecimal digits".into());
    }
    let bytes = text.as_bytes();
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let offset = index.checked_mul(2).ok_or("digest offset overflow")?;
        let high = *bytes.get(offset).ok_or("short digest")?;
        let low = *bytes.get(offset + 1).ok_or("short digest")?;
        *byte = (nibble(high)? << 4) | nibble(low)?;
    }
    Ok(digest)
}

fn nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("digest must use lowercase hexadecimal".into()),
    }
}
