//! Public route conversion command: exact model identities, refusal accounting and sealed input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use oa_airspace::core::model::{point_key, PointIx};
use oa_airspace::shell::route_cli::{
    parse_route_steps, run_route, ParseReason, MAX_ROUTE_TEXT_BYTES,
};

// REQ: OA-RTE-010
#[test]
fn normalized_instructions_preserve_full_keys_and_reject_truncation() {
    let plan = parse_route_steps("FIX:ALPHA:K3:1:- DCT:BRAVO:K5:1:KORD").unwrap();
    assert_eq!(plan.len, 2);
    match plan.steps[1] {
        oa_airspace::core::route::RouteStep::Direct(key) => {
            assert_eq!(key.ident.bytes.get(..5).unwrap(), b"BRAVO");
            assert_eq!(key.region.bytes.get(..2).unwrap(), b"K5");
            assert_eq!(key.scope.bytes.get(..4).unwrap(), b"KORD");
        }
        _ => unreachable!(),
    }
    assert_eq!(
        parse_route_steps("FIX:ABCDEFGHIJKLM:K3:1:-")
            .unwrap_err()
            .reason,
        ParseReason::InvalidLabel
    );
    assert_eq!(
        parse_route_steps("FIX:ALPHA:K3:256:-").unwrap_err().reason,
        ParseReason::InvalidNumber
    );
}

// REQ: OA-RTE-011
#[test]
fn parser_refusal_accounts_for_the_unexamined_suffix_and_byte_cap_is_unknown() {
    let refused = parse_route_steps("FIX:ALPHA:K3:1:- BAD FIX:BRAVO:K3:1:-").unwrap_err();
    assert_eq!(
        (
            refused.offered,
            refused.admitted,
            refused.refused,
            refused.unexamined
        ),
        (Some(3), 1, 1, 1)
    );
    let long = "X".repeat(MAX_ROUTE_TEXT_BYTES + 1);
    assert_eq!(parse_route_steps(&long).unwrap_err().offered, None);
    let many = "FIX:A:K3:1:- ".repeat(33);
    let over = parse_route_steps(&many).unwrap_err();
    assert_eq!(over.reason, ParseReason::StepLimit);
    assert_eq!(
        (over.offered, over.admitted, over.refused, over.unexamined),
        (Some(33), 0, 0, 33)
    );
}

// REQ: OA-RTE-012
#[test]
fn command_converts_a_model_snapshot_and_binds_the_exact_inputs() {
    let model = common::fixture_build().model;
    let bytes = oa_airspace::snapshot::encode(&model).unwrap();
    let path = std::env::temp_dir().join(format!("oa-route-cli-{}.snap", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    // The independent fixture builder has already checked its model; choose one complete
    // stable identity, then require the command to preserve that model's integer coordinates.
    let index = 0;
    let key = point_key(&model, PointIx::new(index)).unwrap();
    let empty = |s: &str| {
        if s.is_empty() {
            "-".to_owned()
        } else {
            s.to_owned()
        }
    };
    let text = format!(
        "FIX:{}:{}:{}:{}",
        key.ident,
        empty(key.region),
        key.class,
        empty(key.scope)
    );
    let args = vec![
        "--snapshot".to_owned(),
        path.to_string_lossy().into_owned(),
        "--steps".into(),
        text.clone(),
    ];
    let mut out = Vec::new();
    let result = run_route(&args, &mut out);
    std::fs::remove_file(path).unwrap();
    result.unwrap();
    let report = String::from_utf8(out).unwrap();
    assert!(report.contains(&format!(
        "snapshot_sha256\t{}",
        oa_sha256::to_hex(&oa_sha256::sha256(&bytes))
    )));
    assert!(report.contains(&format!(
        "instructions_sha256\t{}",
        oa_sha256::to_hex(&oa_sha256::sha256(text.as_bytes()))
    )));
    assert!(report.contains("points\t1\n"));
    assert!(report.contains(&format!(
        "\t{}\t{}\t",
        model.points.lat[index as usize], model.points.lon[index as usize]
    )));
}

// REQ: OA-RTE-013
#[test]
fn command_refuses_sealed_paths_before_any_conversion_output() {
    let args = vec![
        "--snapshot".into(),
        "/tmp/scds/2026.11.21/never-open.snap".into(),
        "--steps".into(),
        "FIX:A:K3:1:-".into(),
    ];
    let mut out = Vec::new();
    let error = run_route(&args, &mut out).unwrap_err().to_string();
    assert!(error.contains("OA-SEAL"), "{error}");
    assert!(out.is_empty());
}
