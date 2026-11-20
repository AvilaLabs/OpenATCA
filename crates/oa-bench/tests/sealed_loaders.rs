//! The loaders refuse a path that leads into sealed data, however it is reached: a link to a
//! file, a link to a folder, a link in a symlinked parent, a link inside an unsealed folder. The
//! sealed tree here is a fake one in a temp dir whose names are the sealed names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use oa_bench::input::{hash_files, list_inputs, load, load_adsb, read_capped_file, FeedInput};
use oa_replay::FeedKind;

/// `T/scds/rec-20261119/x/clip.oaf` and, outside it, links into it.
struct Tree {
    t: PathBuf,
    link_file: PathBuf,
    link_dir: PathBuf,
    through_parent: PathBuf,
    dir_with_link: PathBuf,
    clean_dir: PathBuf,
}

fn tree(name: &str) -> Tree {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sealed_{name}"));
    let _ = fs::remove_dir_all(&t);
    let sealed = t.join("scds/rec-20261119/x");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("clip.oaf"), b"x").unwrap();
    fs::write(sealed.join("metar1.json"), b"{}").unwrap();
    let open = t.join("open");
    fs::create_dir_all(&open).unwrap();
    symlink(sealed.join("clip.oaf"), open.join("link_file.oaf")).unwrap();
    symlink(&sealed, open.join("link_dir")).unwrap();
    symlink(t.join("scds/rec-20261119"), open.join("parent")).unwrap();
    let dir_with_link = open.join("dir_with_link");
    fs::create_dir_all(&dir_with_link).unwrap();
    symlink(sealed.join("clip.oaf"), dir_with_link.join("a.oaf")).unwrap();
    symlink(
        sealed.join("metar1.json"),
        dir_with_link.join("metar1.json"),
    )
    .unwrap();
    let clean_dir = open.join("clean");
    fs::create_dir_all(&clean_dir).unwrap();
    fs::write(clean_dir.join("a.oaf"), b"x").unwrap();
    Tree {
        link_file: open.join("link_file.oaf"),
        link_dir: open.join("link_dir"),
        through_parent: open.join("parent/x/clip.oaf"),
        dir_with_link,
        clean_dir,
        t,
    }
}

fn refused<T>(r: std::io::Result<T>, what: &str, path: &Path) {
    let e = match r {
        Ok(_) => panic!("{what}: not refused"),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("OA-SEAL-001"), "{what}: {e}");
    // The path is named once, not nested.
    assert!(
        e.matches(&path.display().to_string()).count() <= 1,
        "{what}: {e}"
    );
}

// REQ: OA-SEAL-005
#[test]
fn the_oa_bench_loaders_refuse_every_way_into_sealed_data() {
    let x = tree("bench");
    for p in [&x.link_file, &x.through_parent] {
        refused(load_adsb(p), "load_adsb", p);
        refused(read_capped_file(p, 1 << 20), "read_capped_file", p);
        refused(hash_files("l", std::slice::from_ref(p)), "hash_files", p);
    }
    for d in [&x.link_dir, &x.dir_with_link] {
        refused(list_inputs(d), "list_inputs", d);
        refused(
            load(&[FeedInput {
                feed: FeedKind::Sfdps,
                dir: d.clone(),
            }])
            .map(|_| ()),
            "load",
            d,
        );
        refused(
            oa_bench::altimeters::load_metar_dir(d).map(|_| ()),
            "load_metar_dir",
            d,
        );
    }
    // A folder with no link in it is not refused (the guard does not over-refuse).
    assert!(list_inputs(&x.clean_dir).is_ok());
    let _ = fs::remove_dir_all(&x.t);
}

// REQ: OA-SEAL-005
#[test]
fn the_oa_bench_program_exits_2_for_a_link_into_sealed_data_and_creates_nothing() {
    let x = tree("benchbin");
    let out = x.t.join("out");
    for args in [
        vec!["--adsb".into(), x.link_file.display().to_string()],
        vec!["--adsb".into(), x.through_parent.display().to_string()],
        vec![
            "--sfdps-dir".into(),
            x.link_dir.display().to_string(),
            "--extract-only".into(),
        ],
        vec![
            "--tbfm-dir".into(),
            x.dir_with_link.display().to_string(),
            "--extract-only".into(),
        ],
        vec![
            "--tfms-dir".into(),
            x.link_dir.display().to_string(),
            "--extract-only".into(),
        ],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_oa-bench"))
            .args(&args)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap();
        assert!(!o.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"),
            "{args:?}"
        );
        assert!(!out.exists());
    }
    let _ = fs::remove_dir_all(&x.t);
}
