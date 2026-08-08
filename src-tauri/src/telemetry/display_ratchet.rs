//! The path-in-message ratchet — insertion-site hygiene, asserted in source.
//!
//! ## The gap this closes
//!
//! `sundayrec_core::telemetry::strip_absolute_paths` is a SCANNER over free
//! text, and a scanner has a documented blind spot: a path run ends at
//! whitespace, so a filename containing a space leaves its tail behind
//! (`~/Opptak/gudstjeneste 9. november.wav` → `<path> 9. november.wav` — a
//! service date on the wire, and one the endpoint's own validator does NOT
//! reject). The audit's verdict was that the real fix belongs where the name is
//! INSERTED into the message, not in the scrubber: a message that can become a
//! panic message — and therefore a crash-ring record, and therefore a telemetry
//! `CrashReport` — must be born without the path.
//! [`sundayrec_core::telemetry::telemetry_path`] is that fix.
//!
//! Nothing but a convention keeps the next `format!("… {}", p.display())` out
//! of a string that panics. Reviews do not reliably catch it: the leak looks
//! exactly like normal error handling, and the consequence (user content in a
//! crash report) is invisible until someone reads the wire. So the invariant is
//! asserted instead, in the style [`crate::commands::path_ratchet`] proved:
//! parse the crate's own sources and hold every use to a rule.
//!
//! ## The rule
//!
//! Every `.display()` in PRODUCTION code (test modules are cut away — a test's
//! panic runs in CI, never on an operator's machine) must be one of:
//!
//!   - **local logging** — the statement is a `tracing::…!`, `eprintln!` or
//!     `println!` call. The log file and stdout stay on the machine; telemetry
//!     never reads them (its wire surface is the crash ring + numeric/enum
//!     projections, see `telemetry::payload`);
//!   - **exempt with a reason** — listed in [`EXEMPT`] below. Today these are
//!     the `AppError` strings shown to the operator in the UI: an operator
//!     WANTS the path in "kunne ikke opprette opptaksmappe …", and command
//!     errors go to the renderer, not to the ring.
//!
//! Anything else fails this test, naming the file and line, until the author
//! either renders the path with `telemetry_path` (message can reach the ring)
//! or classifies the site here (it cannot). `.to_string_lossy()` is NOT
//! ratcheted: it is this codebase's argv/data-plumbing idiom, not its
//! message-formatting idiom, and paths that travel as DATA (episode lists, the
//! diagnose report) go to the renderer, never to the wire.

#![cfg(test)]

/// `.display()` sites that build operator-facing error strings, not
/// wire-reachable messages. Keyed by (path suffix, context marker); every entry
/// carries the reason it is safe.
///
/// The shared reason, spelled out once: an `AppError` returned from a command
/// or sent over an engine channel is rendered in the UI and (for the recorder)
/// written to `last-error.json`. Both stay local. Telemetry's only free-text
/// intake is the crash ring (`CrashReport.message/location/task`), which these
/// strings can only enter if some future code PANICS on them — and that route
/// is what `strip_absolute_paths` remains the safety net for.
const EXEMPT: &[(&str, &str, &str)] = &[
    (
        "recorder/concat.rs",
        "failed to write list",
        "AppError::Recording → UI + last-error.json; the operator needs the list path to clean up",
    ),
    (
        "recorder/concat.rs",
        "failed to copy",
        "AppError::Recording → UI; names the two files of a failed Windows copy fallback",
    ),
    (
        "recorder/concat.rs",
        "failed to rename",
        "AppError::Recording → UI; names the two files of a failed finalize rename",
    ),
    (
        "recorder/engine.rs",
        "kunne ikke opprette opptaksmappe",
        "AppError::Recording over the ready-channel → UI; the operator must see WHICH folder failed",
    ),
    (
        "commands/path_guard.rs",
        "cannot resolve the save folder",
        "AppError::Validation → renderer; tells the operator their save-folder setting is broken",
    ),
    (
        "commands/path_guard.rs",
        "path is outside the save folder",
        "AppError::Validation → renderer; the rejected path is the operator's own input",
    ),
];

/// One flagged `.display()` use.
struct Site {
    file: String,
    line: usize,
    context: String,
}

/// The statement-ish context around line `i` (0-based) of `lines`: the line
/// itself plus up to 12 preceding lines, stopping at the previous statement
/// boundary (a line ending in `;`, `{` or `}`). Enough to see which macro or
/// constructor the `.display()` feeds, which is what classification needs.
fn context_of(lines: &[&str], i: usize) -> String {
    let mut start = i;
    for back in 1..=12 {
        let Some(j) = i.checked_sub(back) else { break };
        let t = lines[j].trim_end();
        if t.ends_with(';') || t.ends_with('{') || t.ends_with('}') {
            break;
        }
        start = j;
    }
    lines[start..=i].join("\n")
}

/// Cut `text` down to its production prefix: everything before the first
/// `#[cfg(test)]`/`#![cfg(test)]` that gates a MODULE (the attribute's next
/// non-empty, non-comment, non-attribute line declares a `mod`). A
/// `#[cfg(test)]` on a single item (engine.rs has one) does not cut — only the
/// item's own line would wrongly survive, and no flagged pattern sits on an
/// attribute line.
fn production_prefix(lines: &[&str]) -> usize {
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if !(t.starts_with("#[cfg(test)]") || t.starts_with("#![cfg(test)]")) {
            continue;
        }
        if t.starts_with("#![cfg(test)]") {
            return i; // whole-file gate: nothing after is production
        }
        // Attribute form: look ahead for what it gates.
        for next in lines.iter().skip(i + 1) {
            let n = next.trim();
            if n.is_empty() || n.starts_with("//") || n.starts_with('#') {
                continue;
            }
            if n.starts_with("mod ")
                || n.starts_with("pub mod ")
                || n.starts_with("pub(crate) mod ")
            {
                return i;
            }
            break; // gates a single item — keep scanning below it
        }
    }
    lines.len()
}

/// Every production `.display()` site in `src/**/*.rs` that is neither local
/// logging nor exempt.
fn violations() -> Vec<Site> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    assert!(
        files.len() > 40,
        "the ratchet found only {} source files under {} — it is looking in the \
         wrong place, which would make it pass vacuously",
        files.len(),
        root.display()
    );

    let mut out = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        let cut = production_prefix(&lines);
        let rel = path
            .to_string_lossy()
            .replace('\\', "/")
            .rsplit("/src/")
            .next()
            .unwrap_or_default()
            .to_string();
        for (i, line) in lines[..cut].iter().enumerate() {
            let t = line.trim();
            if t.starts_with("//") || !t.contains(".display()") {
                continue;
            }
            let ctx = context_of(&lines, i);
            let local_log = ["tracing::", "eprintln!", "println!"]
                .iter()
                .any(|m| ctx.contains(m));
            if local_log {
                continue;
            }
            let exempt = EXEMPT
                .iter()
                .any(|(file, marker, _)| rel.ends_with(file) && ctx.contains(marker));
            if exempt {
                continue;
            }
            out.push(Site {
                file: rel.clone(),
                line: i + 1,
                context: ctx,
            });
        }
    }
    out
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
    out.sort();
}

#[test]
fn a_path_formatted_into_a_message_must_be_born_telemetry_safe() {
    let found = violations();
    let listing: String = found
        .iter()
        .map(|s| format!("\n  {}:{} —\n{}\n", s.file, s.line, s.context))
        .collect();
    assert!(
        found.is_empty(),
        "{} production `.display()` site(s) build a string that is neither local \
         logging nor classified in EXEMPT:\n{listing}\n\
         If the string can become a panic/setup-error message (and so a crash-ring \
         record and a telemetry CrashReport), render the path with \
         `sundayrec_core::telemetry::telemetry_path` and put the full path in a \
         `tracing::` line instead — the log stays local, the ring does not. If it \
         cannot reach the ring, add it to EXEMPT in {} with the reason.",
        found.len(),
        file!()
    );
}

#[test]
fn the_exempt_list_matches_reality() {
    // Every EXEMPT entry must still match a real site — a stale entry is an
    // allowlist hole waiting to hide a new leak behind an old reason.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    for (file, marker, reason) in EXEMPT {
        assert!(
            !reason.is_empty(),
            "EXEMPT entry {file}:{marker} carries no reason"
        );
        let hit = files.iter().any(|p| {
            p.to_string_lossy().replace('\\', "/").ends_with(file)
                && std::fs::read_to_string(p).is_ok_and(|t| t.contains(marker))
        });
        assert!(
            hit,
            "EXEMPT entry ({file}, {marker:?}) matches nothing — the site moved \
             or was fixed; remove the entry"
        );
    }
}
