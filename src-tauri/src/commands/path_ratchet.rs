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
//! `file`. That deliberately over-matches — a command whose `folder` is a
//! remote id rather than a filesystem path (the old `cloud_set_folder` was one)
//! goes in EXEMPT with the reason. A false positive costs one line in a list; a
//! false negative costs a security hole, which is the whole point of the
//! ratchet.
//!
//! The parser tolerates what real source contains: doc-comments and `//` lines
//! between the attribute and the `fn`, other attributes (`#[allow(...)]`),
//! multi-line signatures, and generic/tuple types with commas inside them
//! (parameters are split at paren depth zero). It does NOT try to understand
//! types — a parameter is judged by its NAME, because that is what a reviewer
//! judges it by too.
//!
//! ## One level down: path-shaped FIELDS (finding E1)
//!
//! Judging parameters by name had a blind spot exactly one struct deep.
//! `start_recording(opts: RecordingOpts)` took the recording's `output_path`
//! straight from the renderer — the engine created that folder and wrote the
//! capture there — and the ratchet passed it, because `opts` is not a
//! path-shaped NAME. So the ratchet now also opens every struct a command
//! takes: for each parameter whose type names a struct defined in this crate
//! or in `sundayrec-core`, every field (and the fields of structs nested in
//! it) is judged by the same name rule, plus [`PATH_BEARING_FIELDS`] — names
//! that do not look like paths but become PART of one. Each hit must be listed
//! in [`PATH_FIELDS`]: guarded (with the guard's name, which must appear in
//! the command's own body — the same lexical rule as [`GUARDED`]), sanitised
//! into a single path component (with the sanitiser and the test that proves
//! it), or exempt with a reason. Tauri's injected parameters (`State`,
//! `AppHandle`, windows, channels — matched by exact type name, see
//! [`is_injected`]) are not renderer input and are skipped.
//!
//! `start_recording` itself now takes a `ManualStartRequest`: no path, and
//! ONE string that still ends up in one — `custom_name`, the file-name stem —
//! which the field list states explicitly as sanitised. `RecordingOpts` is not
//! `Deserialize` at all, so no command can take it back.
//! [`start_recording_takes_nothing_that_names_a_place`] pins the first;
//! `recorder::engine::payloads` pins the second at compile time.

#![cfg(test)]

use std::collections::BTreeSet;

/// Commands that take a path-shaped parameter AND run it through
/// [`crate::commands::path_guard`]. The policy each one applies is documented on
/// the command itself; see the table in the `path_guard` module docs.
const GUARDED: &[&str] = &[
    // ── R1 editor: every ffmpeg/fs entry point ──────────────────────────────
    "editor_load_recording",
    "editor_peaks",
    "editor_extract_playback_proxy",
    "editor_allow_asset_path",
    "editor_segments",
    "editor_diagnose_channels",
    "editor_auto_process",
    "editor_mastering_analyze",
    "editor_read_sidecar",
    "editor_write_sidecar",
    "editor_delete_sidecar",
    "editor_record_sermon_pick",
    "editor_sermon_pick",
    // ── Papirkurv ────────────────────────────────────────────────────────────
    "trash_move",
    // ── «Vis i Finder»: the webview lost `opener:allow-reveal-item-in-dir` ────
    // checked_input_file + delivered export / recordings root / known recording.
    "recordings_reveal",
];

/// Path-taking commands that were not guarded but REPLACED: the webview no
/// longer names the path at all, because the successor opens the native dialog
/// itself and acts only on what that dialog answered (finding A1 — SECURITY.md:
/// «a settings file's location comes from a dialog Rust opens»).
///
/// Strictly stronger than a guard, and therefore easy to undo by accident: a
/// later "convenience" overload that takes the path again would sail through
/// the GUARDED list with a guard call and reopen the hole. So
/// [`replaced_commands_stay_replaced`] holds both halves — the old name is gone
/// from the sources AND from `generate_handler!`, and the successor is
/// registered and has no path-shaped parameter.
const REPLACED: &[(&str, &str)] = &[
    ("settings_export_to_file", "settings_export_profile"),
    ("settings_import_from_file", "settings_import_profile"),
];

/// Commands whose path-shaped parameter is NOT a filesystem path the process
/// acts on. Every entry carries the reason it is safe; an entry without one is a
/// hole waiting to be found.
const EXEMPT: &[(&str, &str)] = &[];

/// How a path-shaped FIELD on a command's struct parameter is handled.
#[derive(Debug, Clone, Copy)]
enum FieldHandling {
    /// Validated before use by the named function, which must appear in the
    /// command's own body (lexically — a guard one call further away is a
    /// guard a refactor can drop without anything noticing).
    Guarded(&'static str),
    /// A renderer string that DOES end up in a path, but only after `into`
    /// has reduced it to a single path component (no separators, so it cannot
    /// leave the folder it is joined onto). `into` must be a function in the
    /// sources and `proof` a test that feeds the field hostile input and
    /// checks where the result lands — stricter than [`Exempt`], whose reason
    /// nothing checks.
    Sanitised {
        into: &'static str,
        proof: &'static str,
    },
    /// Not a path this command acts on. The reason is mandatory.
    Exempt(&'static str),
}

use FieldHandling::{Exempt, Guarded, Sanitised};

/// Every path-shaped field reachable through a command's struct parameters,
/// as `(command, "Struct.field", handling)`. See the module docs, «One level
/// down». A new field, or a new command taking an old struct, is a failing
/// test until it is listed here.
const PATH_FIELDS: &[(&str, &str, FieldHandling)] = &[
    // ── start_recording: the take's name becomes the file-name STEM
    //    (`<name>_<date>.<ext>`), joined onto a save folder Rust resolved.
    //    `build_filename` runs it through `sanitize_filename` first, so it is
    //    one path component whatever the renderer sent.
    (
        "start_recording",
        "ManualStartRequest.custom_name",
        Sanitised {
            into: "sanitize_filename",
            proof: "a_hostile_custom_name_stays_a_file_name_in_the_save_folder",
        },
    ),
    // ── editor_export: guarded by its own E5.3 helper, which runs a
    //    path_guard policy on each of these (see `check_export_paths`). The
    //    NEXT step for the output folder — Rust choosing the destination
    //    through its own dialog, as `docs/PLAN.md` asks — is a later PR; this
    //    list only records what guards them today.
    (
        "editor_export",
        "EditorExportRequest.input_path",
        Guarded("check_export_paths(&request)"),
    ),
    (
        "editor_export",
        "EditorExportRequest.output_folder",
        Guarded("check_export_paths(&request)"),
    ),
    (
        "editor_export",
        "EditorExportRequest.intro_path",
        Guarded("check_export_paths(&request)"),
    ),
    (
        "editor_export",
        "EditorExportRequest.outro_path",
        Guarded("check_export_paths(&request)"),
    ),
    // ── editor_master_preview: the source the preview renders from.
    (
        "editor_master_preview",
        "EditorMasterPreviewRequest.input_path",
        Guarded("path_guard::checked_input_file(&request.input_path)"),
    ),
    // ── settings_save: the persisted profile.
    (
        "settings_save",
        "Settings.save_folder",
        Guarded("vet_new_save_folder"),
    ),
    (
        "settings_save",
        "Settings.editor_intro_path",
        Exempt(
            "stored, never opened here: the editor sends it back as \
             EditorExportRequest.intro_path, which check_export_paths guards",
        ),
    ),
    (
        "settings_save",
        "Settings.editor_outro_path",
        Exempt(
            "stored, never opened here: the editor sends it back as \
             EditorExportRequest.outro_path, which check_export_paths guards",
        ),
    ),
];

/// Field names that do not LOOK like paths but become part of one, so they
/// are judged as paths too.
///
/// - `separate_audio_format` is the sidecar's file extension: `finalize.rs`
///   joins `{stem}.{separate_audio_format}` beside the recording, so a
///   renderer that chose it could have walked the sidecar out of the folder
///   with an "extension" holding separators. It reaches the engine only
///   inside `RecordingOpts`, which no command can take any more — this is the
///   tripwire for the day a struct with it becomes a parameter again.
/// - `custom_name` is a recording's file-name stem. It IS a parameter field
///   today (`ManualStartRequest`), so it is listed in [`PATH_FIELDS`] as
///   sanitised — the ratchet says out loud that one renderer string reaches
///   the output path, instead of passing it because of its name.
const PATH_BEARING_FIELDS: &[&str] = &["separate_audio_format", "custom_name"];

/// One `#[tauri::command]` found in the sources.
#[derive(Debug)]
struct Command {
    name: String,
    file: String,
    path_params: Vec<String>,
    /// Every parameter as `(name, type)`, for the field-level check.
    params: Vec<(String, String)>,
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

/// The parameters of a signature as `(name, type)`, `self` and pattern noise
/// excluded.
fn params(signature: &str) -> Vec<(String, String)> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    // The parameter list's OWN closing paren — not the last `)` in the
    // signature, which may belong to a tuple in the return type.
    let mut depth = 0i32;
    let mut close = None;
    for (i, ch) in signature[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Vec::new();
    };
    split_params(&signature[open + 1..close])
        .into_iter()
        .filter_map(|p| {
            let p = p.trim();
            let (name, ty) = p.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name == "self" || name.contains(char::is_whitespace) {
                return None;
            }
            Some((name.to_string(), ty.trim().to_string()))
        })
        .collect()
}

/// The parameter names of a signature, `self` and pattern noise excluded.
fn param_names(signature: &str) -> Vec<String> {
    params(signature)
        .into_iter()
        .map(|(name, _)| name)
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
            params: params(&signature),
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

// ── One level down: struct fields ───────────────────────────────────────────

/// Struct name → its named fields as `(field, type)`. Same-named structs in
/// different modules are MERGED: over-matching costs a line in a list, and a
/// false negative is the hole this exists to close.
type StructFields = std::collections::BTreeMap<String, Vec<(String, String)>>;

/// Whether a FIELD name is path-shaped: the parameter rule, plus the names in
/// [`PATH_BEARING_FIELDS`] that become part of a path without looking like one.
fn is_path_like_field(name: &str) -> bool {
    is_path_like(name) || PATH_BEARING_FIELDS.contains(&name)
}

/// Is `s` a plain Rust identifier?
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse every braced `struct` in `source` into `out`.
///
/// Line-based, like the command parser, and tolerant of the same things:
/// doc-comments and `//` lines (skipped — a `{when}` in a doc comment must not
/// move the brace count), attributes (skipped, multi-line ones included —
/// `#[serde(\n default = …,\n)]` exists), visibility, generics. Tuple and
/// unit structs carry no field NAMES, so they are not entered.
fn parse_structs(source: &str, out: &mut StructFields) {
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let decl = t
            .strip_prefix("pub(crate) ")
            .or_else(|| t.strip_prefix("pub(super) "))
            .or_else(|| t.strip_prefix("pub "))
            .unwrap_or(t);
        let Some(rest) = decl.strip_prefix("struct ") else {
            i += 1;
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || !lines[i].contains('{') || lines[i].contains(';') {
            i += 1;
            continue;
        }
        // The body: everything until the brace that opened on this line
        // closes, minus comment and attribute lines.
        let mut body = String::new();
        let mut depth = lines[i].matches('{').count() as i32 - lines[i].matches('}').count() as i32;
        let mut attr_depth = 0i32;
        i += 1;
        while i < lines.len() && depth > 0 {
            let l = lines[i].trim_start();
            if attr_depth > 0 || l.starts_with("#[") {
                attr_depth += l.matches('[').count() as i32 - l.matches(']').count() as i32;
                i += 1;
                continue;
            }
            if l.starts_with("//") {
                i += 1;
                continue;
            }
            depth += l.matches('{').count() as i32 - l.matches('}').count() as i32;
            if depth > 0 {
                body.push_str(l);
                body.push('\n');
            }
            i += 1;
        }
        let fields = out.entry(name).or_default();
        for piece in split_params(&body) {
            let Some((lhs, ty)) = piece.split_once(':') else {
                continue;
            };
            // `pub(crate) name` / `pub name` / `name` → `name`.
            let Some(field) = lhs.split_whitespace().last() else {
                continue;
            };
            if is_ident(field) {
                fields.push((field.to_string(), ty.trim().to_string()));
            }
        }
    }
}

/// Every `.rs` file under `dir`, recursively, sorted.
fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The structs a command parameter can name: this crate's and the core's.
fn all_structs() -> StructFields {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&manifest.join("src"), &mut files);
    rust_files(&manifest.join("../crates/sundayrec-core/src"), &mut files);
    files.sort();
    let mut out = StructFields::new();
    for f in &files {
        // This file holds fixture structs for the parser's own tests.
        if f.file_name().is_some_and(|n| n == "path_ratchet.rs") {
            continue;
        }
        parse_structs(&std::fs::read_to_string(f).unwrap_or_default(), &mut out);
    }
    out
}

/// Parameters Tauri INJECTS rather than deserialising from the renderer.
///
/// Judged by the type's own NAME — the last path segment before any generics
/// (`tauri::State<'_, Db>` → `State`) — and never by substring: a future
/// `EditorWindowRequest` or `StateImport` is renderer input, and a substring
/// match would have skipped it silently. `Request` counts only as
/// `tauri::ipc::Request` (a crate struct could well be called `Request`).
fn is_injected(ty: &str) -> bool {
    let head = ty.trim().split('<').next().unwrap_or_default().trim();
    let name = head.rsplit("::").next().unwrap_or(head);
    match name {
        "State" | "AppHandle" | "Window" | "WebviewWindow" | "Webview" | "Channel" => true,
        "Request" => head.ends_with("ipc::Request"),
        _ => false,
    }
}

/// The identifiers in a type (`Option<Vec<EditorCutRegion>>` → `Option`,
/// `Vec`, `EditorCutRegion`).
fn type_idents(ty: &str) -> impl Iterator<Item = &str> {
    ty.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| is_ident(s))
}

/// Path-shaped fields reachable from `ty`, as `Struct.field`, walking into
/// nested structs (each struct once, so a cycle cannot loop).
fn path_fields_of(
    ty: &str,
    structs: &StructFields,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<String>,
) {
    for ident in type_idents(ty) {
        let Some(fields) = structs.get(ident) else {
            continue;
        };
        if !seen.insert(ident.to_string()) {
            continue;
        }
        for (field, fty) in fields {
            if is_path_like_field(field) {
                out.push(format!("{ident}.{field}"));
            }
            path_fields_of(fty, structs, seen, out);
        }
    }
}

/// Every `(command, "Struct.field")` the field-level rule flags.
fn field_findings(commands: &[Command], structs: &StructFields) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for cmd in commands {
        for (_, ty) in cmd.params.iter().filter(|(_, ty)| !is_injected(ty)) {
            let mut fields = Vec::new();
            path_fields_of(ty, structs, &mut BTreeSet::new(), &mut fields);
            out.extend(fields.into_iter().map(|f| (cmd.name.clone(), f)));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_struct_parser_actually_finds_fields() {
    // Same reason as the command floor: a parser that matched nothing would
    // make every field assertion below a no-op.
    let structs = all_structs();
    assert!(
        structs.len() > 100,
        "only {} structs parsed — the field parser is broken",
        structs.len()
    );
    let names = |s: &str| -> Vec<String> {
        structs
            .get(s)
            .unwrap_or_else(|| panic!("{s} was not parsed"))
            .iter()
            .map(|(f, _)| f.clone())
            .collect()
    };
    // A request across two files, the core's settings (with its multi-line
    // `#[serde(…)]` attribute), and the engine's opts.
    assert!(names("EditorExportRequest").contains(&"output_folder".to_string()));
    let settings = names("Settings");
    assert!(settings.contains(&"save_folder".to_string()));
    assert!(settings.contains(&"update_channel".to_string()));
    assert!(settings.contains(&"ask_open_editor".to_string()));
    assert!(names("RecordingOpts").contains(&"output_path".to_string()));
    assert_eq!(
        names("ManualStartRequest"),
        vec!["custom_name", "max_minutes", "video"],
        "the manual start request must carry exactly the three values the \
         renderer decides — nothing that names a place"
    );
}

#[test]
fn every_path_shaped_field_on_a_command_parameter_is_classified() {
    let findings = field_findings(&all_commands(), &all_structs());
    let unclassified: Vec<String> = findings
        .iter()
        .filter(|(cmd, field)| !PATH_FIELDS.iter().any(|(c, f, _)| c == cmd && f == field))
        .map(|(cmd, field)| format!("  {cmd} — {field}"))
        .collect();
    assert!(
        unclassified.is_empty(),
        "\n\
         ────────────────────────────────────────────────────────────────────\n\
         A #[tauri::command] takes a struct with a path-shaped FIELD that has\n\
         not been classified. This is finding E1's shape: `start_recording`\n\
         took `opts: RecordingOpts`, and `opts.output_path` — one field down —\n\
         decided where the recorder wrote, straight from the renderer.\n\
         \n\
         {}\n\
         \n\
         Do ONE of these:\n\
         \n\
         1. KEEP THE PATH IN RUST (the best answer): take only the values the\n\
            renderer really decides, and compute the path server-side — the\n\
            way `start_recording` takes a `ManualStartRequest` now.\n\
         \n\
         2. GUARD IT: run the path_guard policy on the field in the command\n\
            (or in a helper the command calls by name) and add\n\
            `(command, \"Struct.field\", Guarded(\"<helper>\"))` to PATH_FIELDS.\n\
         \n\
         3. EXEMPT IT, if the field is not a path this command acts on, with\n\
            `Exempt(\"<a real reason>\")`.\n\
         ────────────────────────────────────────────────────────────────────",
        unclassified.join("\n")
    );
}

#[test]
fn every_classified_field_still_exists_and_its_guard_is_called() {
    let commands = all_commands();
    let findings = field_findings(&commands, &all_structs());
    for (cmd, field, handling) in PATH_FIELDS {
        assert!(
            findings.iter().any(|(c, f)| c == cmd && f == field),
            "PATH_FIELDS lists `{cmd}` / `{field}`, but that command no longer \
             takes a struct with that field — remove the stale entry"
        );
        match handling {
            Guarded(guard) => {
                let segment = &commands
                    .iter()
                    .find(|c| &c.name == cmd)
                    .expect("findings came from this command")
                    .segment;
                assert!(
                    segment.contains(guard),
                    "`{cmd}` lists `{field}` as guarded by `{guard}`, but its \
                     body never calls it"
                );
            }
            Sanitised { into, proof } => {
                assert!(
                    source_defines_fn(into),
                    "`{cmd}` lists `{field}` as sanitised by `{into}`, but no \
                     such function exists any more"
                );
                assert!(
                    source_defines_fn(proof),
                    "`{cmd}` lists `{field}` as sanitised, proven by `{proof}` \
                     — but that test no longer exists"
                );
            }
            Exempt(reason) => assert!(
                reason.trim().len() >= 20,
                "the exemption for `{cmd}` / `{field}` needs a real reason"
            ),
        }
    }
    let mut keys: Vec<(&str, &str)> = PATH_FIELDS.iter().map(|(c, f, _)| (*c, *f)).collect();
    keys.sort();
    let before = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), before, "PATH_FIELDS has duplicates");
}

/// Whether `fn <name>(` appears in this crate's or the core's sources.
fn source_defines_fn(name: &str) -> bool {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&manifest.join("src"), &mut files);
    rust_files(&manifest.join("../crates/sundayrec-core/src"), &mut files);
    let needle = format!("fn {name}(");
    files.iter().any(|f| {
        std::fs::read_to_string(f)
            .unwrap_or_default()
            .contains(&needle)
    })
}

/// The fix for E1, pinned from the outside: `start_recording`'s parameters
/// are Tauri's injected handles plus ONE request, and nothing in that request
/// names a place. Its one string that reaches the output path —
/// `custom_name`, the file-name stem — is the request's only path-bearing
/// field, and the field list says it is sanitised, not guarded or waved
/// through. Put `opts: RecordingOpts` back and this — and the field rule
/// above, and the compile-time check on `RecordingOpts` — fail.
#[test]
fn start_recording_takes_nothing_that_names_a_place() {
    let commands = all_commands();
    let start = commands
        .iter()
        .find(|c| c.name == "start_recording")
        .expect("start_recording was not parsed");
    let renderer_params: Vec<&(String, String)> = start
        .params
        .iter()
        .filter(|(_, ty)| !is_injected(ty))
        .collect();
    assert_eq!(
        renderer_params,
        vec![&("request".to_string(), "ManualStartRequest".to_string())],
        "start_recording must take only a ManualStartRequest from the renderer"
    );
    assert!(start.path_params.is_empty());
    let structs = all_structs();
    let mut fields = Vec::new();
    path_fields_of(
        "ManualStartRequest",
        &structs,
        &mut BTreeSet::new(),
        &mut fields,
    );
    assert_eq!(
        fields,
        vec!["ManualStartRequest.custom_name".to_string()],
        "the name is the only renderer string allowed to reach the path"
    );
    // No field is even NAMED like a path…
    assert!(
        structs["ManualStartRequest"]
            .iter()
            .all(|(f, _)| !is_path_like(f)),
        "{:?}",
        structs["ManualStartRequest"]
    );
    // …and the name is listed as SANITISED — not guarded, not exempt.
    let listed: Vec<&FieldHandling> = PATH_FIELDS
        .iter()
        .filter(|(c, _, _)| *c == "start_recording")
        .map(|(_, _, h)| h)
        .collect();
    assert!(
        matches!(listed.as_slice(), [Sanitised { .. }]),
        "start_recording's field list must be exactly the sanitised name: {listed:?}"
    );
}

#[test]
fn injected_parameters_are_matched_by_type_name_not_substring() {
    for ty in [
        "State<'_, Db>",
        "tauri::State<'_, RecorderEngine>",
        "AppHandle",
        "tauri::AppHandle",
        "AppHandle<R>",
        "tauri::Window",
        "WebviewWindow",
        "tauri::Webview",
        "tauri::ipc::Channel<EditorExportProgress>",
        "tauri::ipc::Request<'_>",
    ] {
        assert!(is_injected(ty), "{ty} is injected by Tauri");
    }
    for ty in [
        "EditorWindowRequest",
        "WindowState",
        "AppHandleConfig",
        "StateImport",
        "ManualStartRequest",
        "Request",
        "crate::editor::EditorSermonPickRequest",
        "Option<String>",
    ] {
        assert!(!is_injected(ty), "{ty} is renderer input, not injected");
    }
}

#[test]
fn the_field_detector_would_have_caught_finding_e1() {
    // The shape main shipped until the fix, as a fixture: the old command and
    // the old struct. Both the output path and the sidecar extension must be
    // flagged — and an injected `State<'_, Settings>` must NOT drag the
    // settings' own `save_folder` in with it.
    let cmd_src = r#"
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    db: State<'_, Db>,
    cfg: State<'_, Fixture>,
    opts: RecordingOpts,
) -> AppResult<()> {
    Ok(())
}
"#;
    let struct_src = r#"
/// Doc with braces {when} that must not move the count.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "RecordingOpts.ts")]
pub struct RecordingOpts {
    /// Stored microphone name.
    pub audio_device_name: String,
    pub output_path: String,
    #[serde(
        default,
        rename = "x"
    )]
    pub separate_audio_format: String,
    pub nested: Option<Inner>,
}

pub(crate) struct Inner {
    temp_dir: PathBuf,
}

pub struct Fixture {
    pub save_folder: Option<String>,
}

pub struct Tuple(String);
"#;
    let commands = parse_file("fixture.rs", cmd_src);
    let mut structs = StructFields::new();
    parse_structs(struct_src, &mut structs);
    assert!(
        !structs.contains_key("Tuple"),
        "a tuple struct has no field names"
    );
    assert_eq!(
        field_findings(&commands, &structs),
        vec![
            ("start_recording".to_string(), "Inner.temp_dir".to_string()),
            (
                "start_recording".to_string(),
                "RecordingOpts.output_path".to_string()
            ),
            (
                "start_recording".to_string(),
                "RecordingOpts.separate_audio_format".to_string()
            ),
        ]
    );
}

#[test]
fn the_parser_actually_finds_commands() {
    // A parser that silently matched nothing would turn every assertion below
    // into a no-op. Pin the floor.
    //
    // ⚠️ The floor is a PARSER-sanity floor, not a command-count ratchet: it
    // asks "did this thing read the sources at all", and the honest answer to
    // "there are fewer commands than yesterday" is to lower it, not to keep a
    // command alive so a number holds. V1/PR3 deleted 12 dark commands (see the
    // PR), taking the real count from 111 to ~91, so the floor moved 100 → 80.
    // The named-command assertions below are what actually pins the behaviour.
    let commands = all_commands();
    assert!(
        commands.len() > 80,
        "only {} #[tauri::command] functions parsed — the parser is broken",
        commands.len()
    );
    for known in [
        "editor_peaks",
        "settings_export_profile",
        "editor_read_sidecar",
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

/// The body of `tauri::generate_handler![…]` in `src/lib.rs` — what the webview
/// can actually invoke. Comments are stripped line by line BEFORE the closing
/// `]` is looked for, so a note that names a retired command is not mistaken
/// for its registration, and a `[…]` in a note does not end the block early.
fn registered_handler_block() -> String {
    let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let src = std::fs::read_to_string(&lib)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", lib.display()));
    handler_block_of(&src).expect("lib.rs has a closed generate_handler![…] block")
}

/// [`registered_handler_block`] over a given source, so the reader can be held
/// to a fixture.
fn handler_block_of(src: &str) -> Option<String> {
    const OPEN: &str = "tauri::generate_handler![";
    let start = src.find(OPEN)? + OPEN.len();
    let mut block = Vec::new();
    for line in src[start..].lines() {
        let code = line.split("//").next().unwrap_or_default();
        if let Some(end) = code.find(']') {
            block.push(&code[..end]);
            return Some(block.join("\n"));
        }
        block.push(code);
    }
    None
}

#[test]
fn replaced_commands_stay_replaced() {
    let commands = all_commands();
    let handler = registered_handler_block();
    // Did it read anything? A block that parsed empty would pass every
    // "is not registered" below vacuously.
    assert!(
        handler.contains("commands::settings::settings_get"),
        "the generate_handler! block was not found or not read"
    );
    for (old, new) in REPLACED {
        assert!(
            !commands.iter().any(|c| c.name == *old),
            "`{old}` is back as a #[tauri::command]. It was REPLACED by `{new}`, \
             which opens the dialog in Rust so the webview never names the path \
             (finding A1). Do not restore a path-taking twin."
        );
        assert!(
            !handler.contains(old),
            "`{old}` is registered in generate_handler! again — see REPLACED"
        );
        let Some(successor) = commands.iter().find(|c| c.name == *new) else {
            panic!("REPLACED names `{new}`, but no such #[tauri::command] exists");
        };
        assert!(
            successor.path_params.is_empty(),
            "`{new}` takes a path-shaped parameter ({}). Its whole point is that \
             the dialog Rust opens decides the file — not the webview.",
            successor.path_params.join(", ")
        );
        // Stricter than "no path-shaped NAME": the name detector over-matches
        // on purpose but cannot know every spelling — `file_name`, `target`,
        // `name` would all slip through it and still let the webview steer the
        // file. A successor takes NOTHING the webview sends: only what Tauri
        // injects ([`is_injected`]).
        let sent: Vec<String> = successor
            .params
            .iter()
            .filter(|(_, ty)| !is_injected(ty))
            .map(|(n, ty)| format!("{n}: {ty}"))
            .collect();
        assert!(
            sent.is_empty(),
            "`{new}` takes parameter(s) from the webview ({}). A REPLACED \
             successor may only take what Tauri injects (State, Window, \
             AppHandle …): the file is the dialog's answer, and nothing the \
             webview sends may shape it.",
            sent.join(", ")
        );
        assert!(
            handler.contains(&format!("::{new},")),
            "`{new}` is not registered in generate_handler!"
        );
    }
}

#[test]
fn the_handler_reader_holds_to_its_fixture() {
    // A `]` inside a comment must not end the block, and a commented-out
    // registration is not a registration.
    let src = r#"
        .invoke_handler(tauri::generate_handler![
            // see [the audit] for why
            commands::a::one,
            // commands::a::retired,
            commands::a::two, // trailing [note]
        ])
    "#;
    let block = handler_block_of(src).expect("the fixture block closes");
    assert!(block.contains("commands::a::one,"));
    assert!(block.contains("commands::a::two,"));
    assert!(!block.contains("retired"));
    assert!(handler_block_of("no handler here").is_none());
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
