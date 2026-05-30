#!/usr/bin/env node
// Copy static ffmpeg + ffprobe into src-tauri/binaries/ with the Rust
// target-triple suffix Tauri's `externalBin` expects (e.g.
// ffmpeg-aarch64-apple-darwin). Run before `tauri build`, both locally and
// in CI (each platform's runner copies its own binaries).
//
// Binaries come from the `ffmpeg-static` + `@ffprobe-installer/ffprobe` npm
// packages — GPL/LGPL ffmpeg builds. See docs/DISTRIBUTION.md for the
// licensing note before any public release.

import { execSync } from "node:child_process";
import { mkdirSync, copyFileSync, chmodSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const ffmpegSrc = require("ffmpeg-static");
const ffprobeSrc = require("@ffprobe-installer/ffprobe").path;

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "src-tauri", "binaries");

// Rust host triple — what `externalBin` matches against.
const host = execSync("rustc -vV", { encoding: "utf8" })
  .split("\n")
  .find((l) => l.startsWith("host:"))
  .slice("host:".length)
  .trim();
const ext = host.includes("windows") ? ".exe" : "";

mkdirSync(outDir, { recursive: true });

for (const [name, src] of [
  ["ffmpeg", ffmpegSrc],
  ["ffprobe", ffprobeSrc],
]) {
  if (!src || !existsSync(src)) {
    console.error(
      `✗ ${name}: source binary missing (${src}). Run \`npm install\` first.`,
    );
    process.exit(1);
  }
  const dest = join(outDir, `${name}-${host}${ext}`);
  copyFileSync(src, dest);
  chmodSync(dest, 0o755);
  console.log(`✓ ${name} → src-tauri/binaries/${name}-${host}${ext}`);
}
