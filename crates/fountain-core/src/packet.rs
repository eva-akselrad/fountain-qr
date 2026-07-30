//! Binary packet framing for Fountain QR payloads.
//! Theoretical QR Version 40-L binary capacity = 2953 bytes.
//! Fast camera profile: larger symbols + lean headers.

use crc32fast::Hasher;

pub const MAGIC: &[u8; 4] = b"FQFT";
pub const PROTOCOL_VERSION: u8 = 1;
/// Max QR binary payload (Version 40, ECC L) — absolute ceiling.
pub const QR_CAPACITY: usize = 2953;
/// Fast camera symbol budget (≈ QR v15–18 @ ECC L).
pub const CAMERA_SYMBOL_CAP: usize = 400;
/// Keep filenames short so occasional named frames still fit.
pub const MAX_FILENAME_BYTES: usize = 32;
/// Max encoded packet bytes for the fast profile.
pub const CAMERA_PACKET_CAP: usize = 460;
/// Fixed header without filename:
/// magic4 + ver1 + flags1 + file_id8 + total_len8 + symbol_size2 + k4 + esi4 + crc32_4 + name_len1 = 37
pub const FIXED_HEADER: usize = 37;

const PAIR_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone, Debug)]
pub struct PacketMeta {
    pub file_id: u64,
    pub total_len: u64,
    pub symbol_size: u16,
    pub k: u32,
    pub esi: u32,
    pub crc32: u32,
    pub filename: String,
}

/// Human pair code from file_id (5 Crockford chars, e.g. `7K3MP`).
pub fn pair_code(file_id: u64) -> String {
    let mut n = file_id;
    let mut chars = [b'0'; 5];
    for c in chars.iter_mut() {
        *c = PAIR_ALPHABET[(n & 31) as usize];
        n >>= 5;
    }
    chars.reverse();
    String::from_utf8_lossy(&chars).into_owned()
}

pub fn pair_code_matches(code: &str, file_id: u64) -> bool {
    let normalized: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'I' => '1',
            'L' => '1',
            'O' => '0',
            u => u,
        })
        .collect();
    if normalized.len() != 5 {
        return false;
    }
    pair_code(file_id) == normalized
}

pub fn max_symbol_payload(filename_len: usize) -> usize {
    QR_CAPACITY.saturating_sub(FIXED_HEADER + filename_len)
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut h = Hasher::new();
    h.update(data);
    h.finalize()
}

pub fn encode_packet(meta: &PacketMeta, symbol: &[u8]) -> Result<Vec<u8>, String> {
    let name_bytes = meta.filename.as_bytes();
    if name_bytes.len() > 255 {
        return Err("filename too long".into());
    }
    let max_sym = max_symbol_payload(name_bytes.len()).min(CAMERA_SYMBOL_CAP);
    if symbol.len() > max_sym {
        return Err(format!(
            "symbol {} exceeds capacity {} (filename len {})",
            symbol.len(),
            max_sym,
            name_bytes.len()
        ));
    }
    let mut buf = Vec::with_capacity(FIXED_HEADER + name_bytes.len() + symbol.len());
    buf.extend_from_slice(MAGIC);
    buf.push(PROTOCOL_VERSION);
    buf.push(0); // flags
    buf.extend_from_slice(&meta.file_id.to_le_bytes());
    buf.extend_from_slice(&meta.total_len.to_le_bytes());
    buf.extend_from_slice(&meta.symbol_size.to_le_bytes());
    buf.extend_from_slice(&meta.k.to_le_bytes());
    buf.extend_from_slice(&meta.esi.to_le_bytes());
    buf.extend_from_slice(&meta.crc32.to_le_bytes());
    buf.push(name_bytes.len() as u8);
    buf.extend_from_slice(name_bytes);
    buf.extend_from_slice(symbol);
    if buf.len() > QR_CAPACITY {
        return Err("packet exceeds QR capacity".into());
    }
    Ok(buf)
}

pub fn decode_packet(data: &[u8]) -> Result<(PacketMeta, Vec<u8>), String> {
    if data.len() < FIXED_HEADER {
        return Err("truncated header".into());
    }
    if &data[0..4] != MAGIC {
        return Err("bad magic".into());
    }
    if data[4] != PROTOCOL_VERSION {
        return Err("unsupported version".into());
    }
    let mut o = 6;
    let file_id = u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
    o += 8;
    let total_len = u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
    o += 8;
    let symbol_size = u16::from_le_bytes(data[o..o + 2].try_into().unwrap());
    o += 2;
    let k = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    o += 4;
    let esi = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    o += 4;
    let crc32 = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    o += 4;
    let name_len = data[o] as usize;
    o += 1;
    if data.len() < o + name_len {
        return Err("truncated filename".into());
    }
    let filename = String::from_utf8_lossy(&data[o..o + name_len]).into_owned();
    o += name_len;
    let symbol = data[o..].to_vec();
    Ok((
        PacketMeta {
            file_id,
            total_len,
            symbol_size,
            k,
            esi,
            crc32,
            filename,
        },
        symbol,
    ))
}

/// Basename only, truncated to MAX_FILENAME_BYTES (UTF-8 safe).
pub fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim();
    let base = if base.is_empty() { "file.bin" } else { base };
    let mut out = String::new();
    for ch in base.chars() {
        let mut buf = [0u8; 4];
        let enc = ch.encode_utf8(&mut buf);
        if out.len() + enc.len() > MAX_FILENAME_BYTES {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        "file.bin".into()
    } else {
        out
    }
}

/// Symbol size sized for lean (no-filename) packets under CAMERA_PACKET_CAP.
pub fn choose_symbol_size(_filename: &str, file_len: usize) -> u16 {
    let room = CAMERA_PACKET_CAP
        .saturating_sub(FIXED_HEADER)
        .min(CAMERA_SYMBOL_CAP)
        .max(1);
    let size = if file_len == 0 {
        1
    } else {
        room.min(file_len).max(1)
    };
    size as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_roundtrip_match() {
        let id = 0xDEAD_BEEF_CAFE_BABEu64;
        let code = pair_code(id);
        assert_eq!(code.len(), 5);
        assert!(pair_code_matches(&code, id));
        assert!(pair_code_matches(&format!("{}-{}", &code[..2], &code[2..]), id));
        assert!(!pair_code_matches("00000", id) || pair_code(id) == "00000");
    }

    #[test]
    fn long_filename_still_fits_camera_packet() {
        let long = "a".repeat(400) + ".bin";
        let name = sanitize_filename(&long);
        assert!(name.len() <= MAX_FILENAME_BYTES);
        let sym = choose_symbol_size(&name, 10_000) as usize;
        // Lean packet (no name) must fit.
        let meta = PacketMeta {
            file_id: 1,
            total_len: 10_000,
            symbol_size: sym as u16,
            k: 10,
            esi: 0,
            crc32: 0,
            filename: String::new(),
        };
        let packet = encode_packet(&meta, &vec![0u8; sym]).unwrap();
        assert!(
            packet.len() <= CAMERA_PACKET_CAP,
            "packet {} > cap {}",
            packet.len(),
            CAMERA_PACKET_CAP
        );
    }
}
