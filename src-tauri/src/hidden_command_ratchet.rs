//! The hidden-console ratchet (F2-W2) — a test, and nothing else.
//!
//! ## The gap this closes
//!
//! `main.rs` carries `windows_subsystem = "windows"`, so on Windows this is a
//! GUI process with no console of its own. Every child we start is a CONSOLE
//! program — `ffmpeg.exe`, `ffprobe.exe`, `powershell.exe`, `powercfg.exe`,
//! `pgrep`-shaped helpers — and when a console child has no console to inherit,
//! `CreateProcess` ALLOCATES A NEW, VISIBLE ONE for it. So a plain
//! `tokio::process::Command::new(ffmpeg)` puts a black window on the operator's
//! screen: one per device enumeration, one that stands for the minutes a
//! delivery transcode runs, and — with video — one that stands for the entire
//! service. That last window is not cosmetic. A volunteer who tidies it away
//! sends ffmpeg `CTRL_CLOSE_EVENT`; ffmpeg exits; the recorder sees
//! `ffmpeg_exited` and the service recording is over.
//!
//! [`crate::util::hidden_command`] and [`crate::util::hidden_std_command`] add
//! `CREATE_NO_WINDOW` and are otherwise the identity function. Nothing but a
//! convention keeps the next spawn from skipping them, and the omission is
//! invisible to every macOS reviewer and every macOS/Linux CI lane: the code
//! compiles, the tests pass, and the window only ever appears on the church PC.
//! `windows-check` compiles the Windows lane but cannot see a missing flag
//! either — a raw spawn is perfectly valid Windows code.
//!
//! So the invariant is asserted instead, in the style
//! [`crate::commands::path_ratchet`] and [`crate::telemetry::display_ratchet`]
//! established: parse the workspace's own sources and hold every spawn to a
//! rule.
//!
//! ## The rule
//!
//! In `src-tauri/src` and `crates/*/src`, a `Command::new(` may appear only:
//!
//!   - inside the two helper functions in [`HELPER_FILE`] — they are what the
//!     rule exists to funnel everything through;
//!   - inside a `#[cfg(test)]`-gated region — a test's stray console window
//!     appears on a CI runner, never on an operator's machine;
//!   - or in [`EXEMPT`], with a reason. "It's fine" is not a reason.
//!
//! Platform-gated production spawns (`#[cfg(unix)]`, `#[cfg(target_os =
//! "macos")]`) are deliberately NOT exempt. Off Windows the helper is the
//! identity function, so routing them through it costs nothing at run time and
//! buys a rule with no exceptions — and an exception list is exactly where the
//! next raw spawn would hide, especially if such a block is ever widened to
//! Windows.
//!
//! `src-tauri/examples/` is out of scope, and that is a real distinction rather
//! than an oversight: an example is a console binary a developer starts FROM a
//! console and which inherits it, so it has no console to be given. Only what
//! ships inside the GUI process is scanned.
//!
//! ## How the sources are read
//!
//! Comments and string/char literals are blanked out first
//! ([`strip_to_code`]), keeping newlines, so line numbers survive and a
//! `Command::new` merely *mentioned* in prose (this file is full of them) can
//! never be flagged. `#[cfg(test)]` regions are then removed by brace-matching
//! the item each attribute gates ([`test_regions`]) rather than by cutting the
//! file at the first one: a file whose test module sits ABOVE later production
//! code must not go dark below it.

#![cfg(test)]

/// Spawns that must NOT go through the helpers, each with the reason.
///
/// Empty, and meant to stay that way — see the module docs on why a
/// platform-gated spawn is not a reason.
const EXEMPT: &[(&str, &str, &str)] = &[];

/// The workspace-relative file that defines the helpers.
const HELPER_FILE: &str = "src-tauri/src/util.rs";

/// The helper signatures inside [`HELPER_FILE`] that are allowed to name
/// `Command::new`. A raw spawn anywhere ELSE in that file is still a violation.
const HELPER_FNS: &[&str] = &["pub fn hidden_command(", "pub fn hidden_std_command("];

/// One raw spawn the ratchet objects to.
#[derive(Debug)]
struct Site {
    /// Workspace-relative, `/`-separated.
    file: String,
    /// 1-based.
    line: usize,
    text: String,
}

// ─────────────────────────────────────────────────────────────────────────────
//   Reading Rust source without being fooled by it
// ─────────────────────────────────────────────────────────────────────────────

/// Replace every byte inside a comment or a string/char literal with a space,
/// keeping newlines (and therefore every line number) exactly where they were.
///
/// Two jobs at once: a `Command::new` written in prose stops being a match, and
/// the brace counting [`test_regions`] does cannot be thrown off by a `"{"` in a
/// `format!` inside a test.
///
/// Handles what this tree's sources actually contain: `//` and nestable `/* */`
/// comments, normal strings with `\` escapes, raw strings (`r"…"`, `r#"…"#`, any
/// hash count), byte strings (the `b` is an ordinary ident char, so the `"`
/// branch takes over), and char literals — while leaving a LIFETIME tick alone,
/// which is the one place a naive `'`-to-`'` scan would eat real code.
fn strip_to_code(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = vec![b' '; b.len()];
    let mut i = 0usize;

    // Copy `b[from..to]` through untouched.
    macro_rules! keep {
        ($from:expr, $to:expr) => {
            out[$from..$to].copy_from_slice(&b[$from..$to]);
        };
    }
    // Blank `b[from..to]`, but keep its newlines so lines stay aligned.
    macro_rules! blank {
        ($from:expr, $to:expr) => {
            for k in $from..$to {
                if b[k] == b'\n' {
                    out[k] = b'\n';
                }
            }
        };
    }

    while i < b.len() {
        // ── line comment ────────────────────────────────────────────────────
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            let end = b[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(b.len(), |p| i + p);
            blank!(i, end);
            i = end;
            continue;
        }
        // ── block comment (nestable, per the Rust grammar) ───────────────────
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let start = i;
            let mut depth = 1usize;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            blank!(start, i);
            continue;
        }
        // ── raw string: r"…" / r#"…"# / r##"…"## (also br"…") ────────────────
        if b[i] == b'r' {
            let mut j = i + 1;
            let mut hashes = 0usize;
            while j < b.len() && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                // Not a raw string if `r` continues an identifier (`for r#foo`
                // is not a thing, but `my_r"x"` is not either — a preceding
                // ident char means this `r` belongs to that ident).
                let ident_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
                if !ident_before {
                    let start = i;
                    let closer: Vec<u8> = std::iter::once(b'"')
                        .chain(std::iter::repeat_n(b'#', hashes))
                        .collect();
                    let mut k = j + 1;
                    while k < b.len() {
                        if b[k..].starts_with(&closer) {
                            k += closer.len();
                            break;
                        }
                        k += 1;
                    }
                    blank!(start, k.min(b.len()));
                    i = k.min(b.len());
                    continue;
                }
            }
        }
        // ── normal string ───────────────────────────────────────────────────
        if b[i] == b'"' {
            let start = i;
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            blank!(start, i.min(b.len()));
            continue;
        }
        // ── char literal, but never a lifetime ───────────────────────────────
        if b[i] == b'\'' {
            if let Some(end) = char_literal_end(b, i) {
                blank!(i, end);
                i = end;
                continue;
            }
            // A lifetime tick (`'a`, `'static`, `'_`): ordinary code.
        }
        keep!(i, i + 1);
        i += 1;
    }

    // Every byte we touched is ASCII or was copied verbatim from valid UTF-8,
    // so this cannot fail; `from_utf8_lossy` rather than `unwrap` keeps the
    // ratchet from being the thing that panics.
    String::from_utf8_lossy(&out).into_owned()
}

/// If a `'` at `i` opens a CHAR LITERAL, the index just past its closing quote.
/// `None` for a lifetime tick — the distinction is `'x'` (closed) versus `'x`
/// (not).
fn char_literal_end(b: &[u8], i: usize) -> Option<usize> {
    debug_assert_eq!(b[i], b'\'');
    // `'\n'`, `'\''`, `'\u{1F600}'` — an escape runs to the next unescaped `'`.
    if b.get(i + 1) == Some(&b'\\') {
        let mut k = i + 2;
        while k < b.len() && b[k] != b'\'' {
            k += 1;
        }
        return (k < b.len()).then_some(k + 1);
    }
    // `'a'` — exactly one char (which may be multi-byte UTF-8) then a `'`.
    let mut k = i + 1;
    // Walk one UTF-8 scalar.
    if k >= b.len() {
        return None;
    }
    k += 1;
    while k < b.len() && (b[k] & 0b1100_0000) == 0b1000_0000 {
        k += 1;
    }
    (b.get(k) == Some(&b'\'')).then_some(k + 1)
}

/// Byte offset of the start of each line, for offset → line-number lookups.
fn line_starts(code: &str) -> Vec<usize> {
    let mut v = vec![0usize];
    v.extend(code.bytes().enumerate().filter_map(
        |(i, c)| {
            if c == b'\n' {
                Some(i + 1)
            } else {
                None
            }
        },
    ));
    v
}

/// 0-based line index containing byte `offset`.
fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

/// The inclusive, 0-based line ranges gated by `#[cfg(test)]` in `code`
/// (already stripped). `None` means the WHOLE file is test-only
/// (`#![cfg(test)]`).
///
/// Each attribute's item is found by brace-matching from the first `{` that
/// follows it — or, when a `;` comes first, by ending at that `;` (a
/// `#[cfg(test)] use …;` / `mod foo;`). Ranges are collected independently, so a
/// test module in the MIDDLE of a file blinds the ratchet to nothing below it.
fn test_regions(code: &str) -> Option<Vec<(usize, usize)>> {
    let starts = line_starts(code);
    let lines: Vec<&str> = code.lines().collect();
    let mut regions = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("#![cfg(test)]") {
            return None;
        }
        // ONLY a bare `#[cfg(test)]`. `#[cfg(any(feature = "editor", test))]`
        // compiles in production builds and must keep being checked.
        if t != "#[cfg(test)]" {
            continue;
        }
        // Search from the line AFTER the attribute.
        let from = *starts.get(i + 1).unwrap_or(&code.len());
        let rest = &code[from..];
        let brace = rest.find('{');
        let semi = rest.find(';');
        // A `{` before any `;` means the attribute gates a braced item (mod,
        // fn, impl); a `;` first means it gates a `use`/`mod foo;` one-liner.
        let end_off = match (brace, semi) {
            (Some(bo), Some(so)) if so < bo => from + so,
            (None, Some(so)) => from + so,
            (Some(bo), _) => {
                match_brace(code, from + bo).unwrap_or_else(|| code.len().saturating_sub(1))
            }
            (None, None) => code.len().saturating_sub(1),
        };
        regions.push((i, line_of(&starts, end_off)));
    }
    Some(regions)
}

/// Index of the `}` closing the `{` at `open`, counting braces in `code`
/// (already stripped, so no brace inside a string or comment can confuse it).
fn match_brace(code: &str, open: usize) -> Option<usize> {
    let b = code.as_bytes();
    debug_assert_eq!(b[open], b'{');
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// Inclusive, 0-based line ranges of the helper functions in [`HELPER_FILE`].
fn helper_regions(code: &str) -> Vec<(usize, usize)> {
    let starts = line_starts(code);
    let mut out = Vec::new();
    for needle in HELPER_FNS {
        let Some(at) = code.find(needle) else {
            continue;
        };
        let Some(open) = code[at..].find('{').map(|o| at + o) else {
            continue;
        };
        let close = match_brace(code, open).unwrap_or_else(|| code.len().saturating_sub(1));
        out.push((line_of(&starts, at), line_of(&starts, close)));
    }
    out
}

/// Every `Command::new(` in `code` (already stripped) that is real code and not
/// part of a longer identifier — `PlannedCommand::new(` is a different type, and
/// flagging it would teach people to work around the ratchet.
fn spawn_offsets(code: &str) -> Vec<usize> {
    let b = code.as_bytes();
    let needle = "Command::new(";
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = code[from..].find(needle) {
        let at = from + rel;
        from = at + 1;
        let joined_left = at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_');
        if !joined_left {
            out.push(at);
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//   Walking the workspace
// ─────────────────────────────────────────────────────────────────────────────

/// The workspace root — the parent of `src-tauri`.
fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri always has a parent")
        .to_path_buf()
}

/// Every `.rs` file the rule covers: the shell's `src` and every crate's `src`.
///
/// A crate's `tests/` directory is skipped for the same reason a
/// `#[cfg(test)]` region is: an integration test is test code by construction —
/// it has no non-test build — and its console, if any, is a CI runner's.
fn sources() -> Vec<std::path::PathBuf> {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rs(&root.join("src-tauri/src"), &mut files);
    collect_rs(&root.join("crates"), &mut files);
    files.retain(|p| !relative(p).contains("/tests/"));
    files.sort();
    files
}

fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// `path` relative to the workspace root, `/`-separated.
fn relative(path: &std::path::Path) -> String {
    let root = workspace_root();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every raw spawn that is neither a helper definition, nor test code, nor
/// exempt.
fn violations() -> Vec<Site> {
    let mut out = Vec::new();
    for path in sources() {
        let rel = relative(&path);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let code = strip_to_code(&text);
        let Some(test_ranges) = test_regions(&code) else {
            continue; // whole file is `#![cfg(test)]`
        };
        let helper_ranges = if rel == HELPER_FILE {
            helper_regions(&code)
        } else {
            Vec::new()
        };
        let starts = line_starts(&code);
        let raw_lines: Vec<&str> = text.lines().collect();

        for offset in spawn_offsets(&code) {
            let line0 = line_of(&starts, offset);
            let in_range =
                |ranges: &[(usize, usize)]| ranges.iter().any(|&(a, z)| line0 >= a && line0 <= z);
            if in_range(&test_ranges) || in_range(&helper_ranges) {
                continue;
            }
            let text_of_line = raw_lines.get(line0).unwrap_or(&"").trim().to_string();
            if EXEMPT
                .iter()
                .any(|(file, marker, _)| rel.ends_with(file) && text_of_line.contains(marker))
            {
                continue;
            }
            out.push(Site {
                file: rel.clone(),
                line: line0 + 1,
                text: text_of_line,
            });
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//   The assertions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_scanner_actually_reads_the_workspace() {
    // A scanner that quietly found nothing would make every assertion below a
    // no-op. Pin a floor, and pin that BOTH roots were reached.
    let files = sources();
    assert!(
        files.len() > 100,
        "the ratchet found only {} source files — it is looking in the wrong \
         place, which would make it pass vacuously",
        files.len()
    );
    for root in ["src-tauri/src/", "crates/"] {
        assert!(
            files.iter().any(|p| relative(p).starts_with(root)),
            "no source file under {root} was scanned"
        );
    }
    // …and that it can see the thing it is looking for at all. `util.rs` is the
    // one file that legitimately contains raw `Command::new`s (two per helper —
    // one per cfg branch), so it is also the positive control: the detector must
    // find them, and the helper regions must be what covers them. If a raw spawn
    // ever appears in util.rs OUTSIDE a helper, this is what says so.
    let util = files
        .iter()
        .find(|p| relative(p) == HELPER_FILE)
        .expect("util.rs must be among the scanned files");
    let code = strip_to_code(&std::fs::read_to_string(util).unwrap());
    let starts = line_starts(&code);
    let regions = helper_regions(&code);
    let offsets = spawn_offsets(&code);
    assert!(
        offsets.len() >= HELPER_FNS.len(),
        "the detector found {} Command::new in {HELPER_FILE} — the helpers \
         themselves contain at least {}, so it is not matching real source",
        offsets.len(),
        HELPER_FNS.len()
    );
    for offset in offsets {
        let line0 = line_of(&starts, offset);
        assert!(
            regions.iter().any(|&(a, z)| line0 >= a && line0 <= z),
            "{HELPER_FILE}:{} spawns outside both helper functions",
            line0 + 1
        );
    }
}

#[test]
fn no_production_spawn_can_open_a_console_window() {
    let found = violations();
    let listing: String = found
        .iter()
        .map(|s| format!("\n  {}:{} — {}", s.file, s.line, s.text))
        .collect();
    assert!(
        found.is_empty(),
        "{} raw process spawn(s) bypass the hidden-console helpers:\n{listing}\n\n\
         ────────────────────────────────────────────────────────────────────\n\
         On Windows this app is a GUI process (`windows_subsystem = \"windows\"` \
         in main.rs) with no console. Starting a console child — ffmpeg, \
         ffprobe, powershell, powercfg — without CREATE_NO_WINDOW makes Windows \
         allocate a VISIBLE console for it. During a service that window stands \
         for the whole recording, and a volunteer who closes it sends ffmpeg \
         CTRL_CLOSE_EVENT: ffmpeg exits, and the recording ends.\n\n\
         Do ONE of these:\n\n\
         1. ROUTE IT THROUGH THE HELPER (the default). Replace\n\
            `tokio::process::Command::new(x)` with `crate::util::hidden_command(x)`,\n\
            or `std::process::Command::new(x)` with\n\
            `crate::util::hidden_std_command(x)`. Nothing else changes: off \
         Windows both are the identity function, and the arguments, stdio and \
         kill-on-drop behaviour are untouched.\n\n\
         2. MOVE IT INTO TEST CODE, if it is a fixture. A spawn inside a \
         `#[cfg(test)]` region is not checked — its console appears on a CI \
         runner, not on the church PC.\n\n\
         3. EXEMPT IT in src-tauri/src/hidden_command_ratchet.rs, with a reason. \
         `#[cfg(unix)]` is NOT a reason — the helper is free there, and a rule \
         with no exceptions is the only kind this test can keep.\n\
         ────────────────────────────────────────────────────────────────────",
        found.len()
    );
}

#[test]
fn the_helpers_still_apply_create_no_window() {
    // The other direction. A future edit could leave every call site pointing
    // at helpers that no longer set the flag, and everything above would still
    // be green — the ratchet would be guarding an empty box.
    let util = workspace_root().join(HELPER_FILE);
    let text = std::fs::read_to_string(&util)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", util.display()));
    let code = strip_to_code(&text);

    assert!(
        text.contains("0x0800_0000"),
        "{HELPER_FILE} no longer names CREATE_NO_WINDOW's value (0x0800_0000)"
    );
    assert!(
        !code.contains("DETACHED_PROCESS") && !text.contains("0x0000_0008"),
        "the helpers must not use DETACHED_PROCESS: the child has to keep our \
         piped stdio (the recorder reads ffmpeg's stderr and writes `q` to its \
         stdin) and stay in our Job Object"
    );

    for (region, needle) in helper_regions(&code).iter().zip(HELPER_FNS) {
        let body: String = code
            .lines()
            .skip(region.0)
            .take(region.1 - region.0 + 1)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("cfg(windows)"),
            "`{needle}` no longer gates its flag on cfg(windows)"
        );
        assert!(
            body.contains("creation_flags(CREATE_NO_WINDOW)"),
            "`{needle}` no longer applies CREATE_NO_WINDOW"
        );
    }
    assert_eq!(
        helper_regions(&code).len(),
        HELPER_FNS.len(),
        "one of the helper functions in {HELPER_FILE} was renamed or removed"
    );
}

#[test]
fn every_exemption_carries_a_reason_and_still_matches_something() {
    for (file, marker, reason) in EXEMPT {
        assert!(
            reason.trim().len() >= 30,
            "the exemption for {file}:{marker} needs a real reason, not `{reason}`"
        );
        let hit = sources().iter().any(|p| {
            relative(p).ends_with(file)
                && std::fs::read_to_string(p).is_ok_and(|t| t.contains(marker))
        });
        assert!(
            hit,
            "EXEMPT entry ({file}, {marker:?}) matches nothing — the site moved \
             or was fixed; remove the entry rather than leave an open door"
        );
    }
}

// ── The parser's own tests ───────────────────────────────────────────────────

#[test]
fn stripping_removes_prose_and_literals_without_moving_a_single_line() {
    let src = "let a = \"Command::new(x)\"; // Command::new(y)\n\
               /* Command::new(z)\n   still a comment */\n\
               let r = r#\"Command::new(w) \"#;\n\
               let brace = \"{\"; // an unbalanced brace inside a string\n\
               real();\n";
    let code = strip_to_code(src);
    assert_eq!(
        code.lines().count(),
        src.lines().count(),
        "line count must survive stripping, or every reported line is wrong"
    );
    assert!(
        spawn_offsets(&code).is_empty(),
        "a Command::new inside a string or comment must not be a match: {code}"
    );
    assert!(code.contains("real();"), "real code must survive: {code}");
    assert!(
        !code.contains('{'),
        "the brace inside the string literal must be gone, or brace-matching \
         drifts: {code}"
    );
    // A lifetime must not be mistaken for a char literal and eat the code after
    // it (the one shape a naive `'`-to-`'` scan destroys).
    let lt = strip_to_code("fn f<'a>(x: &'a str) { Command::new(x); }");
    assert_eq!(spawn_offsets(&lt).len(), 1, "lifetimes ate the code: {lt}");
    let ch = strip_to_code("let q = '\\''; Command::new(x);");
    assert_eq!(spawn_offsets(&ch).len(), 1, "escaped char literal: {ch}");
}

/// Run the real detector over a source fixture, as `violations` would.
fn flagged(src: &str) -> Vec<usize> {
    let code = strip_to_code(src);
    let Some(test_ranges) = test_regions(&code) else {
        return Vec::new();
    };
    let starts = line_starts(&code);
    spawn_offsets(&code)
        .into_iter()
        .map(|o| line_of(&starts, o))
        .filter(|l| !test_ranges.iter().any(|&(a, z)| *l >= a && *l <= z))
        .map(|l| l + 1)
        .collect()
}

#[test]
fn the_detector_flags_a_raw_spawn_and_only_a_raw_spawn() {
    let flags = flagged(
        "use tokio::process::Command;\n\
         fn a() { tokio::process::Command::new(\"ffmpeg\"); }\n\
         fn b() { std::process::Command::new(\"ffprobe\"); }\n\
         fn c() { Command::new(\"powershell\"); }\n\
         fn d() { PlannedCommand::new(\"pmset\", &[], 3); }\n\
         fn e() { crate::util::hidden_command(\"ffmpeg\"); }\n\
         // fn f() { Command::new(\"commented out\"); }\n",
    );
    assert_eq!(
        flags,
        vec![2, 3, 4],
        "the three raw spawns — and NOT PlannedCommand::new, the helper call, \
         or the commented-out line"
    );
}

#[test]
fn a_spawn_inside_a_cfg_test_region_is_not_flagged() {
    let flags = flagged(
        "fn prod() { crate::util::hidden_command(\"ffmpeg\"); }\n\
         #[cfg(test)]\n\
         mod tests {\n\
             fn fixture() { tokio::process::Command::new(\"sleep\").spawn(); }\n\
             fn braces() { let s = format!(\"{}\", 1); }\n\
         }\n",
    );
    assert!(
        flags.is_empty(),
        "test-module spawn must be ignored: {flags:?}"
    );

    // A single gated ITEM, not a module — and a gated `use`, which has no braces
    // at all.
    let flags = flagged(
        "#[cfg(test)]\n\
         use std::process::Command;\n\
         #[cfg(test)]\n\
         impl Fake {\n\
             fn go() { Command::new(\"true\"); }\n\
         }\n",
    );
    assert!(
        flags.is_empty(),
        "gated item spawn must be ignored: {flags:?}"
    );
}

#[test]
fn a_test_module_does_not_blind_the_ratchet_to_production_code_below_it() {
    // The blind spot of "cut the file at the first #[cfg(test)]": everything
    // after the cut goes unchecked. Several files in this tree put a test module
    // in the middle, so this is not hypothetical.
    let flags = flagged(
        "#[cfg(test)]\n\
         mod tests {\n\
             fn fixture() { Command::new(\"sleep\"); }\n\
         }\n\
         fn added_later() { tokio::process::Command::new(\"ffmpeg\"); }\n",
    );
    assert_eq!(
        flags,
        vec![5],
        "a production spawn BELOW a test module must still be flagged"
    );
}

#[test]
fn a_cfg_that_merely_mentions_test_is_still_production() {
    // `#[cfg(any(feature = "editor", test))]` compiles in real builds — the
    // editor's ffmpeg spawns live behind exactly this attribute.
    let flags = flagged(
        "#[cfg(any(feature = \"editor\", test))]\n\
         fn export() { tokio::process::Command::new(\"ffmpeg\"); }\n",
    );
    assert_eq!(flags, vec![2], "a feature-or-test gate is not a test gate");
}

#[test]
fn a_whole_file_gated_with_an_inner_attribute_is_test_code() {
    let flags = flagged("#![cfg(test)]\nfn f() { Command::new(\"true\"); }\n");
    assert!(
        flags.is_empty(),
        "`#![cfg(test)]` covers the file: {flags:?}"
    );
}
