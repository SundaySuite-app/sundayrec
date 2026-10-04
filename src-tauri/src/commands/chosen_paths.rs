//! Places the operator picked in a native dialog RUST opened, and the opaque
//! session tokens the webview refers back to them by.
//!
//! ## The rule
//!
//! A user-chosen file or folder location comes from a native dialog the
//! PROCESS opens — never from a path the webview sends. The app has no
//! per-command ACL, so a compromised webview can call any `#[tauri::command]`
//! with any arguments; a path parameter is therefore only a claim that a
//! dialog was shown, and `path_guard` can judge such a claim only against the
//! protected home folders. That is how finding A2 worked: the export page
//! opened the folder picker in JavaScript and sent the answer back as
//! `editor_export`'s `output_folder`, so ffmpeg would render into any folder
//! the user can write to — no dialog needed.
//!
//! When the dialog and the use of its answer are one step (the settings
//! profile, `commands::settings`), the command simply opens the dialog and
//! acts on the answer. When they are NOT — «Velg mappe …» is clicked on the
//! export form and the export runs on a later click, possibly several times
//! («Eksporter i annet format») — the webview has to name the place in between.
//! It does so with a TOKEN from this store: a random UUID that Rust minted when
//! the dialog answered, that means nothing outside this process, and that the
//! webview can only hand back, never make up.
//!
//! ## What the store promises
//!
//! - **Minted only here, only after a dialog Rust opened answered** and the
//!   answer passed [`vet`]. No command takes a path to mint from; the only
//!   callers of [`ChosenPaths::mint`] are dialog commands.
//! - **Session-scoped.** The map lives in memory: a restart forgets every
//!   token, so a token can never outlive the operator's own sense of «I picked
//!   that folder just now». Nothing is persisted.
//! - **Bounded** ([`CHOSEN_PATHS_MAX`]), oldest evicted first — like
//!   `DeliveredExports`, so a process that runs for weeks cannot grow it
//!   without limit. Picking the same place again reuses its token and makes it
//!   the newest.
//! - **Typed.** A token is minted for a [`ChosenKind`]; a folder token never
//!   resolves where a file is asked for, or the other way round.
//! - **Re-validated at use** ([`revalidate`]): the place must still exist,
//!   still be that kind, still canonicalise to the very place that was picked
//!   (a folder swapped for a symlink since then is not that folder), and still
//!   pass `path_guard` — which is defence in depth, no longer the only thing
//!   between the webview and the file system.
//!
//! Errors are [`ChosenError`]s, which carry no path: each caller maps them to
//! its own error code and sentence (`commands::editor` → `export_folder_*`).
//!
//! ## The display name
//!
//! The webview still needs to SHOW where the export will land. It gets the
//! place's last component only ([`display_name`]) — «Skrivebord», «USB-PINNE»
//! — which is what the export page already showed (`folderLabel`), and never
//! the full path, which it has no use for any more.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::oneshot;
use ts_rs::TS;

use super::path_guard::{self, compare_key, strip_verbatim};
use crate::error::{AppError, AppResult};

/// How many picked places one session remembers. A person picks an export
/// folder a handful of times in a session; the bound exists so the map cannot
/// grow in a process that runs for weeks. An evicted token answers
/// [`ChosenError::Unknown`], and the page asks for the folder again.
pub const CHOSEN_PATHS_MAX: usize = 64;

/// What kind of place a token stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChosenKind {
    /// A folder something will be written INTO (the editor's export folder).
    Folder,
    /// An existing file something will be read FROM.
    File,
}

/// Why a picked place cannot be used. Deliberately pathless: the caller turns
/// it into its own code and sentence, and nothing here can carry the place
/// into a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChosenError {
    /// No such token in this session — made up, from an earlier session,
    /// evicted, or minted for the other [`ChosenKind`].
    Unknown,
    /// The place is no longer there, no longer that kind, or no longer the
    /// place that was picked (it now canonicalises somewhere else).
    Gone,
    /// The place fails `path_guard` (a protected folder), or its name is not
    /// valid UTF-8 and so cannot be judged — or handed to ffmpeg — at all.
    Refused,
}

/// What a dialog command hands the webview: the token, and the name to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "ChosenPlace.ts")]
#[serde(rename_all = "camelCase")]
pub struct ChosenPlace {
    /// Opaque, random, session-scoped. Hand it back; never parse it.
    pub token: String,
    /// The place's last component — what the page shows («Skrivebord»).
    pub display_name: String,
}

#[derive(Debug)]
struct Chosen {
    token: String,
    kind: ChosenKind,
    /// Canonical when minted.
    place: PathBuf,
}

/// The session's picked places, by token. Managed state (`lib.rs`).
#[derive(Debug, Default)]
pub struct ChosenPaths {
    chosen: Mutex<Vec<Chosen>>,
}

impl ChosenPaths {
    pub fn new() -> Self {
        Self::default()
    }

    /// The token for `place`, a CANONICAL path that has just passed [`vet`]
    /// as `kind`. The same place and kind again gets the same token back (and
    /// becomes the newest entry), so picking one folder ten times costs one
    /// slot.
    ///
    /// The token is a v4 UUID: 122 random bits, nothing derived from the path
    /// or the clock, so it says nothing about the place and cannot be guessed
    /// from a neighbouring one.
    pub fn mint(&self, kind: ChosenKind, place: PathBuf) -> String {
        let key = compare_key(&place);
        let mut chosen = self.chosen.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = chosen
            .iter()
            .position(|c| c.kind == kind && compare_key(&c.place) == key)
        {
            let again = chosen.remove(i);
            let token = again.token.clone();
            chosen.push(again);
            return token;
        }
        if chosen.len() >= CHOSEN_PATHS_MAX {
            chosen.remove(0);
        }
        let token = uuid::Uuid::new_v4().to_string();
        chosen.push(Chosen {
            token: token.clone(),
            kind,
            place,
        });
        token
    }

    /// The place `token` was minted for, as it was minted — no file system
    /// access, so it is safe on the async runtime. A token of the other kind
    /// is as unknown as a made-up one. Use [`revalidate`] before acting on it.
    pub fn lookup(&self, token: &str, kind: ChosenKind) -> Result<PathBuf, ChosenError> {
        self.chosen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|c| c.kind == kind && c.token == token)
            .map(|c| c.place.clone())
            .ok_or(ChosenError::Unknown)
    }
}

/// Canonicalise and judge a place a dialog just answered, as `kind`: it must
/// exist, be that kind, have a UTF-8 name, and pass `path_guard` (a folder as
/// [`path_guard::checked_path`], a file as [`path_guard::checked_input_file`]).
/// Returns the canonical path — what [`ChosenPaths::mint`] stores.
///
/// BLOCKING: run it through `crate::util::off_runtime`. The place may be a
/// USB stick or a network share that is slow to answer.
pub fn vet(place: &Path, kind: ChosenKind) -> Result<PathBuf, ChosenError> {
    let canonical = place.canonicalize().map_err(|_| ChosenError::Gone)?;
    let is_kind = match kind {
        ChosenKind::Folder => canonical.is_dir(),
        ChosenKind::File => canonical.is_file(),
    };
    if !is_kind {
        return Err(ChosenError::Gone);
    }
    let plain = plain_string(&canonical).ok_or(ChosenError::Refused)?;
    let guarded = match kind {
        ChosenKind::Folder => path_guard::checked_path(&plain),
        ChosenKind::File => path_guard::checked_input_file(&plain),
    };
    guarded.map_err(|_| ChosenError::Refused)?;
    Ok(canonical)
}

/// [`vet`] again, at the moment a token's place is about to be USED — and the
/// place must still be the one that was picked: canonicalising it now must
/// land on the same path it was minted as. A folder that was deleted and
/// recreated under the same name passes (it is the place the operator chose);
/// one replaced by a symlink to somewhere else does not.
///
/// BLOCKING, like [`vet`].
pub fn revalidate(minted: &Path, kind: ChosenKind) -> Result<PathBuf, ChosenError> {
    let now = vet(minted, kind)?;
    if compare_key(&now) != compare_key(minted) {
        return Err(ChosenError::Gone);
    }
    Ok(now)
}

/// A canonical path as the plain string the seams and ffmpeg take: valid
/// UTF-8 or nothing, with Windows' verbatim prefix folded away (`\\?\C:\x` →
/// `C:\x`) — the spelling the dialog itself answered with before, and the one
/// ffmpeg and the file manager expect.
pub fn plain_string(canonical: &Path) -> Option<String> {
    canonical.to_str().map(strip_verbatim)
}

/// The name the page shows for a place: its last component («Skrivebord») —
/// the answer `folderLabel` in `app/editor/export-core.ts` gave for every
/// folder when the page still held the path. A root has no last component, so
/// it is shown as itself without the trailing separator («E:»), and `/` as
/// `/` (where `folderLabel` showed nothing at all).
pub fn display_name(plain: &str) -> String {
    let trimmed = plain.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(last) if !last.is_empty() => last.to_string(),
        _ if trimmed.is_empty() => plain.to_string(),
        _ => trimmed.to_string(),
    }
}

// ── The dialog ──────────────────────────────────────────────────────────────

/// Ask for a folder: the native folder picker over `window`, where the
/// operator may also create a new folder — asked for explicitly, where the JS
/// picker left it to the platform. `None` is a cancel.
///
/// Parented on macOS and Windows only — exactly what the dialog plugin's own
/// `open` command does, which is what this picker was until now. The callback
/// form, awaited, and not `blocking_pick_folder`: the plugin hands the dialog
/// to the main thread either way, and the blocking twin would park a runtime
/// worker for as long as the operator looks for the USB stick.
///
/// Unlike the plugin's `open` command, this does NOT widen the webview's
/// `asset://` scope to the picked folder (the plugin adds it recursively). The
/// export page never loads anything from it; the editor widens the scope one
/// file at a time through `editor_allow_asset_path`.
pub async fn ask_for_folder(window: &tauri::Window) -> AppResult<Option<PathBuf>> {
    let (tx, rx) = oneshot::channel();
    let dialog = window.dialog().file().set_can_create_directories(true);
    #[cfg(any(windows, target_os = "macos"))]
    let dialog = dialog.set_parent(window);
    dialog.pick_folder(move |answer| {
        // The receiver is gone only if the command itself was dropped; then
        // there is nobody left to tell.
        let _ = tx.send(answer);
    });
    dialog_answer(rx.await)
}

/// What a dialog's answer means: `None` is a cancel, a path is the operator's
/// pick — and a dialog that went away WITHOUT answering is an error
/// (`dialog_failed`). The plugin drops its callback when it cannot reach the
/// main thread (the app is quitting); a silent «cancelled» would make a pick
/// that never happened look like one nobody asked for.
pub fn dialog_answer(
    answer: Result<Option<FilePath>, oneshot::error::RecvError>,
) -> AppResult<Option<PathBuf>> {
    match answer {
        Ok(None) => Ok(None),
        Ok(Some(file)) => file.simplified().into_path().map(Some).map_err(|_| {
            AppError::Internal(
                "dialog_failed: the dialog answered with something that is not a local path".into(),
            )
        }),
        Err(_) => Err(AppError::Internal(
            "dialog_failed: the dialog closed without answering".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical temp folder of its own (macOS' `/var` is `/private/var`).
    fn folder(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn a_minted_token_resolves_to_its_place_and_only_as_its_kind() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();

        let token = store.mint(ChosenKind::Folder, place.clone());

        assert_eq!(store.lookup(&token, ChosenKind::Folder), Ok(place));
        assert_eq!(
            store.lookup(&token, ChosenKind::File),
            Err(ChosenError::Unknown),
            "a folder token is not a file token"
        );
    }

    #[test]
    fn a_made_up_token_resolves_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let real = store.mint(ChosenKind::Folder, folder(dir.path(), "Eksport"));
        let forged: Vec<String> = vec![
            String::new(),
            " ".into(),
            "00000000-0000-0000-0000-000000000000".into(),
            uuid::Uuid::new_v4().to_string(),
            real.to_uppercase(),
            format!("{real} "),
            real[..real.len() - 1].to_string(),
            // The old wire value: a path where the token goes.
            dir.path().join("Eksport").to_string_lossy().into_owned(),
        ];
        for forged in &forged {
            assert_eq!(
                store.lookup(forged, ChosenKind::Folder),
                Err(ChosenError::Unknown),
                "{forged:?} must not resolve"
            );
        }
    }

    #[test]
    fn tokens_are_random_and_say_nothing_about_the_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let a = store.mint(ChosenKind::Folder, folder(dir.path(), "Skrivebord"));
        let b = store.mint(ChosenKind::Folder, folder(dir.path(), "USB"));
        assert_ne!(a, b);
        for token in [&a, &b] {
            assert_eq!(
                uuid::Uuid::parse_str(token).map(|u| u.get_version_num()),
                Ok(4),
                "{token} is a random v4 UUID"
            );
            assert!(!token.contains("Skrivebord") && !token.contains("USB"));
        }
    }

    #[test]
    fn the_same_place_again_reuses_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let first = store.mint(ChosenKind::Folder, place.clone());
        let again = store.mint(ChosenKind::Folder, place);
        assert_eq!(first, again);
        assert_eq!(store.chosen.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_store_is_bounded_and_evicts_the_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let first = store.mint(ChosenKind::Folder, folder(dir.path(), "f0"));
        let second = store.mint(ChosenKind::Folder, folder(dir.path(), "f1"));
        // Re-picking the first makes it the NEWEST, so `second` goes first.
        store.mint(ChosenKind::Folder, folder(dir.path(), "f0"));
        for i in 2..=CHOSEN_PATHS_MAX {
            store.mint(ChosenKind::Folder, folder(dir.path(), &format!("f{i}")));
        }
        assert_eq!(store.chosen.lock().unwrap().len(), CHOSEN_PATHS_MAX);
        assert_eq!(
            store.lookup(&second, ChosenKind::Folder),
            Err(ChosenError::Unknown),
            "the least recently picked place was evicted"
        );
        assert!(store.lookup(&first, ChosenKind::Folder).is_ok());
    }

    #[test]
    fn revalidation_refuses_a_place_deleted_since_the_pick() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "USB");
        assert_eq!(revalidate(&place, ChosenKind::Folder), Ok(place.clone()));

        std::fs::remove_dir(&place).unwrap();
        assert_eq!(
            revalidate(&place, ChosenKind::Folder),
            Err(ChosenError::Gone)
        );

        // A FILE where the folder was is not the folder either.
        std::fs::write(&place, b"x").unwrap();
        assert_eq!(
            revalidate(&place, ChosenKind::Folder),
            Err(ChosenError::Gone)
        );
    }

    #[test]
    fn revalidation_accepts_a_folder_recreated_under_the_same_name() {
        // The operator picked the PLACE; a folder deleted and made again at
        // the same path is still it.
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        std::fs::remove_dir(&place).unwrap();
        std::fs::create_dir(&place).unwrap();
        assert_eq!(revalidate(&place, ChosenKind::Folder), Ok(place));
    }

    #[cfg(unix)]
    #[test]
    fn revalidation_refuses_a_folder_swapped_for_a_symlink_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let elsewhere = folder(dir.path(), "Et annet sted");
        std::fs::remove_dir(&place).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &place).unwrap();
        assert_eq!(
            revalidate(&place, ChosenKind::Folder),
            Err(ChosenError::Gone),
            "the token stands for the folder that was picked, not for whatever \
             its name points at now"
        );
    }

    #[test]
    fn the_vet_holds_a_picked_place_to_its_kind_and_to_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let file = place.join("opptak.mp3");
        std::fs::write(&file, b"x").unwrap();

        assert_eq!(vet(&place, ChosenKind::Folder), Ok(place.clone()));
        assert_eq!(
            vet(&file, ChosenKind::File),
            Ok(file.canonicalize().unwrap())
        );
        assert_eq!(vet(&file, ChosenKind::Folder), Err(ChosenError::Gone));
        assert_eq!(vet(&place, ChosenKind::File), Err(ChosenError::Gone));
        assert_eq!(
            vet(&dir.path().join("finnes-ikke"), ChosenKind::Folder),
            Err(ChosenError::Gone)
        );
    }

    #[test]
    fn the_vet_refuses_a_protected_folder() {
        // `path_guard` reads the protected folders from HOME, and a test must
        // neither change HOME (other tests read it) nor create `~/.ssh`. So
        // this judges whichever protected folders really exist on the machine
        // running it — on a developer's Mac, `~/.ssh` at least. The deny list
        // itself is `path_guard`'s to test; what is pinned here is that its
        // refusal comes out as `Refused` and is never minted.
        let Some(home) = path_guard::home_dir() else {
            return;
        };
        for sub in path_guard::SENSITIVE_HOME_SUBPATHS {
            let protected = home.join(sub);
            if protected.is_dir() {
                assert_eq!(
                    vet(&protected, ChosenKind::Folder),
                    Err(ChosenError::Refused),
                    "~/{sub}"
                );
            }
        }
    }

    #[test]
    fn the_display_name_is_the_last_component_only() {
        assert_eq!(display_name("/Users/kari/Desktop"), "Desktop");
        assert_eq!(display_name("/Volumes/USB-PINNE/"), "USB-PINNE");
        assert_eq!(display_name(r"C:\Users\kari\Skrivebord"), "Skrivebord");
        assert_eq!(display_name(r"\\server\share\Opptak"), "Opptak");
        assert_eq!(display_name(r"E:\"), "E:");
        assert_eq!(display_name("/"), "/");
        // …and never anything above it.
        assert!(!display_name("/Users/kari/Desktop").contains("kari"));
    }

    #[test]
    fn the_plain_string_folds_the_windows_verbatim_prefix() {
        assert_eq!(
            plain_string(Path::new(r"\\?\C:\Users\kari")).as_deref(),
            Some(r"C:\Users\kari")
        );
        assert_eq!(
            plain_string(Path::new("/Users/kari")).as_deref(),
            Some("/Users/kari")
        );
    }

    #[test]
    fn a_dialog_answer_is_a_pick_a_cancel_or_an_error() {
        assert_eq!(dialog_answer(Ok(None)).unwrap(), None, "cancel");
        let pick = std::env::temp_dir().join("Eksport");
        assert_eq!(
            dialog_answer(Ok(Some(FilePath::Path(pick.clone())))).unwrap(),
            Some(pick),
            "a pick is the path, as picked"
        );
        // A dialog that went away without answering is NOT a quiet cancel.
        let (tx, rx) = oneshot::channel::<Option<FilePath>>();
        drop(tx);
        match dialog_answer(rx.blocking_recv()) {
            Err(AppError::Internal(msg)) => assert!(msg.starts_with("dialog_failed"), "{msg}"),
            other => panic!("expected dialog_failed, got {other:?}"),
        }
    }
}
