mod lt;
mod packet;
mod qr;

use lt::{Decoder, Encoder};
use packet::{choose_symbol_size, crc32, decode_packet, encode_packet, PacketMeta, QR_CAPACITY};
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
}

#[wasm_bindgen]
impl TxSession {
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8], filename: &str) -> Result<TxSession, JsValue> {
        let filename = if filename.is_empty() {
            "file.bin".to_string()
        } else {
            filename.to_string()
        };
        let symbol_size = choose_symbol_size(&filename, data.len()) as usize;
        let file_id = random_file_id();
        let encoder = Encoder::new(file_id, data, symbol_size);
        let crc = crc32(data);
        Ok(TxSession {
            encoder,
            filename,
            total_len: data.len() as u64,
            crc,
            frames: 0,
        })
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
    pub fn frames_emitted(&self) -> u64 {
        self.frames
    }

    #[wasm_bindgen(getter)]
    pub fn qr_capacity(&self) -> u32 {
        QR_CAPACITY as u32
    }

    /// Encode next fountain symbol into a QR module matrix.
    /// Returns `{ size, modules: Uint8Array (packed bits), esi, packetLen }`.
    pub fn next_frame(&mut self) -> Result<JsValue, JsValue> {
        let (esi, symbol) = self.encoder.encode_next();
        let meta = PacketMeta {
            file_id: self.encoder.file_id,
            total_len: self.total_len,
            symbol_size: self.encoder.symbol_size as u16,
            k: self.encoder.k,
            esi,
            crc32: self.crc,
            filename: self.filename.clone(),
        };
        let packet = encode_packet(&meta, &symbol).map_err(|e| JsValue::from_str(&e))?;
        let packet_len = packet.len() as u32;
        let (size, modules) = qr::encode_matrix(&packet).map_err(|e| JsValue::from_str(&e))?;
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
        Ok(obj.into())
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
        }
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
    /// "none" | "dup" | "red" | "new" | "complete" | "err:..."
    pub fn ingest_luma(&mut self, width: u32, height: u32, luma: &[u8]) -> String {
        let raw = match qr::decode_luma_bytes(width, height, luma) {
            Ok(b) => b,
            Err(_) => return "none".into(),
        };
        self.ingest_packet(&raw)
    }

    /// Ingest already-extracted packet bytes (for testing / JS QR path).
    pub fn ingest_packet(&mut self, data: &[u8]) -> String {
        let (meta, symbol) = match decode_packet(data) {
            Ok(v) => v,
            Err(e) => return format!("err:{e}"),
        };

        self.locked = true;
        if self.decoder.is_none() {
            self.decoder = Some(Decoder::new(
                meta.file_id,
                meta.k,
                meta.symbol_size as usize,
                meta.total_len,
            ));
            self.filename = meta.filename.clone();
            self.expected_crc = meta.crc32;
        } else if let Some(ref d) = self.decoder {
            if d.file_id != meta.file_id {
                return "err:file_id_mismatch".into();
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
        Ok(obj.into())
    }

    pub fn reset(&mut self) {
        self.decoder = None;
        self.filename.clear();
        self.expected_crc = 0;
        self.useful_bytes = 0;
        self.locked = false;
    }
}

#[wasm_bindgen]
pub fn qr_capacity() -> u32 {
    QR_CAPACITY as u32
}
