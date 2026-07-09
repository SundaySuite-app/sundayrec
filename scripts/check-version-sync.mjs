// Fail when package.json, src-tauri/Cargo.toml and src-tauri/tauri.conf.json
// disagree about the app version. The release flow bumps package.json with
// `npm version` and relies on a hand-edit of the other two (see
// .github/workflows/release.yml) — this check makes a forgotten edit loud,
// both locally (`npm run check`) and in CI.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");

const pkg = JSON.parse(read("package.json")).version;
const conf = JSON.parse(read("src-tauri/tauri.conf.json")).version;
// First `version = "…"` in the [package] section (the manifest's own version
// always precedes the [dependencies] table).
const cargoMatch = read("src-tauri/Cargo.toml").match(
  /^version\s*=\s*"([^"]+)"/m,
);
const cargo = cargoMatch?.[1];

const versions = {
  "package.json": pkg,
  "src-tauri/Cargo.toml": cargo,
  "src-tauri/tauri.conf.json": conf,
};
const unique = new Set(Object.values(versions));

if (unique.size !== 1 || unique.has(undefined)) {
  console.error("✗ app version is out of sync:");
  for (const [file, v] of Object.entries(versions)) {
    console.error(`    ${file}: ${v ?? "«not found»"}`);
  }
  console.error(
    "  Bump all three to the same version (npm version bumps only package.json).",
  );
  process.exit(1);
}
console.log(
  `✓ version ${pkg} is in sync across package.json / Cargo.toml / tauri.conf.json`,
);
