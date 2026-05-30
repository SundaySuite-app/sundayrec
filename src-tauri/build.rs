fn main() {
    // Expose the build target triple to the crate so tests can locate the
    // fetched ffmpeg sidecar at `binaries/<name>-<triple>` (the suffix
    // scripts/fetch-ffmpeg.mjs uses). Cargo sets `TARGET` for build scripts.
    println!(
        "cargo:rustc-env=SUNDAYREC_TARGET_TRIPLE={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    tauri_build::build()
}
