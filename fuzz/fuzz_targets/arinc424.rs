#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_arinc424::{ProcedureKind, RECORD_LEN};

// The whole-file parser (on the bytes and through a small buffer) and the per-record decoders
// on each line, also padded or cut to the record length.
fuzz_target!(|data: &[u8]| {
    if let Ok(c) = oa_arinc424::parse_bytes(data, "fuzz") {
        let _ = (oa_arinc424::is_clean(&c), oa_arinc424::cifp_cycle(&c));
        for a in &c.airports {
            let _ = oa_arinc424::find_airport(&c, &a.ident);
            for r in oa_arinc424::runways_at(&c, &a.ident) {
                let _ = oa_arinc424::runway_designator(r);
            }
            for k in [
                ProcedureKind::Sid,
                ProcedureKind::Star,
                ProcedureKind::Approach,
            ] {
                for p in oa_arinc424::procedures_at(&c, &a.ident, k) {
                    for rt in &p.routes {
                        for l in &rt.legs {
                            for lc in &l.continuations {
                                let _ = oa_arinc424::levels_of_service(lc);
                            }
                        }
                    }
                }
            }
        }
        for s in c.controlled_airspace.iter().chain(&c.special_use_airspace) {
            let _ = (
                oa_arinc424::is_closed(s),
                oa_arinc424::controlling_agency(s),
            );
        }
        for m in &c.mora {
            let _ = oa_arinc424::mora_value_at(m, 30.0, -90.0);
        }
        let _ = oa_arinc424::mora_at(&c, 30.0, -90.0);
        let _ = c.errors.iter().map(|e| e.to_string()).count();
    }
    let _ = oa_arinc424::parse_reader(std::io::BufReader::with_capacity(7, data), "fuzz");
    for (n, line) in data.split(|&b| b == b'\n').take(64).enumerate() {
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        // Record lengths are bytes; a truncated UTF-8 prefix need not be valid text.
        let mut padded = line.to_vec();
        padded.resize(RECORD_LEN, b' ');
        for t in [Some(text), std::str::from_utf8(&padded).ok()]
            .into_iter()
            .flatten()
        {
            if let Ok(r) = oa_arinc424::parse_record(n, t) {
                let _ = oa_arinc424::record_meta(&r);
            }
        }
        let _ = (
            oa_arinc424::parse_lat(text),
            oa_arinc424::parse_lon(text),
            oa_arinc424::parse_path_terminator(text),
            oa_arinc424::parse_cycle(text),
        );
    }
});
