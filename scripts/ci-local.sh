#!/usr/bin/env bash
# Run the exact gate CI runs (.github/workflows/ci.yml) locally on this Mac, so
# you can verify a branch BEFORE tagging. CI now triggers only on v* tags, so a
# green run here means a green release CI without spending any Actions minutes.
#
# Benign differences from CI (everything else is identical):
#   - runs on your arch (aarch64-apple-darwin), not the ubuntu x86 runner
#   - skips `npm ci` — uses your existing node_modules (run `npm ci` yourself if
#     you suspect dependency drift from package-lock.json)
#   - skips the apt system deps (webkit/gtk/alsa) — already present on macOS
#
# Each step below is the same command CI runs, in the same order. Reuses the
# package.json scripts so this mirror can't silently drift from them.
set -euo pipefail
cd "$(dirname "$0")/.."

CURRENT="startup"
trap 'printf "\n\033[1;31m✗ CI FAILED at: %s\033[0m\n" "$CURRENT" >&2' ERR
step() { CURRENT="$1"; printf "\n\033[1;36m▶ %s\033[0m\n" "$1"; }

step "ffmpeg/ffprobe sidecars";        npm run ffmpeg

step "frontend — eslint";              npm run lint
step "frontend — prettier --check";    npm run format:check
step "frontend — tsc --noEmit";        npm run typecheck
step "frontend — vitest";              npm run test

step "app version in sync";            npm run version-sync

step "rust — cargo fmt --check";       npm run fmt:rust:check
step "rust — cargo clippy -D warnings"; npm run lint:rust
step "rust — cargo test --workspace";  npm run test:rust

# status --porcelain (not diff): also catches brand-new binding files, which
# are untracked and invisible to `git diff`.
step "ts-rs bindings up to date";      npm run bindings
if [ -n "$(git status --porcelain -- legacy/bindings)" ]; then
  printf "\033[1;31m✗ ts-rs bindings are stale — regenerate and commit:\033[0m\n"
  git status --porcelain -- legacy/bindings
  exit 1
fi

step "tauri build (no bundle)";        npm run tauri build -- --no-bundle

CURRENT="done"
printf "\n\033[1;32m✓ all CI checks passed locally — safe to tag a release\033[0m\n"
