//! The path-guard coverage ratchet (E1.3) — a test, and nothing else.
//!
//! E1.2 closed twelve `#[tauri::command]`s that took a filesystem path and never
//! validated it. Nothing stopped the thirteenth. Reviews do not reliably catch
//! "this new command takes a `path` and forgot the guard": the omission looks
//! exactly like normal code, and the consequence (an arbitrary read, an
//! arbitrary write, an upload of a file nobody chose) is invisible until someone
//! goes looking.
//!
//! So the invariant is asserted instead. This module parses the crate's OWN
//! sources — `src/commands/*.rs`, reachable at test time through
//! `CARGO_MANIFEST_DIR` — finds every `#[tauri::command]` whose parameters look
//! like paths, and requires each one to appear in exactly one of two lists
//! below: [`GUARDED`] or [`EXEMPT`] (with a mandatory reason). A new command is a
//! failing test until a human has classified it.
//!
//! The style is the one [`sundayrec_core::timeouts`] uses: assert the property
//! the codebase must hold, in the codebase, rather than write it down in a doc
//! nobody re-reads.
//!
//! ## Detection rules
//!
//! A command is IN SCOPE when at least one parameter name (with any leading `_`
//! stripped, lowercased) either contains `path` or ends with `folder`, `dir` or
//! `file`. That deliberately over-matches — `cloud_set_folder` takes a Google
//! Drive folder id, not a filesystem path, and is EXEMPT for exactly that
//! reason. A false positive costs one line in a list; a false negative costs a
//! security hole, which is the whole point of the ratchet.
//!
//! The parser tolerates what real source contains: doc-comments and `//` lines
//! between the attribute and the `fn`, other attributes (`#[allow(...)]`),
//! multi-line signatures, and generic/tuple types with commas inside them
//! (parameters are split at paren depth zero). It does NOT try to understand
//! types — a parameter is judged by its NAME, because that is what a reviewer
//! judges it by too.

#![cfg(test)]

use std::collections::BTreeSet;

/// Commands that take a path-shaped parameter AND run it through
/// [`crate::commands::path_guard`]. The policy each one applies is documented on
/// the command itself; see the table in the `path_guard` module docs.
const GUARDED: &[&str] = &[
    // ── R1 editor: every ffmpeg/fs entry point ──────────────────────────────
    "editor_load_recording",
    "editor_peaks",
    "editor_probe_peak",
    "editor_extract_playback_proxy",
    "editor_allow_asset_path",
    "editor_segments",
    "editor_diagnose_channels",
    "editor_auto_process",
    "editor_mastering_analyze",
    "editor_extract_frame",
    "editor_read_sidecar",
    "editor_write_sidecar",
    "editor_delete_sidecar",
    "editor_record_sermon_pick",
    "editor_sermon_pick",
    "editor_record_companion_suggestion",
    "editor_probe_streams",
    "editor_read_file",
    // ── Episode images ───────────────────────────────────────────────────────
    "thumbnail_set_default",
    "thumbnail_set_episode",
    "thumbnail_clear_episode",
    "thumbnail_resolve",
    // ── Papirkurv ────────────────────────────────────────────────────────────
    "trash_move",
    // ── E1.2 ─────────────────────────────────────────────────────────────────
    "whisper_transcribe",
    "whisper_export_transcript",
    "integrations_get_service_link",
    "integrations_song_submit_usage",
    "integrations_sundayedit_send",
    "integrations_sundayedit_import",
    "settings_export_to_file",
    "settings_import_from_file",
    "cloud_enqueue_backup",
    "open_in_sundayedit",
    "open_in_sundaystudio",
];

/// Commands whose path-shaped parameter is NOT a filesystem path the process
/// acts on. Every entry carries the reason it is safe; an entry without one is a
/// hole waiting to be found.
const EXEMPT: &[(&str, &str)] = &[(
    "cloud_set_folder",
    "`folder` is a Google Drive folder id + display name (CloudFolder), never a \
     local path — it is persisted to the settings bag and sent to the Drive API",
)];

/// One `#[tauri::command]` found in the sources.
#[derive(Debug)]
struct Command {
    name: String,
    file: String,
    path_params: Vec<String>,
    /// The source from this command's `fn` line up to the next command (or EOF).
    /// Used only as a cross-check that a GUARDED entry really does call a guard.
    segment: String,
}

/// Whether a parameter NAME looks like a filesystem path.
fn is_path_like(name: &str) -> bool {
    let n = name.trim_start_matches('_').to_ascii_lowercase();
    n.contains("path") || n.ends_with("folder") || n.ends_with("dir") || n.ends_with("file")
}

/// Split a parameter list body at commas that sit at depth zero, so
/// `State<'_, Db>` and `Option<Vec<String>>` stay in one piece.
fn split_params(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in body.chars() {
        match ch {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur);
    out
}

/// The parameter names of a signature, `self` and pattern noise excluded.
fn param_names(signature: &str) -> Vec<String> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let Some(close) = signature.rfind(')') else {
        return Vec::new();
    };
    split_params(&signature[open + 1..close])
        .into_iter()
        .filter_map(|p| {
            let p = p.trim();
            let (name, _ty) = p.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name == "self" || name.contains(char::is_whitespace) {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

/// Parse one `commands/*.rs` file into its `#[tauri::command]` functions.
fn parse_file(file: &str, source: &str) -> Vec<Command> {
    let lines: Vec<&str> = source.lines().collect();
    // Where each command's attribute sits, so a segment can end at the next one.
    let attr_lines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("#[tauri::command]"))
        .map(|(i, _)| i)
        .collect();

    let mut commands = Vec::new();
    for (nth, &attr) in attr_lines.iter().enumerate() {
        // Walk to the `fn` line, tolerating further attributes and comments.
        let Some(fn_line) = (attr + 1..lines.len()).find(|&i| {
            let t = lines[i].trim_start();
            !t.starts_with('#') && !t.starts_with("//") && t.contains("fn ")
        }) else {
            continue;
        };
        // Accumulate the signature until the parameter list closes.
        let mut signature = String::new();
        let mut depth = 0i32;
        let mut opened = false;
        let mut end = fn_line;
        for (i, line) in lines.iter().enumerate().skip(fn_line) {
            signature.push_str(line);
            signature.push('\n');
            depth += line.matches('(').count() as i32;
            depth -= line.matches(')').count() as i32;
            if line.contains('(') {
                opened = true;
            }
            end = i;
            if opened && depth <= 0 {
                break;
            }
        }
        let name = signature
            .split_once("fn ")
            .and_then(|(_, rest)| rest.split(['(', '<', ' ']).next())
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let path_params: Vec<String> = param_names(&signature)
            .into_iter()
            .filter(|n| is_path_like(n))
            .collect();
        let segment_end = attr_lines.get(nth + 1).copied().unwrap_or(lines.len());
        let segment = lines[end.min(segment_end)..segment_end].join("\n");
        commands.push(Command {
            name,
            file: file.to_string(),
            path_params,
            segment,
        });
    }
    commands
}

/// Read + parse every `src/commands/*.rs` (this module's own directory).
///
/// This FILE is skipped: it contains no commands, only the parser's own fixture
/// source, and scanning it would make the ratchet flag its own test data. (That
/// it did so on the first run is the nicest possible proof the detector works.)
fn all_commands() -> Vec<Command> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter(|p| p.file_name().is_some_and(|n| n != "path_ratchet.rs"))
        .collect();
    entries.sort();
    assert!(
        entries.len() > 10,
        "the ratchet found only {} source files in {} — the parser is looking in \
         the wrong place, which would make it pass vacuously",
        entries.len(),
        dir.display()
    );
    entries
        .iter()
        .flat_map(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let src = std::fs::read_to_string(p).unwrap_or_default();
            parse_file(&name, &src)
        })
        .collect()
}

#[test]
fn the_parser_actually_finds_commands() {
    // A parser that silently matched nothing would turn every assertion below
    // into a no-op. Pin the floor.
    let commands = all_commands();
    assert!(
        commands.len() > 100,
        "only {} #[tauri::command] functions parsed — the parser is broken",
        commands.len()
    );
    for known in [
        "editor_peaks",
        "whisper_transcribe",
        "settings_export_to_file",
        "deeplink_confirm_captions",
    ] {
        assert!(
            commands.iter().any(|c| c.name == known),
            "{known} was not parsed out of src/commands"
        );
    }
    // …and that it reads the SIGNATURE, not just the name.
    let peaks = commands.iter().find(|c| c.name == "editor_peaks").unwrap();
    assert_eq!(peaks.path_params, vec!["input_path".to_string()]);
}

#[test]
fn every_path_taking_command_is_classified() {
    let commands = all_commands();
    let guarded: BTreeSet<&str> = GUARDED.iter().copied().collect();
    let exempt: BTreeSet<&str> = EXEMPT.iter().map(|(n, _)| *n).collect();

    let mut unclassified = Vec::new();
    for cmd in commands.iter().filter(|c| !c.path_params.is_empty()) {
        let in_guarded = guarded.contains(cmd.name.as_str());
        let in_exempt = exempt.contains(cmd.name.as_str());
        if in_guarded && in_exempt {
            panic!("{} is in BOTH lists — it can only be one thing", cmd.name);
        }
        if !in_guarded && !in_exempt {
            unclassified.push(format!(
                "  {} ({}) — parameter(s): {}",
                cmd.name,
                cmd.file,
                cmd.path_params.join(", ")
            ));
        }
    }

    assert!(
        unclassified.is_empty(),
        "\n\
         ────────────────────────────────────────────────────────────────────\n\
         A new #[tauri::command] takes a filesystem path and has not been\n\
         classified. The renderer is CSP-locked, but IPC is still an attack\n\
         surface: an unguarded path is an arbitrary read, an arbitrary write,\n\
         or a file uploaded to someone's Drive.\n\
         \n\
         {}\n\
         \n\
         Do ONE of these, in src/commands/path_ratchet.rs:\n\
         \n\
         1. GUARD IT (the default). Pick a PathPolicy from the table in\n\
            commands/path_guard.rs — RecordingsRooted / ReadOnlyMedia /\n\
            UserChosenRead / UserChosenWrite — call\n\
            `path_guard::check(&the_path, policy)?` as the FIRST thing in the\n\
            command, name the policy in the command's doc-comment, then add the\n\
            command to GUARDED.\n\
         \n\
         2. EXEMPT IT, if the parameter is not a filesystem path the process\n\
            acts on (a remote folder id, an opaque handle). Add it to EXEMPT\n\
            with a one-line reason. \"It's fine\" is not a reason.\n\
         ────────────────────────────────────────────────────────────────────",
        unclassified.join("\n")
    );
}

#[test]
fn every_guarded_command_still_exists_and_calls_a_guard() {
    // The other direction: a GUARDED entry that no longer guards (or no longer
    // exists) would leave the list looking complete while the hole is open.
    let commands = all_commands();
    for name in GUARDED {
        let Some(cmd) = commands.iter().find(|c| &c.name == name) else {
            panic!(
                "GUARDED lists `{name}`, but no such #[tauri::command] exists any \
                 more — remove the stale entry"
            );
        };
        assert!(
            !cmd.path_params.is_empty(),
            "GUARDED lists `{name}`, but it no longer takes a path-shaped \
             parameter — remove the stale entry"
        );
        assert!(
            cmd.segment.contains("path_guard"),
            "`{name}` is listed as GUARDED but its body never mentions \
             path_guard. Either call the guard or move it to EXEMPT with a reason."
        );
    }
}

#[test]
fn every_exemption_carries_a_reason() {
    for (name, reason) in EXEMPT {
        assert!(
            reason.trim().len() >= 20,
            "the exemption for `{name}` needs a real reason, not `{reason}`"
        );
    }
    let commands = all_commands();
    for (name, _) in EXEMPT {
        assert!(
            commands.iter().any(|c| &c.name == name),
            "EXEMPT lists `{name}`, which no longer exists — remove the stale entry"
        );
    }
}

#[test]
fn the_lists_have_no_duplicates() {
    let guarded: BTreeSet<&str> = GUARDED.iter().copied().collect();
    assert_eq!(guarded.len(), GUARDED.len(), "GUARDED has duplicates");
    let exempt: BTreeSet<&str> = EXEMPT.iter().map(|(n, _)| *n).collect();
    assert_eq!(exempt.len(), EXEMPT.len(), "EXEMPT has duplicates");
    assert!(
        guarded.is_disjoint(&exempt),
        "a command cannot be both guarded and exempt"
    );
}

#[test]
fn the_detector_matches_the_names_a_reviewer_would_flag() {
    for name in [
        "path",
        "_path",
        "file_path",
        "input_path",
        "source_path",
        "recording_path",
        "output_path",
        "media_path",
        "paths",
        "save_folder",
        "temp_dir",
        "subtitle_file",
        "PATH",
    ] {
        assert!(is_path_like(name), "{name} must be detected as path-shaped");
    }
    for name in [
        "id", "service", "job_id", "language", "format", "data", "url",
    ] {
        assert!(!is_path_like(name), "{name} must not be detected as a path");
    }
}

#[test]
fn the_parser_survives_the_shapes_real_source_has() {
    // Doc-comments and extra attributes between the marker and the fn, a
    // multi-line signature, and a generic type carrying its own commas.
    let src = r#"
/// A doc comment mentioning input_path, which is not a parameter.
// and a plain comment with fn decoy(path: String)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn awkward(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    input_path: String,
    opts: Option<Vec<(String, String)>>,
) -> AppResult<()> {
    super::path_guard::checked_input_file(&input_path)?;
    Ok(())
}

#[tauri::command]
pub fn plain(id: String) -> bool { true }
"#;
    let parsed = parse_file("test.rs", src);
    assert_eq!(parsed.len(), 2, "both commands must be found: {parsed:?}");
    assert_eq!(parsed[0].name, "awkward");
    assert_eq!(parsed[0].path_params, vec!["input_path".to_string()]);
    assert!(parsed[0].segment.contains("path_guard"));
    assert_eq!(parsed[1].name, "plain");
    assert!(parsed[1].path_params.is_empty());
    // A command's segment must NOT bleed into the next one's.
    assert!(!parsed[1].segment.contains("path_guard"));
}
