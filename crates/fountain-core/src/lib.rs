mod lt;
mod packet;
mod qr;

use lt::{Decoder, Encoder};
use packet::{
    choose_symbol_size, crc32, decode_packet, encode_packet, pair_code, pair_code_matches,
    sanitize_filename, PacketMeta, CAMERA_PACKET_CAP, QR_CAPACITY,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init() {
    // no-op hook for wasm load
}

fn random_file_id() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).ok();
    u64::from_le_bytes(bytes)
}

/// Transmitter session: chunks file and streams fountain-encoded QR matrices.
#[wasm_bindgen]
pub struct TxSession {
    encoder: Encoder,
    filename: String,
    total_len: u64,
    crc: u32,
    frames: u64,
    pair: String,
    /// Milliseconds each QR is held on screen (set from JS; default 160).
    dwell_ms: u32,
}

fn fit_symbol_size(filename: &str, file_len: usize) -> u16 {
    let mut sym = choose_symbol_size(filename, file_len) as usize;
    // Probe lean (no-filename) packets; shrink until QR accepts.
    while sym >= 1 {
        let meta = PacketMeta {
            file_id: 0,
            total_len: file_len as u64,
            symbol_size: sym as u16,
            k: 1,
            esi: 0,
            crc32: 0,
            filename: String::new(),
        };
        let dummy = vec![0u8; sym];
        if let Ok(packet) = encode_packet(&meta, &dummy) {
            if packet.len() <= CAMERA_PACKET_CAP && qr::can_encode(&packet) {
                return sym as u16;
            }
        }
        if sym == 1 {
            break;
        }
        sym = (sym * 3 / 4).max(1);
    }
    1
}

#[wasm_bindgen]
impl TxSession {
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8], filename: &str) -> Result<TxSession, JsValue> {
        let filename = sanitize_filename(filename);
        let symbol_size = fit_symbol_size(&filename, data.len()) as usize;
        let file_id = random_file_id();
        let encoder = Encoder::new(file_id, data, symbol_size);
        let crc = crc32(data);
        let pair = pair_code(file_id);
        Ok(TxSession {
            encoder,
            filename,
            total_len: data.len() as u64,
            crc,
            frames: 0,
            pair,
            dwell_ms: 70,
        })
    }

    pub fn set_dwell_ms(&mut self, ms: u32) {
        self.dwell_ms = ms.clamp(40, 2000);
    }

    #[wasm_bindgen(getter)]
    pub fn k(&self) -> u32 {
        self.encoder.k
    }

    #[wasm_bindgen(getter)]
    pub fn symbol_size(&self) -> u32 {
        self.encoder.symbol_size as u32
    }

    #[wasm_bindgen(getter)]
    pub fn file_id(&self) -> String {
        format!("{:016x}", self.encoder.file_id)
    }

    #[wasm_bindgen(getter)]
    pub fn pair_code(&self) -> String {
        self.pair.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn frames_emitted(&self) -> u64 {
        self.frames
    }

    #[wasm_bindgen(getter)]
    pub fn qr_capacity(&self) -> u32 {
        QR_CAPACITY as u32
    }

    #[wasm_bindgen(getter)]
    pub fn filename(&self) -> String {
        self.filename.clone()
    }

    /// Best-case ETA seconds (≈ k symbols @ dwell, ideal decode).
    #[wasm_bindgen(getter)]
    pub fn eta_best_sec(&self) -> f64 {
        let rate = 1000.0 / self.dwell_ms.max(1) as f64;
        self.encoder.k as f64 / rate
    }

    /// Typical ETA seconds (fountain overhead + miss margin ≈ 1.5×).
    #[wasm_bindgen(getter)]
    pub fn eta_typical_sec(&self) -> f64 {
        self.eta_best_sec() * 1.5
    }

    /// Encode next fountain symbol into a QR module matrix.
    /// Returns `{ size, modules, esi, packetLen, pairCode }`.
    pub fn next_frame(&mut self) -> Result<JsValue, JsValue> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let (esi, symbol) = self.encoder.encode_next();
            // Most frames skip the filename so QR stays smaller/faster to scan.
            // Include name on the first few + every 8th for late joiners.
            let include_name = esi < 6 || esi % 8 == 0;
            let name = if include_name {
                self.filename.clone()
            } else {
                String::new()
            };
            let meta = PacketMeta {
                file_id: self.encoder.file_id,
                total_len: self.total_len,
                symbol_size: self.encoder.symbol_size as u16,
                k: self.encoder.k,
                esi,
                crc32: self.crc,
                filename: name,
            };
            let packet = match encode_packet(&meta, &symbol) {
                Ok(p) if p.len() <= CAMERA_PACKET_CAP || !include_name => p,
                Ok(_) | Err(_) if include_name => {
                    // Fall back to lean packet without filename.
                    let lean = PacketMeta {
                        filename: String::new(),
                        ..meta
                    };
                    match encode_packet(&lean, &symbol) {
                        Ok(p) => p,
                        Err(_) if attempts < 4 => continue,
                        Err(e) => return Err(JsValue::from_str(&e)),
                    }
                }
                Ok(p) => p,
                Err(_) if attempts < 4 => continue,
                Err(e) => return Err(JsValue::from_str(&e)),
            };
            match qr::encode_matrix(&packet) {
                Ok((size, modules)) => {
                    let packet_len = packet.len() as u32;
                    self.frames += 1;
                    let obj = js_sys::Object::new();
                    js_sys::Reflect::set(&obj, &"size".into(), &JsValue::from(size))?;
                    js_sys::Reflect::set(
                        &obj,
                        &"modules".into(),
                        &js_sys::Uint8Array::from(modules.as_slice()).into(),
                    )?;
                    js_sys::Reflect::set(&obj, &"esi".into(), &JsValue::from(esi))?;
                    js_sys::Reflect::set(&obj, &"packetLen".into(), &JsValue::from(packet_len))?;
                    js_sys::Reflect::set(
                        &obj,
                        &"pairCode".into(),
                        &JsValue::from_str(&self.pair),
                    )?;
                    return Ok(obj.into());
                }
                Err(_) if attempts < 8 => continue,
                Err(e) => return Err(JsValue::from_str(&e)),
            }
        }
    }
}

/// Receiver session: accumulates fountain symbols until reconstruction.
#[wasm_bindgen]
pub struct RxSession {
    decoder: Option<Decoder>,
    filename: String,
    expected_crc: u32,
    useful_bytes: u64,
    locked: bool,
    expected_pair: String,
    seen_pair: String,
}

#[wasm_bindgen]
impl RxSession {
    #[wasm_bindgen(constructor)]
    pub fn new() -> RxSession {
        RxSession {
            decoder: None,
            filename: String::new(),
            expected_crc: 0,
            useful_bytes: 0,
            locked: false,
            expected_pair: String::new(),
            seen_pair: String::new(),
        }
    }

    /// Set the pair code shown on the TX device. Packets with other codes are ignored.
    pub fn set_expected_pair(&mut self, code: &str) {
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
        self.expected_pair = normalized;
    }

    #[wasm_bindgen(getter)]
    pub fn expected_pair(&self) -> String {
        self.expected_pair.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn pair_code(&self) -> String {
        self.seen_pair.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn locked(&self) -> bool {
        self.locked
    }

    #[wasm_bindgen(getter)]
    pub fn filename(&self) -> String {
        self.filename.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn recovered(&self) -> u32 {
        self.decoder.as_ref().map(|d| d.recovered_count()).unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn k(&self) -> u32 {
        self.decoder.as_ref().map(|d| d.k).unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn symbol_size(&self) -> u32 {
        self.decoder
            .as_ref()
            .map(|d| d.symbol_size as u32)
            .unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn progress(&self) -> f64 {
        match &self.decoder {
            Some(d) if d.k > 0 => d.recovered_count() as f64 / d.k as f64,
            _ => 0.0,
        }
    }

    #[wasm_bindgen(getter)]
    pub fn complete(&self) -> bool {
        self.decoder.as_ref().map(|d| d.is_complete()).unwrap_or(false)
    }

    #[wasm_bindgen(getter)]
    pub fn new_frames(&self) -> u64 {
        self.decoder.as_ref().map(|d| d.new_count).unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn dup_frames(&self) -> u64 {
        self.decoder.as_ref().map(|d| d.dup_count).unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn red_frames(&self) -> u64 {
        self.decoder.as_ref().map(|d| d.red_count).unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn useful_bytes(&self) -> u64 {
        self.useful_bytes
    }

    /// Ingest grayscale camera frame. Returns status string:
    /// "none" | "dup" | "red" | "new" | "complete" | "wrong_pair" | "need_pair" | "err:..."
    pub fn ingest_luma(&mut self, width: u32, height: u32, luma: &[u8]) -> String {
        let raw = match qr::decode_luma_bytes(width, height, luma) {
            Ok(b) => b,
            Err(_) => return "none".into(),
        };
        self.ingest_packet(&raw)
    }

    /// Ingest already-extracted packet bytes.
    pub fn ingest_packet(&mut self, data: &[u8]) -> String {
        let (meta, symbol) = match decode_packet(data) {
            Ok(v) => v,
            Err(e) => return format!("err:{e}"),
        };

        let code = pair_code(meta.file_id);
        self.seen_pair = code.clone();

        if self.expected_pair.is_empty() {
            return "need_pair".into();
        }
        if !pair_code_matches(&self.expected_pair, meta.file_id) {
            return "wrong_pair".into();
        }

        self.locked = true;
        if self.decoder.is_none() {
            self.decoder = Some(Decoder::new(
                meta.file_id,
                meta.k,
                meta.symbol_size as usize,
                meta.total_len,
            ));
            if !meta.filename.is_empty() {
                self.filename = meta.filename.clone();
            }
            self.expected_crc = meta.crc32;
        } else if let Some(ref d) = self.decoder {
            if d.file_id != meta.file_id {
                return "err:file_id_mismatch".into();
            }
            if self.filename.is_empty() && !meta.filename.is_empty() {
                self.filename = meta.filename.clone();
            }
        }

        let before = self.decoder.as_ref().map(|d| d.recovered_count()).unwrap_or(0);
        let dec = self.decoder.as_mut().unwrap();
        let kind = dec.ingest(meta.esi, &symbol);
        let after = dec.recovered_count();
        if after > before {
            self.useful_bytes += ((after - before) as u64) * (dec.symbol_size as u64);
        }

        if dec.is_complete() {
            "complete".into()
        } else {
            match kind {
                0 => "dup".into(),
                1 => "red".into(),
                _ => "new".into(),
            }
        }
    }

    /// Assemble file bytes if complete; verifies CRC32.
    pub fn take_file(&mut self) -> Result<JsValue, JsValue> {
        let dec = self
            .decoder
            .as_ref()
            .ok_or_else(|| JsValue::from_str("no session"))?;
        let data = dec
            .assemble()
            .ok_or_else(|| JsValue::from_str("incomplete"))?;
        let got = crc32(&data);
        if got != self.expected_crc {
            return Err(JsValue::from_str(&format!(
                "crc mismatch: got {got:08x} expected {:08x}",
                self.expected_crc
            )));
        }
        let obj = js_sys::Object::new();
        js_sys::Reflect::set(
            &obj,
            &"filename".into(),
            &JsValue::from_str(&self.filename),
        )?;
        js_sys::Reflect::set(
            &obj,
            &"data".into(),
            &js_sys::Uint8Array::from(data.as_slice()).into(),
        )?;
        js_sys::Reflect::set(
            &obj,
            &"crc32".into(),
            &JsValue::from_str(&format!("{got:08x}")),
        )?;
        js_sys::Reflect::set(
            &obj,
            &"pairCode".into(),
            &JsValue::from_str(&self.seen_pair),
        )?;
        Ok(obj.into())
    }

    pub fn reset(&mut self) {
        self.decoder = None;
        self.filename.clear();
        self.expected_crc = 0;
        self.useful_bytes = 0;
        self.locked = false;
        self.seen_pair.clear();
        // keep expected_pair so user doesn't retype
    }
}

#[wasm_bindgen]
pub fn qr_capacity() -> u32 {
    QR_CAPACITY as u32
}

#[wasm_bindgen]
pub fn format_pair_code(file_id_hex: &str) -> String {
    let id = u64::from_str_radix(file_id_hex.trim(), 16).unwrap_or(0);
    pair_code(id)
}
