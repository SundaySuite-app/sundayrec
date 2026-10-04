fn main() {
    // Expose the build target triple to the crate so tests can locate the
    // fetched ffmpeg sidecar at `binaries/<name>-<triple>` (the suffix
    // scripts/fetch-ffmpeg.mjs uses). Cargo sets `TARGET` for build scripts.
    println!(
        "cargo:rustc-env=SUNDAYREC_TARGET_TRIPLE={}",
        std::env::var("TARGET").unwrap_or_default()
    );

    // Link AVFoundation on macOS so the camera/mic authorization query
    // (`media::permissions`) can resolve `AVCaptureDevice` at runtime. Without it
    // the class lookup returns `None` and we degrade to "Unknown" (proceed) — this
    // is what makes the TCC pre-check actually functional. `CARGO_CFG_TARGET_OS`
    // reflects the BUILD TARGET (unlike `cfg!`, which would read the host).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }

    // The webview holds NO updater permission. This used to GENERATE
    // `capabilities/updater.generated.json` (`updater:default`) whenever the
    // `updater` feature was on, which gave the page `plugin:updater|check` — with
    // `allowDowngrades`, `proxy` and `headers` — and so a way to fetch an older,
    // validly signed release without the fixes of the newer ones (the #314
    // review, S2). The frontend never used it: updating goes through Rust's own
    // `update_check`/`update_install`, which call the plugin from Rust and need
    // no capability. `commands::recordings_open`'s tripwire fails if any
    // capability names `updater:` again.
    //
    // What is left is the clean-up: a checkout that built before this change
    // still has the generated file (it is git-ignored, so nothing else removes
    // it), and a capability the build no longer owns must not keep granting.
    let _ = std::fs::remove_file("capabilities/updater.generated.json");

    // The vendored VAD model must be byte-identical to the one the code was
    // written against. A swapped or truncated ONNX graph does NOT crash a VAD —
    // it returns confident, wrong probabilities — so the check happens at the
    // earliest moment there is: before the bytes are ever linked in. Only under
    // `--features vad` (build scripts see enabled features as CARGO_FEATURE_*),
    // so a default build pays nothing.
    if std::env::var_os("CARGO_FEATURE_VAD").is_some() {
        use sha2::{Digest, Sha256};
        // Duplicated from `sundayrec_core::vad::{VAD_MODEL_FILE_NAME,
        // VAD_MODEL_SHA256}` because a build script cannot depend on a workspace
        // member. `build_script_pins_the_same_digest_as_the_core` in
        // src/vad/mod.rs reads these two literals back out of this file and
        // fails if they ever drift from the core constants.
        const MODEL: &str = "resources/vad/silero_vad_op18_ifless.onnx";
        const SHA256: &str = "7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28";
        println!("cargo:rerun-if-changed={MODEL}");
        let bytes = std::fs::read(MODEL)
            .unwrap_or_else(|e| panic!("--features vad needs the vendored model at {MODEL}: {e}"));
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex, SHA256,
            "{MODEL} does not match the pinned SHA-256 — the checkout is corrupt or the model \
             was swapped. Re-fetch it from the URL in sundayrec_core::vad::VAD_MODEL_SOURCE_URL."
        );
    }

    tauri_build::build()
}
