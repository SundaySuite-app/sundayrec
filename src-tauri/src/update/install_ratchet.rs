//! The update-install ratchet (F2-W1) — a test, and nothing else.
//!
//! ## The gap this closes
//!
//! `update.download_and_install(…)` is a perfectly ordinary line to write. It
//! is the plugin's own headline API, it is what every tauri example shows, and
//! on macOS — the only machine this project is developed and reviewed on — it
//! does precisely what it says: fetch, verify, swap the bundle, return.
//!
//! On Windows it does not return. `tauri-plugin-updater` 2.11.0 extracts the
//! installer, starts it with `ShellExecuteW` and calls `std::process::exit(0)`
//! from inside the call ([`updater.rs` `install_inner`]). That exit:
//!
//!   * skipped every line the seam wrote after it — the `ReadyToInstall`
//!     status, the wait for the recording's finalisation, the app's own exit
//!     cleanup — so all of them were macOS-only truths;
//!   * closed our only handle to the kill-on-close Job Object, so the OS
//!     killed the installer that had just started unpacking. NSIS never
//!     reached `.onInstSuccess`, so the `/R` restart never happened either;
//!   * took the live recording with it, with no dialog and no log line.
//!
//! No Mac reviewer could see it, no Mac test could reach it, and
//! `windows-check` compiles the call perfectly happily: it is valid Windows
//! code. So the invariant is asserted instead, in the style
//! [`crate::hidden_command_ratchet`] (#237) and
//! [`crate::commands::path_ratchet`] established — parse the seam's own
//! sources and hold them to a rule.
//!
//! ## The two rules
//!
//! 1. **No `download_and_install(` anywhere under `src-tauri/src/update/`.**
//!    The seam calls `Update::download` and `Update::install` separately, so
//!    it decides WHEN the installer starts ([`super::INSTALL_IS_DEFERRED`]).
//!    Re-combining them puts `exit(0)` back inside the download.
//!
//! 2. **No `SILENT_BREAKAWAY` anywhere under `src-tauri/src/`.** It is the
//!    obvious-looking way to let the installer survive, and it is the wrong
//!    one: `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` takes every FUTURE child out
//!    of the job — that is the ffmpeg orphan guard, removed for the rest of
//!    the session rather than for the last second of it. The right lever is
//!    [`crate::platform::disarm_kill_on_close`], which clears the limit and
//!    keeps every process in the job.
//!
//! Prose and string literals are blanked out first (the shared
//! [`crate::hidden_command_ratchet::strip_to_code`]), so this module's own
//! docs — which necessarily name both forbidden spellings — cannot trip it.

#![cfg(test)]

use crate::hidden_command_ratchet::{
    line_of, line_starts, match_brace, strip_to_code, workspace_root,
};

/// The combined call the seam must never make again, and why.
const COMBINED_CALL: &str = "download_and_install(";

/// The wrong way to let a child outlive us.
const WRONG_BREAKAWAY: &str = "SILENT_BREAKAWAY";

/// One offending line.
#[derive(Debug)]
struct Site {
    /// Workspace-relative, `/`-separated.
    file: String,
    /// 1-based.
    line: usize,
    text: String,
}

/// Every `.rs` file under `dir`, recursively, sorted.
fn rs_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// `path` relative to the workspace root, `/`-separated.
fn relative(path: &std::path::Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every occurrence of `needle` in the real CODE of `files` — comments and
/// string literals do not count, so this module's own documentation of the
/// forbidden spelling is not a violation of it.
fn hits(files: &[std::path::PathBuf], needle: &str) -> Vec<Site> {
    let mut out = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let code = strip_to_code(&text);
        let starts = line_starts(&code);
        let raw: Vec<&str> = text.lines().collect();
        let mut from = 0usize;
        while let Some(rel) = code[from..].find(needle) {
            let at = from + rel;
            from = at + 1;
            let line0 = line_of(&starts, at);
            out.push(Site {
                file: relative(path),
                line: line0 + 1,
                text: raw.get(line0).unwrap_or(&"").trim().to_string(),
            });
        }
    }
    out
}

/// The stripped source of one workspace file.
fn code_of(rel: &str) -> String {
    let path = workspace_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    strip_to_code(&text)
}

/// The body of the braced item that `header` opens, from already-stripped
/// `code`. Panics with a readable message when the header is gone — a renamed
/// function must fail loudly here rather than make an assertion vacuous.
fn body_after(code: &str, header: &str, what: &str) -> String {
    let at = code
        .find(header)
        .unwrap_or_else(|| panic!("`{header}` is gone — {what} cannot be checked any more"));
    let open = code[at..]
        .find('{')
        .map(|o| at + o)
        .unwrap_or_else(|| panic!("`{header}` has no body"));
    let close = match_brace(code, open).unwrap_or_else(|| panic!("`{header}`'s body never closes"));
    code[open..=close].to_string()
}

/// The byte offset of `needle` in `body`, or a panic naming what was missing.
fn at_of(body: &str, needle: &str, what: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is missing from {what}"))
}

fn listing(found: &[Site]) -> String {
    found
        .iter()
        .map(|s| format!("\n  {}:{} — {}", s.file, s.line, s.text))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
//   The assertions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_scanner_actually_reads_the_seam() {
    // A scanner that quietly found nothing would make both rules below
    // vacuous. Pin that the directory exists, that it holds the module the
    // rules are about, and that the detector can find its needle in real code.
    let files = rs_files(&workspace_root().join("src-tauri/src/update"));
    assert!(
        !files.is_empty(),
        "no source file under src-tauri/src/update was scanned — the ratchet \
         is looking in the wrong place"
    );
    assert!(
        files.iter().any(|p| relative(p).ends_with("update/mod.rs")),
        "the seam itself (update/mod.rs) was not among the scanned files"
    );

    // Positive control: the detector must find a needle that IS in real code,
    // and must NOT find one that only appears in prose. `INSTALL_IS_DEFERRED`
    // is named in this file's docs above and defined in `update/mod.rs`.
    let real = hits(&files, "INSTALL_IS_DEFERRED");
    assert!(
        !real.is_empty(),
        "the detector found no `INSTALL_IS_DEFERRED` in update/mod.rs — it is \
         not matching real source at all"
    );
    let prose_only = hits(
        &rs_files(&workspace_root().join("src-tauri/src/update"))
            .into_iter()
            .filter(|p| relative(p).ends_with("install_ratchet.rs"))
            .collect::<Vec<_>>(),
        COMBINED_CALL,
    );
    assert!(
        prose_only.is_empty(),
        "the stripper let this module's OWN documentation of `{COMBINED_CALL}` \
         count as code — every assertion below would then be unfalsifiable:{}",
        listing(&prose_only)
    );
}

#[test]
fn the_seam_never_downloads_and_installs_in_one_call() {
    let files = rs_files(&workspace_root().join("src-tauri/src/update"));
    let found = hits(&files, COMBINED_CALL);
    assert!(
        found.is_empty(),
        "{} call(s) to `{COMBINED_CALL}` in the update seam:{}\n\n\
         ────────────────────────────────────────────────────────────────────\n\
         On Windows `Update::download_and_install` DOES NOT RETURN. The plugin \
         extracts the installer, starts it with ShellExecuteW and calls \
         `std::process::exit(0)` from inside the call \
         (tauri-plugin-updater 2.11.0, updater.rs `install_inner`).\n\n\
         That exit is the F2-W1 bug in one line: everything below the call \
         becomes macOS-only (the ReadyToInstall status, the wait for the \
         recording's finalisation, RunEvent::ExitRequested's cleanup), and \
         closing our last job-object handle kills the installer we just \
         started — so no Windows install could ever update itself.\n\n\
         Call `Update::download(…)` and `Update::install(bytes)` separately \
         instead, and let `INSTALL_IS_DEFERRED` decide when the second half \
         may run. On Windows that is `relaunch_now`, and nowhere else.\n\
         ────────────────────────────────────────────────────────────────────",
        found.len(),
        listing(&found)
    );
}

#[test]
fn nothing_lets_children_break_away_from_the_orphan_guard() {
    let files = rs_files(&workspace_root().join("src-tauri/src"));
    let found = hits(&files, WRONG_BREAKAWAY);
    assert!(
        found.is_empty(),
        "{} use(s) of `{WRONG_BREAKAWAY}`:{}\n\n\
         ────────────────────────────────────────────────────────────────────\n\
         `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` looks like the way to let the \
         update installer survive our exit. It is not: it takes every FUTURE \
         child OUT of the job object, which is the ffmpeg orphan guard itself \
         — removed for the rest of the session rather than for the last \
         second of it. A force-quit would then leave ffmpeg holding the audio \
         device, which is the incident `platform::guard_child_processes` \
         exists for.\n\n\
         Use `platform::disarm_kill_on_close()` instead. It clears the \
         kill-on-close limit and keeps every process in the job, and it is \
         called from exactly one place: immediately before the installer is \
         started, after the recorder has stopped.\n\
         ────────────────────────────────────────────────────────────────────",
        found.len(),
        listing(&found)
    );
}

#[test]
fn install_is_called_from_exactly_the_two_moments_that_are_allowed() {
    // The rule the two above cannot state: `install(` may appear in the seam,
    // but only at the two moments that have been reasoned about — right after
    // the download on a platform whose installer RETURNS, and in
    // `relaunch_now` on one whose installer ends the process. A third site is
    // a moment nobody waited for.
    let seam = workspace_root().join("src-tauri/src/update/mod.rs");
    let text = std::fs::read_to_string(&seam)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", seam.display()));
    let code = strip_to_code(&text);
    let starts = line_starts(&code);

    let mut sites = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = code[from..].find(".install(") {
        let at = from + rel;
        from = at + 1;
        sites.push(line_of(&starts, at) + 1);
    }
    assert_eq!(
        sites.len(),
        2,
        "expected exactly two `.install(` sites in update/mod.rs — the \
         immediate one for platforms whose installer returns, and the deferred \
         one in `relaunch_now`. Found {sites:?}. A third is a moment nobody \
         has reasoned about; a first-and-only means the platform split was \
         collapsed."
    );
}

#[test]
fn the_finalisation_wait_ends_in_the_install_path_and_not_in_a_bare_restart() {
    // THE Windows invariant, and the one no Mac test can otherwise reach:
    // `AfterWait::Relaunch` — the arm that runs once the recording's file is
    // safe — must go through `relaunch_now`, because that is the only function
    // that consults the staged bytes. An arm that called `app.restart()` (or
    // `app.exit`) directly would restart into the OLD version on Windows and
    // look perfectly correct on macOS, where the install already happened
    // during the download.
    let window = code_of("src-tauri/src/window.rs");
    let arm = body_after(
        &window,
        "AfterWait::Relaunch =>",
        "the updater's end of the finalisation wait",
    );
    assert!(
        arm.contains("relaunch_now"),
        "the `AfterWait::Relaunch` arm no longer calls `relaunch_now` — on \
         Windows the staged installer would never be started:\n{arm}"
    );
    for forbidden in [".restart()", ".exit("] {
        assert!(
            !arm.contains(forbidden),
            "the `AfterWait::Relaunch` arm calls `{forbidden}` directly, \
             stepping over the staged install:\n{arm}"
        );
    }
}

#[test]
fn the_installer_is_only_started_after_the_guard_has_let_go() {
    // The sequence inside `relaunch_now`, as a rule rather than as a comment.
    // Every step is invisible from macOS and each one alone is the whole bug:
    //
    //   * asking `has_staged()` BEFORE disarming keeps the ffmpeg guard on a
    //     plain restart, where no installer needs protecting;
    //   * disarming BEFORE the install is what stops the OS killing the
    //     installer along with us (F2-W1's second layer);
    //   * the log line BEFORE the install is the only thing that will ever be
    //     written — `install` ends in `std::process::exit(0)`.
    let seam = code_of("src-tauri/src/update/mod.rs");
    let body = body_after(&seam, "fn relaunch_now", "the one place that may install");

    let staged_at = at_of(&body, "has_staged()", "`relaunch_now`");
    let disarm_at = at_of(&body, "disarm_kill_on_close()", "`relaunch_now`");
    let install_at = at_of(&body, ".install(", "`relaunch_now`");
    let restart_at = at_of(&body, "app.restart()", "`relaunch_now`");
    // The log line is found by its CALL, not by its text: `strip_to_code`
    // blanks string literals (that is what keeps this file's own prose from
    // tripping the other rules), so the sentence itself is not there to match.
    let logs_the_handover = body[disarm_at..install_at].contains("relaunch_log(");

    assert!(
        staged_at < disarm_at,
        "`relaunch_now` disarms the job object before it knows whether an \
         installer is even waiting — a plain restart would then drop the \
         ffmpeg orphan guard on its way out"
    );
    assert!(
        disarm_at < install_at,
        "`relaunch_now` starts the installer before disarming the kill-on-close \
         job object — the OS would kill it with us, which IS F2-W1"
    );
    assert!(
        logs_the_handover,
        "nothing is written to update-relaunch.log between the disarm and the \
         install. That line must exist and must come FIRST: everything after \
         `install` is the plugin's own `std::process::exit(0)`, so a Windows \
         update that fails would leave the log empty again — which is exactly \
         why F2-W1 took three releases to find"
    );
    assert!(
        install_at < restart_at,
        "`app.restart()` comes before the staged install — the process would be \
         replaced by the OLD binary while the new one sat in memory"
    );
}
