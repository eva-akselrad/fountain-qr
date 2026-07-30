import init, { TxSession, RxSession, qr_capacity } from "./wasm/fountain_core.js";

type FrameResult = {
  size: number;
  modules: Uint8Array;
  esi: number;
  packetLen: number;
  pairCode: string;
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
  pairCode: string;
  eta: string;
};

/** Hold each QR — lower = faster transfer (needs steady aim). */
const TX_DWELL_MS = 70;
/** Prefer higher scan resolution so denser modules stay resolvable. */
const SCAN_MAX_WIDTH = 1440;

const els = {
  tabTx: document.getElementById("tab-tx") as HTMLButtonElement,
  tabRx: document.getElementById("tab-rx") as HTMLButtonElement,
  panelTx: document.getElementById("panel-tx") as HTMLElement,
  panelRx: document.getElementById("panel-rx") as HTMLElement,
  install: document.getElementById("btn-install") as HTMLButtonElement,
  fileInput: document.getElementById("file-input") as HTMLInputElement,
  txStart: document.getElementById("btn-tx-start") as HTMLButtonElement,
  txStop: document.getElementById("btn-tx-stop") as HTMLButtonElement,
  txNewPair: document.getElementById("btn-tx-newpair") as HTMLButtonElement,
  txFile: document.getElementById("tx-file") as HTMLElement,
  txPair: document.getElementById("tx-pair") as HTMLElement,
  txPairCode: document.getElementById("tx-pair-code") as HTMLElement,
  txQrWrap: document.getElementById("tx-qr-wrap") as HTMLElement,
  qrCanvas: document.getElementById("qr-canvas") as HTMLCanvasElement,
  pairInput: document.getElementById("pair-input") as HTMLInputElement,
  rxStart: document.getElementById("btn-rx-start") as HTMLButtonElement,
  rxStop: document.getElementById("btn-rx-stop") as HTMLButtonElement,
  rxReset: document.getElementById("btn-rx-reset") as HTMLButtonElement,
  rxPair: document.getElementById("rx-pair") as HTMLElement,
  rxPairCode: document.getElementById("rx-pair-code") as HTMLElement,
  rxPairHint: document.getElementById("rx-pair-hint") as HTMLElement,
  viewfinder: document.getElementById("viewfinder") as HTMLElement,
  aimText: document.getElementById("aim-text") as HTMLElement,
  alignProgressBar: document.getElementById("align-progress-bar") as HTMLElement,
  download: document.getElementById("btn-download") as HTMLAnchorElement,
  cam: document.getElementById("cam") as HTMLVideoElement,
  scanCanvas: document.getElementById("scan-canvas") as HTMLCanvasElement,
  status: document.getElementById("status") as HTMLElement,
  mPair: document.getElementById("m-pair") as HTMLElement,
  mCapture: document.getElementById("m-capture") as HTMLElement,
  mDecode: document.getElementById("m-decode") as HTMLElement,
  mGoodput: document.getElementById("m-goodput") as HTMLElement,
  mFrames: document.getElementById("m-frames") as HTMLElement,
  mLock: document.getElementById("m-lock") as HTMLElement,
  mProgress: document.getElementById("m-progress") as HTMLElement,
  mEta: document.getElementById("m-eta") as HTMLElement,
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
let lastUseful = 0;
let goodputEma = 0;
let downloadDone = false;
let didLockBuzz = false;
let lastHitTs = 0;

type AimState = "idle" | "seeking" | "signal" | "locked" | "receiving" | "mismatch" | "done";

function setAimState(state: AimState, text: string, progress = 0) {
  const classes = [
    "aim-idle",
    "aim-seeking",
    "aim-signal",
    "aim-locked",
    "aim-receiving",
    "aim-mismatch",
    "aim-done",
  ];
  els.viewfinder.classList.remove(...classes);
  els.viewfinder.classList.add(`aim-${state}`);
  els.aimText.textContent = text;
  els.alignProgressBar.style.width = `${Math.max(0, Math.min(100, progress * 100))}%`;
}

function buzz(pattern: number | number[] = 40) {
  try {
    navigator.vibrate?.(pattern);
  } catch {
    /* ignore */
  }
}

function setTxWrap(mode: "idle" | "ready" | "streaming") {
  els.txQrWrap.classList.remove("idle", "ready", "streaming");
  els.txQrWrap.classList.add(mode);
}

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

function formatPairDisplay(code: string): string {
  const c = code.replace(/[^0-9A-Z]/gi, "").toUpperCase();
  if (c.length <= 3) return c || "-----";
  return `${c.slice(0, 3)}-${c.slice(3)}`;
}

function normalizePairInput(raw: string): string {
  return raw
    .toUpperCase()
    .replace(/[IL]/g, "1")
    .replace(/O/g, "0")
    .replace(/[^0-9A-Z]/g, "")
    .slice(0, 5);
}

function setPairBanner(
  el: HTMLElement,
  digits: HTMLElement,
  code: string,
  state: "idle" | "live" | "mismatch",
  label: string,
  hint: string
) {
  digits.textContent = formatPairDisplay(code || "-----");
  el.classList.remove("idle", "live", "mismatch");
  el.classList.add(state);
  const labelEl = el.querySelector(".pair-label");
  if (labelEl) labelEl.textContent = label;
  const hintEl = el.querySelector(".pair-hint") as HTMLElement | null;
  if (hintEl) hintEl.textContent = hint;
}

function fmtEta(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0) return "--";
  if (sec < 1) return "<1s";
  if (sec < 60) return `${Math.ceil(sec)}s`;
  const m = Math.floor(sec / 60);
  const s = Math.ceil(sec % 60);
  return `${m}m ${s.toString().padStart(2, "0")}s`;
}

function fmtEtaRange(best: number, typical: number): string {
  return `${fmtEta(best)}–${fmtEta(typical)}`;
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
  if (m.pairCode !== undefined) els.mPair.textContent = formatPairDisplay(m.pairCode);
  if (m.eta !== undefined) els.mEta.textContent = m.eta;
}

/** Draw QR with quiet zone + large modules (pixelated, camera-friendly). */
function drawModules(canvas: HTMLCanvasElement, size: number, packed: Uint8Array) {
  const quiet = 4;
  const modulePx = Math.max(4, Math.floor(640 / (size + quiet * 2)));
  const dim = (size + quiet * 2) * modulePx;
  const ctx = canvas.getContext("2d", { alpha: false })!;
  canvas.width = dim;
  canvas.height = dim;
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, dim, dim);
  ctx.fillStyle = "#000000";
  let bitIndex = 0;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const byte = packed[bitIndex >> 3];
      const on = ((byte >> (7 - (bitIndex & 7))) & 1) === 1;
      bitIndex++;
      if (on) {
        ctx.fillRect(
          (x + quiet) * modulePx,
          (y + quiet) * modulePx,
          modulePx,
          modulePx
        );
      }
    }
  }
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

function showPreparedPair(session: TxSession, streaming: boolean) {
  const pair = session.pair_code;
  const etaText = fmtEtaRange(session.eta_best_sec, session.eta_typical_sec);
  setPairBanner(
    els.txPair,
    els.txPairCode,
    pair,
    streaming ? "live" : "live",
    streaming ? "STREAMING" : "PAIR READY",
    streaming
      ? `ETA ${etaText} · keep QR on screen`
      : `enter ${formatPairDisplay(pair)} on phone → OPEN CAMERA → then START STREAM here`
  );
  renderMetrics({
    k: session.k,
    symbolSize: session.symbol_size,
    fileId: session.file_id,
    pairCode: pair,
    locked: false,
    progress: 0,
    eta: etaText,
  });
  return { pair, etaText };
}

/** Create (or recreate) TX session as soon as a file is chosen — pair code is stable until NEW PAIR / new file. */
function prepareTxSession(bytes: Uint8Array, name: string): TxSession {
  stopTxLoopOnly();
  const session = new TxSession(bytes, name);
  session.set_dwell_ms(TX_DWELL_MS);
  txSession = session;
  const { pair, etaText } = showPreparedPair(session, false);
  els.txStart.disabled = false;
  els.txStop.disabled = true;
  els.txNewPair.disabled = false;
  setTxWrap("ready");
  els.txFile.textContent = `${name} · ${bytes.byteLength.toLocaleString()} B · ETA ${etaText}`;
  setStatus(
    `PAIR ${formatPairDisplay(pair)} locked · type it on the phone first, open camera, then press START STREAM`
  );
  // Clear canvas until stream starts
  const ctx = els.qrCanvas.getContext("2d");
  if (ctx) {
    els.qrCanvas.width = 256;
    els.qrCanvas.height = 256;
    ctx.fillStyle = "#111";
    ctx.fillRect(0, 0, 256, 256);
    ctx.fillStyle = "#3dff9a";
    ctx.font = "14px monospace";
    ctx.textAlign = "center";
    ctx.fillText("WAITING TO START", 128, 120);
    ctx.fillStyle = "#888";
    ctx.fillText(formatPairDisplay(pair), 128, 148);
  }
  return session;
}

function stopTxLoopOnly() {
  txRunning = false;
  if (txRaf) cancelAnimationFrame(txRaf);
  txRaf = 0;
}

async function startTx() {
  if (!fileBytes) return;
  // Reuse prepared session so the pair code does NOT change on start.
  if (!txSession) {
    prepareTxSession(fileBytes, fileName);
  }
  stopTxLoopOnly();
  txRunning = true;
  els.txStart.disabled = true;
  els.txStop.disabled = false;
  els.txNewPair.disabled = true;
  setTxWrap("streaming");

  const session = txSession!;
  const { pair, etaText } = showPreparedPair(session, true);
  setStatus(
    `TX streaming · PAIR ${formatPairDisplay(pair)} · fill phone green box with this QR · ETA ${etaText}`
  );

  let frames = 0;
  let lastMetric = performance.now();
  let lastSwap = 0;
  let current: FrameResult | null = null;
  let encodeFails = 0;

  const loop = (now: number) => {
    if (!txRunning || !txSession) return;
    try {
      if (!current || now - lastSwap >= TX_DWELL_MS) {
        current = txSession.next_frame() as unknown as FrameResult;
        drawModules(els.qrCanvas, current.size, current.modules);
        lastSwap = now;
        frames++;
        encodeFails = 0;
      }
      if (now - lastMetric >= 1000) {
        const fps = (frames * 1000) / (now - lastMetric);
        renderMetrics({
          captureFps: fps,
          decodeFps: 0,
          goodputBps: current ? current.packetLen * fps : 0,
          fileId: txSession.file_id,
          pairCode: txSession.pair_code,
          k: txSession.k,
          symbolSize: txSession.symbol_size,
          eta: fmtEtaRange(txSession.eta_best_sec, txSession.eta_typical_sec),
        });
        frames = 0;
        lastMetric = now;
      }
    } catch (e) {
      encodeFails++;
      if (encodeFails > 12) {
        setStatus(`TX stopped after repeated encode errors: ${e}`);
        stopTx();
        return;
      }
      current = null;
      lastSwap = 0;
    }
    txRaf = requestAnimationFrame(loop);
  };
  txRaf = requestAnimationFrame(loop);
}

function stopTx() {
  stopTxLoopOnly();
  els.txStart.disabled = !txSession;
  els.txStop.disabled = true;
  els.txNewPair.disabled = !txSession;
  if (txSession) {
    setTxWrap("ready");
    showPreparedPair(txSession, false);
    setStatus(
      `TX paused · PAIR ${formatPairDisplay(txSession.pair_code)} unchanged · phone can keep waiting, then START STREAM`
    );
  } else {
    setTxWrap("idle");
    setPairBanner(
      els.txPair,
      els.txPairCode,
      "-----",
      "idle",
      "PAIR",
      "select a file to lock a pair code"
    );
  }
}

async function startRx() {
  const pair = normalizePairInput(els.pairInput.value);
  if (pair.length !== 5) {
    setStatus("Enter the 5-character PAIR code shown on TX first");
    els.pairInput.focus();
    return;
  }
  els.pairInput.value = formatPairDisplay(pair).replace("-", "");

  stopRx();
  downloadDone = false;
  didLockBuzz = false;
  lastHitTs = 0;
  rxSession = new RxSession();
  rxSession.set_expected_pair(pair);
  setAimState("seeking", "AIM QR INTO THE BOX");
  setPairBanner(
    els.rxPair,
    els.rxPairCode,
    pair,
    "idle",
    "PAIRING",
    "fill the pulsing box with the TX QR — corners turn green when locked"
  );
  renderMetrics({ pairCode: pair, locked: false });

  try {
    mediaStream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: {
        facingMode: { ideal: "environment" },
        width: { ideal: 1920 },
        height: { ideal: 1080 },
        frameRate: { ideal: 30, max: 60 },
      },
    });
  } catch (e) {
    setStatus(`Camera error: ${e}`);
    return;
  }

  // Keep preview unmirrored so left/right match the real world (QR alignment).
  els.cam.style.transform = "none";
  els.cam.srcObject = mediaStream;
  await els.cam.play();
  rxRunning = true;
  els.rxStart.disabled = true;
  els.rxStop.disabled = false;
  els.download.classList.add("hidden");
  setStatus(`RX · PAIR ${formatPairDisplay(pair)} · fill the box with the QR until HUD says LOCKED`);

  const scanCtx = els.scanCanvas.getContext("2d", {
    willReadFrequently: true,
    alpha: false,
  })!;

  let last = performance.now();
  let localCapture = 0;
  let localDecode = 0;
  let busy = false;
  let rxStartTs = performance.now();
  let lastProgress = 0;

  const loop = () => {
    if (!rxRunning || !rxSession) return;
    const vw = els.cam.videoWidth;
    const vh = els.cam.videoHeight;
    if (vw && vh && !busy) {
      busy = true;
      const scale = Math.min(1, SCAN_MAX_WIDTH / vw);
      const w = Math.max(1, Math.floor(vw * scale));
      const h = Math.max(1, Math.floor(vh * scale));
      if (els.scanCanvas.width !== w || els.scanCanvas.height !== h) {
        els.scanCanvas.width = w;
        els.scanCanvas.height = h;
      }
      const short = Math.min(vw, vh);
      const crop = short * 0.78;
      const sx = (vw - crop) / 2;
      const sy = (vh - crop) / 2;
      scanCtx.drawImage(els.cam, sx, sy, crop, crop, 0, 0, w, h);
      const rgba = scanCtx.getImageData(0, 0, w, h).data;
      const luma = new Uint8Array(w * h);
      for (let i = 0, j = 0; i < rgba.length; i += 4, j++) {
        luma[j] = (rgba[i] * 77 + rgba[i + 1] * 150 + rgba[i + 2] * 29) >> 8;
      }
      localCapture++;
      const status = rxSession.ingest_luma(w, h, luma);
      const nowHit = performance.now();

      if (status === "new" || status === "dup" || status === "red" || status === "complete") {
        localDecode++;
        lastHitTs = nowHit;
        const p = rxSession.progress;
        let etaLeft = "--";
        if (p > 0.02 && p < 1) {
          const elapsed = (performance.now() - rxStartTs) / 1000;
          const totalEst = elapsed / p;
          etaLeft = `~${fmtEta(totalEst * (1 - p))} left`;
        } else if (p >= 1) {
          etaLeft = "done";
        } else if (rxSession.k > 0) {
          const typ = (rxSession.k * 1.5 * TX_DWELL_MS) / 1000;
          etaLeft = `~${fmtEta(typ)} est`;
        }
        if (p > lastProgress) lastProgress = p;

        if (!didLockBuzz) {
          didLockBuzz = true;
          buzz([30, 40, 30]);
        }

        if (status === "complete" || p >= 1) {
          setAimState("done", "COMPLETE — DOWNLOAD", 1);
        } else if (p > 0) {
          setAimState(
            "receiving",
            `RECEIVING ${(p * 100).toFixed(0)}% · HOLD STEADY`,
            p
          );
        } else {
          setAimState("locked", "LOCKED — HOLD STEADY", 0.02);
        }

        setPairBanner(
          els.rxPair,
          els.rxPairCode,
          rxSession.pair_code || pair,
          "live",
          "PAIRED",
          `${rxSession.filename || "file"} · ${(p * 100).toFixed(0)}% · ${etaLeft}`
        );
        renderMetrics({ eta: etaLeft });
      } else if (status === "wrong_pair") {
        lastHitTs = nowHit;
        setAimState("mismatch", `WRONG PAIR · SAW ${formatPairDisplay(rxSession.pair_code)}`);
        setPairBanner(
          els.rxPair,
          els.rxPairCode,
          rxSession.pair_code,
          "mismatch",
          "WRONG PAIR",
          `saw ${formatPairDisplay(rxSession.pair_code)} — expected ${formatPairDisplay(pair)}`
        );
        buzz(80);
      } else if (status === "need_pair") {
        setAimState("idle", "ENTER PAIR CODE");
        setStatus("Enter pair code on RX");
      } else {
        // none — no QR in frame
        const since = nowHit - lastHitTs;
        if (rxSession.locked && since < 900) {
          setAimState(
            "receiving",
            `RECEIVING ${(rxSession.progress * 100).toFixed(0)}% · RE-AIM`,
            rxSession.progress
          );
        } else if (rxSession.locked && since >= 900) {
          setAimState("seeking", "LOST LOCK — CENTER THE QR", rxSession.progress);
        } else if (since < 500 && lastHitTs > 0) {
          setAimState("signal", "SIGNAL — CENTER / MOVE CLOSER");
        } else {
          setAimState("seeking", "AIM QR INTO THE BOX");
        }
      }

      if (status === "complete" && !downloadDone) {
        finishDownload();
      }

      renderMetrics({
        locked: rxSession.locked,
        progress: rxSession.progress,
        newF: Number(rxSession.new_frames),
        dupF: Number(rxSession.dup_frames),
        redF: Number(rxSession.red_frames),
        k: rxSession.k,
        symbolSize: rxSession.symbol_size,
        pairCode: rxSession.pair_code || pair,
        fileId: rxSession.filename || "--",
      });
      busy = false;
    }

    const now = performance.now();
    if (now - last >= 1000 && rxSession) {
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
    }
    rxRaf = requestAnimationFrame(loop);
  };
  rxRaf = requestAnimationFrame(loop);
}

function finishDownload() {
  if (!rxSession || !rxSession.complete || downloadDone) return;
  downloadDone = true;
  try {
    const result = rxSession.take_file() as unknown as {
      filename: string;
      data: Uint8Array;
      crc32: string;
      pairCode: string;
    };
    const copy = new Uint8Array(result.data.byteLength);
    copy.set(result.data);
    const blob = new Blob([copy], { type: "application/octet-stream" });
    const url = URL.createObjectURL(blob);
    els.download.href = url;
    els.download.download = result.filename || "received.bin";
    els.download.textContent = `DOWNLOAD ${result.filename}`;
    els.download.classList.remove("hidden");
    setPairBanner(
      els.rxPair,
      els.rxPairCode,
      result.pairCode,
      "live",
      "COMPLETE",
      `crc32=${result.crc32} · tap DOWNLOAD`
    );
    setAimState("done", "COMPLETE — TAP DOWNLOAD", 1);
    buzz([40, 50, 40, 50, 80]);
    setStatus(`RX complete · PAIR ${formatPairDisplay(result.pairCode)} · ${result.data.byteLength} bytes`);
    renderMetrics({ progress: 1, locked: true, pairCode: result.pairCode, eta: "done" });
  } catch (e) {
    downloadDone = false;
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
  if (!downloadDone) {
    setAimState("idle", "CAMERA OFF", 0);
  }
}

function resetRx() {
  stopRx();
  rxSession?.reset();
  rxSession = new RxSession();
  lastUseful = 0;
  goodputEma = 0;
  downloadDone = false;
  didLockBuzz = false;
  lastHitTs = 0;
  els.download.classList.add("hidden");
  const pair = normalizePairInput(els.pairInput.value);
  setAimState("idle", pair.length === 5 ? "OPEN CAMERA" : "ENTER PAIR CODE", 0);
  setPairBanner(
    els.rxPair,
    els.rxPairCode,
    pair || "-----",
    "idle",
    "WAITING",
    "enter TX pair code, then aim at QR"
  );
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
    pairCode: pair || "-----",
    eta: "--",
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
    // Pair code is created NOW — before streaming — so the phone can be set up first.
    prepareTxSession(buf, f.name);
  });

  els.txNewPair.addEventListener("click", () => {
    if (!fileBytes) return;
    prepareTxSession(fileBytes, fileName);
    setStatus(
      `New PAIR ${formatPairDisplay(txSession!.pair_code)} · re-enter on phone, then START STREAM`
    );
  });

  els.pairInput.addEventListener("input", () => {
    const n = normalizePairInput(els.pairInput.value);
    const caret = els.pairInput.selectionStart ?? n.length;
    els.pairInput.value = n;
    els.pairInput.setSelectionRange(Math.min(caret, n.length), Math.min(caret, n.length));
    renderMetrics({ pairCode: n || "-----" });
  });

  els.txStart.addEventListener("click", () => void startTx());
  els.txStop.addEventListener("click", () => {
    stopTx();
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
  setStatus(
    `WASM ready · capacity ceiling ${qr_capacity()} B · camera profile ≤200 B/frame · unmirrored RX`
  );
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
    pairCode: "-----",
    eta: "--",
  });
}

main().catch((e) => setStatus(`Boot failure: ${e}`));
