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

    tauri_build::build()
}
