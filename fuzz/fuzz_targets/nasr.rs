#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_nasr::procedure::ProcedureKind;
use oa_nasr::{apt, arb, awy, cls, fix, nav, procedure, schema};

// Touches every field of the parsed rows through their Debug text and the rejected rows through
// Display.
fn touch<T: std::fmt::Debug>(r: Result<oa_nasr::Parsed<T>, oa_nasr::NasrError>) {
    match r {
        Ok(p) => {
            for x in &p.records {
                let _ = format!("{x:?}");
            }
            for e in &p.rejected {
                let _ = format!("{e:?}");
            }
            let _ = p.rejected_dropped;
        }
        Err(e) => {
            let _ = e.to_string();
        }
    }
}

// Form feed separates the files of a parser that takes several; none occurs in a NASR file.
fn parts(body: &[u8]) -> [&[u8]; 3] {
    let mut it = body.splitn(3, |&b| b == 0x0c);
    [(); 3].map(|()| it.next().unwrap_or(&[]))
}

// The first byte picks the NASR file parser (bit 7 the STAR or DP family); the rest is the CSV
// file, or the files separated by form feeds for the parsers that read more than one.
fuzz_target!(|data: &[u8]| {
    let Some((&sel, body)) = data.split_first() else {
        return;
    };
    let s = "fuzz.csv";
    let kind = if sel & 0x80 == 0 {
        ProcedureKind::Star
    } else {
        ProcedureKind::Dp
    };
    match sel & 0x7f {
        0 => touch(apt::parse_airports(s, body)),
        1 => touch(apt::parse_runways(s, body)),
        2 => touch(apt::parse_runway_ends(s, body)),
        3 => touch(fix::parse_fixes(s, body)),
        4 => touch(fix::parse_fix_navs(s, body)),
        5 => touch(nav::parse_navaids(s, body)),
        6 => touch(awy::parse_airway_bases(s, body)),
        7 => touch(awy::parse_airway_segments(s, body)),
        8 => {
            let [a, b, _] = parts(body);
            touch(awy::parse_airways(s, a, s, b));
        }
        9 => touch(arb::parse_base(s, body)),
        10 => touch(arb::parse_boundaries(s, body)),
        11 => touch(cls::parse_class_airspace(s, body)),
        12 => {
            if let Ok(p) = procedure::parse_bases(kind, s, body) {
                for b in &p.records {
                    for a in procedure::served_airports(b) {
                        let _ = a.len();
                    }
                }
            }
        }
        13 => touch(procedure::parse_airports(kind, s, body)),
        14 => {
            let [a, b, c] = parts(body);
            if let Ok(p) = procedure::parse_procedures(kind, (s, a), (s, b), (s, c)) {
                for pr in &p.records {
                    for r in &pr.routes {
                        for pt in &r.points {
                            let _ = procedure::associations(pt);
                        }
                    }
                }
                let _ = format!("{:?}", p.records);
            }
        }
        15 => {
            if let Ok(sc) = schema::parse_schemas(s, body) {
                let _ = format!("{sc:?}");
            }
        }
        _ => {
            // A structure file, then a data file, so `validate` runs against a schema that parsed.
            let [head, rest, _] = parts(body);
            if let Ok(sc) = schema::parse_schemas(s, head) {
                for fs in sc.files.values().take(2) {
                    if let Ok(v) = schema::validate(fs, s, rest) {
                        let _ = schema::total_violations(&v);
                        let _ = format!("{v:?}");
                    }
                }
            }
        }
    }
});
