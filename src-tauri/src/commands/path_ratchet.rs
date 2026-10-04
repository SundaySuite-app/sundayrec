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
//!
//! ## Tokens that stand for a place (finding A2)
//!
//! A place the operator picked in a dialog Rust opened is handed to the
//! webview as an opaque session token (`commands::chosen_paths`), and comes
//! back as one — `editor_export`'s `output_folder_token`. A token is not a
//! path, but it DECIDES one, so a field named like a place plus `_token` is
//! judged here too ([`is_path_like_field`]) and must be listed as
//! [`FieldHandling::Token`]: the resolver that turns it back into the place —
//! called in the command's own body, like a guard — and the test that feeds it
//! a forged token. [`editor_export_names_its_folder_only_by_token`] pins the
//! A2 fix from the outside.
//!
//! The same goes for a token that is a PARAMETER of the command rather than a
//! field of its request: a `#[tauri::command]` parameter named like a place
//! plus `_token` must be listed in [`PARAM_TOKENS`], with the same resolver and
//! proof. A token is not a path, and it is exactly as good as one for deciding
//! where the command acts, so it cannot be the one shape the ratchet does not
//! look at. And a resolver is not taken on its word: following the calls from
//! the command's body must reach `ChosenPaths::resolve` — the one door a token
//! gives its place back through — or the entry is a name with nothing behind it.
//!
//! ## What «the body calls it» means: code, not comments
//!
//! Every lexical rule here — a guard in the body, a resolver in the body, a
//! `path_guard` mention — is read from the source with its COMMENTS STRIPPED
//! (`//`, `///`, `//!` and nested `/* */`; [`strip_comments`]). Without that, a
//! comment that merely names the guard — «// no longer calls
//! `check_export_paths(&request)`», or a doc comment on the NEXT command, which
//! sits in the previous command's segment — made the ratchet go green over an
//! unguarded command.

#![cfg(test)]

use std::collections::BTreeSet;

/// The source with its comments removed — `//`, `///`, `//!` and (nested)
/// `/* … */` — and every newline kept, so line numbers and the line-based
/// parsers below still line up. String, raw-string and character literals are
/// read as literals: a `//` inside `"https://…"` is not a comment, and a `"`
/// inside a comment does not open a string.
fn strip_comments(src: &str) -> String {
    lex(src, false)
}

/// [`strip_comments`] and the contents of string and character literals
/// blanked too (the quotes stay), so a `{` or a function name inside a literal
/// can neither move a brace count nor pass for a call.
fn code_only(src: &str) -> String {
    lex(src, true)
}

fn lex(src: &str, blank_literals: bool) -> String {
    let c: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let is_ident_char = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    // One literal character: kept, or a blank (newlines always kept).
    let lit = |out: &mut String, ch: char| {
        out.push(if blank_literals && ch != '\n' {
            ' '
        } else {
            ch
        })
    };
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        if ch == '/' && next == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && next == Some('*') {
            let mut depth = 1;
            i += 2;
            while i < c.len() && depth > 0 {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    if c[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
            out.push(' ');
        } else if ch == 'r'
            && (i == 0
                || !is_ident_char(c[i - 1])
                || (c[i - 1] == 'b' && (i < 2 || !is_ident_char(c[i - 2]))))
            && matches!(next, Some('"' | '#'))
            && {
                let hashes = c[i + 1..].iter().take_while(|&&h| h == '#').count();
                c.get(i + 1 + hashes) == Some(&'"')
            }
        {
            // A raw string: r"…", r#"…"#, br#"…"#.
            let hashes = c[i + 1..].iter().take_while(|&&h| h == '#').count();
            out.push('r');
            out.extend(std::iter::repeat_n('#', hashes));
            out.push('"');
            i += hashes + 2;
            while i < c.len() {
                if c[i] == '"' && (1..=hashes).all(|k| c.get(i + k) == Some(&'#')) {
                    break;
                }
                lit(&mut out, c[i]);
                i += 1;
            }
            out.push('"');
            out.extend(std::iter::repeat_n('#', hashes));
            i += hashes + 1;
        } else if ch == '"' {
            out.push('"');
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    lit(&mut out, c[i]);
                    i += 1;
                }
                lit(&mut out, c[i]);
                i += 1;
            }
            out.push('"');
            i += 1;
        } else if ch == '\'' {
            // A character literal ('x', '\n', '\u{1F600}') — or a lifetime ('a),
            // which has no closing quote and is just kept.
            let close = if next == Some('\\') {
                c[i + 2..]
                    .iter()
                    .position(|&q| q == '\'')
                    .map(|p| i + 2 + p)
            } else if c.get(i + 2) == Some(&'\'') && next != Some('\'') {
                Some(i + 2)
            } else {
                None
            };
            match close {
                Some(end) => {
                    out.push('\'');
                    for &q in &c[i + 1..end] {
                        lit(&mut out, q);
                    }
                    out.push('\'');
                    i = end + 1;
                }
                None => {
                    out.push('\'');
                    i += 1;
                }
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

/// Commands that take a path-shaped parameter AND run it through
/// [`crate::commands::path_guard`]. The policy each one applies is documented on
/// the command itself; see the table in the `path_guard` module docs.
const GUARDED: &[&str] = &[
    // ── R1 editor: the sidecar and sermon-pick commands. (The commands that
    //    read or render the recording left this list in PR-C: they take a File
    //    token now, see PARAM_TOKENS. Their sidecars are PR-D.) ───────────────
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
    // The webview widened its own `asset://` scope to any file it named (behind
    // `path_guard`). Now the scope grows only when RUST opens a recording — the
    // picker, or a drop on the window — and the other way in, a history row's
    // id (`editor_open_known`), is pinned by
    // `editor_open_known_takes_only_a_history_row_id`.
    ("editor_allow_asset_path", "editor_open_recording"),
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
    /// Not a path, but an opaque session token for a place the operator
    /// picked in a dialog Rust opened (`commands::chosen_paths`). The webview
    /// can only hand back a token it was given. `resolver` turns it into the
    /// place and re-validates it, and must appear in the command's own body
    /// (lexically, like [`Guarded`], comments stripped) — and following the
    /// calls from there must reach `ChosenPaths::resolve`
    /// ([`token_problem`]); `proof` is a test that hands the resolver a
    /// made-up token and checks it is refused.
    Token {
        resolver: &'static str,
        proof: &'static str,
    },
    /// A field the webview can still SEND (it is part of the wire struct) but
    /// whose value the command never uses: `by` overwrites it with what is
    /// stored before anything is saved, and `proof` is a test that sends a
    /// different value and sees the stored one survive. Both must exist in the
    /// sources. (`settings_save`'s intro/outro clips: a file the export reads
    /// is not the webview's to name.)
    Overwritten {
        by: &'static str,
        proof: &'static str,
    },
    /// Not a path this command acts on. The reason is mandatory.
    ///
    /// Nothing is exempt today (the intro/outro clips, the last two, became
    /// [`Overwritten`] in PR-C) — the variant stays for the next field that is
    /// genuinely not a place, and its check (`every_classified_field_…`) with it.
    #[allow(dead_code)]
    Exempt(&'static str),
}

use FieldHandling::{Exempt, Guarded, Overwritten, Sanitised, Token};

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
    // ── editor_export: what ffmpeg reads and where it writes are TOKENS (A2).
    //    The destination: the old `output_folder` is gone, and the request names
    //    a folder only by the token `editor_pick_output_folder` minted after the
    //    native dialog RUST opened answered. The source: the old `input_path` is
    //    gone too (PR-C), and the request names the recording only by the File
    //    token `editor_open_recording`/`editor_open_known`/the drop handler
    //    minted. The jingles are not in the request at all — `use_intro` and
    //    `use_outro` are switches, and the clips come from the saved settings.
    (
        "editor_export",
        "EditorExportRequest.source_token",
        Token {
            resolver: "run_export(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_export",
        "EditorExportRequest.output_folder_token",
        Token {
            resolver: "run_export(&chosen",
            proof: "a_made_up_or_foreign_token_is_refused_with_its_own_code",
        },
    ),
    // ── editor_master_preview: the source the preview renders from.
    (
        "editor_master_preview",
        "EditorMasterPreviewRequest.source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    // ── settings_save: the persisted profile.
    (
        "settings_save",
        "Settings.save_folder",
        Guarded("vet_new_save_folder"),
    ),
    // The clips are a file the export reads, so the webview may not name them:
    // `settings_save` keeps the stored values whatever it sends, and the only
    // ways to change them are the Rust dialogs `settings_pick_editor_intro`/
    // `_outro` (and the clears).
    (
        "settings_save",
        "Settings.editor_intro_path",
        Overwritten {
            by: "keep_stored_clips",
            proof: "settings_save_keeps_the_stored_intro_and_outro",
        },
    ),
    (
        "settings_save",
        "Settings.editor_outro_path",
        Overwritten {
            by: "keep_stored_clips",
            proof: "settings_save_keeps_the_stored_intro_and_outro",
        },
    ),
];

/// Every `#[tauri::command]` PARAMETER named `*_token` ([`is_token_param`]), as
/// `(command, parameter, Token { … })`. The shape [`PATH_FIELDS`] gives a token
/// field, for a token the command takes directly: here the File token of the
/// recording every editor command works on (`source_token`, A2/PR-C). A new
/// command taking one is a failing test until it is listed here — or, for a
/// token that is NOT a place, in [`NOT_PLACE_TOKENS`] with the reason.
const PARAM_TOKENS: &[(&str, &str, FieldHandling)] = &[
    (
        "editor_load_recording",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_peaks",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_extract_playback_proxy",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_segments",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_diagnose_channels",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_auto_process",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
    (
        "editor_mastering_analyze",
        "source_token",
        Token {
            resolver: "resolve_source(&chosen",
            proof: "a_made_up_or_foreign_source_token_is_refused_with_its_own_code",
        },
    ),
];

/// `*_token` parameters that are NOT a token for a place, as `(command,
/// parameter, reason)`. [`is_token_param`] matches EVERY `_token` name — a token
/// is exactly as good as a path for deciding where a command acts, and a name
/// rule that guessed which tokens count (it used to ask for a path-shaped word
/// in front of `_token`, which `source_token` is not) is how one slips by. So
/// a token that is not a place says so here.
const NOT_PLACE_TOKENS: &[(&str, &str, &str)] = &[(
    "get_camera_capabilities",
    "device_token",
    "the camera's id as ffmpeg's device list names it (`avfoundation` index or \
     `dshow` name), handed to the camera probe as a device argument — it names \
     hardware, not a file or a folder",
)];

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
    /// The parameters that are TOKENS for a place (`output_folder_token`):
    /// see [`is_token_param`].
    token_params: Vec<String>,
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

/// Whether a command PARAMETER is a token: any name ending `_token` — the same
/// rule [`is_path_like_field`] applies to a request's fields. Every one must be
/// listed in [`PARAM_TOKENS`] (a place) or [`NOT_PLACE_TOKENS`] (with the
/// reason): the earlier rule, a path-shaped word in front of `_token`, let
/// `source_token` through unlisted. Over-matching costs a line in a list.
fn is_token_param(name: &str) -> bool {
    name.trim_start_matches('_').ends_with("_token")
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
///
/// The source is read with its comments stripped ([`strip_comments`]): what a
/// command's segment «calls» is what it calls, not what a comment says.
fn parse_file(file: &str, source: &str) -> Vec<Command> {
    let stripped = strip_comments(source);
    let lines: Vec<&str> = stripped.lines().collect();
    // What a command's segment «calls» is read from CODE ONLY: comments gone
    // (S4) and the contents of string literals blanked, so `let _unused =
    // "check_export_paths(&request)";` is not a call to it (S4a). Same lexer,
    // same newlines — the line numbers line up with `lines`.
    let code = code_only(source);
    let code_lines: Vec<&str> = code.lines().collect();
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
        let token_params: Vec<String> = param_names(&signature)
            .into_iter()
            .filter(|n| is_token_param(n))
            .collect();
        let segment_end = attr_lines.get(nth + 1).copied().unwrap_or(lines.len());
        let segment = code_lines[end.min(segment_end)..segment_end].join("\n");
        commands.push(Command {
            name,
            file: file.to_string(),
            path_params,
            token_params,
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

/// Whether a FIELD name is path-shaped: the parameter rule, the names in
/// [`PATH_BEARING_FIELDS`] that become part of a path without looking like
/// one, and any TOKEN (`output_folder_token`, `source_token`). A token decides
/// where the command acts as surely as the path it stands for, so it is held to
/// a list too (A2) — every `_token`, not only the path-shaped ones.
fn is_path_like_field(name: &str) -> bool {
    is_path_like(name) || PATH_BEARING_FIELDS.contains(&name) || name.ends_with("_token")
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
    let source = strip_comments(source);
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
    assert!(names("EditorExportRequest").contains(&"output_folder_token".to_string()));
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
            Token { resolver, proof } => {
                let command = commands
                    .iter()
                    .find(|c| &c.name == cmd)
                    .expect("findings came from this command");
                if let Some(problem) = token_problem(command, resolver, proof, &crate_code()) {
                    panic!("`{cmd}` lists `{field}` as a token: {problem}");
                }
            }
            Overwritten { by, proof } => {
                assert!(
                    source_defines_fn(by),
                    "`{cmd}` lists `{field}` as overwritten by `{by}`, but no such \
                     function exists any more"
                );
                assert!(
                    source_defines_fn(proof),
                    "`{cmd}` lists `{field}` as overwritten, proven by `{proof}` — but \
                     that test no longer exists"
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

/// This crate's and the core's sources, as code only ([`code_only`]): no
/// comments, no literal contents. What a name «appears in» is what is there.
fn crate_code() -> Vec<String> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&manifest.join("src"), &mut files);
    rust_files(&manifest.join("../crates/sundayrec-core/src"), &mut files);
    files.sort();
    files
        .iter()
        .map(|f| code_only(&std::fs::read_to_string(f).unwrap_or_default()))
        .collect()
}

/// Whether `fn <name>(` is defined in this crate's or the core's sources (in
/// code — a `fn name(` inside a comment does not define it).
fn source_defines_fn(name: &str) -> bool {
    defines_fn(&crate_code(), name)
}

fn defines_fn(sources: &[String], name: &str) -> bool {
    let needle = format!("fn {name}(");
    sources.iter().any(|src| src.contains(&needle))
}

/// The body of the function `name` — from its opening brace to the matching
/// one — in the first of `sources` (as [`code_only`] text, so no brace in a
/// comment or a literal can end it early).
fn fn_body(sources: &[String], name: &str) -> Option<String> {
    let needles = [format!("fn {name}("), format!("fn {name}<")];
    for src in sources {
        let Some(at) = needles.iter().filter_map(|n| src.find(n)).min() else {
            continue;
        };
        let open = at + src[at..].find('{')?;
        let mut depth = 0i32;
        for (i, ch) in src[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(src[open + 1..open + i].to_string());
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// The identifiers a body calls: every `name(` (free function or method).
fn called_names(body: &str) -> BTreeSet<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if chars.get(i) == Some(&'(') {
                out.insert(chars[start..i].iter().collect());
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Whether following the calls from function `name` — a few levels, through
/// functions defined in `sources` — reaches the store's one door,
/// `ChosenPaths::resolve` (a `.resolve(…, ChosenKind::…)` call).
fn reaches_store(
    sources: &[String],
    name: &str,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> bool {
    if depth == 0 || !seen.insert(name.to_string()) {
        return false;
    }
    let Some(body) = fn_body(sources, name) else {
        return false;
    };
    if body.contains(".resolve(") && body.contains("ChosenKind::") {
        return true;
    }
    called_names(&body)
        .iter()
        .any(|callee| reaches_store(sources, callee, depth - 1, seen))
}

/// What is wrong with a Token entry, if anything: the resolver is not called
/// in the command's own body (comments stripped), the chain behind it never
/// reaches `ChosenPaths::resolve`, or its proof test is gone. One function for
/// the real lists and the fixtures that hold it to its word.
fn token_problem(cmd: &Command, resolver: &str, proof: &str, sources: &[String]) -> Option<String> {
    if !cmd.segment.contains(resolver) {
        return Some(format!(
            "its body never calls the resolver `{resolver}` (a comment that \
             names it does not count)"
        ));
    }
    let start = resolver.split('(').next().unwrap_or(resolver);
    if !reaches_store(sources, start, 4, &mut BTreeSet::new()) {
        return Some(format!(
            "following `{start}` never reaches `ChosenPaths::resolve` — the \
             resolver does not turn the token back into a checked place"
        ));
    }
    if !defines_fn(sources, proof) {
        return Some(format!("the proof test `{proof}` no longer exists"));
    }
    None
}

/// Every `(command, parameter)` that is a place-token parameter and is not
/// listed in `listed` — what [`every_token_parameter_is_classified`] requires
/// to be empty, and what the fixture below holds it to.
fn unclassified_token_params(
    commands: &[Command],
    listed: &[(&str, &str, FieldHandling)],
) -> Vec<String> {
    commands
        .iter()
        .flat_map(|cmd| {
            cmd.token_params
                .iter()
                .filter(|p| !listed.iter().any(|(c, l, _)| *c == cmd.name && l == p))
                .filter(|p| {
                    !NOT_PLACE_TOKENS
                        .iter()
                        .any(|(c, l, _)| *c == cmd.name && l == p)
                })
                .map(|p| format!("{} — {p}", cmd.name))
        })
        .collect()
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

/// The fix for A2, pinned from the outside: `editor_export`'s request names
/// its destination ONLY by a token. No field of the request is named like a
/// folder any more — `output_folder` is gone, and a new `export_dir` or
/// `target_folder` would fail here before it failed review — the one
/// place-shaped field left besides the guarded source and jingles is
/// `output_folder_token`, listed as a TOKEN (not guarded, not exempt), and the
/// command that mints those tokens takes nothing from the webview at all.
#[test]
fn editor_export_names_its_folder_only_by_token() {
    let commands = all_commands();
    let structs = all_structs();
    let fields = &structs["EditorExportRequest"];
    let folder_like: Vec<&str> = fields
        .iter()
        .map(|(f, _)| f.as_str())
        .filter(|f| {
            let f = f.to_ascii_lowercase();
            f.contains("folder") || f.contains("dir") || f.contains("dest")
        })
        .collect();
    assert_eq!(
        folder_like,
        vec!["output_folder_token"],
        "the export request may name its folder only by a token"
    );
    let listed: Vec<&FieldHandling> = PATH_FIELDS
        .iter()
        .filter(|(c, f, _)| {
            *c == "editor_export" && *f == "EditorExportRequest.output_folder_token"
        })
        .map(|(_, _, h)| h)
        .collect();
    assert!(
        matches!(listed.as_slice(), [Token { .. }]),
        "output_folder_token must be listed as a Token: {listed:?}"
    );

    let pick = commands
        .iter()
        .find(|c| c.name == "editor_pick_output_folder")
        .expect("editor_pick_output_folder was not parsed");
    let sent: Vec<&(String, String)> = pick
        .params
        .iter()
        .filter(|(_, ty)| !is_injected(ty))
        .collect();
    assert!(
        sent.is_empty(),
        "editor_pick_output_folder takes {sent:?} from the webview — the folder is \
         the answer of the dialog Rust opens, and nothing the webview sends may \
         shape it"
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
    let sidecar = commands
        .iter()
        .find(|c| c.name == "editor_read_sidecar")
        .unwrap();
    assert_eq!(sidecar.path_params, vec!["media_path".to_string()]);
    let peaks = commands.iter().find(|c| c.name == "editor_peaks").unwrap();
    assert!(peaks.path_params.is_empty());
    assert_eq!(peaks.token_params, vec!["source_token".to_string()]);
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
/// can actually invoke. Comments are stripped BEFORE the closing
/// `]` is looked for (the lexer of [`strip_comments`]), so a note that names a
/// retired command is not mistaken for its registration, and a `[…]` in a note
/// does not end the block early.
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
    let src = strip_comments(src);
    let start = src.find(OPEN)? + OPEN.len();
    let end = src[start..].find(']')?;
    Some(src[start..start + end].to_string())
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
    // One level down, EVERY token is judged as a place (A2, widened in PR-C: a
    // `source_token` has no path-shaped word in front of it and slipped by) —
    // a bare `token` is not a name for anything.
    for name in [
        "output_folder_token",
        "intro_file_token",
        "save_dir_token",
        "source_token",
        "job_token",
    ] {
        assert!(is_path_like_field(name), "{name} is a token");
    }
    for name in ["token", "tokens", "tokenizer"] {
        assert!(!is_path_like_field(name), "{name} is not a token");
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

// ── S2/S4: token parameters, and comments that name a guard ─────────────────

#[test]
fn every_token_parameter_is_classified() {
    let unclassified = unclassified_token_params(&all_commands(), PARAM_TOKENS);
    assert!(
        unclassified.is_empty(),
        "\n\
         ────────────────────────────────────────────────────────────────────\n\
         A #[tauri::command] takes a TOKEN for a place as a parameter and it\n\
         is not listed in PARAM_TOKENS. A token is not a path, but it decides\n\
         where the command acts exactly as the path it stands for (finding A2).\n\
         \n\
         {}\n\
         \n\
         List it as `(command, \"param\", Token {{ resolver, proof }})`: the call\n\
         in the command's own body that turns the token back into a CHECKED\n\
         place (`ChosenPaths::resolve`, directly or through a helper), and the\n\
         test that hands it a made-up token and sees it refused.\n\
         ────────────────────────────────────────────────────────────────────",
        unclassified
            .iter()
            .map(|u| format!("  {u}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_listed_token_parameter_exists_and_is_resolved() {
    let commands = all_commands();
    let code = crate_code();
    for (cmd, param, handling) in PARAM_TOKENS {
        let Some(command) = commands.iter().find(|c| &c.name == cmd) else {
            panic!("PARAM_TOKENS lists `{cmd}`, which no longer exists — remove the entry");
        };
        assert!(
            command.token_params.iter().any(|p| p == param),
            "PARAM_TOKENS lists `{cmd}` / `{param}`, but that command no longer \
             takes it — remove the stale entry"
        );
        let Token { resolver, proof } = handling else {
            panic!("`{cmd}` / `{param}`: a token parameter must be listed as a Token");
        };
        if let Some(problem) = token_problem(command, resolver, proof, &code) {
            panic!("`{cmd}` lists the token parameter `{param}`: {problem}");
        }
    }
}

#[test]
fn an_unlisted_token_parameter_is_found() {
    // The shape of a command taking its token directly. Not listed → the
    // ratchet names it; listed → it does not. EVERY `_token` counts, a
    // `source_token` as much as a `recording_file_token`.
    let src = r#"
#[tauri::command]
pub async fn editor_open_recording(
    chosen: State<'_, ChosenPaths>,
    recording_file_token: String,
    source_token: String,
) -> AppResult<()> {
    Ok(())
}
"#;
    let commands = parse_file("fixture.rs", src);
    assert_eq!(
        commands[0].token_params,
        vec![
            "recording_file_token".to_string(),
            "source_token".to_string()
        ],
        "every `_token` is a token parameter"
    );
    assert_eq!(
        unclassified_token_params(&commands, &[]),
        vec![
            "editor_open_recording — recording_file_token".to_string(),
            "editor_open_recording — source_token".to_string()
        ],
        "an unlisted token parameter must be reported"
    );
    let listed = [(
        "editor_open_recording",
        "recording_file_token",
        Token {
            resolver: "x",
            proof: "y",
        },
    )];
    assert_eq!(
        unclassified_token_params(&commands, &listed),
        vec!["editor_open_recording — source_token".to_string()]
    );
}

#[test]
fn the_token_parameter_rule_matches_every_token() {
    for name in [
        "output_folder_token",
        "_recording_file_token",
        "save_dir_token",
        "media_path_token",
        "source_token",
        "device_token",
        "job_token",
    ] {
        assert!(is_token_param(name), "{name} is a token");
    }
    for name in ["token", "folder", "tokenizer", "source"] {
        assert!(!is_token_param(name), "{name} is not a token");
    }
}

#[test]
fn every_token_that_is_not_a_place_says_why_and_still_exists() {
    let commands = all_commands();
    for (cmd, param, reason) in NOT_PLACE_TOKENS {
        let Some(command) = commands.iter().find(|c| &c.name == cmd) else {
            panic!("NOT_PLACE_TOKENS lists `{cmd}`, which no longer exists — remove the entry");
        };
        assert!(
            command.token_params.iter().any(|p| p == param),
            "NOT_PLACE_TOKENS lists `{cmd}` / `{param}`, but that command no longer \
             takes it — remove the stale entry"
        );
        assert!(
            reason.trim().len() >= 20,
            "`{cmd}` / `{param}` needs a real reason"
        );
        assert!(
            !PARAM_TOKENS.iter().any(|(c, p, _)| c == cmd && p == param),
            "`{cmd}` / `{param}` is both a place token and not one"
        );
    }
}

#[test]
fn the_comment_stripper_removes_every_kind_and_keeps_the_lines() {
    let src = "a // line\n/// doc\n//! inner\nb /* block */ c\n/* multi\nline */ d\n\
               /* outer /* nested */ still outer */ e\n";
    let out = strip_comments(src);
    for gone in ["line", "doc", "inner", "block", "multi", "nested", "outer"] {
        assert!(!out.contains(gone), "`{gone}` survived: {out:?}");
    }
    let tokens: Vec<&str> = out.split_whitespace().collect();
    assert_eq!(tokens, ["a", "b", "c", "d", "e"]);
    assert_eq!(out.lines().count(), src.lines().count(), "newlines kept");
}

#[test]
fn the_comment_stripper_leaves_literals_alone() {
    // `//` in a string is not a comment, a quote in a comment opens nothing,
    // and raw strings, chars and lifetimes do not confuse it.
    let src = r##"let u = "https://example.org/x"; // "unterminated
let r = r#"a // b "quoted" c"#; /* " */ let c = '"'; let d = '\'';
fn f<'a>(x: &'a str) -> &'a str { x } // done
"##;
    let out = strip_comments(src);
    assert!(out.contains(r#""https://example.org/x""#), "{out}");
    assert!(out.contains(r##"r#"a // b "quoted" c"#"##), "{out}");
    assert!(
        out.contains("fn f<'a>(x: &'a str) -> &'a str { x }"),
        "{out}"
    );
    assert!(
        !out.contains("unterminated") && !out.contains("done"),
        "{out}"
    );
    // And in code-only mode the literals' contents are blank — a name inside
    // one cannot be a call.
    let code = code_only(r#"let s = "path_guard::checked_path(x)"; let t = '{';"#);
    assert!(
        !code.contains("path_guard") && !code.contains('{'),
        "{code}"
    );
}

/// A guard that only a COMMENT names must not count (S4). Three shapes of
/// comment, one fixture: a `//` line in the body, a `/* */` block in the body,
/// and the doc comment of the NEXT command, which sits in this command's
/// segment (the segment runs to the next `#[tauri::command]`, and a doc comment
/// comes before it).
#[test]
fn a_guard_named_only_in_a_comment_does_not_count() {
    let src = r#"
#[tauri::command]
pub async fn editor_peaks(input_path: String) -> AppResult<()> {
    // path_guard::checked_input_file(&input_path)?;
    /* path_guard::checked_input_file(&input_path)?; */
    do_it(&input_path)
}

/// Reads a file — behind `path_guard::checked_input_file`, honest.
#[tauri::command]
pub async fn editor_other(input_path: String) -> AppResult<()> {
    path_guard::checked_input_file(&input_path)?;
    Ok(())
}
"#;
    let commands = parse_file("fixture.rs", src);
    let (unguarded, guarded) = (&commands[0], &commands[1]);
    assert!(
        !unguarded.segment.contains("path_guard"),
        "a comment is not a call: {:?}",
        unguarded.segment
    );
    assert!(guarded.segment.contains("path_guard::checked_input_file"));
}

/// The same for a field's guard and a token's resolver: named only in a
/// comment, the entry FAILS.
#[test]
fn a_resolver_named_only_in_a_comment_fails_the_token_check() {
    let sources = vec![code_only(
        r#"
async fn run_export() {
    let _ = store.resolve(&token, ChosenKind::Folder);
}
async fn a_forged_token_is_refused() {}
"#,
    )];
    let real = r#"
#[tauri::command]
pub async fn editor_export(chosen: State<'_, ChosenPaths>, request: R) -> AppResult<()> {
    run_export(&chosen, &request).await
}
"#;
    let commented = r#"
#[tauri::command]
pub async fn editor_export(chosen: State<'_, ChosenPaths>, request: R) -> AppResult<()> {
    // run_export(&chosen, &request).await
    /// run_export(&chosen, &request)
    render(&request).await
}
"#;
    let resolver = "run_export(&chosen";
    let proof = "a_forged_token_is_refused";
    let ok = &parse_file("fixture.rs", real)[0];
    assert_eq!(token_problem(ok, resolver, proof, &sources), None);
    let bad = &parse_file("fixture.rs", commented)[0];
    let problem = token_problem(bad, resolver, proof, &sources)
        .expect("a resolver that only a comment names must fail");
    assert!(problem.contains("never calls the resolver"), "{problem}");
}

#[test]
fn a_resolver_that_never_reaches_the_store_fails_the_token_check() {
    // The command calls its resolver, and the resolver hands back the stored
    // string without going through `ChosenPaths::resolve` — the unchecked
    // lookup the store no longer offers. Name only, nothing behind it.
    let sources = vec![code_only(
        r#"
async fn run_export() { let _ = lookup_raw(token); }
async fn lookup_raw(t: &str) {}
async fn a_forged_token_is_refused() {}
"#,
    )];
    let cmd = &parse_file(
        "fixture.rs",
        r#"
#[tauri::command]
pub async fn editor_export(chosen: State<'_, ChosenPaths>) -> AppResult<()> {
    run_export(&chosen).await
}
"#,
    )[0];
    let problem = token_problem(
        cmd,
        "run_export(&chosen",
        "a_forged_token_is_refused",
        &sources,
    )
    .expect("no path to `ChosenPaths::resolve`");
    assert!(problem.contains("never reaches"), "{problem}");
    // …and a proof test that is gone is its own failure.
    let reaching = vec![code_only(
        "async fn run_export() { s.resolve(&t, ChosenKind::Folder); }",
    )];
    let problem =
        token_problem(cmd, "run_export(&chosen", "gone", &reaching).expect("the proof must exist");
    assert!(problem.contains("no longer exists"), "{problem}");
}

// ── PR-C: literals are not calls, the source is a token, a mint has a dialog ──

/// S4a: a guard named only in a STRING LITERAL must not count. The segment is
/// read as code with its literals blanked, so `let _unused =
/// "check_export_paths(&request)";` — which the comment-stripper alone left
/// standing — is not a call to it. (The review of #311 built exactly this
/// mutant: the guarded entry stayed green over an unguarded command.)
#[test]
fn a_guard_named_only_in_a_string_literal_does_not_count() {
    let src = r##"
#[tauri::command]
pub async fn editor_export(request: R) -> AppResult<()> {
    let _unused = "check_export_paths(&request)";
    let _raw = r#"run_export(&chosen, &request)"#;
    let _ch = ';';
    render(&request).await
}

#[tauri::command]
pub async fn editor_other(request: R) -> AppResult<()> {
    check_export_paths(&request)?;
    Ok(())
}
"##;
    let commands = parse_file("fixture.rs", src);
    let (mutant, real) = (&commands[0], &commands[1]);
    for named in ["check_export_paths(&request)", "run_export(&chosen"] {
        assert!(
            !mutant.segment.contains(named),
            "a literal is not a call to `{named}`: {:?}",
            mutant.segment
        );
    }
    assert!(real.segment.contains("check_export_paths(&request)"));
    // And through the same check the real lists use: the Guarded field entry
    // would fail on the mutant and pass on the real one.
    assert!(!mutant.segment.contains("check_export_paths(&request)"));
}

/// The literal is blanked in the token check too: a resolver named only in a
/// string fails `token_problem` like one named only in a comment.
#[test]
fn a_resolver_named_only_in_a_string_literal_fails_the_token_check() {
    let sources = vec![code_only(
        r#"
async fn run_export() { s.resolve(&t, ChosenKind::File); }
async fn a_forged_token_is_refused() {}
"#,
    )];
    let mutant = &parse_file(
        "fixture.rs",
        r#"
#[tauri::command]
pub async fn editor_export(chosen: State<'_, ChosenPaths>) -> AppResult<()> {
    let _unused = "run_export(&chosen)";
    render().await
}
"#,
    )[0];
    let problem = token_problem(
        mutant,
        "run_export(&chosen",
        "a_forged_token_is_refused",
        &sources,
    )
    .expect("a literal is not a call");
    assert!(problem.contains("never calls the resolver"), "{problem}");
}

/// The fix for A2's second half, pinned from the outside: the export request
/// and the preview request name the recording ONLY by a token. No field of
/// either is named like a path, a file or a source besides `source_token` —
/// `input_path` and the jingle paths are gone, and a new `media_file` or
/// `intro_path` would fail here before it failed review.
#[test]
fn the_requests_name_the_recording_only_by_token() {
    let structs = all_structs();
    for request in ["EditorExportRequest", "EditorMasterPreviewRequest"] {
        let named: Vec<&str> = structs[request]
            .iter()
            .map(|(f, _)| f.as_str())
            .filter(|f| {
                let f = f.to_ascii_lowercase();
                f.contains("path")
                    || f.contains("file")
                    || f.contains("source")
                    || f.contains("input")
                    || f.contains("intro")
                    || f.contains("outro")
                    || f.contains("clip")
            })
            .collect();
        let allowed: &[&str] = if request == "EditorExportRequest" {
            // The two switches: Rust reads the clips from the saved settings.
            &["source_token", "use_intro", "use_outro"]
        } else {
            &["source_token"]
        };
        assert_eq!(named, allowed, "{request} may name a place only by a token");
    }
    // The opening commands take nothing a place could ride in on.
    let commands = all_commands();
    let open = commands
        .iter()
        .find(|c| c.name == "editor_open_recording")
        .expect("editor_open_recording was not parsed");
    assert!(
        open.params.iter().all(|(_, ty)| is_injected(ty)),
        "editor_open_recording takes {:?} from the webview",
        open.params
    );
}

/// `editor_open_known` is the one opening command the webview sends something
/// to, and what it sends is a history ROW'S ID — the database decides the file.
/// Not path-shaped, not a token, and not allowed to become either: the lookup is
/// by id (`recording_file_path`), and a second parameter would be a second way
/// to say where.
#[test]
fn editor_open_known_takes_only_a_history_row_id() {
    let commands = all_commands();
    let known = commands
        .iter()
        .find(|c| c.name == "editor_open_known")
        .expect("editor_open_known was not parsed");
    let sent: Vec<&(String, String)> = known
        .params
        .iter()
        .filter(|(_, ty)| !is_injected(ty))
        .collect();
    assert_eq!(
        sent,
        vec![&("recording_id".to_string(), "String".to_string())],
        "editor_open_known takes a row id and nothing else"
    );
    assert!(known.path_params.is_empty() && known.token_params.is_empty());
    assert!(
        known.segment.contains("open_known("),
        "its body is the database lookup: {:?}",
        known.segment
    );
}

// ── A mint needs a dialog, a database row or the OS ──────────────────────────

/// The CLOSED list of functions that may hold `.mint(` in production code, and
/// why each one may. A token stands for a place the OPERATOR chose, so a new
/// way to make one is a decision a person makes here, with a sentence, not a
/// thing a lexical anchor waves through (review of #313: a command that CALLED
/// `recording_file_path(` and ignored the answer passed the old anchor check
/// while minting a token for whatever the webview sent).
///
/// A function on this list is only half of it: every door INTO one is on
/// [`MINT_DOORS`], with the evidence that the place it is handed is one Rust
/// obtained.
const MINTERS: &[(&str, &str)] = &[
    (
        "open_source",
        "the one door a file enters the editor by: vets the place as a file, \
         opens `asset://` to exactly it and mints its token. Takes a place, \
         never decides one — what hands it a place is on MINT_DOORS.",
    ),
    (
        "choose_output_folder",
        "mints the token for the export folder `editor_pick_output_folder` \
         picked in a dialog Rust opened; a cancel mints nothing.",
    ),
];

/// How a door proves the place it hands a minter is one Rust obtained.
enum Evidence {
    /// The argument is bound from `source` in the caller's own body — the dialog's
    /// answer, the database's row — and from nothing else. Checked structurally
    /// by [`bound_only_from`]: an unused call to the source does not pass it.
    Flows { source: &'static str },
    /// The caller is the OS' own event handler: its body names `anchor`, the
    /// event the OS reported to the process.
    Os { anchor: &'static str },
    /// The caller only forwards its parameter; ITS callers are rows of their own.
    Forwards,
}

/// `(callee, caller, evidence, why)`: every function that may call a minter (or
/// a function that forwards to one), and what makes the place it passes one the
/// webview did not name. A call to a minter from a function not on this list is
/// a violation, whatever else the function does.
const MINT_DOORS: &[(&str, &str, Evidence, &str)] = &[
    (
        "open_source",
        "editor_open_recording",
        Evidence::Flows {
            source: "ask_for_file(",
        },
        "The open-file dialog: the file picker Rust opens; the place is its answer.",
    ),
    (
        "open_source",
        "open_known",
        Evidence::Flows {
            source: "recording_file_path(",
        },
        "A history row: the webview sends a row id, the database holds the file.",
    ),
    (
        "open_source",
        "dropped_recording",
        Evidence::Forwards,
        "Opens the dropped file like a picked one; only `note_drop` calls it.",
    ),
    (
        "dropped_recording",
        "note_drop",
        Evidence::Os {
            anchor: "DragDropEvent::Drop",
        },
        "The window's own drop handler: the OS reported the drop to the process, \
         and the path never reaches the page.",
    ),
    (
        "choose_output_folder",
        "editor_pick_output_folder",
        Evidence::Flows {
            source: "ask_for_folder(",
        },
        "The folder picker Rust opens; the place is its answer.",
    ),
];

/// `src` with every `#[cfg(test)] mod … { … }` removed (the tests mint freely).
/// Works on [`code_only`] text, so braces in literals and comments are gone.
fn without_test_modules(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(at) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(open) = after.find('{') else {
            rest = "";
            break;
        };
        // Only a `mod` is cut; any other `#[cfg(test)]` item keeps its text.
        if !after[..open].contains("mod ") {
            out.push_str(&after[..open]);
            rest = &after[open..];
            continue;
        }
        let mut depth = 0i32;
        let mut end = after.len();
        for (i, ch) in after[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Every `fn name … { body }` in `src` (code-only text), with its body. A
/// declaration without a body (`fn f();`) is skipped.
fn fn_bodies(src: &str) -> Vec<(String, String)> {
    let is_ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = src[from..].find("fn ") {
        let at = from + rel;
        from = at + 3;
        if src[..at].chars().next_back().is_some_and(is_ident_char) {
            continue;
        }
        let name: String = src[at + 3..]
            .chars()
            .take_while(|c| is_ident_char(*c))
            .collect();
        if name.is_empty() {
            continue;
        }
        // The body opens at the first `{` that comes before a `;` outside the
        // parameter list.
        let after = &src[at + 3 + name.len()..];
        let mut paren = 0i32;
        let mut open = None;
        for (i, ch) in after.char_indices() {
            match ch {
                '(' => paren += 1,
                ')' => paren -= 1,
                ';' if paren == 0 => break,
                '{' if paren == 0 => {
                    open = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(open) = open else { continue };
        let mut depth = 0i32;
        for (i, ch) in after[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        out.push((name.clone(), after[open + 1..open + i].to_string()));
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// The whole-word occurrence of `word` in `hay`.
fn contains_word(hay: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    hay.match_indices(word).any(|(at, _)| {
        !hay[..at].chars().next_back().is_some_and(is_ident)
            && !hay[at + word.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The right-hand sides of every `let` in `body` whose pattern binds `ident`
/// (`let ident =`, `let Some(ident) = …`), each up to its closing `;`.
fn bindings_of(body: &str, ident: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (at, _) in body.match_indices("let ") {
        if body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let rest = &body[at + 4..];
        let Some(eq) = rest.find('=') else { continue };
        if !contains_word(&rest[..eq], ident) {
            continue;
        }
        let mut depth = 0i32;
        let mut end = rest.len();
        for (i, ch) in rest[eq + 1..].char_indices() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ';' if depth <= 0 => {
                    end = eq + 1 + i;
                    break;
                }
                _ => {}
            }
        }
        out.push(rest[eq + 1..end].to_string());
    }
    out
}

/// Whether `ident` comes from `source` and nothing else: it is bound at least
/// once, and EVERY `let` that binds it takes its value from a statement that
/// holds the `source` call or from `ident` itself (`let Some(picked) = picked
/// else …`). A `let _ = source(…)` followed by `let place = <something else>`
/// binds `place` to nothing the source said.
fn bound_only_from(body: &str, ident: &str, source: &str) -> bool {
    let rhs = bindings_of(body, ident);
    !rhs.is_empty()
        && rhs
            .iter()
            .all(|r| r.contains(source) || contains_word(r, ident))
}

/// The place argument (the second one) a call to `callee` is handed in `body`:
/// `place`, or `PathBuf::from(place)`. `None` for any other shape — a door that
/// builds its place in the call is not one this check can vouch for.
fn place_argument(body: &str, callee: &str) -> Option<String> {
    let arg = call_args(body, callee)?.get(1)?.clone();
    let inner = arg
        .strip_prefix("PathBuf::from(")
        .and_then(|a| a.strip_suffix(')'))
        .unwrap_or(&arg)
        .trim()
        .to_string();
    let is_ident =
        !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    is_ident.then_some(inner)
}

/// What is wrong with how `sources` mint tokens, against the closed lists:
///
/// 1. `.mint(` appears only inside a [`MINTERS`] function, and every one of
///    them is still there;
/// 2. a call to a minter — or to a function that forwards to one — is made only
///    by a [`MINT_DOORS`] caller, and every row is still true (the caller exists
///    and calls the callee);
/// 3. each door's evidence holds: the place it hands over is bound from the
///    dialog or the row and from nothing else (`Flows`), is the OS' own drop
///    (`Os`), or is forwarded by a function whose own callers are rows
///    (`Forwards`).
fn mint_violations(sources: &[String]) -> Vec<String> {
    let fns: Vec<(String, String)> = sources
        .iter()
        .flat_map(|src| fn_bodies(&without_test_modules(src)))
        .collect();
    let body_of = |name: &str| fns.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_str());
    let mut bad = Vec::new();

    // 1. The closed list of functions that hold `.mint(`.
    for (name, body) in &fns {
        if body.contains(".mint(") && !MINTERS.iter().any(|(m, _)| m == name) {
            bad.push(format!(
                "`{name}` mints a token and is not on MINTERS — a new way to make \
                 a token is decided there, with a reason"
            ));
        }
    }
    for (minter, why) in MINTERS {
        if why.trim().is_empty() {
            bad.push(format!("MINTERS: `{minter}` has no reason"));
        }
        if !body_of(minter).is_some_and(|b| b.contains(".mint(")) {
            bad.push(format!("MINTERS lists `{minter}`, which no longer mints"));
        }
    }

    // 2. Who may call a minter (or a forwarder).
    let callees: BTreeSet<&str> = MINT_DOORS.iter().map(|(callee, ..)| *callee).collect();
    for (name, body) in &fns {
        for callee in called_names(body)
            .iter()
            .filter(|c| callees.contains(c.as_str()))
        {
            if name != callee
                && !MINT_DOORS
                    .iter()
                    .any(|(c, caller, ..)| c == callee && caller == name)
            {
                bad.push(format!(
                    "`{name}` calls `{callee}` and is not a door on MINT_DOORS"
                ));
            }
        }
    }

    // 3. Every row is still true, and its evidence holds.
    for (callee, caller, evidence, why) in MINT_DOORS {
        if why.trim().is_empty() {
            bad.push(format!("MINT_DOORS: `{caller}` → `{callee}` has no reason"));
        }
        let Some(body) = body_of(caller) else {
            bad.push(format!("MINT_DOORS lists `{caller}`, which is gone"));
            continue;
        };
        if !called_names(body).contains(*callee) {
            bad.push(format!("MINT_DOORS: `{caller}` no longer calls `{callee}`"));
            continue;
        }
        match evidence {
            Evidence::Flows { source } => match place_argument(body, callee) {
                None => bad.push(format!(
                    "`{caller}` does not hand `{callee}` a plain place variable"
                )),
                Some(place) if !bound_only_from(body, &place, source) => bad.push(format!(
                    "`{caller}` hands `{callee}` `{place}`, which does not come from \
                     `{source}` alone"
                )),
                Some(_) => {}
            },
            Evidence::Os { anchor } => {
                if !body.contains(anchor) {
                    bad.push(format!("`{caller}` has no `{anchor}` behind its call"));
                }
            }
            Evidence::Forwards => {
                if !MINT_DOORS.iter().any(|(c, ..)| c == caller) {
                    bad.push(format!(
                        "`{caller}` forwards a place but its own callers are not on MINT_DOORS"
                    ));
                }
            }
        }
    }
    bad.sort();
    bad.dedup();
    bad
}

/// The crate's sources as code only, test modules and this file excluded.
fn production_code() -> Vec<String> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&manifest.join("src"), &mut files);
    files.sort();
    files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| n != "path_ratchet.rs"))
        .map(|f| code_only(&std::fs::read_to_string(f).unwrap_or_default()))
        .collect()
}

#[test]
fn every_token_is_minted_by_a_listed_function_behind_a_listed_door() {
    let sources = production_code();
    // Did it find the mints at all? A reader that saw none would pass vacuously.
    let minting: Vec<String> = sources
        .iter()
        .flat_map(|src| fn_bodies(&without_test_modules(src)))
        .filter(|(_, body)| body.contains(".mint("))
        .map(|(name, _)| name)
        .collect();
    for (known, _) in MINTERS {
        assert!(
            minting.iter().any(|n| n == known),
            "the mint reader did not find `{known}` ({minting:?})"
        );
    }
    assert_eq!(
        mint_violations(&sources),
        Vec::<String>::new(),
        "\n\
         ────────────────────────────────────────────────────────────────────\n\
         A token (`ChosenPaths::mint`) is made outside MINTERS, or a minter is\n\
         reached from outside MINT_DOORS, or a door's place no longer comes from\n\
         a dialog Rust opened, a history row or the OS' own drop. A token stands\n\
         for a place the OPERATOR chose; minting one from something the webview\n\
         sent turns the token store into the path parameter it replaced (A2).\n\
         Adding a way in is a decision: put it on the list, with its reason.\n\
         ────────────────────────────────────────────────────────────────────"
    );
}

/// The real doors, as the fixtures' starting point: the ratchet must pass them,
/// and every mutant below is one edit away from them.
const REAL_DOORS: &str = r#"
pub async fn editor_open_recording(window: Window, chosen: State<'_, ChosenPaths>) -> AppResult<Option<OpenedRecording>> {
    let picked = chosen_paths::ask_for_file(&window, &[]).await?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    open_source(&chosen, picked, grant_asset_file(&app)).await.map(Some)
}
pub async fn open_known(pool: &SqlitePool, chosen: &ChosenPaths, recording_id: &str, grant: G) -> AppResult<OpenedRecording> {
    let file = store::recording_file_path(pool, recording_id)
        .await?
        .ok_or_else(|| source_error(ChosenError::Unknown))?;
    open_source(chosen, PathBuf::from(file), grant).await
}
pub fn note_drop(window: &Window, event: &DragDropEvent) {
    let DragDropEvent::Drop { paths, position } = event else { return };
    let Some(first) = paths.first().cloned() else { return };
    let _ = dropped_recording(&chosen, first, (0.0, 0.0), grant);
}
pub async fn dropped_recording(chosen: &ChosenPaths, file: PathBuf, at: (f64, f64), grant: G) -> DroppedRecording {
    match open_source(chosen, file, grant).await { _ => todo() }
}
pub async fn open_source(chosen: &ChosenPaths, place: PathBuf, grant: G) -> AppResult<OpenedRecording> {
    let vetted = vet(&place).await?;
    let token = chosen.mint(vetted);
    Ok(token)
}
pub async fn editor_pick_output_folder(window: Window, chosen: State<'_, ChosenPaths>) -> AppResult<Option<ChosenPlace>> {
    let picked = chosen_paths::ask_for_folder(&window).await?;
    choose_output_folder(&chosen, picked).await
}
pub(crate) async fn choose_output_folder(chosen: &ChosenPaths, picked: Option<PathBuf>) -> AppResult<Option<ChosenPlace>> {
    let Some(picked) = picked else { return Ok(None) };
    let token = chosen.mint(vet(&picked).await?);
    Ok(Some(token))
}
"#;

/// [`REAL_DOORS`] with `from` replaced by `to` — exactly once, so a mutant that
/// stops matching the fixture fails loudly instead of testing nothing.
fn mutated(from: &str, to: &str) -> String {
    assert_eq!(
        REAL_DOORS.matches(from).count(),
        1,
        "mutation site {from:?}"
    );
    code_only(&REAL_DOORS.replace(from, to))
}

#[test]
fn the_real_doors_pass_the_mint_ratchet() {
    assert_eq!(
        mint_violations(&[code_only(REAL_DOORS)]),
        Vec::<String>::new()
    );
}

#[test]
fn a_command_that_mints_a_path_from_the_webview_is_found() {
    // The shape A2 closes, as a fixture: a command that takes a path, vets it
    // (so `mint` accepts it) and mints a token — no dialog anywhere. The vet
    // proves the path is a file; it proves nothing about who chose it.
    let bad = format!(
        "{REAL_DOORS}\n{}",
        r#"
#[tauri::command]
pub async fn editor_register_recording(chosen: State<'_, ChosenPaths>, path: String) -> AppResult<String> {
    let vetted = chosen_paths::vet(&PathBuf::from(path), ChosenKind::File)?;
    Ok(chosen.mint(vetted))
}
"#
    );
    let found = mint_violations(&[code_only(&bad)]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`editor_register_recording` mints a token"));

    // The same mint, split into a helper, called by a command: the helper is
    // not on the list, and neither is the command's call to `open_source`.
    let split_bad = format!(
        "{REAL_DOORS}\n{}",
        r#"
async fn remember(chosen: &ChosenPaths, p: PathBuf) -> String {
    chosen.mint(vet(&p).unwrap())
}
#[tauri::command]
pub async fn editor_register_recording(chosen: State<'_, ChosenPaths>, path: String) -> String {
    remember(&chosen, PathBuf::from(path)).await
}
"#
    );
    let found = mint_violations(&[code_only(&split_bad)]);
    assert!(
        found.iter().any(|f| f.contains("`remember` mints a token")),
        "{found:?}"
    );
}

#[test]
fn a_command_that_reaches_a_minter_without_being_a_door_is_found() {
    // No `.mint(` of its own — it hands the webview's string to `open_source`.
    let bad = format!(
        "{REAL_DOORS}\n{}",
        r#"
#[tauri::command]
pub async fn editor_open_path(chosen: State<'_, ChosenPaths>, path: String) -> AppResult<OpenedRecording> {
    open_source(&chosen, PathBuf::from(path), grant).await
}
"#
    );
    assert_eq!(
        mint_violations(&[code_only(&bad)]),
        vec!["`editor_open_path` calls `open_source` and is not a door on MINT_DOORS"]
    );
}

#[test]
fn a_row_lookup_whose_answer_is_ignored_does_not_anchor_a_mint() {
    // The mutant of the review of #313: the command CALLS `recording_file_path(`
    // — the old lexical anchor — and then mints for something else.

    // (a) in a new function: not on the list.
    let new_fn = format!(
        "{REAL_DOORS}\n{}",
        r#"
pub async fn editor_open_by_name(pool: &SqlitePool, chosen: &ChosenPaths, id: String, path: String) -> String {
    let _ = store::recording_file_path(pool, &id).await;
    chosen.mint(vet(&PathBuf::from(path)).unwrap())
}
"#
    );
    let found = mint_violations(&[code_only(&new_fn)]);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`editor_open_by_name` mints a token")),
        "{found:?}"
    );

    // (b) in the real door, no `.mint(` of its own: the row is read and dropped,
    // the place is the webview's.
    let in_door = mutated(
        "let file = store::recording_file_path(pool, recording_id)",
        "let _unused = store::recording_file_path(pool, recording_id)",
    );
    let found = mint_violations(&[in_door]);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`open_known` hands `open_source` `file`")),
        "{found:?}"
    );

    // (c) the row is kept, then the place is swapped for something else.
    let swapped = mutated(
        "open_source(chosen, PathBuf::from(file), grant).await\n}",
        "let file = recording_id.to_string();\n    open_source(chosen, PathBuf::from(file), grant).await\n}",
    );
    let found = mint_violations(&[swapped]);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`open_known` hands `open_source` `file`")),
        "{found:?}"
    );

    // (d) the id itself is handed over.
    let by_id = mutated(
        "open_source(chosen, PathBuf::from(file), grant).await\n}",
        "let _ = file;\n    open_source(chosen, PathBuf::from(recording_id), grant).await\n}",
    );
    let found = mint_violations(&[by_id]);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`open_known` hands `open_source` `recording_id`")),
        "{found:?}"
    );
}

#[test]
fn a_dialog_whose_answer_is_ignored_does_not_anchor_a_mint_either() {
    let ignored = mutated(
        "let picked = chosen_paths::ask_for_file(&window, &[]).await?;",
        "let _ = chosen_paths::ask_for_file(&window, &[]).await?;\n    let picked = PathBuf::from(typed_path);",
    );
    let found = mint_violations(&[ignored]);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`editor_open_recording` hands `open_source` `picked`")),
        "{found:?}"
    );
}

#[test]
fn a_stale_or_reasonless_list_entry_is_found() {
    // A door that no longer calls its minter, or a minter that no longer mints,
    // is a list that has stopped describing the code.
    let gone = mutated(
        "let token = chosen.mint(vetted);",
        "let token = vetted.token();",
    );
    let found = mint_violations(&[gone]);
    assert!(
        found
            .iter()
            .any(|f| f.contains("MINTERS lists `open_source`, which no longer mints")),
        "{found:?}"
    );
}

// ── editor_export hands the seam what run_export resolved ────────────────────

/// The top-level arguments of the first call to `name(` in `body` (code-only
/// text), each trimmed. `None` if there is no such call.
fn call_args(body: &str, name: &str) -> Option<Vec<String>> {
    let at = body.find(&format!("{name}("))? + name.len();
    let mut depth = 0i32;
    let mut args = vec![String::new()];
    for ch in body[at..].chars() {
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => {
                args.push(String::new());
                continue;
            }
            _ => {}
        }
        args.last_mut()?.push(ch);
    }
    Some(args.into_iter().map(|a| a.trim().to_string()).collect())
}

/// What is wrong with an `editor_export` body, if anything: the seam
/// (`editor::export(`) must be handed the closure's `&resolved` — the places
/// `run_export` resolved — as its third argument, and the body must not build
/// a folder or a `ResolvedExport` of its own. M3c's text check, made
/// structural: the test that runs the render with a stand-in sees what
/// `run_export` passes the closure, and cannot see that the command then
/// ignored it.
fn export_hands_the_resolved_places(body: &str) -> Option<String> {
    let Some(args) = call_args(body, "editor::export") else {
        return Some("it never calls `editor::export(`".into());
    };
    if args.get(2).map(String::as_str) != Some("&resolved") {
        return Some(format!(
            "`editor::export` is not handed `&resolved` as its third argument ({args:?})"
        ));
    }
    if !body.contains("|resolved|") {
        return Some("`resolved` is not the closure `run_export` passes the places to".into());
    }
    for forged in ["ExportFolder", "ResolvedExport"] {
        if body.contains(forged) {
            return Some(format!(
                "the command names `{forged}` itself — it may only forward what \
                 `run_export` resolved"
            ));
        }
    }
    None
}

#[test]
fn editor_export_hands_the_seam_the_resolved_places() {
    let sources = production_code();
    let body = fn_body(&sources, "editor_export").expect("editor_export has a body");
    assert_eq!(export_hands_the_resolved_places(&body), None);
}

/// M3c2: the mutants the review of #311 built. `&folder` swapped for
/// `&ExportFolder::BesideSource` with `let _ = &folder;` kept (so the unused
/// variable, and every text check for the name, stays quiet), the same in this
/// design's names, the argument named only in a literal, and a `ResolvedExport`
/// built in the command.
#[test]
fn an_export_that_ignores_the_resolved_places_is_found() {
    let real = code_only(
        r#"
    let result = run_export(&chosen, &db.pool, request_ref, |resolved| async move {
        editor::export(engine, request_ref, &resolved, editor::HW_ENCODE_FIRST, progress).await
    })
    .await?;
"#,
    );
    assert_eq!(export_hands_the_resolved_places(&real), None);

    for (what, mutant) in [
        (
            "BesideSource for the folder, `let _ =` keeping the name quiet",
            r#"
    let result = run_export(&chosen, &db.pool, request_ref, |resolved| async move {
        let _ = &resolved;
        editor::export(engine, request_ref, &ExportFolder::BesideSource, HW, progress).await
    }).await?;"#,
        ),
        (
            "a ResolvedExport built in the command",
            r#"
    let result = run_export(&chosen, &db.pool, request_ref, |resolved| async move {
        let _ = &resolved;
        editor::export(engine, request_ref, &ResolvedExport { source: s, intro: None, outro: None, folder: f }, HW, progress).await
    }).await?;"#,
        ),
        (
            "the argument only in a string literal",
            r#"
    let result = run_export(&chosen, &db.pool, request_ref, |resolved| async move {
        let _unused = "&resolved";
        editor::export(engine, request_ref, &other, HW, progress).await
    }).await?;"#,
        ),
        (
            "the seam called with the request's own folder",
            r#"
    let result = run_export(&chosen, &db.pool, request_ref, |resolved| async move {
        let _ = &resolved;
        editor::export(engine, request_ref, &request_ref.out, HW, progress).await
    }).await?;"#,
        ),
        (
            "no run_export closure at all",
            r#"
    let result = editor::export(engine, request_ref, &resolved, HW, progress).await?;"#,
        ),
    ] {
        assert!(
            export_hands_the_resolved_places(&code_only(mutant)).is_some(),
            "not caught: {what}"
        );
    }
}
