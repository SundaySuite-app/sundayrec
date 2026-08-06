#!/usr/bin/env node
// Download static ffmpeg + ffprobe into src-tauri/binaries/ with the Rust
// target-triple suffix Tauri's `externalBin` expects (e.g.
// ffmpeg-aarch64-apple-darwin). Run before `tauri build`, both locally and in
// CI (each platform's runner fetches its own binaries).
//
// ── WHY NOT npm ────────────────────────────────────────────────────────────
// This used to pull `ffmpeg-static` + `@ffprobe-installer/ffprobe`.
// `ffmpeg-static` has been frozen at ffmpeg 6.0 (2023) for years, and its
// binary is not even in the npm tarball — a postinstall step downloads it from
// GitHub Releases with no integrity check, which failed and truncated often
// enough on CI to need its own retry loop. Both packages are gone; we now
// fetch VERSION-PINNED archives straight from the two build servers below and
// verify them by SHA-256 before a single byte is unpacked.
//
// ── SOURCES (see docs/DISTRIBUTION.md for the licensing note) ──────────────
// macOS + Linux  ffmpeg.martin-riedl.de — release channel, per-binary .zip,
//                signed + notarized on macOS, publishes a .sha256 per archive.
// Windows        gyan.dev release "essentials" build — the long-standing
//                Windows ffmpeg distribution, versioned archives kept in
//                /builds/packages/, .sha256 published alongside.
// Both are GPL builds, same as what we shipped before. `essentials` carries
// everything this app asks ffmpeg for (native aac/flac/pcm/mjpeg, libmp3lame,
// libx264, dshow, lavfi, and every filter we use is built-in).
//
// ── PINNING (two layers) ──────────────────────────────────────────────────
// 1. ARCHIVE: each entry below carries the publisher's SHA-256 for the .zip.
//    A mismatch is a hard failure — a moved or altered download never gets
//    unpacked. These are frozen per FFMPEG_VERSION; bumping the version means
//    re-reading the publisher's .sha256 files.
// 2. BUNDLED BINARY: `scripts/ffmpeg-checksums.json` maps `<name>-<host>` to
//    the hash of the file that actually ships. A pinned entry that mismatches
//    is a hard failure; a MISSING entry logs the computed hash and proceeds,
//    so a platform can be pinned from a trusted run's log (that is how the
//    Windows pins were captured — see the note in that file).
//
// Archives are cached under node_modules/.cache/ so `npm run dev` doesn't
// re-download ~60 MB every launch; an already-correct sidecar short-circuits
// the whole script. Pass --force to re-fetch anyway.

import { execFileSync, spawnSync } from "node:child_process";
import {
  mkdirSync,
  writeFileSync,
  readFileSync,
  existsSync,
  chmodSync,
  rmSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { inflateRawSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { dirname, join, basename } from "node:path";

// The ffmpeg release this app is built and tested against. 9.0 shipped
// 2026-08-04 and is deliberately NOT used: a two-day-old major is not
// something a Sunday-morning recording should depend on.
const FFMPEG_VERSION = "8.1.2";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "src-tauri", "binaries");
const cacheDir = join(root, "node_modules", ".cache", "sundayrec-ffmpeg");
const force = process.argv.includes("--force");

// ── Download table ─────────────────────────────────────────────────────────

// martin-riedl publishes one .zip per binary, holding a single file named
// `ffmpeg` / `ffprobe` at the archive root. `build` is the immutable
// `<unix-ts>_<version>` directory from the host's release history page.
function martinRiedl(label, build, sha) {
  const entry = (name) => ({
    url: `https://ffmpeg.martin-riedl.de/download/${build}/${name}.zip`,
    sha256: sha[name],
    member: name,
  });
  return {
    label: `ffmpeg.martin-riedl.de ${label} (release ${FFMPEG_VERSION})`,
    binaries: { ffmpeg: entry("ffmpeg"), ffprobe: entry("ffprobe") },
  };
}

// gyan ships both binaries in one archive under `<stem>/bin/<name>.exe`.
function gyan(stem, sha256) {
  const url = `https://www.gyan.dev/ffmpeg/builds/packages/${stem}.zip`;
  const entry = (name) => ({
    url,
    sha256,
    member: `${stem}/bin/${name}.exe`,
  });
  return {
    label: `gyan.dev ${stem} (release ${FFMPEG_VERSION})`,
    binaries: { ffmpeg: entry("ffmpeg"), ffprobe: entry("ffprobe") },
  };
}

const SOURCES = {
  // Ships in the macOS release.
  "aarch64-apple-darwin": martinRiedl(
    "macOS arm64",
    "macos/arm64/1783011502_8.1.2",
    {
      ffmpeg:
        "ef1aa60006c7b77ce170c1608c08d8e4ba1c30c5746f2ac986ded932d0ac2c3c",
      ffprobe:
        "c39787f4af7a3932502d2d48db6f6feaaa836b48a73ef78c32cc3285df61dfaf",
    },
  ),
  // Intel macs — not built today, kept so a universal build is one line away.
  "x86_64-apple-darwin": martinRiedl(
    "macOS x86_64",
    "macos/amd64/1783018342_8.1.2",
    {
      ffmpeg:
        "a52ef43883f44c219766d4b3bdde4e635b35465d0b704c01c3a0566b59775df9",
      ffprobe:
        "5408ca588c8c72b0dde3afe676d0a7acf25ef97e55ae6eba5c7bede1cda42695",
    },
  ),
  // CI only — the ubuntu job needs a real sidecar so the self-skipping
  // real-ffmpeg smokes actually RUN. Nothing is shipped from Linux.
  "x86_64-unknown-linux-gnu": martinRiedl(
    "Linux x86_64",
    "linux/amd64/1783011670_8.1.2",
    {
      ffmpeg:
        "56452c0bfc4ee0325cd615d62f46ba8264f62eed34f727c2224c6c84fa7b8719",
      ffprobe:
        "c6f2d36e98f9a4445fad0b0be539f4c4faf13fd502116bf131becd53f56cd390",
    },
  ),
  "aarch64-unknown-linux-gnu": martinRiedl(
    "Linux arm64",
    "linux/arm64/1783010599_8.1.2",
    {
      ffmpeg:
        "ab9e16864b6bf4ae7e13bbdbdc29621be11a5c547c57af8d4250e9fa2f5e6461",
      ffprobe:
        "fb78317b81cdeb614533be59e489019b754afd199670666af28f0e9574be395b",
    },
  ),
  // Ships in the Windows release.
  "x86_64-pc-windows-msvc": gyan(
    `ffmpeg-${FFMPEG_VERSION}-essentials_build`,
    "db580001caa24ac104c8cb856cd113a87b0a443f7bdf47d8c12b1d740584a2ec",
  ),
};

// ── Host triple ────────────────────────────────────────────────────────────

function hostTriple() {
  let out;
  try {
    out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  } catch {
    fail(
      "rustc is not on PATH — this script reads the host target triple from " +
        "`rustc -vV`.\n  Install Rust (https://rustup.rs) and re-run.",
    );
  }
  const line = out.split("\n").find((l) => l.startsWith("host:"));
  if (!line)
    fail("`rustc -vV` printed no `host:` line — cannot name the sidecars.");
  return line.slice("host:".length).trim();
}

function fail(msg) {
  console.error(`✗ ${msg}`);
  process.exit(1);
}

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

// ── Archive download (cached, integrity-checked) ───────────────────────────

async function archive(url, expected) {
  const cached = join(
    cacheDir,
    `${expected.slice(0, 16)}-${basename(new URL(url).pathname)}`,
  );
  if (existsSync(cached) && !force) {
    const buf = readFileSync(cached);
    if (sha256(buf) === expected) {
      console.log(
        `  · cached ${basename(cached)} (${(buf.length / 1e6).toFixed(1)} MB)`,
      );
      return buf;
    }
    rmSync(cached, { force: true });
  }

  let last;
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      const res = await fetch(url, {
        redirect: "follow",
        // Generous: these are 30–110 MB archives on a church's connection.
        signal: AbortSignal.timeout(15 * 60 * 1000),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText}`);
      const buf = Buffer.from(await res.arrayBuffer());
      const actual = sha256(buf);
      if (actual !== expected) {
        throw new Error(
          `SHA-256 mismatch\n    expected ${expected}\n    actual   ${actual}\n` +
            `    (${buf.length} bytes — a truncated download retries, a stable ` +
            `mismatch means the published archive changed)`,
        );
      }
      mkdirSync(cacheDir, { recursive: true });
      writeFileSync(cached, buf);
      console.log(`  · downloaded ${(buf.length / 1e6).toFixed(1)} MB`);
      return buf;
    } catch (err) {
      last = err;
      console.warn(`  ⚠ attempt ${attempt}/3 failed: ${err.message}`);
    }
  }
  fail(`could not fetch ${url}\n  ${last?.message ?? "unknown error"}`);
}

// ── Minimal zip reader ─────────────────────────────────────────────────────
// Pure Node (zlib) on purpose: `unzip`, `tar`, `7z` and Expand-Archive are all
// present on SOME of our three runners, none on all three with the same flags.
// The archives are plain 32-bit zips with deflate or stored entries.

const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

function crc32(buf) {
  let c = -1;
  for (let i = 0; i < buf.length; i++)
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

function zipEntries(zip) {
  // End-of-central-directory lives in the last 64 KB (comment can pad it).
  let eocd = -1;
  for (let i = zip.length - 22; i >= Math.max(0, zip.length - 65557); i--) {
    if (zip.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0)
    fail("archive is not a zip (no end-of-central-directory record)");

  const count = zip.readUInt16LE(eocd + 10);
  let p = zip.readUInt32LE(eocd + 16);
  if (p === 0xffffffff)
    fail("zip64 archive — this reader only handles 32-bit zips");

  const entries = [];
  for (let i = 0; i < count && zip.readUInt32LE(p) === 0x02014b50; i++) {
    const nameLen = zip.readUInt16LE(p + 28);
    entries.push({
      name: zip.toString("utf8", p + 46, p + 46 + nameLen),
      method: zip.readUInt16LE(p + 10),
      crc: zip.readUInt32LE(p + 16),
      csize: zip.readUInt32LE(p + 20),
      usize: zip.readUInt32LE(p + 24),
      offset: zip.readUInt32LE(p + 42),
    });
    p += 46 + nameLen + zip.readUInt16LE(p + 30) + zip.readUInt16LE(p + 32);
  }
  return entries;
}

function extract(zip, member) {
  const entries = zipEntries(zip);
  const leaf = member.split("/").pop();
  const hits = entries.filter(
    (e) => e.name === member || e.name.endsWith(`/${leaf}`),
  );
  if (hits.length !== 1) {
    fail(
      `expected exactly one \`${member}\` in the archive, found ${hits.length}.\n` +
        `  Archive holds: ${entries
          .map((e) => e.name)
          .slice(0, 12)
          .join(", ")}${entries.length > 12 ? ", …" : ""}`,
    );
  }
  const e = hits[0];
  if (e.offset === 0xffffffff || e.csize === 0xffffffff) {
    fail(`\`${e.name}\` uses zip64 fields — unsupported`);
  }
  if (zip.readUInt32LE(e.offset) !== 0x04034b50) {
    fail(`\`${e.name}\` has a corrupt local header`);
  }
  const start =
    e.offset +
    30 +
    zip.readUInt16LE(e.offset + 26) +
    zip.readUInt16LE(e.offset + 28);
  const raw = zip.subarray(start, start + e.csize);

  let out;
  if (e.method === 0) out = Buffer.from(raw);
  else if (e.method === 8)
    out = inflateRawSync(raw, { maxOutputLength: e.usize });
  else fail(`\`${e.name}\` uses unsupported compression method ${e.method}`);

  if (out.length !== e.usize) {
    fail(`\`${e.name}\` unpacked to ${out.length} bytes, expected ${e.usize}`);
  }
  if (crc32(out) !== e.crc) fail(`\`${e.name}\` failed its CRC-32 check`);
  return out;
}

// ── Verification ───────────────────────────────────────────────────────────

// `<bin> -version` exits 0 and names itself AND its version — the cheapest
// proof the file is a real, runnable executable for this arch (not a truncated
// download or an HTML error page saved under the binary's name) AND that it is
// the ffmpeg the Rust arg builders and output parsers were tested against.
// spawnSync never throws and runs no shell, so paths with spaces are safe.
function probe(bin, name) {
  if (!existsSync(bin)) return null;
  const r = spawnSync(bin, ["-version"], { encoding: "utf8", timeout: 60_000 });
  if (r.error || r.status !== 0) return null;
  const m = `${r.stdout}${r.stderr}`.match(/^(ffmpeg|ffprobe) version (\S+)/m);
  if (!m || m[1] !== name) return null;
  // BtbN-style builds prefix the tag with `n`; martin-riedl/gyan don't.
  const version = m[2].replace(/^n/, "");
  return version.startsWith(FFMPEG_VERSION) ? version : null;
}

const checksums = JSON.parse(
  readFileSync(join(root, "scripts", "ffmpeg-checksums.json"), "utf8"),
);

// Returns true when the bytes are pinned and match (or unpinned — logged).
function checkPin(name, host, buf) {
  const key = `${name}-${host}`;
  const actual = sha256(buf);
  const expected = checksums[key];
  if (expected && actual !== expected) {
    fail(
      `${name}: SHA-256 mismatch for ${key}\n` +
        `  expected ${expected}\n` +
        `  actual   ${actual}\n` +
        `  The unpacked binary is not the pinned one — refusing to bundle it.`,
    );
  }
  if (expected) {
    console.log(`  ✓ ${name}: SHA-256 verified (${key})`);
  } else {
    console.warn(
      `  ⚠ ${name}: no pinned SHA-256 for ${key} — computed ${actual}\n` +
        `    Pin it by adding "${key}": "${actual}" to scripts/ffmpeg-checksums.json`,
    );
  }
  return actual;
}

// ── Main ───────────────────────────────────────────────────────────────────

const host = hostTriple();
const ext = host.includes("windows") ? ".exe" : "";
const source = SOURCES[host];
if (!source) {
  fail(
    `no ffmpeg ${FFMPEG_VERSION} download is configured for host \`${host}\`.\n` +
      `  Known hosts: ${Object.keys(SOURCES).join(", ")}\n` +
      `  Add one to the SOURCES table in scripts/fetch-ffmpeg.mjs.`,
  );
}

mkdirSync(outDir, { recursive: true });
console.log(`ffmpeg ${FFMPEG_VERSION} for ${host} — ${source.label}`);

for (const [name, spec] of Object.entries(source.binaries)) {
  const dest = join(outDir, `${name}-${host}${ext}`);

  // Fast path: the right binary is already there. `npm run dev` runs this
  // script on every launch — it must not re-download 60 MB each time.
  if (!force) {
    const have = probe(dest, name);
    if (
      have &&
      (!checksums[`${name}-${host}`] ||
        sha256(readFileSync(dest)) === checksums[`${name}-${host}`])
    ) {
      console.log(`✓ ${name} ${have} already in place`);
      continue;
    }
  }

  console.log(`→ ${name}`);
  const zip = await archive(spec.url, spec.sha256);
  const bin = extract(zip, spec.member);
  checkPin(name, host, bin);

  writeFileSync(dest, bin);
  chmodSync(dest, 0o755);

  const version = probe(dest, name);
  if (!version) {
    rmSync(dest, { force: true });
    fail(
      `${name}: the unpacked binary does not run, or is not ffmpeg ${FFMPEG_VERSION}.\n` +
        `  Source: ${spec.url}\n` +
        `  (removed it — a broken sidecar must never reach a build)`,
    );
  }
  console.log(
    `✓ ${name} ${version} → src-tauri/binaries/${name}-${host}${ext} ` +
      `(${(bin.length / 1e6).toFixed(1)} MB, runs)`,
  );
}
