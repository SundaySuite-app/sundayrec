# Needs Richard — Electron-parity seams (PU-1…PU-4)

The pure decision logic for these features is ported into `sundayrec-core` and
fully unit-tested; the impure seams compile behind **default-off** cargo
features (`email`, `tray`, `publish`) or are already wired (scheduler/wake). The
items below need a real account / desktop session / device that the headless
gate cannot provide. None block the default build or the gate.

## PU-1 — Email alerts (`--features email`)

- **A Gmail OAuth connection or SMTP credentials.** The Gmail path reuses the
  cloud OAuth refresh token (connect Gmail first); the SMTP path needs a host,
  port, user, and app-password. There is no UI to enter SMTP settings yet — the
  Tauri `Settings` struct still defers the `email*` fields to Fase 6, so the
  seam (`src-tauri/src/email/mod.rs`) takes its transport config as explicit
  parameters. Wiring the Settings fields + a `send_test_email` command is the
  remaining glue once the email card lands in the UI.
- **Deliverability check.** Confirm a real "✓ email works" message arrives and
  the throttle suppresses a 2nd identical alert within 10 min (smoke §8).

## PU-2 — Tray + deep links (`--features tray`)

- **A desktop session.** The native menubar/tray item and the `sundayrec://`
  scheme registration (`tauri-plugin-deep-link`) need a real GUI to verify.
- **Tray icon assets.** The Electron app shipped `tray-idle/recording/error`
  PNGs (+ macOS `Template` + Windows dark variants) under `assets/`. The Tauri
  build needs equivalent assets bundled and a `tray.rs` shell that maps
  `sundayrec_core::tray::{build_menu, icon_for, tooltip}` to `tauri::menu` +
  `tauri::tray::TrayIconBuilder` and wires each `TrayAction` to its command/
  event. The model + routing are unit-tested; the menubar shell is the glue.
- **Scheme registration in `tauri.conf.json`** (`plugins.deep-link.desktop.schemes
= ["sundayrec"]`) + the macOS `Info.plist` `CFBundleURLTypes` entry, then the
  `lib.rs` `setup` hook calling `parse_deep_link` on each inbound URL.

## PU-3 — Podcast RSS publish (`--features publish`)

- **A connected Drive + a public-share capable account.** The orchestration
  (write `podcast.xml`, upload via the existing resumable worker, create a
  public share URL, cache the feed URL) needs a real Drive connection and
  network. Only the XML builder (`sundayrec_core::feed`) is tested.
- A `publish` seam module + the share-URL helper on the Drive worker are the
  remaining glue (the Electron `createPublicShareUrl` / `uploadFile` path).

## PU-4 — OS wake-timers + scheduled launch (no feature flag)

- **A real Mac/Windows box.** The scheduler supervisor's wall-clock timing, the
  `pmset`/`osascript`/`powershell`/`powercfg` shell-outs, the admin/UAC prompts,
  and whether the machine _truly_ wakes from sleep are all HARDWARE-UNVERIFIED.
  The next-fire / catch-up / missed / wake-point decisions are unit-tested in
  `sundayrec_core::{schedule, wake}`; this is the "validated on a real rig" exit
  the migration tracks (smoke §11).
- **Missed-recording persistence** still waits on a `status`/`error` column on
  the `recording` table (see the `scheduler/mod.rs` honest-gaps note).
