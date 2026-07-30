//! Binary packet framing for Fountain QR payloads.
//! QR Version 40-L binary capacity = 2953 bytes.

use crc32fast::Hasher;

pub const MAGIC: &[u8; 4] = b"FQFT";
pub const PROTOCOL_VERSION: u8 = 1;
/// Max QR binary payload (Version 40, ECC L).
pub const QR_CAPACITY: usize = 2953;
/// Fixed header without filename:
/// magic4 + ver1 + flags1 + file_id8 + total_len8 + symbol_size2 + k4 + esi4 + crc32_4 + name_len1 = 37
pub const FIXED_HEADER: usize = 37;

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
    let max_sym = max_symbol_payload(name_bytes.len());
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

/// Choose symbol size to pack into QR capacity for a given filename.
pub fn choose_symbol_size(filename: &str, file_len: usize) -> u16 {
    let max = max_symbol_payload(filename.len());
    // Prefer filling the QR; for tiny files use file_len (min 1).
    let size = if file_len == 0 {
        1
    } else {
        max.min(file_len).max(1)
    };
    size as u16
}
