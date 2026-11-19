#![no_main]
use libfuzzer_sys::fuzz_target;

// Raw bytes into the NAM ACC-to-ACC codec: the transport frame, the strict and the tolerant
// message parser. Anything accepted must encode again, and the encoding must parse back to the
// same message. A refusal is fine; a panic or a failed round trip is a finding.
fuzz_target!(|data: &[u8]| {
    let _ = oa_nam::frame::read_header(data);
    let _ = oa_nam::frame::unframe(data);
    if let Ok(m) = oa_nam::parse(data) {
        let enc = oa_nam::encode(&m).expect("a parsed message encodes");
        assert_eq!(enc, data, "parse then encode is byte-exact");
        assert_eq!(oa_nam::parse(&enc).as_ref(), Ok(&m));
    }
    if let Ok(m) = oa_nam::parse_with(data, oa_nam::Profile::Core) {
        let enc = oa_nam::encode_with(&m, oa_nam::Profile::Core).expect("a Core message encodes");
        assert_eq!(enc, data, "parse then encode is byte-exact under Core");
    }
    if let Ok((m, _)) = oa_nam::parse_tolerant(data) {
        let enc = oa_nam::encode(&m).expect("a tolerantly parsed message encodes");
        assert_eq!(oa_nam::parse(&enc).as_ref(), Ok(&m));
    }
});
