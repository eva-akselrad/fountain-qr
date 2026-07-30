//! QR encode (matrix) + decode helpers for WASM.

use qrcode::QrCode;
use qrcode::types::EcLevel;

/// Encode binary payload as a QR module matrix (packed bits, row-major).
/// Uses ECC M and automatic version sizing for phone-camera reliability.
/// Returns (size, packed_bits) where packed_bits is MSB-first within each byte.
pub fn encode_matrix(data: &[u8]) -> Result<(u32, Vec<u8>), String> {
    let code = QrCode::with_error_correction_level(data, EcLevel::M)
        .or_else(|_| QrCode::with_error_correction_level(data, EcLevel::L))
        .map_err(|e| format!("qr encode: {e:?}"))?;
    let w = code.width();
    let mut bits = Vec::with_capacity((w * w + 7) / 8);
    let mut acc = 0u8;
    let mut n = 0u8;
    for y in 0..w {
        for x in 0..w {
            acc = (acc << 1) | u8::from(code[(x, y)] == qrcode::Color::Dark);
            n += 1;
            if n == 8 {
                bits.push(acc);
                acc = 0;
                n = 0;
            }
        }
    }
    if n > 0 {
        bits.push(acc << (8 - n));
    }
    Ok((w as u32, bits))
}

/// Decode QR from grayscale luma buffer (row-major, width*height bytes).
pub fn decode_luma_bytes(width: u32, height: u32, luma: &[u8]) -> Result<Vec<u8>, String> {
    if (width as usize) * (height as usize) != luma.len() {
        return Err("luma size mismatch".into());
    }
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(width as usize, height as usize, |x, y| {
        luma[y * width as usize + x]
    });
    let grids = img.detect_grids();
    for g in grids {
        let mut data = Vec::new();
        match g.decode_to(&mut data) {
            Ok(_meta) => return Ok(data),
            Err(_) => continue,
        }
    }
    Err("no qr".into())
}
