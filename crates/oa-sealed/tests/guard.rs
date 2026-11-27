//! The sealed hold-out guard on fake data roots in temp directories (never the real ones).
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

use oa_sealed::{parse_list, Sealed, LIST};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-sealed-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A fake data root with the three sealed folders and one development folder.
fn fake_root(name: &str) -> PathBuf {
    let root = scratch(name).join("data");
    for d in [
        "scds/rec-20261119/day2/sfdps",
        "scds/rec-20261119/day1",
        "scds/rec-20261119/bin",
        "scds/stdds-20261119-holdout",
        "scds/stdds-smoke-20261119T192936Z",
        "scds/day-20261118/rec",
    ] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("scds/rec-20261119/day2/sfdps/a.bin"), b"x").unwrap();
    fs::write(root.join("scds/day-20261118/rec/b.bin"), b"y").unwrap();
    fs::write(root.join(".openatca-data-root"), b"openatca-data-root-v1\n").unwrap();
    root
}

// REQ: OA-SEAL-004
#[test]
fn root_identity_sentinel_is_required_and_checked_before_path_or_time_clearance() {
    let parent = scratch("root-identity");
    let missing = parent.join("missing");
    let empty = parent.join("empty");
    let wrong = parent.join("wrong");
    let directory_sentinel = parent.join("directory-sentinel");
    let linked = parent.join("linked");
    let valid = fake_root("root-identity-valid");
    let outside = parent.join("ordinary-unsealed-path");
    fs::create_dir_all(&empty).unwrap();
    fs::create_dir_all(&wrong).unwrap();
    fs::create_dir_all(directory_sentinel.join(".openatca-data-root")).unwrap();
    fs::write(wrong.join(".openatca-data-root"), b"wrong\n").unwrap();
    fs::create_dir_all(&linked).unwrap();
    symlink(
        valid.join(".openatca-data-root"),
        linked.join(".openatca-data-root"),
    )
    .unwrap();

    for root in [
        missing.as_path(),
        empty.as_path(),
        wrong.as_path(),
        directory_sentinel.as_path(),
        linked.as_path(),
        Path::new("/"),
    ] {
        let guard = Sealed::with_root(root);
        assert!(guard.check(&outside).is_err());
        assert!(guard.in_window(i64::MIN));
        assert!(guard.in_window(i64::MAX));
        assert_eq!(guard.drop_window(), oa_time_window::DropWindow::all_time());
    }

    let guard = Sealed::with_root(&valid);
    assert!(guard.check(&valid.join("ordinary-unsealed-path")).is_ok());
    assert!(guard
        .check(&valid.join("scds/rec-20261119/day2/sfdps/a.bin"))
        .is_err());
    let alias = parent.join("valid-root-link");
    symlink(&valid, &alias).unwrap();
    let linked_guard = Sealed::with_root(&alias);
    assert!(linked_guard
        .check(&alias.join("ordinary-unsealed-path"))
        .is_ok());
}

// REQ: OA-SEAL-001
#[test]
fn the_list_has_the_sealed_folders_and_the_python_guard_parses_it_to_the_same_set() {
    let l = parse_list(LIST).unwrap();
    let texts: Vec<&str> = l.entries.iter().map(|e| e.text.as_str()).collect();
    let want: Vec<String> = [
        "scds/rec-20261119/day2*",
        "scds/rec-20261119/day3*",
        "scds/rec-20261119/day4*",
        "scds/rec-20261119/runs*",
        "scds/stdds-20261119-holdout*",
        "scds/stdds-smoke-20261119*",
    ]
    .iter()
    .map(|t| t.to_string())
    .collect();
    assert_eq!(texts, want);
    // The sealed days, the directories they are sealed below and the ones left out.
    assert_eq!(l.days.days, ["2026-11-20", "2026-11-21", "2026-11-22"]);
    let t = |v: &[(String, Vec<String>)]| v.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
    assert_eq!(t(&l.days.scopes), ["raw", "scds", "derived"]);
    assert_eq!(t(&l.days.exempt), ["raw/rap", "raw/awc"]);
    let w: Vec<&str> = l.windows.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(w, ["2026-11-20T00:00:00Z 2026-11-23T00:17:00Z"]);
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/sealed_guard.py");
    // seal-allow: this launches the parser on its synthetic list dump and supplies no data path.
    let out = Command::new("python3")
        .arg("-I")
        .arg(&py)
        .arg("--dump")
        .output()
        .expect("python3 is needed to compare the two parsers");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), l.dump());
}

// REQ: OA-SEAL-001
#[test]
fn a_list_that_does_not_parse_is_an_error_and_the_guard_then_refuses_everything() {
    for bad in [
        "",
        "root /d\n",
        "seal scds/x\n",
        "root /d\nseal scds/../x\n",
        "root /d\nroot /e\nseal a\n",
        "root d\nseal a\n",
        "root /d\nseal *\n",
        "root /d\nfoo bar\n",
        "root $OTHER_ROOT\nseal a\n",
        "root /d\nseal\n",
        "root /d\nseal **\n",
        "root /d\nseal a/*x\n",
        "root /d\nseal a*/b\n",
        "root /d\nseal a/b*c\n",
        "root /d\nseal a\nwindow 2026-11-20T00:00:00Z\n",
        "root /d\nseal a\nwindow 2026-11-20T00:00:00Z 2026-11-20T00:00:00Z\n",
        "root /d\nseal a\nwindow 2026-11-21T00:00:00Z 2026-11-20T00:00:00Z\n",
        "root /d\nseal a\nwindow 2026-11-20 2026-11-21\n",
        "root /d\nseal a\nwindow 2026-13-20T00:00:00Z 2026-14-01T00:00:00Z\n",
        "root /d\nseal a\nwindow 2026-02-29T00:00:00Z 2026-03-01T00:00:00Z\n",
        "root /d\nseal a\nwindow 2026-11-20T00:00:00Z  2026-11-21T00:00:00Z\n",
    ] {
        assert!(parse_list(bad).is_err(), "{bad:?}");
    }
    let g = Sealed::with_list("garbage", None);
    let e = g.check(Path::new("/tmp/anything")).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
}

#[test]
fn a_sealed_data_path_is_refused_below_an_independently_resolved_root() {
    let repository_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let default_data_root = repository_parent.join("openatca-data");
    let root = std::env::var_os("OPENATCA_DATA")
        .map(PathBuf::from)
        .unwrap_or(default_data_root);
    let guard = Sealed::production();
    let synthetic = root.join("derived/public/2026.11.20.synthetic.oaf");
    assert!(guard.check(&synthetic).is_err());
}

#[test]
fn an_invalid_data_root_fails_closed() {
    let file_root = scratch("file-data-root").join("ordinary-file");
    fs::write(&file_root, b"synthetic").unwrap();
    let guard = Sealed::with_list(LIST, Some(&file_root));
    let refusal = guard
        .check(Path::new("/tmp/ordinary.synthetic"))
        .unwrap_err();
    assert!(refusal.why.contains("OA-SEAL-001"));
}

// REQ: OA-SEAL-002
#[test]
fn sealed_names_are_refused_as_given_with_the_rule_named_and_near_misses_are_not() {
    let g = Sealed::with_root(&fake_root("names"));
    for p in [
        "x/scds/rec-20261119/day2",
        "/data/scds/rec-20261119/day2/sub/file.oaf",
        "/data/scds/rec-20261119/day2x",
        "/data/scds/rec-20261119/day3.partial",
        "/data/scds/rec-20261119/runs.log",
        "/data/scds/stdds-20261119-holdout/a",
        "/data/scds/stdds-smoke-20261119T192936Z/b.bin",
        "/data/scds/stdds-smoke-20261119",
    ] {
        let e = g.check(Path::new(p)).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001") && e.contains(p), "{e}");
    }
    for p in [
        "/data/scds/day-20261114/rec/sfdps",
        "/data/scds/rec-20261119x",
        "/data/scds/rec-20261119/day1/x",
        "/data/rec-20261119",
        "/data/scds/x/rec-20261119",
        "/data/scds/stdds-smoke-20261118T1",
        "/data/scds/stdds-bin",
    ] {
        assert!(g.check(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-SEAL-010, OA-SEAL-002
#[test]
fn every_sealed_adsb_day_in_both_name_forms_and_derived_clips_and_the_stdds_prefix_are_sealed() {
    let g = Sealed::with_root(&fake_root("adsb"));
    for p in [
        "/data/raw/adsblol/2026.11.20",
        "/data/raw/adsblol/2026.11.21/v2026.11.21-planes-readsb-prod-0.tar.aa",
        "/data/raw/adsblol/2026.11.22/x.oaf",
        "/data/raw/adsblol/v2026.11.20-x.tar.aa",
        "/data/raw/adsblol/v2026.11.22-planes-readsb-prod-0.tar.aa",
        "/data/raw/adsblol/2026.11.21.bak",
        "/data/derived/adsb/zau-2026.11.21-1s.oaf",
        "/data/derived/zau-2026.11.20.oaf",
        "/data/derived/adsb/2026.11.22/zau.oaf",
        "/data/scds/stdds-20261119-holdout.tar",
        "/data/scds/stdds-20261119-holdout.bak",
        "/data/scds/stdds-20261119-holdout-copy/x",
        // Any spelling of a sealed day, as a substring, at any depth below the three directories.
        "/data/raw/adsblol/dev/2026.11.21/x.oaf",
        "/data/raw/adsblol/zau-2026.11.20.tar",
        "/data/raw/adsblol/zau-2026-11-21.tar",
        "/data/raw/adsblol/zau_2026_11_22.tar",
        "/data/scds/day-20261120",
        "/data/scds/rec-20261120",
        "/data/scds/a/b/c/2026-11-22",
        "/data/derived/zau-20261121.oaf",
    ] {
        let e = g.check(Path::new(p)).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001") && e.contains(p), "{e}");
    }
    for p in [
        "/data/raw/adsblol/2026.11.24",
        "/data/raw/adsblol/2026.11.19/x.oaf",
        "/data/raw/adsblol/v2026.11.24-planes-readsb-prod-0.tar.aa",
        "/data/raw/adsblol",
        "/data/derived",
        "/data/derived/adsb",
        "/data/derived/adsb/zau-2026.11.14.oaf",
        "/data/derived/adsb/zau-2026.11.2.oaf",
        "/data/scds/stdds-20261119",
        "/data/scds/stdds-2026111",
        "/data/scds/day-20261124",
        "/data/scds/rec-20261119/day1",
        "/data/raw/adsblol/2026.11.14",
        // Case-sensitive, and the weather inputs are not scored data.
        "/data/raw/rap/rap.20261120.t00z.grib2",
        "/data/raw/awc/2026-11-21.json",
    ] {
        assert!(g.check(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-SEAL-010
#[test]
fn a_sealed_day_is_matched_on_normalised_components_split_components_and_every_raw_feed() {
    let g = Sealed::with_root(&fake_root("daynames"));
    for p in [
        // The four spellings, normalised.
        "/data/raw/sfdps/2026.11.20/a.xml",
        "/data/raw/stdds/2026-11-21.oaf",
        "/data/raw/tfms/x_2026_11_22_y",
        "/data/raw/tbfm/20261120.bin",
        // Mixed separators in one name.
        "/data/derived/zau-2026.11_20.oaf",
        // The consecutive components year / month / day, and year-month / day.
        "/data/raw/sfdps/2026/11/20/a.xml",
        "/data/scds/2026/11/22",
        "/data/derived/2026-11/21/x",
        "/data/derived/202611/21/x",
        // Fails closed on a longer number that holds the compact day, and on an epoch with it.
        "/data/derived/120261120",
        "/data/scds/1720261120.bin",
        "/data/raw/adsblol/120261121.tar",
        // A copy of the data under another root.
        "/elsewhere/copy/raw/sfdps/2026.11.21/a",
    ] {
        let e = g.check(Path::new(p)).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001") && e.contains(p), "{e}");
    }
    for p in [
        // The weather inputs, under raw, stay open.
        "/data/raw/rap/2026/11/20/x.grib2",
        "/data/raw/awc/20261122.json",
        // Other days, and a month and day that are not consecutive components of the year.
        "/data/raw/sfdps/2026.11.19/a.xml",
        "/data/raw/sfdps/2026/11/19",
        "/data/raw/sfdps/2026/12/20",
        "/data/raw/sfdps/2026/20/11",
        "/data/derived/2026-11/19/x",
        // Not below a scope directory.
        "/data/other/2026.11.20/a",
        "/data/2026.11.20",
        // Day-first, worded and ordinal forms are not matched (the window rule covers rows).
        "/data/raw/sfdps/20.11.2026/a",
        "/data/raw/sfdps/20-nov-2026/a",
    ] {
        assert!(g.check(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-SEAL-011, OA-SEAL-001
#[test]
fn the_window_is_half_open_and_both_parsers_read_it_the_same() {
    let l = parse_list(LIST).unwrap();
    let w = &l.windows[0];
    assert_eq!(
        (w.from_ns, w.to_ns),
        // The listed 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z, widened by 15 minutes.
        (
            1_795_132_800_000_000_000 - 900_000_000_000,
            1_795_393_020_000_000_000 + 900_000_000_000
        )
    );
    assert_eq!(oa_sealed::WINDOW_MARGIN_NS, 900_000_000_000);
    assert_eq!(
        l.windows[0].text,
        "2026-11-20T00:00:00Z 2026-11-23T00:17:00Z"
    );
    let g = Sealed::with_root(&fake_root("window"));
    for (t, sealed) in [
        (w.from_ns - 1, false),
        (w.from_ns, true),
        (w.to_ns - 1, true),
        (w.to_ns, false),
        (0, false),
        (i64::MIN, false),
    ] {
        assert_eq!(g.in_window(t), sealed, "{t}");
        assert_eq!(oa_sealed::in_sealed_window(t), sealed, "{t}");
    }
    // A list that does not parse seals every instant.
    assert!(Sealed::with_list("garbage", None).in_window(0));
    // Two windows and a leap day.
    let two = "root /d\nseal a\nwindow 2024-02-29T00:00:00Z 2024-03-01T00:00:00Z\nwindow 1970-01-01T00:00:01Z 1970-01-01T00:00:02Z\n";
    let l2 = parse_list(two).unwrap();
    assert_eq!(
        l2.windows[0].from_ns,
        1_709_164_800_000_000_000 - 900_000_000_000
    );
    assert_eq!(l2.windows[1].from_ns, 1_000_000_000 - 900_000_000_000);
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/sealed_guard.py");
    let f = scratch("window-parity").join("two.txt");
    fs::write(&f, two).unwrap();
    // seal-allow: this parses a temporary synthetic window list, not an input dataset.
    let out = Command::new("python3")
        .arg("-I")
        .arg(&py)
        .arg("--parse-list")
        .arg(&f)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), l2.dump());
}

// REQ: OA-SEAL-012, OA-SEAL-003
#[test]
fn walk_unsealed_skips_and_counts_sealed_entries_and_never_descends_into_them() {
    let root = fake_root("walkhelper");
    let g = Sealed::with_root(&root);
    let adsb = root.join("raw/adsblol");
    for d in ["2026.11.19", "2026.11.20", "2026.11.24"] {
        fs::create_dir_all(adsb.join(d)).unwrap();
        fs::write(adsb.join(d).join("a.oaf"), b"x").unwrap();
    }
    fs::write(adsb.join("v2026.11.21-x.tar.aa"), b"x").unwrap();
    fs::write(adsb.join("v2026.11.23-x.tar.aa"), b"x").unwrap();
    let w = g.walk_unsealed(&adsb).unwrap();
    let names: Vec<_> = w
        .files
        .iter()
        .map(|p| {
            p.strip_prefix(&adsb)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "2026.11.19/a.oaf",
            "2026.11.24/a.oaf",
            "v2026.11.23-x.tar.aa"
        ]
    );
    assert_eq!(w.skipped, 2);
    // A link to a sealed file or folder is skipped as well.
    symlink(root.join("scds/rec-20261119/day2"), adsb.join("2026.11.30")).unwrap();
    symlink(adsb.join("2026.11.20/a.oaf"), adsb.join("2026.11.31")).unwrap();
    let w = g.walk_unsealed(&adsb).unwrap();
    assert_eq!((w.files.len(), w.skipped), (3, 4));
    // A sealed root is an error; a file as the root is the only file.
    assert!(g.walk_unsealed(&adsb.join("2026.11.20")).is_err());
    // The data root holds sealed entries: it is walked, and they are skipped and counted.
    let all = g.walk_unsealed(&root).unwrap();
    assert!(all.skipped >= 6, "{}", all.skipped);
    assert!(all.files.iter().all(|f| g.check(f).is_ok()));
    assert!(all
        .files
        .iter()
        .any(|f| f.ends_with("scds/day-20261118/rec/b.bin")));
    let one = g.walk_unsealed(&adsb.join("2026.11.24/a.oaf")).unwrap();
    assert_eq!((one.files.len(), one.skipped), (1, 0));
}

// REQ: OA-SEAL-014
#[test]
fn the_drop_counter_adds_up_and_the_line_has_the_fixed_text() {
    let before = oa_sealed::window_dropped_total();
    oa_sealed::note_window_drops(3);
    oa_sealed::note_window_drops(0);
    assert!(oa_sealed::window_dropped_total() >= before + 3);
    assert!(oa_sealed::window_drop_line().starts_with("sealed-window rows dropped: "));
}

// REQ: OA-SEAL-012, OA-SEAL-003
#[test]
fn read_and_read_to_string_go_through_the_opener() {
    let dir = scratch("readers");
    fs::write(dir.join("a.txt"), "hello").unwrap();
    fs::write(dir.join("b.bin"), [0xff, 0xfe]).unwrap();
    assert_eq!(oa_sealed::read(dir.join("a.txt")).unwrap(), b"hello");
    assert_eq!(
        oa_sealed::read_to_string(dir.join("a.txt")).unwrap(),
        "hello"
    );
    assert!(oa_sealed::read_to_string(dir.join("b.bin")).is_err());
    assert!(oa_sealed::read(dir.join("missing")).is_err());
    let sealed = dir.join("raw/adsblol/2026.11.20");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("x"), "s").unwrap();
    let e = oa_sealed::read(sealed.join("x")).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
    assert!(oa_sealed::read_to_string(sealed.join("x")).is_err());
}

// REQ: OA-SEAL-009, OA-SEAL-001, OA-SEAL-002
#[test]
fn only_days_two_to_four_and_the_run_log_of_the_recording_are_sealed() {
    let root = fake_root("tune");
    let g = Sealed::with_root(&root);
    let rec = root.join("scds/rec-20261119");
    fs::write(rec.join("day1/x"), b"x").unwrap();
    fs::create_dir_all(rec.join("day3")).unwrap();
    fs::create_dir_all(rec.join("day4")).unwrap();
    fs::write(rec.join("bin/run-days.sh"), b"x").unwrap();
    for p in ["day1/x", "bin/run-days.sh", "bin", "day1", "day1x", "day5"] {
        assert!(g.check(&rec.join(p)).is_ok(), "{p}");
    }
    // The folder itself holds sealed entries, so it is refused as a holding directory (as
    // `scds` is): a tool names day1 or bin directly.
    let e = g.check(&rec).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001") && e.contains("holds"), "{e}");
    for p in [
        "day2/x",
        "day3",
        "day3/x",
        "day4/x",
        "runs.log",
        "runs.log.1",
        "day2.tmp",
        "day30",
        "day1/../day2/x",
    ] {
        let e = g.check(&rec.join(p)).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001"), "{p} {e}");
    }
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    // A link inside day1 into day2, and a link named day1 elsewhere to day2.
    symlink(rec.join("day2/sfdps/a.bin"), rec.join("day1/link")).unwrap();
    assert!(g.check(&rec.join("day1/link")).is_err());
    symlink(rec.join("day2"), work.join("day1")).unwrap();
    assert!(g.check(&work.join("day1")).is_err());
    assert!(g.check(&work.join("day1/sfdps/a.bin")).is_err());
    // A copy under another root: the names decide.
    assert!(g
        .check(Path::new("/elsewhere/scds/rec-20261119/day2/x"))
        .is_err());
    assert!(g
        .check(Path::new("/elsewhere/scds/rec-20261119/runs.log"))
        .is_err());
    assert!(g
        .check(Path::new("/elsewhere/scds/rec-20261119/day1/x"))
        .is_ok());
}

// REQ: OA-SEAL-002
#[test]
fn a_path_is_resolved_before_it_is_checked() {
    let root = fake_root("resolve");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    let sealed = root.join("scds/rec-20261119/day2");
    // A link to the folder, to a file in it, a link to a link, and a symlinked parent directory.
    symlink(&sealed, work.join("dir")).unwrap();
    symlink(sealed.join("sfdps/a.bin"), work.join("file")).unwrap();
    symlink(work.join("dir"), work.join("chain")).unwrap();
    for p in [
        work.join("dir"),
        work.join("dir/sfdps/a.bin"),
        work.join("file"),
        work.join("chain/sfdps"),
        // `..` that leads into the folder, and a path through a dir that is not there yet.
        root.join("scds/day-20261118/../rec-20261119/day2/sfdps"),
        work.join("dir/new-output"),
        work.join("dir/not/yet/here"),
        root.join("scds/day-20261118/nothing/../../rec-20261119/day2"),
    ] {
        let e = g.check(&p).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001"), "{} {e}", p.display());
    }
    // A relative path (from the working directory) is resolved too.
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count());
    let rel = PathBuf::from(up).join(sealed.strip_prefix("/").unwrap());
    assert!(g.check(&rel).is_err());
    // A link to development data, and plain unsealed paths, are fine.
    symlink(root.join("scds/day-20261118"), work.join("dev")).unwrap();
    assert!(g.check(&work.join("dev/rec/b.bin")).is_ok());
    assert!(g.check(&work.join("new-file")).is_ok());
    assert!(g.check(&root.join("scds/day-20261118/rec/b.bin")).is_ok());
}

// REQ: OA-SEAL-002
#[test]
fn a_link_that_cannot_be_resolved_is_refused() {
    let root = fake_root("loop");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    symlink(work.join("b"), work.join("a")).unwrap();
    symlink(work.join("a"), work.join("b")).unwrap();
    assert!(g.check(&work.join("a")).is_err());
    // A dangling link whose target lies in a sealed folder (not yet created) is refused too.
    symlink(
        root.join("scds/stdds-20261119-holdout/later"),
        work.join("dangling"),
    )
    .unwrap();
    assert!(g.check(&work.join("dangling")).is_err());
    // A dangling link elsewhere is not a sealed path.
    symlink(work.join("nowhere"), work.join("plain")).unwrap();
    assert!(g.check(&work.join("plain")).is_ok());
}

// REQ: OA-SEAL-002
#[test]
fn a_directory_that_holds_a_sealed_folder_is_refused_and_one_below_it_is_not() {
    let root = fake_root("anc");
    let g = Sealed::with_root(&root);
    assert!(g.check(&root).is_err());
    assert!(g.check(&root.join("scds")).is_err());
    assert!(g.check(&root.join("scds/day-20261118")).is_ok());
    assert!(g.check(root.parent().unwrap()).is_ok());
    // The same on a root reached through a link.
    let via = root.parent().unwrap().join("via");
    symlink(&root, &via).unwrap();
    assert!(g.check(&via.join("scds/rec-20261119/day2")).is_err());
    assert!(g.check(&via.join("scds/day-20261118/rec")).is_ok());
}

// REQ: OA-SEAL-003
#[test]
fn every_file_a_walk_opens_goes_through_the_opener() {
    let root = fake_root("walk");
    let g = Sealed::with_root(&root);
    // A development directory with an ordinary file and a link to a sealed file: a walker that
    // opens each entry through the guard reads the first and is refused on the second.
    let dev = root.join("scds/day-20261118/rec");
    symlink(
        root.join("scds/rec-20261119/day2/sfdps/a.bin"),
        dev.join("sneaky"),
    )
    .unwrap();
    let mut read = 0;
    let mut refused = 0;
    // seal-allow: enumeration is limited to the synthetic fake_root fixture created above.
    let mut entries: Vec<_> = fs::read_dir(&dev)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        match g.open(&p) {
            Ok(_) => read += 1,
            Err(e) => {
                assert!(e.to_string().contains("OA-SEAL-001"));
                refused += 1;
            }
        }
    }
    assert_eq!((read, refused), (1, 1));
}

// REQ: OA-SEAL-004
#[test]
fn nothing_switches_the_guard_off() {
    let root = fake_root("nooff");
    let g = Sealed::with_root(&root);
    let sealed = root.join("scds/rec-20261119/day2/sfdps/a.bin");
    // The only supported environment read selects the data root; it cannot disable the guard.
    let src = include_str!("../src/lib.rs");
    let allowed_data_root = "std::env::var_os(\"OPENATCA_DATA\")";
    assert_eq!(src.matches(allowed_data_root).count(), 1);
    let src = src.replace(allowed_data_root, "");
    for word in ["env::var", "env::args", "getenv", "OA_SEAL"] {
        assert!(!src.contains(word), "{word}");
    }
    assert!(g.check(&sealed).is_err());
    let py = include_str!("../../../tools/sealed_guard.py");
    let allowed_data_root = "os.environ.get(\"OPENATCA_DATA\")";
    assert_eq!(py.matches(allowed_data_root).count(), 1);
    let py = py.replace(allowed_data_root, "");
    for word in ["os.environ", "getenv", "argparse"] {
        assert!(!py.contains(word), "{word}");
    }
}

// REQ: OA-SEAL-007
#[test]
fn a_file_opened_after_a_swapped_in_link_is_caught_on_the_descriptor() {
    let root = fake_root("toctou");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    let sealed_file = root.join("scds/rec-20261119/day2/sfdps/a.bin");
    symlink(&sealed_file, work.join("late")).unwrap();
    // As if the path check had passed before the link was put there: the file is opened without
    // the check and then verified.
    // seal-allow: open of the synthetic swapped-link fixture is the subject of the TOCTOU test.
    let f = fs::File::open(work.join("late")).unwrap();
    let e = g
        .verify_open(&work.join("fine"), &f)
        .unwrap_err()
        .to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
    // A file that is not sealed passes the same verification.
    // seal-allow: open of the synthetic ordinary fake_root file verifies the unsealed case.
    let ok = fs::File::open(root.join("scds/day-20261118/rec/b.bin")).unwrap();
    g.verify_open(&work.join("fine"), &ok).unwrap();
    // The opener does both steps and refuses the link (the error has the rule and no path).
    let e = g.open(&work.join("late")).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001") && !e.contains("late"), "{e}");
}

// REQ: OA-SEAL-008
#[test]
fn odd_list_files_are_refused_by_both_parsers_and_good_ones_read_the_same() {
    let dir = scratch("parity");
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/sealed_guard.py");
    let cases: Vec<(&str, bool)> = vec![
        ("root /d\nseal a/b\n", true),
        (
            "# a comment ~!@ with odd printable characters\nroot /d\nseal a/b*",
            true,
        ),
        ("root /d\n\n\nseal a\nseal b/c\n", true),
        (
            "root /d\nseal a\nwindow 2026-11-20T00:00:00Z 2026-11-21T00:00:00Z\n",
            true,
        ),
        ("root /d\nseal a\nseal b/*c*\nseal d*\n", true),
        ("root /d\nseal a\nwindow 2026-11-20T00:00:00Z\n", false),
        (
            "root /d\nseal a\nwindow 2026-11-21T00:00:00Z 2026-11-20T00:00:00Z\n",
            false,
        ),
        (
            "root /d\nseal a\nwindow 2026-02-29T00:00:00Z 2026-03-01T00:00:00Z\n",
            false,
        ),
        ("root /d\nseal a/*b\n", false),
        ("root /d\nseal a*/b\n", false),
        ("root /d\nseal a\tb\n", false),
        ("root /d\r\nseal a\n", false),
        ("root  /d\nseal a\n", false),
        ("root /d\nseal a \n", false),
        ("root /d\n seal a\n", false),
        ("root /d\n \nseal a\n", false),
        ("root /d\nseal \u{0}a\n", false),
        ("root /d\nseal a\u{b}b\n", false),
        ("root /d\nseal a\u{c}b\n", false),
        ("root /d\nseal a\u{1c}b\n", false),
        ("root /d\nseal a\u{85}b\n", false),
        ("root /d\nseal \u{e9}\n", false),
        ("root /d\nseal a\u{2028}b\n", false),
        ("root /d\nseal a//b\n", false),
        ("root /d\nseal ../b\n", false),
        ("root d\nseal a\n", false),
        // The day words: a good set, a `dayexempt` alone, and the ways to break them. Both
        // parsers must agree on every one.
        ("root /d\nseal a\nday 2026-11-20\ndayscope raw\n", true),
        ("root /d\nseal a\ndayexempt raw/rap\n", true),
        ("root /d\nseal a\nday 2026-13-20\ndayscope raw\n", false),
        ("root /d\nseal a\nday 2026-11-32\ndayscope raw\n", false),
        ("root /d\nseal a\nday 2026-11-20\ndayscope raw/*\n", false),
        ("root /d\nseal a\nday 2026-11-20\ndayscope /raw\n", false),
        (
            "root /d\nseal a\nday 2026-11-20\ndayscope raw//rap\n",
            false,
        ),
        ("root /d\nseal a\nday 2026-11-20\n", false),
        ("root /d\nseal a\ndayscope raw\n", false),
        ("root /d\nseal a\nday 2026-11-20 \ndayscope raw\n", false),
        ("root /d\nseal a\nday 2026-11-20\ndayscope raw \n", false),
        (
            "root /d\nseal a\nday 2026-11-2\u{660}\ndayscope raw\n",
            false,
        ),
        (
            "root /d\nseal a\nday 2026-1\u{ff11}-20\ndayscope raw\n",
            false,
        ),
    ];
    for (i, (text, good)) in cases.iter().enumerate() {
        let f = dir.join(format!("case{i}.txt"));
        fs::write(&f, text.as_bytes()).unwrap();
        let rust = parse_list(text);
        assert_eq!(rust.is_ok(), *good, "rust {text:?}");
        // seal-allow: parses a synthetic temporary list for Rust/Python parser parity.
        let out = Command::new("python3")
            .arg("-I")
            .arg(&py)
            .arg("--parse-list")
            .arg(&f)
            .output()
            .unwrap();
        assert_eq!(out.status.success(), *good, "python {text:?}");
        if let Ok(l) = rust {
            assert_eq!(String::from_utf8(out.stdout).unwrap(), l.dump(), "{text:?}");
        } else {
            assert_eq!(out.status.code(), Some(2), "{text:?}");
        }
    }
}

// REQ: OA-SEAL-012
#[test]
fn walk_unsealed_counts_links_back_errors_on_depth_and_a_missing_root_and_sorts_bytewise() {
    let root = fake_root("walkloops");
    let g = Sealed::with_root(&root);
    let w = root.join("work");
    fs::create_dir_all(w.join("a")).unwrap();
    fs::write(w.join("a/f.bin"), b"x").unwrap();
    // A self-link: the directory is visited once, the link is skipped and counted.
    symlink(&w, w.join("a/self")).unwrap();
    let r = g.walk_unsealed(&w).unwrap();
    assert_eq!((r.files.len(), r.skipped, r.already_visited), (1, 0, 1));
    // Two self-links.
    symlink(&w, w.join("a/self2")).unwrap();
    let r = g.walk_unsealed(&w).unwrap();
    assert_eq!((r.files.len(), r.skipped, r.already_visited), (1, 0, 2));
    // A sibling link to a directory next to it: the first name in bytewise order is walked, the
    // other is counted as already visited, never as sealed.
    // Eight such pairs, so that a walk in the order the directory lists them (not sorted) would
    // reach `real` first in some pair and show it in the names visited.
    let sib = root.join("sib");
    for k in 0..8 {
        let p = sib.join(format!("p{k}"));
        fs::create_dir_all(p.join("real")).unwrap();
        fs::write(p.join("real/f.bin"), b"x").unwrap();
        fs::create_dir_all(p.join("a")).unwrap();
        symlink("../real", p.join("a/x")).unwrap();
    }
    let r = g.walk_unsealed(&sib).unwrap();
    let want: Vec<PathBuf> = (0..8)
        .map(|k| sib.join(format!("p{k}/a/x/f.bin")))
        .collect();
    assert_eq!(r.files, want);
    assert_eq!((r.skipped, r.already_visited), (0, 8));
    // A tree deeper than the limit is an error, not a silent stop.
    let mut deep = root.join("deep");
    for _ in 0..70 {
        deep = deep.join("d");
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("f"), b"x").unwrap();
    let e = g.walk_unsealed(&root.join("deep")).unwrap_err().to_string();
    assert!(e.contains("deeper"), "{e}");
    // A missing root is an error.
    assert!(g.walk_unsealed(&root.join("nothing-here")).is_err());
    // The order is bytewise on the string form of the path: `a.b` sorts before `a/b` ('.' < '/'),
    // which a component-wise sort would not give.
    let s = root.join("sorted");
    fs::create_dir_all(s.join("a")).unwrap();
    fs::write(s.join("a/b"), b"x").unwrap();
    fs::write(s.join("a.b"), b"x").unwrap();
    let names: Vec<String> = g
        .walk_unsealed(&s)
        .unwrap()
        .files
        .iter()
        .map(|p| p.strip_prefix(&s).unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.b", "a/b"]);
}

// REQ: OA-SEAL-002, OA-SEAL-007
#[test]
fn a_file_with_a_second_hard_link_is_refused_when_opened() {
    let root = fake_root("hardlink");
    let g = Sealed::with_root(&root);
    let sealed = root.join("scds/rec-20261119/day2/sfdps/a.bin");
    let neutral = root.join("derived-copy.bin");
    // seal-allow: creates a hard link only within the synthetic fake_root fixture.
    fs::hard_link(&sealed, &neutral).unwrap();
    let e = g.open(&neutral).unwrap_err().to_string();
    assert!(e.contains("hard links"), "{e}");
    // The branch that refuses a file whose metadata cannot be read has no test: `fstat` on a
    // descriptor this process just opened does not fail on Linux (the descriptor is valid and the
    // file is on a filesystem that answers), and a test would need a fake filesystem or an
    // injected failure, which would be code only the test runs. The branch is a plain
    // `map_err` that returns the refusal; it was read, not run.
    // A file with one name opens.
    let one = root.join("one.bin");
    fs::write(&one, b"x").unwrap();
    assert!(g.open(&one).is_ok());
}
