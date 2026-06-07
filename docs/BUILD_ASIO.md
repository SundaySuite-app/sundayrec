# Building SundayRec with ASIO (Windows)

ASIO support is a **Windows-only, default-off** Cargo feature (`asio`). It turns
on cpal's `asio` backend so the recorder can open a pro audio interface as one
low-latency multichannel device (the DirectShow/WASAPI path splits such cards
into stereo pairs). macOS/Linux never compile the ASIO path — this document is
only relevant when building the Windows installer with `--features asio`.

This file is the **canonical env-setup fasit** for both local Windows builds and
the GitHub Actions Windows job (Fase 7). Keep it in sync with what actually had
to be set for the build to succeed.

> **The driver is not ours.** ASIO talks to the manufacturer's ASIO driver
> (Soundcraft/Focusrite/ASIO4ALL/…); it does not replace it. The end user must
> have that driver installed. For a build/test machine, **ASIO4ALL** is enough.

---

## cpal version

We currently build ASIO on **cpal 0.15** (the version already used for VU
metering). The riskiest unknown is whether `cpal/asio` links on our toolchain —
prove it with the spike (below) before building anything on top.

> If 0.15's ASIO path fails to compile/link, bump cpal toward **0.17.3** (0.17.1
> had broken ASIO; 0.17.3 carries the linker fixes). A bump is a breaking change
> for `src-tauri/src/audio/{vu,devices}.rs` and must be re-verified. **Record the
> version that actually worked here** when this is settled on a real Windows box.

---

## Required toolchain on Windows

| Requirement                                                                          | Why                                                     | Env var                                            |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------- | -------------------------------------------------- |
| **MSVC C++ build tools** (Visual Studio Build Tools, "Desktop development with C++") | cpal's asio-sys compiles C++ glue against the SDK       | (loaded via `vcvars64.bat`)                        |
| **LLVM / Clang**                                                                     | asio-sys uses bindgen to generate the ASIO FFI bindings | `LIBCLANG_PATH` → e.g. `C:\Program Files\LLVM\bin` |
| **Steinberg ASIO SDK**                                                               | the proprietary headers cpal binds against              | `CPAL_ASIO_DIR` → the extracted SDK root           |

### 1. Steinberg ASIO SDK

Download the ASIO SDK from Steinberg (free, proprietary licence — attribution
required, see `docs/` license note / about-box). Extract it, e.g. to
`C:\asiosdk`, and point cpal at it:

```powershell
$env:CPAL_ASIO_DIR = "C:\asiosdk"
```

`CPAL_ASIO_DIR` must contain the SDK's `common`, `host`, `driver` folders.

### 2. LLVM / Clang (for bindgen)

Install LLVM (e.g. `choco install llvm` or the official installer) and set:

```powershell
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
```

### 3. MSVC environment

Run the build from a **Developer PowerShell for VS**, or first load vcvars:

```powershell
& "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
```

---

## Prove it: the spike

```powershell
cd src-tauri
cargo run --example asio_spike --features asio
```

Expected: the ASIO host opens and at least one device is listed with its input
channel count and supported sample rates. (`ASIO4ALL` shows up as one device
wrapping the system audio endpoints.) If this fails, fix the env above before
touching any recorder code — the rest of the feature builds on this.

---

## Building the app with ASIO

```powershell
# from repo root
npm run tauri build -- --features asio
```

macOS builds must **not** pass `--features asio` (the path is compiled out and
Core Audio already exposes aggregate devices as one).

---

## Notes for CI (Fase 7)

The Windows job must, before `cargo`/`tauri build`:

1. Install LLVM, set `LIBCLANG_PATH`.
2. Download + extract the ASIO SDK, set `CPAL_ASIO_DIR` (download in-step — do
   not commit the SDK to the repo, for licence cleanliness).
3. Load the MSVC environment (the `ilammy/msvc-dev-cmd` action or vcvars).
4. Build with `--features asio`; cache cargo + the SDK download.

The macOS job stays exactly as-is (no `asio` feature).
