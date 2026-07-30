# Fountain QR

Offline **Fountain Code** (Luby Transform) file transfer over sequential QR frames. Any sufficient subset of frames reconstructs the file — no network back-channel required for dropped packets.

## Architecture

```
TX: File → Rust/WASM LT encoder → QR V40-L matrices (≤2953 B) → Canvas @ rAF (~60 FPS)
RX: getUserMedia 60 FPS → luma buffer → WASM QR decode → LT peel decoder → Blob download
```

| Layer | Choice |
| --- | --- |
| Codec | Luby Transform (robust soliton) in Rust → WASM |
| QR | Version 40, ECC L, binary payload capacity **2953 bytes** |
| Frontend | Vite PWA + `beforeinstallprompt` |
| Hosting | Cloudflare Pages (`dist/`) |
| UI | Utilitarian dark CSS grid, pure black, monospace metrics |

### Packet (`FQFT`)

`magic(4) · ver · flags · file_id(8) · total_len(8) · symbol_size(2) · k(4) · esi(4) · crc32(4) · name_len · name · symbol`

Neighbors for each encoded symbol ID (`esi`) are derived from a deterministic PRNG seeded by `(file_id, esi)` — receivers never need an explicit degree list.

### Metrics

- **Capture FPS** — camera / TX frame loop rate  
- **Decode FPS** — successful QR extractions / s  
- **Goodput** — newly recovered source bytes / s  
- **Frames N/D/R** — new / duplicate / redundant fountain symbols  
- **Lock** — optical stream aligned (valid `FQFT` seen)

## Develop

```bash
npm install
npm run dev          # builds WASM then Vite
npm run build        # production → dist/
npm run test:rust    # LT roundtrip tests
```

Requirements: Rust (`wasm32-unknown-unknown`), `wasm-bindgen-cli` 0.2.100, Node 20+.

## Deploy (Cloudflare Pages)

- Build command: `npm run build`
- Output directory: `dist`
- Or: `npx wrangler pages deploy dist`

## Pairing

1. **TX** starts a stream → large **PAIR** code (5 chars) appears above the QR.
2. **RX** types that code, then opens the camera (preview is **not** mirrored).
3. Fill the green square with the QR; RX shows **PAIRED** when codes match.

Camera profile caps symbols at ~200 B/frame with ECC M and ~160 ms dwell so phones can lock. Theoretical Version 40 capacity (2953 B) remains the protocol ceiling.
