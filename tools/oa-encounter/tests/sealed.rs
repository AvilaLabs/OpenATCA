//! `oa-encounter` refuses a link into sealed data (a fake tree in a temp dir): exit 2, the rule
//! named, nothing created.
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

// REQ: OA-SEAL-005
#[test]
fn a_link_into_sealed_data_is_refused_for_the_clip_the_output_and_the_nasr_file() {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("enc-sealed");
    let _ = fs::remove_dir_all(&t);
    let sealed = t.join("scds/stdds-smoke-20261119T192936Z");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("clip.oaf"), b"x").unwrap();
    symlink(sealed.join("clip.oaf"), t.join("clip-link.oaf")).unwrap();
    symlink(&sealed, t.join("dir-link")).unwrap();
    fs::write(t.join("fine.oaf"), b"x").unwrap();
    let out = t.join("out");
    let run = |adsb: PathBuf, o: PathBuf, nasr: Option<PathBuf>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_oa-encounter"));
        c.arg("--adsb").arg(adsb).arg("--out").arg(o);
        if let Some(n) = nasr {
            c.arg("--nasr").arg(n);
        }
        c.output().unwrap()
    };
    for o in [
        run(t.join("clip-link.oaf"), out.clone(), None),
        run(t.join("dir-link/clip.oaf"), out.clone(), None),
        run(t.join("fine.oaf"), t.join("dir-link/new"), None),
        run(
            t.join("fine.oaf"),
            out.clone(),
            Some(t.join("clip-link.oaf")),
        ),
    ] {
        assert_eq!(o.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    }
    assert!(!out.exists() && !sealed.join("new").exists());
    let _ = fs::remove_dir_all(&t);
}
