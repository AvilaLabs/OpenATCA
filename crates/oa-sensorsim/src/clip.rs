//! Reads an OAF1 clip (docs/ADSB_PAYLOAD.md) into ADS-B reports. Shell: owns the file.

use std::fs::File;
use std::io::Read;

use oa_adsb::{decode_payload, Reports};
use oa_feed::{FrameReader, Kind, MAX_FRAME_LEN};

/// Every ADS-B payload frame of the file at `path` as one set of reports, and the number of
/// frames that did not decode.
pub fn read_clip(path: &str) -> Result<(Reports, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).map_err(|e| format!("{e:?}"))?;
    let mut out = Reports::default();
    let mut bad = 0u64;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut chunk).map_err(|e| format!("{path}: {e}"))?;
        if n == 0 {
            break;
        }
        let mut at = 0usize;
        while at < n {
            let Some(rest) = chunk.get(at..n) else { break };
            let took = rd.push(rest);
            at += took;
            while let Some(fr) = rd.next() {
                match fr {
                    Ok(fr) if fr.kind == Kind::Message => {
                        if decode_payload(fr.payload, &mut out).is_err() {
                            bad += 1;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => bad += 1,
                }
            }
            if took == 0 {
                break;
            }
        }
    }
    Ok((out, bad))
}
