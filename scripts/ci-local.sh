#!/usr/bin/env bash
# Run the exact gate CI runs (.github/workflows/ci.yml) locally on this Mac, so
# you can verify a branch BEFORE pushing (CI runs on every push to main and
# every PR — the repo is public, so Actions minutes are free).
#
# Benign differences from CI (everything else is identical):
#   - runs on your arch (aarch64-apple-darwin), not the ubuntu x86 runner
#   - skips `npm ci` — uses your existing node_modules (run `npm ci` yourself if
#     you suspect dependency drift from package-lock.json)
#   - skips the apt system deps (webkit/gtk/alsa) — already present on macOS
#   - does NOT mirror CI's separate `audit` job (npm audit + cargo audit) or the
#     `windows-check` job — advisories/Windows compile surface on the PR instead
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
step "i18n fallbacks match no.json";   npm run i18n-fallbacks

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

step "command reachability regression"; npm run reachability

# The feature-off degradation path must keep COMPILING (ci.yml's "cargo check
# (feature-off build)" step) — cheap with the shared incremental cache.
step "rust — cargo check (feature-off)"; cargo check --workspace --no-default-features

# ci.yml's "Rust clippy + tests (vad feature)" step. E9's neural VAD is
# default-off, so EVERY step above compiles the seam out — these tests are the
# only place the model's failure modes (a 512-sample window instead of 576, `sr`
# at 8000, inputs bound by index, a lost symbolic batch dim) are ever exercised,
# and the only place the feature-ON side is held to `-D warnings`. It was
# missing here while ci.yml had it, so this mirror reported green over an
# untested seam — exactly the "a step in only one place" gap this script exists
# to prevent.
step "rust — clippy + tests (vad feature)"
cargo clippy --workspace --all-targets --features vad -- -D warnings
cargo test --workspace --features vad vad::

step "tauri build (no bundle)";        npm run tauri build -- --no-bundle

CURRENT="done"
printf "\n\033[1;32m✓ all CI checks passed locally — safe to tag a release\033[0m\n"
