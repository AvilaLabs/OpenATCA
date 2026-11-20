//! The arguments, the atomic writes and the refusal of sealed paths.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Command;

use oa_legacy_traj::shell::args::parse;
use oa_legacy_traj::shell::write::write_all;

fn argv(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

const BASE: &str = "--sfdps s --tbfm b --tfms f --out o";

// REQ: OA-LTJ-017
#[test]
fn the_arguments_take_the_inputs_in_any_order_and_state_the_defaults() {
    let a = parse(&argv(&format!("{BASE} --adsb a.oaf --adsb b.oaf"))).unwrap();
    assert_eq!(a.adsb.len(), 2);
    assert!(a.places.is_none());
    let a = parse(&argv(&format!("{BASE} --adsb a.oaf --places p.csv"))).unwrap();
    assert_eq!(a.places.as_deref(), Some(std::path::Path::new("p.csv")));
    assert!(a.nasr.is_none());
    let a = parse(&argv(&format!("{BASE} --adsb a.oaf --nasr n.zip"))).unwrap();
    assert_eq!(a.nasr.as_deref(), Some(std::path::Path::new("n.zip")));
    assert!(parse(&argv(&format!(
        "{BASE} --adsb a.oaf --nasr n.zip --nasr m.zip"
    )))
    .is_err());
    assert_eq!(a.cut_nm, 2.0);
    assert_eq!(a.runway_nm, 5.0);
    assert_eq!(a.landing_gap_s, 30);
    assert_eq!(a.step_s, 5);
    let a = parse(&argv(&format!(
        "--adsb a.oaf {BASE} --cut-nm 1.5 --runway-nm 3 --landing-gap-s 60 --step-s 2"
    )))
    .unwrap();
    assert_eq!(
        (a.cut_nm, a.runway_nm, a.landing_gap_s, a.step_s),
        (1.5, 3.0, 60, 2)
    );
    for bad in [
        BASE.to_string(),
        format!("{BASE} --adsb a --sfdps again"),
        format!("{BASE} --adsb a --places p --places q"),
        format!("{BASE} --adsb a --cut-nm"),
        format!("{BASE} --adsb a --cut-nm 0"),
        format!("{BASE} --adsb a --step-s 2.5"),
        format!("{BASE} --adsb a --cut-nm 1 --cut-nm 2"),
        format!("{BASE} --adsb a --nope"),
        "--sfdps s --tbfm b --adsb a --out o".to_string(),
    ] {
        assert!(parse(&argv(&bad)).is_err(), "{bad}");
    }
}

// REQ: OA-LTJ-018
#[test]
fn files_are_written_under_a_temporary_name_and_renamed_with_none_left_behind() {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ltj-write");
    let _ = fs::remove_dir_all(&t);
    let files = vec![
        ("a.csv".to_string(), "one".to_string()),
        ("b.csv".to_string(), "two".to_string()),
    ];
    write_all(&t, &files).unwrap();
    assert_eq!(fs::read_to_string(t.join("a.csv")).unwrap(), "one");
    assert_eq!(fs::read_to_string(t.join("b.csv")).unwrap(), "two");
    let names: Vec<String> = fs::read_dir(&t)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".partial")), "{names:?}");
    // A second run replaces the files whole.
    write_all(&t, &[("a.csv".to_string(), "three".to_string())]).unwrap();
    assert_eq!(fs::read_to_string(t.join("a.csv")).unwrap(), "three");
    // A write that fails (its temporary name is a directory) renames nothing: the files of the
    // last good run are untouched.
    fs::create_dir(t.join("b.csv.partial")).unwrap();
    let again = vec![
        ("a.csv".to_string(), "four".to_string()),
        ("b.csv".to_string(), "five".to_string()),
    ];
    assert!(write_all(&t, &again).is_err());
    assert_eq!(fs::read_to_string(t.join("a.csv")).unwrap(), "three");
    assert_eq!(fs::read_to_string(t.join("b.csv")).unwrap(), "two");
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-LTJ-019, OA-LTJ-033
#[test]
fn a_link_into_sealed_data_is_refused_for_every_input_and_the_output() {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ltj-sealed");
    let _ = fs::remove_dir_all(&t);
    let sealed = t.join("scds/stdds-smoke-20261119T192936Z");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("clip.oaf"), b"x").unwrap();
    symlink(&sealed, t.join("dir-link")).unwrap();
    symlink(sealed.join("clip.oaf"), t.join("clip-link.oaf")).unwrap();
    fs::write(t.join("fine.oaf"), b"x").unwrap();
    let out = t.join("out");
    let run = |sfdps: PathBuf, adsb: PathBuf, o: PathBuf| {
        Command::new(env!("CARGO_BIN_EXE_oa-legacy-traj"))
            .arg("--sfdps")
            .arg(sfdps)
            .arg("--tbfm")
            .arg(t.join("fine.oaf"))
            .arg("--tfms")
            .arg(t.join("fine.oaf"))
            .arg("--adsb")
            .arg(adsb)
            .arg("--out")
            .arg(o)
            .output()
            .unwrap()
    };
    for o in [
        run(t.join("dir-link"), t.join("fine.oaf"), out.clone()),
        run(t.join("fine.oaf"), t.join("clip-link.oaf"), out.clone()),
        run(
            t.join("fine.oaf"),
            t.join("fine.oaf"),
            t.join("dir-link/new"),
        ),
    ] {
        assert_eq!(o.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    }
    assert!(!out.exists() && !sealed.join("new").exists());
    // The NASR archive and the places file are checked like every other input.
    for flag in ["--nasr", "--places"] {
        let o = Command::new(env!("CARGO_BIN_EXE_oa-legacy-traj"))
            .args(["--sfdps"])
            .arg(t.join("fine.oaf"))
            .arg("--tbfm")
            .arg(t.join("fine.oaf"))
            .arg("--tfms")
            .arg(t.join("fine.oaf"))
            .arg("--adsb")
            .arg(t.join("fine.oaf"))
            .arg(flag)
            .arg(t.join("clip-link.oaf"))
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{flag}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"),
            "{flag}"
        );
        assert!(!out.exists());
    }
    // The readers themselves go through the guard, so a caller that skips the check is refused.
    let e = oa_legacy_traj::shell::places::load_nasr(&t.join("clip-link.oaf")).unwrap_err();
    assert!(e.contains("OA-SEAL"), "{e}");
    let args = parse(&argv(&format!(
        "--sfdps a --tbfm b --tfms c --adsb d.oaf --out {} --places {}",
        t.join("o2").display(),
        t.join("clip-link.oaf").display()
    )))
    .unwrap();
    let mut sealed = 0u64;
    let e = oa_legacy_traj::shell::run::run_counted(&args, &mut sealed).unwrap_err();
    assert!(e.contains("OA-SEAL"), "{e}");
    assert!(!t.join("o2").exists());
    let _ = fs::remove_dir_all(&t);
}
