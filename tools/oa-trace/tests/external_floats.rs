//! Every float read from outside the program is checked where it is parsed (OA-TRC-103).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oa_trace::floatscan::{scan, Allow};
use std::path::Path;

const ALLOW: &[Allow] = &[
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"parse::<f64>\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"parse::<f32>\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f64::from_str\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f32::from_str\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f64::from_bits\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f32::from_bits\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f64::from_le_bytes\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f32::from_le_bytes\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f64::from_be_bytes\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f32::from_be_bytes\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f64::from_ne_bytes\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-trace/src/floatscan.rs",
        needle: "\"f32::from_ne_bytes\"",
        reason: "scanner pattern literal describing a raw-read token, not a float conversion",
    },
    Allow {
        file: "tools/oa-regional-session/src/surveillance.rs",
        needle: "f32::from_bits(",
        reason: "successor-f32 rounding of a checked nonnegative i64 timestamp delta divided by 1e9; the delta is bounded before this conversion and cannot approach infinity",
    },
    Allow {
        file: "tools/oa-msaw/src/run.rs",
        needle: "x.parse::<f64>().ok()",
        reason: "worst-position report coordinates are rejected unless finite and within latitude/longitude bounds before scaling or integer conversion; invalid rows are counted",
    },
    Allow {
        file: "tools/oa-ca/src/config.rs",
        needle: ".parse()",
        reason: "configuration numeric values pass through finite(), which rejects NaN and infinities before returning them",
    },
    Allow {
        file: "tools/oa-legacy-traj/src/shell/places.rs",
        needle: "lat.parse()",
        reason: "latitude and longitude are range-checked immediately after parsing; NaN and infinities fail the inclusive coordinate bounds",
    },
    Allow {
        file: "tools/oa-legacy-traj/src/shell/places.rs",
        needle: "lon.parse()",
        reason: "latitude and longitude are range-checked immediately after parsing; NaN and infinities fail the inclusive coordinate bounds",
    },
    Allow {
        file: "tools/oa-msaw/src/args.rs",
        needle: ".parse()",
        reason: "duration values pass through seconds(), which rejects nonfinite and negative values before use",
    },
    Allow {
        file: "tools/oa-msaw/src/config.rs",
        needle: ".parse()",
        reason: "configuration numeric values pass through finite(), which rejects NaN and infinities before returning them",
    },
    Allow {
        file: "tools/oa-msaw/src/run.rs",
        needle: "f.get(11)?.parse().ok()?",
        reason: "reads the current process's own integer CPU tick fields from /proc/self/stat; the parsed sum is checked finite before conversion to seconds",
    },
    Allow {
        file: "tools/oa-perf/src/stats.rs",
        needle: "self.get(k)?.parse().ok()",
        reason: "reads this tool's own generated KV report; `nan` is an intentional missing-value sentinel consumed by fmt_f, not an outside measurement",
    },
    Allow {
        file: "tools/oa-stdds-census/src/main.rs",
        needle: "v.parse::<f64>().ok()",
        reason: "the shared option parser rejects nonfinite values; each option then has an explicit domain bound (gate and time-window validation, or 0 < max-gap <= 30)",
    },
    Allow {
        file: "tools/oa-legacy-traj/src/shell/args.rs",
        needle: "v.parse::<f64>().ok()",
        reason: "shared CLI number parser rejects nonfinite and nonpositive values; landing-gap and step additionally require whole seconds before integer conversion",
    },
    Allow {
        file: "crates/oa-adsb/src/cli.rs",
        needle: "x.trim().parse().ok()",
        reason: "--box: every value is range-checked next (a NaN or infinity is outside, the comparison is false) and a token that fails to parse leaves the count short, which is refused",
    },
    Allow {
        file: "tools/oa-adapt/src/cli.rs",
        needle: ".parse()",
        reason: "--buffer-nm: the product in metres is checked finite and within 0 to the limit on the next lines",
    },
    Allow {
        file: "crates/oa-awc/src/hazard.rs",
        needle: "s.parse::<f64>().ok()",
        reason: "the value is checked finite and range-checked by deg_e5 on the lines after it",
    },
    Allow {
        file: "crates/oa-awc/src/hazard.rs",
        needle: ".parse::<f64>()",
        reason: "ICAO position digits: the text is ASCII digits only (checked above), so the value is finite",
    },
    Allow {
        file: "crates/oa-awc/src/units.rs",
        needle: "body.parse().ok()?",
        reason: "checked finite and within 0 to 100 on the next line",
    },
    Allow {
        file: "crates/oa-bench/src/main.rs",
        needle: "v.parse::<f64>()",
        reason: "the guard arm accepts only a finite value above zero",
    },
    Allow {
        file: "crates/oa-bench/src/metrics_th.rs",
        needle: "f64::from(f32::from_bits(b))",
        reason: "not an outside read: it decodes the order-preserving sort key built a few lines above from a heading already checked finite, so the bits are that heading's",
    },
    Allow {
        file: "crates/oa-bench/src/scan.rs",
        needle: "it.next()?.parse().ok()?",
        reason: "latitude and longitude are range-checked on the next line (a NaN or infinity is outside)",
    },
    Allow {
        file: "crates/oa-bench/src/stats.rs",
        needle: "f64::from_bits(",
        reason: "bit-twiddling of an internal measurement to find its power of two; not an external value",
    },
    Allow {
        file: "crates/oa-display/src/shell/cli.rs",
        needle: "text.parse::<f64>()",
        reason: "the crate's checked helper parse_finite for command-line numbers",
    },
    Allow {
        file: "crates/oa-scope-replay/src/shell/cli.rs",
        needle: "text.parse::<f64>()",
        reason: "the crate's checked helper parse_finite for command-line numbers",
    },
    Allow {
        file: "crates/oa-grib2/src/bits.rs",
        needle: "map(f32::from_bits)",
        reason: "the only caller, the data representation section, refuses a non-finite reference value",
    },
    Allow {
        file: "crates/oa-json/src/lib.rs",
        needle: "self.number_text(n)?.parse().ok()?",
        reason: "as_f64 returns None for a non-finite value on the next line",
    },
    Allow {
        file: "crates/oa-math/src/nan.rs",
        needle: "f32::from_bits(0x7fc0_0000)",
        reason: "the one fixed single-precision NaN, a constant, not read from outside the program",
    },
    Allow {
        file: "crates/oa-math/src/",
        needle: "f64::from_bits(",
        reason: "the elementary functions' own bit-level arithmetic on values they computed or on internal \
                 tables, never on external input; oa-math reads nothing from outside the program",
    },
    Allow {
        file: "crates/oa-nasr/src/row.rs",
        needle: "t.parse::<f64>()",
        reason: "the guard arm accepts only a finite value, after the decimal shape check",
    },
    Allow {
        file: "crates/oa-sensorsim/src/main.rs",
        needle: "v.parse::<f64>()",
        reason: "the guard arm accepts only a finite value",
    },
    Allow {
        file: "crates/oa-tiff/src/tiff.rs",
        needle: "let v = f64::from_bits(",
        reason: "f64_at, the checked reader for tag doubles: a non-finite value is an error",
    },
    Allow {
        file: "crates/oa-tiff/src/tiff.rs",
        needle: "text.parse::<f64>().ok().filter",
        reason: "the crate's checked text helper",
    },
    Allow {
        file: "crates/oa-tiff/src/tiff.rs",
        needle: "text.parse::<f32>().ok().filter",
        reason: "the crate's checked text helper",
    },
    Allow {
        file: "crates/oa-tiff/src/tiff.rs",
        needle: "out.push(f32::from_bits(",
        reason: "raster samples: a non-finite sample is turned into NaN nodata and counted right after decoding",
    },
    Allow {
        file: "crates/oa-track/src/spill.rs",
        needle: "f32::from_le_bytes",
        reason: "bit-exact round trip of a value this program wrote to its own spill file (NaN marks an absent altitude)",
    },
    Allow {
        file: "crates/oa-truth/src/bin/oa-truth-check.rs",
        needle: "v?.parse().ok()?",
        reason: "finite_positive, the checked reader for the tuning options",
    },
    Allow {
        file: "crates/oa-wind/src/bin/oa-wind-check/adsb_mode.rs",
        needle: "v.parse::<f64>().ok()",
        reason: "the guard arm accepts only a finite value of 0 or more",
    },
    Allow {
        file: "tools/oa-compass/src/shell/radar.rs",
        needle: ".parse::<f64>()",
        reason: "the CLI finite-only parser rejects parse errors and non-finite radar site coordinates/height",
    },
];

// REQ: OA-TRC-103
#[test]
fn no_raw_float_read_outside_a_checked_helper_or_the_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (found, stale) = scan(&root, ALLOW);
    for f in &found {
        eprintln!("{}:{}: {}", f.file, f.line, f.text);
    }
    assert!(
        found.is_empty(),
        "{} raw float reads: parse through the crate's checked helper, or allowlist with a reason",
        found.len()
    );
    assert!(
        stale.is_empty(),
        "allowlist entries match nothing: {stale:?}"
    );
    for a in ALLOW {
        assert!(!a.reason.trim().is_empty(), "{} has no reason", a.file);
    }
}
