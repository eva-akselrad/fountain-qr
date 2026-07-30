import init, { TxSession, RxSession, qr_capacity } from "./wasm/fountain_core.js";

type FrameResult = {
  size: number;
  modules: Uint8Array;
  esi: number;
  packetLen: number;
};

type Metrics = {
  captureFps: number;
  decodeFps: number;
  goodputBps: number;
  newF: number;
  dupF: number;
  redF: number;
  locked: boolean;
  progress: number;
  k: number;
  symbolSize: number;
  fileId: string;
};

const els = {
  tabTx: document.getElementById("tab-tx") as HTMLButtonElement,
  tabRx: document.getElementById("tab-rx") as HTMLButtonElement,
  panelTx: document.getElementById("panel-tx") as HTMLElement,
  panelRx: document.getElementById("panel-rx") as HTMLElement,
  install: document.getElementById("btn-install") as HTMLButtonElement,
  fileInput: document.getElementById("file-input") as HTMLInputElement,
  txStart: document.getElementById("btn-tx-start") as HTMLButtonElement,
  txStop: document.getElementById("btn-tx-stop") as HTMLButtonElement,
  txFile: document.getElementById("tx-file") as HTMLElement,
  qrCanvas: document.getElementById("qr-canvas") as HTMLCanvasElement,
  rxStart: document.getElementById("btn-rx-start") as HTMLButtonElement,
  rxStop: document.getElementById("btn-rx-stop") as HTMLButtonElement,
  rxReset: document.getElementById("btn-rx-reset") as HTMLButtonElement,
  download: document.getElementById("btn-download") as HTMLAnchorElement,
  cam: document.getElementById("cam") as HTMLVideoElement,
  scanCanvas: document.getElementById("scan-canvas") as HTMLCanvasElement,
  status: document.getElementById("status") as HTMLElement,
  mCapture: document.getElementById("m-capture") as HTMLElement,
  mDecode: document.getElementById("m-decode") as HTMLElement,
  mGoodput: document.getElementById("m-goodput") as HTMLElement,
  mFrames: document.getElementById("m-frames") as HTMLElement,
  mLock: document.getElementById("m-lock") as HTMLElement,
  mProgress: document.getElementById("m-progress") as HTMLElement,
  mKsym: document.getElementById("m-ksym") as HTMLElement,
  mFid: document.getElementById("m-fid") as HTMLElement,
};

let deferredPrompt: BeforeInstallPromptEvent | null = null;
let txSession: TxSession | null = null;
let rxSession: RxSession | null = null;
let fileBytes: Uint8Array | null = null;
let fileName = "";
let txRaf = 0;
let rxRaf = 0;
let mediaStream: MediaStream | null = null;
let txRunning = false;
let rxRunning = false;

let captureTicks = 0;
let decodeTicks = 0;
let lastMetricTs = performance.now();
let lastUseful = 0;
let goodputEma = 0;

interface BeforeInstallPromptEvent extends Event {
  prompt: () => Promise<void>;
  userChoice: Promise<{ outcome: "accepted" | "dismissed" }>;
}

function setStatus(msg: string) {
  els.status.textContent = msg;
}

function fmtRate(bps: number): string {
  if (bps < 1000) return `${bps.toFixed(0)} B/s`;
  if (bps < 1_000_000) return `${(bps / 1000).toFixed(1)} KB/s`;
  return `${(bps / 1_000_000).toFixed(2)} MB/s`;
}

function renderMetrics(m: Partial<Metrics>) {
  if (m.captureFps !== undefined) els.mCapture.textContent = m.captureFps.toFixed(1);
  if (m.decodeFps !== undefined) els.mDecode.textContent = m.decodeFps.toFixed(1);
  if (m.goodputBps !== undefined) els.mGoodput.textContent = fmtRate(m.goodputBps);
  if (m.newF !== undefined && m.dupF !== undefined && m.redF !== undefined) {
    els.mFrames.textContent = `${m.newF}/${m.dupF}/${m.redF}`;
  }
  if (m.locked !== undefined) {
    els.mLock.textContent = m.locked ? "LOCKED" : "NO LOCK";
    els.mLock.classList.toggle("lock-on", m.locked);
    els.mLock.classList.toggle("lock-off", !m.locked);
  }
  if (m.progress !== undefined) els.mProgress.textContent = `${(m.progress * 100).toFixed(1)}%`;
  if (m.k !== undefined && m.symbolSize !== undefined) {
    els.mKsym.textContent = `${m.k} / ${m.symbolSize}B`;
  }
  if (m.fileId !== undefined) els.mFid.textContent = m.fileId || "--";
}

function drawModules(canvas: HTMLCanvasElement, size: number, packed: Uint8Array) {
  const ctx = canvas.getContext("2d", { alpha: false })!;
  if (canvas.width !== size) {
    canvas.width = size;
    canvas.height = size;
  }
  const img = ctx.createImageData(size, size);
  const data = img.data;
  let bitIndex = 0;
  for (let i = 0; i < size * size; i++) {
    const byte = packed[bitIndex >> 3];
    const on = ((byte >> (7 - (bitIndex & 7))) & 1) === 1;
    bitIndex++;
    const o = i * 4;
    const v = on ? 0 : 255;
    data[o] = v;
    data[o + 1] = v;
    data[o + 2] = v;
    data[o + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
}

function switchMode(mode: "tx" | "rx") {
  const tx = mode === "tx";
  els.tabTx.classList.toggle("active", tx);
  els.tabRx.classList.toggle("active", !tx);
  els.tabTx.setAttribute("aria-selected", String(tx));
  els.tabRx.setAttribute("aria-selected", String(!tx));
  els.panelTx.classList.toggle("hidden", !tx);
  els.panelRx.classList.toggle("hidden", tx);
}

async function startTx() {
  if (!fileBytes) return;
  stopTx();
  txSession = new TxSession(fileBytes, fileName);
  txRunning = true;
  els.txStart.disabled = true;
  els.txStop.disabled = false;
  renderMetrics({
    k: txSession.k,
    symbolSize: txSession.symbol_size,
    fileId: txSession.file_id,
    locked: true,
    progress: 0,
    newF: 0,
    dupF: 0,
    redF: 0,
  });
  setStatus(
    `TX streaming · k=${txSession.k} · symbol=${txSession.symbol_size}B · capacity=${qr_capacity()}B`
  );

  let frames = 0;
  let last = performance.now();
  const loop = () => {
    if (!txRunning || !txSession) return;
    try {
      const frame = txSession.next_frame() as unknown as FrameResult;
      drawModules(els.qrCanvas, frame.size, frame.modules);
      frames++;
      captureTicks++;
      const now = performance.now();
      if (now - last >= 1000) {
        renderMetrics({
          captureFps: (frames * 1000) / (now - last),
          decodeFps: 0,
          goodputBps: (frame.packetLen * frames * 1000) / (now - last),
          fileId: txSession.file_id,
          k: txSession.k,
          symbolSize: txSession.symbol_size,
        });
        frames = 0;
        last = now;
      }
    } catch (e) {
      setStatus(`TX error: ${e}`);
      stopTx();
      return;
    }
    txRaf = requestAnimationFrame(loop);
  };
  txRaf = requestAnimationFrame(loop);
}

function stopTx() {
  txRunning = false;
  if (txRaf) cancelAnimationFrame(txRaf);
  txRaf = 0;
  els.txStart.disabled = !fileBytes;
  els.txStop.disabled = true;
}

async function startRx() {
  stopRx();
  rxSession = new RxSession();
  try {
    mediaStream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: {
        facingMode: { ideal: "environment" },
        width: { ideal: 1280 },
        height: { ideal: 720 },
        frameRate: { ideal: 60, min: 30 },
      },
    });
  } catch (e) {
    setStatus(`Camera error: ${e}`);
    return;
  }
  els.cam.srcObject = mediaStream;
  await els.cam.play();
  rxRunning = true;
  els.rxStart.disabled = true;
  els.rxStop.disabled = false;
  els.download.classList.add("hidden");
  setStatus("RX scanning · point camera at TX QR stream");

  const scanCtx = els.scanCanvas.getContext("2d", {
    willReadFrequently: true,
    alpha: false,
  })!;

  let last = performance.now();
  let localCapture = 0;
  let localDecode = 0;

  const loop = () => {
    if (!rxRunning || !rxSession) return;
    const vw = els.cam.videoWidth;
    const vh = els.cam.videoHeight;
    if (vw && vh) {
      // Downscale for decode throughput while targeting ~60 capture ticks
      const maxW = 640;
      const scale = Math.min(1, maxW / vw);
      const w = Math.max(1, Math.floor(vw * scale));
      const h = Math.max(1, Math.floor(vh * scale));
      if (els.scanCanvas.width !== w || els.scanCanvas.height !== h) {
        els.scanCanvas.width = w;
        els.scanCanvas.height = h;
      }
      scanCtx.drawImage(els.cam, 0, 0, w, h);
      const rgba = scanCtx.getImageData(0, 0, w, h).data;
      const luma = new Uint8Array(w * h);
      for (let i = 0, j = 0; i < rgba.length; i += 4, j++) {
        luma[j] = (rgba[i] * 77 + rgba[i + 1] * 150 + rgba[i + 2] * 29) >> 8;
      }
      localCapture++;
      captureTicks++;
      const status = rxSession.ingest_luma(w, h, luma);
      if (status !== "none") {
        localDecode++;
        decodeTicks++;
      }
      if (status === "complete") {
        finishDownload();
      }
      renderMetrics({
        locked: rxSession.locked,
        progress: rxSession.progress,
        newF: Number(rxSession.new_frames),
        dupF: Number(rxSession.dup_frames),
        redF: Number(rxSession.red_frames),
        k: rxSession.k,
        symbolSize: 0,
        fileId: rxSession.filename || "--",
      });
    }

    const now = performance.now();
    if (now - last >= 1000) {
      const useful = Number(rxSession.useful_bytes);
      const delta = useful - lastUseful;
      lastUseful = useful;
      const inst = (delta * 1000) / (now - last);
      goodputEma = goodputEma === 0 ? inst : goodputEma * 0.7 + inst * 0.3;
      renderMetrics({
        captureFps: (localCapture * 1000) / (now - last),
        decodeFps: (localDecode * 1000) / (now - last),
        goodputBps: goodputEma,
      });
      localCapture = 0;
      localDecode = 0;
      last = now;
      lastMetricTs = now;
    }
    rxRaf = requestAnimationFrame(loop);
  };
  rxRaf = requestAnimationFrame(loop);
}

function finishDownload() {
  if (!rxSession || !rxSession.complete) return;
  try {
    const result = rxSession.take_file() as unknown as {
      filename: string;
      data: Uint8Array;
      crc32: string;
    };
    const blob = new Blob([result.data.buffer.slice(
      result.data.byteOffset,
      result.data.byteOffset + result.data.byteLength
    )], { type: "application/octet-stream" });
    const url = URL.createObjectURL(blob);
    els.download.href = url;
    els.download.download = result.filename || "received.bin";
    els.download.textContent = `DOWNLOAD ${result.filename}`;
    els.download.classList.remove("hidden");
    setStatus(`RX complete · crc32=${result.crc32} · ${result.data.byteLength} bytes`);
    renderMetrics({ progress: 1, locked: true });
  } catch (e) {
    setStatus(`Assemble error: ${e}`);
  }
}

function stopRx() {
  rxRunning = false;
  if (rxRaf) cancelAnimationFrame(rxRaf);
  rxRaf = 0;
  if (mediaStream) {
    for (const t of mediaStream.getTracks()) t.stop();
    mediaStream = null;
  }
  els.cam.srcObject = null;
  els.rxStart.disabled = false;
  els.rxStop.disabled = true;
}

function resetRx() {
  stopRx();
  rxSession?.reset();
  rxSession = new RxSession();
  lastUseful = 0;
  goodputEma = 0;
  els.download.classList.add("hidden");
  renderMetrics({
    captureFps: 0,
    decodeFps: 0,
    goodputBps: 0,
    newF: 0,
    dupF: 0,
    redF: 0,
    locked: false,
    progress: 0,
    k: 0,
    symbolSize: 0,
    fileId: "--",
  });
  setStatus("RX reset");
}

function wireUi() {
  els.tabTx.addEventListener("click", () => {
    stopRx();
    switchMode("tx");
  });
  els.tabRx.addEventListener("click", () => {
    stopTx();
    switchMode("rx");
  });

  els.fileInput.addEventListener("change", async () => {
    const f = els.fileInput.files?.[0];
    if (!f) return;
    const buf = new Uint8Array(await f.arrayBuffer());
    fileBytes = buf;
    fileName = f.name;
    els.txFile.textContent = `${f.name} · ${f.size.toLocaleString()} B`;
    els.txStart.disabled = false;
    setStatus(`File loaded · ${f.size} bytes`);
  });

  els.txStart.addEventListener("click", () => void startTx());
  els.txStop.addEventListener("click", () => {
    stopTx();
    setStatus("TX stopped");
  });
  els.rxStart.addEventListener("click", () => void startRx());
  els.rxStop.addEventListener("click", () => {
    stopRx();
    setStatus("RX stopped");
  });
  els.rxReset.addEventListener("click", resetRx);

  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    deferredPrompt = e as BeforeInstallPromptEvent;
    els.install.classList.remove("hidden");
  });
  els.install.addEventListener("click", async () => {
    if (!deferredPrompt) return;
    await deferredPrompt.prompt();
    await deferredPrompt.userChoice;
    deferredPrompt = null;
    els.install.classList.add("hidden");
  });
}

async function main() {
  wireUi();
  setStatus("Loading WASM…");
  await init();
  setStatus(`WASM ready · QR capacity ${qr_capacity()} B · LT fountain · Cloudflare Pages PWA`);
  renderMetrics({
    captureFps: 0,
    decodeFps: 0,
    goodputBps: 0,
    newF: 0,
    dupF: 0,
    redF: 0,
    locked: false,
    progress: 0,
    k: 0,
    symbolSize: 0,
    fileId: "--",
  });
}

main().catch((e) => setStatus(`Boot failure: ${e}`));
