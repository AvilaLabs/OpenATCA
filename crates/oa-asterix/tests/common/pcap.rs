//! A minimal bounded libpcap reader for the capture tests: Ethernet II, IPv4, UDP and nothing
//! else. Anything else is refused with a typed error (RULESET E5), never skipped silently.
#![allow(dead_code)]

/// Most packets read from one file.
pub const MAX_PACKETS: usize = 100_000;
/// Largest captured packet accepted (the pcap snaplen ceiling of 262 144 octets).
pub const MAX_PACKET_LEN: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcapError {
    /// The file is shorter than the 24-octet global header or ends inside a packet.
    Truncated,
    /// The magic number is none of the four libpcap magics.
    BadMagic,
    /// A version other than 2.x.
    BadVersion,
    /// A link type other than Ethernet (1).
    LinkType(u32),
    /// A captured length above [`MAX_PACKET_LEN`].
    PacketTooLong,
    /// More than [`MAX_PACKETS`] packets.
    TooManyPackets,
    /// The packet was cut by the snaplen (captured below original length).
    PacketCut,
    /// An Ethernet type other than IPv4 (0x0800).
    EtherType(u16),
    /// An IP version other than 4, a header below five words or a total length that does not fit.
    BadIpHeader,
    /// A fragmented datagram.
    Fragmented,
    /// An IP protocol other than UDP (17).
    Protocol(u8),
    /// A UDP length below eight or past the IP payload.
    BadUdp,
}

fn u16_at(b: &[u8], i: usize, le: bool) -> Option<u16> {
    let a: [u8; 2] = b.get(i..i + 2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(a)
    } else {
        u16::from_be_bytes(a)
    })
}

fn u32_at(b: &[u8], i: usize, le: bool) -> Option<u32> {
    let a: [u8; 4] = b.get(i..i + 4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(a)
    } else {
        u32::from_be_bytes(a)
    })
}

/// The UDP payload of one Ethernet frame.
pub fn udp_payload(frame: &[u8]) -> Result<&[u8], PcapError> {
    let et = u16_at(frame, 12, false).ok_or(PcapError::Truncated)?;
    if et != 0x0800 {
        return Err(PcapError::EtherType(et));
    }
    let ip = frame.get(14..).ok_or(PcapError::Truncated)?;
    let v_ihl = *ip.first().ok_or(PcapError::Truncated)?;
    let ihl = usize::from(v_ihl & 15) * 4;
    if v_ihl >> 4 != 4 || ihl < 20 {
        return Err(PcapError::BadIpHeader);
    }
    let total = usize::from(u16_at(ip, 2, false).ok_or(PcapError::Truncated)?);
    if total < ihl + 8 || total > ip.len() {
        return Err(PcapError::BadIpHeader);
    }
    let frag = u16_at(ip, 6, false).ok_or(PcapError::Truncated)?;
    if frag & 0x3FFF != 0 {
        return Err(PcapError::Fragmented);
    }
    let proto = *ip.get(9).ok_or(PcapError::Truncated)?;
    if proto != 17 {
        return Err(PcapError::Protocol(proto));
    }
    let udp = ip.get(ihl..total).ok_or(PcapError::BadIpHeader)?;
    let ulen = usize::from(u16_at(udp, 4, false).ok_or(PcapError::Truncated)?);
    if ulen < 8 || ulen > udp.len() {
        return Err(PcapError::BadUdp);
    }
    udp.get(8..ulen).ok_or(PcapError::BadUdp)
}

/// The UDP payloads of every packet in a libpcap file, in order.
pub fn read_pcap(file: &[u8]) -> Result<Vec<&[u8]>, PcapError> {
    let magic = u32_at(file, 0, false).ok_or(PcapError::Truncated)?;
    let le = match magic {
        0xA1B2_C3D4 | 0xA1B2_3C4D => false,
        0xD4C3_B2A1 | 0x4D3C_B2A1 => true,
        _ => return Err(PcapError::BadMagic),
    };
    if u16_at(file, 4, le).ok_or(PcapError::Truncated)? != 2 {
        return Err(PcapError::BadVersion);
    }
    // The snaplen at 16 is not needed: a packet cut by it has captured below original length.
    u32_at(file, 16, le).ok_or(PcapError::Truncated)?;
    let link = u32_at(file, 20, le).ok_or(PcapError::Truncated)?;
    if link != 1 {
        return Err(PcapError::LinkType(link));
    }
    let mut out = Vec::new();
    let mut pos = 24;
    while pos < file.len() {
        if out.len() >= MAX_PACKETS {
            return Err(PcapError::TooManyPackets);
        }
        let incl = u32_at(file, pos + 8, le).ok_or(PcapError::Truncated)? as usize;
        let orig = u32_at(file, pos + 12, le).ok_or(PcapError::Truncated)? as usize;
        if incl > MAX_PACKET_LEN {
            return Err(PcapError::PacketTooLong);
        }
        if incl < orig {
            return Err(PcapError::PacketCut);
        }
        let frame = file
            .get(pos + 16..pos + 16 + incl)
            .ok_or(PcapError::Truncated)?;
        out.push(udp_payload(frame)?);
        pos += 16 + incl;
    }
    Ok(out)
}
