import { spawnSync } from "node:child_process";
import { mkdirSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "web", "src", "wasm");

mkdirSync(outDir, { recursive: true });

const cargo = spawnSync(
  "cargo",
  [
    "build",
    "-p",
    "fountain-core",
    "--release",
    "--target",
    "wasm32-unknown-unknown",
  ],
  { cwd: root, stdio: "inherit", shell: true }
);
if (cargo.status !== 0) process.exit(cargo.status ?? 1);

const wasmPath = join(
  root,
  "target",
  "wasm32-unknown-unknown",
  "release",
  "fountain_core.wasm"
);
if (!existsSync(wasmPath)) {
  console.error("missing wasm artifact:", wasmPath);
  process.exit(1);
}

const bindgen = spawnSync(
  "wasm-bindgen",
  [
    wasmPath,
    "--out-dir",
    outDir,
    "--target",
    "web",
    "--out-name",
    "fountain_core",
  ],
  { cwd: root, stdio: "inherit", shell: true }
);
if (bindgen.status !== 0) process.exit(bindgen.status ?? 1);

console.log("WASM ready →", outDir);
