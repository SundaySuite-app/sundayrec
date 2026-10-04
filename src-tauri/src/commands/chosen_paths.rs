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
//! The same holds for a file the editor READS: the recording, and the jingles.
//! `editor_open_recording` opens the file picker FROM RUST, `editor_open_known`
//! names a history row (the database decides the path), and a file dropped on
//! the window is caught by the process itself (`commands::editor::note_drop`).
//! All three mint a File token; every editor command that works on a recording
//! takes that token and nothing else.
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
//!   callers of [`ChosenPaths::mint`] are dialog commands — and `mint` takes a
//!   [`Vetted`], a value only [`vet`] can make, so a path that was not vetted
//!   is not even a type `mint` accepts.
//! - **Session-scoped.** The map lives in memory: a restart forgets every
//!   token, so a token can never outlive the operator's own sense of «I picked
//!   that folder just now». Nothing is persisted.
//! - **Bounded** ([`CHOSEN_PATHS_MAX`]), oldest evicted first — like
//!   `DeliveredExports`, so a process that runs for weeks cannot grow it
//!   without limit. Picking the same place again reuses its token and makes it
//!   the newest — «the same place» being the exact canonical path, not a
//!   case-folded spelling of it (two folders that differ only in case are two
//!   folders on a case-sensitive disk).
//! - **Typed.** A token is minted for a [`ChosenKind`]; a folder token never
//!   resolves where a file is asked for, or the other way round.
//! - **Resolved in one step** ([`ChosenPaths::resolve`]): the lookup and the
//!   re-validation are one call, and the raw lookup is private — there is no
//!   way to get a place back from a token without it being checked again.
//! - **Re-validated at use**: the place must still exist,
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
use std::sync::{Arc, Mutex, PoisonError};

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

/// A place that has passed [`vet`] — and the only thing [`ChosenPaths::mint`]
/// accepts.
///
/// The fields are private and [`vet_for_home`] is the one place that builds
/// one, so «this path was judged» is carried by the TYPE: a caller that holds a
/// `PathBuf` it never vetted cannot turn it into a token, however it got
/// there. Not `Clone`: `mint` consumes it, one vet → one token.
#[derive(Debug, PartialEq, Eq)]
pub struct Vetted {
    kind: ChosenKind,
    /// Canonical, as [`Path::canonicalize`] answered.
    place: PathBuf,
}

impl Vetted {
    /// The canonical place.
    pub fn place(&self) -> &Path {
        &self.place
    }

    /// What it was vetted as.
    pub fn kind(&self) -> ChosenKind {
        self.kind
    }
}

#[derive(Debug)]
struct Chosen {
    token: String,
    kind: ChosenKind,
    /// Canonical when minted.
    place: PathBuf,
}

/// The session's picked places, by token. Managed state (`lib.rs`).
///
/// Cheap to clone — the clones share one store — so a command can move a
/// handle into [`crate::util::off_runtime`] for [`ChosenPaths::resolve`], which
/// blocks.
#[derive(Debug, Default, Clone)]
pub struct ChosenPaths {
    chosen: Arc<Mutex<Vec<Chosen>>>,
}

impl ChosenPaths {
    pub fn new() -> Self {
        Self::default()
    }

    /// The token for a place that has just passed [`vet`], consuming the
    /// proof. The same canonical place and kind again gets the same token back
    /// (and becomes the newest entry), so picking one folder ten times costs
    /// one slot.
    ///
    /// «The same place» is EXACT: the canonical paths are equal. Not
    /// `compare_key`'s case-folded form — that is right for asking «is this
    /// inside a protected folder» (over-matching refuses more), and wrong
    /// here, where on a case-sensitive disk `Eksport` and `eksport` are two
    /// folders and sharing a token would send an export into the wrong one.
    /// Canonicalising already gave both spellings of ONE folder the same
    /// path on a case-insensitive disk.
    ///
    /// The token is a v4 UUID: 122 random bits, nothing derived from the path
    /// or the clock, so it says nothing about the place and cannot be guessed
    /// from a neighbouring one.
    pub fn mint(&self, vetted: Vetted) -> String {
        let Vetted { kind, place } = vetted;
        let mut chosen = self.chosen.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = chosen
            .iter()
            .position(|c| c.kind == kind && c.place == place)
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

    /// The place `token` stands for, as `kind`, checked again NOW: the lookup
    /// and [`revalidate`] in one step, so an unchecked place cannot leave the
    /// store. A token of the other kind is as unknown as a made-up one — a
    /// File token is never a Folder, nor the reverse.
    ///
    /// BLOCKING (the place may be a USB stick or a share): run it through
    /// `crate::util::off_runtime` with a clone of the store.
    pub fn resolve(&self, token: &str, kind: ChosenKind) -> Result<PathBuf, ChosenError> {
        self.resolve_for_home(token, kind, path_guard::home_dir().as_deref())
    }

    /// [`ChosenPaths::resolve`] with the home folder passed in — the seam
    /// [`vet_for_home`] is.
    pub(crate) fn resolve_for_home(
        &self,
        token: &str,
        kind: ChosenKind,
        home: Option<&Path>,
    ) -> Result<PathBuf, ChosenError> {
        let minted = self.lookup(token, kind)?;
        revalidate(&minted, kind, home)
    }

    /// The place `token` was minted for, as it was minted — NOT checked. Private
    /// on purpose: [`ChosenPaths::resolve`] is the way out.
    fn lookup(&self, token: &str, kind: ChosenKind) -> Result<PathBuf, ChosenError> {
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
/// Returns the [`Vetted`] place — what [`ChosenPaths::mint`] takes, and the
/// only way to get one.
///
/// BLOCKING: run it through `crate::util::off_runtime`. The place may be a
/// USB stick or a network share that is slow to answer.
pub fn vet(place: &Path, kind: ChosenKind) -> Result<Vetted, ChosenError> {
    vet_for_home(place, kind, path_guard::home_dir().as_deref())
}

/// [`vet`] with the home folder passed in, so the tests can give it a home with
/// protected folders in it instead of judging whatever the machine running
/// them has (the guard reads its deny list from HOME, which a test must not
/// change — other tests read it). This is where a [`Vetted`] is made.
pub(crate) fn vet_for_home(
    place: &Path,
    kind: ChosenKind,
    home: Option<&Path>,
) -> Result<Vetted, ChosenError> {
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
        ChosenKind::Folder => path_guard::checked_path_for_home(&plain, home),
        ChosenKind::File => path_guard::checked_input_file_for_home(&plain, home),
    };
    guarded.map_err(|_| ChosenError::Refused)?;
    Ok(Vetted {
        kind,
        place: canonical,
    })
}

/// [`vet`] again, at the moment a token's place is about to be USED — and the
/// place must still be the one that was picked: canonicalising it now must
/// land on the same path it was minted as. A folder that was deleted and
/// recreated under the same name passes (it is the place the operator chose);
/// one replaced by a symlink to somewhere else does not.
///
/// Private: [`ChosenPaths::resolve`] is its only caller, so a token can only
/// give its place back through the check.
fn revalidate(
    minted: &Path,
    kind: ChosenKind,
    home: Option<&Path>,
) -> Result<PathBuf, ChosenError> {
    let now = vet_for_home(minted, kind, home)?;
    if compare_key(now.place()) != compare_key(minted) {
        return Err(ChosenError::Gone);
    }
    Ok(now.place)
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
/// file at a time, for the recording it just opened (`commands::editor`).
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
    dialog_answer(rx.await, "dialog_failed")
}

/// Ask for ONE existing file: the native open dialog over `window`, behind the
/// given `(name, extensions)` filters — the first is the one shown first. `None`
/// is a cancel.
///
/// Parented and awaited exactly like [`ask_for_folder`] (see there), and its
/// dialog failure carries the same `dialog_failed` code.
pub async fn ask_for_file(
    window: &tauri::Window,
    filters: &[(&str, &[&str])],
) -> AppResult<Option<PathBuf>> {
    let (tx, rx) = oneshot::channel();
    let mut dialog = window.dialog().file();
    #[cfg(any(windows, target_os = "macos"))]
    {
        dialog = dialog.set_parent(window);
    }
    for (name, extensions) in filters {
        dialog = dialog.add_filter(*name, extensions);
    }
    dialog.pick_file(move |answer| {
        let _ = tx.send(answer);
    });
    dialog_answer(rx.await, "dialog_failed")
}

/// What a dialog's answer means: `None` is a cancel, a path is the operator's
/// pick — and a dialog that went away WITHOUT answering is an error, with
/// `code` as its leading code (`dialog_failed` for the export folder,
/// `profile_dialog_failed` for the settings profile — each page maps its own).
/// The plugin drops its callback when it cannot reach the main thread (the app
/// is quitting); a silent «cancelled» would make a pick that never happened
/// look like one nobody asked for.
///
/// Shared by every dialog Rust opens (this module's folder picker and
/// `commands::settings`' save/open), so the three answers mean the same thing
/// everywhere instead of in two copies that can drift.
pub fn dialog_answer(
    answer: Result<Option<FilePath>, oneshot::error::RecvError>,
    code: &str,
) -> AppResult<Option<PathBuf>> {
    match answer {
        Ok(None) => Ok(None),
        Ok(Some(file)) => file.simplified().into_path().map(Some).map_err(|_| {
            AppError::Internal(format!(
                "{code}: the dialog answered with something that is not a local path"
            ))
        }),
        Err(_) => Err(AppError::Internal(format!(
            "{code}: the dialog closed without answering"
        ))),
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

    /// `vet` with no home folder: the checks that are not the guard's, on any
    /// machine. The guard's own refusals are played with a home below.
    fn vet_plain(place: &Path, kind: ChosenKind) -> Result<Vetted, ChosenError> {
        vet_for_home(place, kind, None)
    }

    /// Vet `place` and mint a token for it.
    fn mint_for(store: &ChosenPaths, place: &Path, kind: ChosenKind) -> String {
        store.mint(vet_plain(place, kind).expect("the place vets"))
    }

    /// `resolve` with no home folder, like [`vet_plain`].
    fn resolve_plain(
        store: &ChosenPaths,
        token: &str,
        kind: ChosenKind,
    ) -> Result<PathBuf, ChosenError> {
        store.resolve_for_home(token, kind, None)
    }

    #[test]
    fn a_minted_token_resolves_to_its_place_and_only_as_its_kind() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let file = place.join("opptak.mp3");
        std::fs::write(&file, b"x").unwrap();
        let store = ChosenPaths::new();

        let folder_token = mint_for(&store, &place, ChosenKind::Folder);
        let file_token = mint_for(&store, &file, ChosenKind::File);

        assert_eq!(
            resolve_plain(&store, &folder_token, ChosenKind::Folder),
            Ok(place)
        );
        assert_eq!(
            resolve_plain(&store, &file_token, ChosenKind::File),
            Ok(file.canonicalize().unwrap())
        );
        // M4: the kind is part of the answer, both ways round.
        assert_eq!(
            resolve_plain(&store, &folder_token, ChosenKind::File),
            Err(ChosenError::Unknown),
            "a folder token is not a file token"
        );
        assert_eq!(
            resolve_plain(&store, &file_token, ChosenKind::Folder),
            Err(ChosenError::Unknown),
            "a file token is not a folder token"
        );
    }

    #[test]
    fn a_made_up_token_resolves_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let real = mint_for(&store, &folder(dir.path(), "Eksport"), ChosenKind::Folder);
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
                resolve_plain(&store, forged, ChosenKind::Folder),
                Err(ChosenError::Unknown),
                "{forged:?} must not resolve"
            );
        }
    }

    #[test]
    fn tokens_are_random_and_say_nothing_about_the_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let a = mint_for(
            &store,
            &folder(dir.path(), "Skrivebord"),
            ChosenKind::Folder,
        );
        let b = mint_for(&store, &folder(dir.path(), "USB"), ChosenKind::Folder);
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
        // M5: exact on the canonical path — one folder picked twice is one
        // slot, one token.
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let first = mint_for(&store, &place, ChosenKind::Folder);
        let again = mint_for(&store, &place, ChosenKind::Folder);
        assert_eq!(first, again);
        assert_eq!(store.chosen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_different_spelling_of_a_place_is_not_the_same_place() {
        // `Eksport` and `eksport` are two folders on a case-sensitive disk, so
        // the de-dup is on the exact canonical path and never on `compare_key`'s
        // case-folded form: sharing a token would send an export into the
        // wrong one. (`mint` never touches the disk, so this holds on every
        // file system; on a case-insensitive one `canonicalize` has already
        // given both spellings of ONE folder one path.)
        let store = ChosenPaths::new();
        let upper = Vetted {
            kind: ChosenKind::Folder,
            place: PathBuf::from("/Volumes/USB/Eksport"),
        };
        let lower = Vetted {
            kind: ChosenKind::Folder,
            place: PathBuf::from("/Volumes/USB/eksport"),
        };
        assert_ne!(store.mint(upper), store.mint(lower));
        assert_eq!(store.chosen.lock().unwrap().len(), 2);
    }

    #[test]
    fn the_store_is_bounded_and_evicts_the_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let place = |n: &str| folder(dir.path(), n);
        let first = mint_for(&store, &place("f0"), ChosenKind::Folder);
        let second = mint_for(&store, &place("f1"), ChosenKind::Folder);
        // Re-picking the first makes it the NEWEST, so `second` goes first.
        mint_for(&store, &place("f0"), ChosenKind::Folder);
        for i in 2..=CHOSEN_PATHS_MAX {
            mint_for(&store, &place(&format!("f{i}")), ChosenKind::Folder);
        }
        assert_eq!(store.chosen.lock().unwrap().len(), CHOSEN_PATHS_MAX);
        assert_eq!(
            resolve_plain(&store, &second, ChosenKind::Folder),
            Err(ChosenError::Unknown),
            "the least recently picked place was evicted"
        );
        assert!(resolve_plain(&store, &first, ChosenKind::Folder).is_ok());
    }

    #[test]
    fn resolving_refuses_a_place_deleted_since_the_pick() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "USB");
        let store = ChosenPaths::new();
        let token = mint_for(&store, &place, ChosenKind::Folder);
        assert_eq!(
            resolve_plain(&store, &token, ChosenKind::Folder),
            Ok(place.clone())
        );

        std::fs::remove_dir(&place).unwrap();
        assert_eq!(
            resolve_plain(&store, &token, ChosenKind::Folder),
            Err(ChosenError::Gone)
        );

        // A FILE where the folder was is not the folder either.
        std::fs::write(&place, b"x").unwrap();
        assert_eq!(
            resolve_plain(&store, &token, ChosenKind::Folder),
            Err(ChosenError::Gone)
        );
    }

    #[test]
    fn resolving_accepts_a_folder_recreated_under_the_same_name() {
        // The operator picked the PLACE; a folder deleted and made again at
        // the same path is still it.
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let token = mint_for(&store, &place, ChosenKind::Folder);
        std::fs::remove_dir(&place).unwrap();
        std::fs::create_dir(&place).unwrap();
        assert_eq!(resolve_plain(&store, &token, ChosenKind::Folder), Ok(place));
    }

    #[cfg(unix)]
    #[test]
    fn resolving_refuses_a_folder_swapped_for_a_symlink_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let elsewhere = folder(dir.path(), "Et annet sted");
        let store = ChosenPaths::new();
        let token = mint_for(&store, &place, ChosenKind::Folder);
        std::fs::remove_dir(&place).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &place).unwrap();
        assert_eq!(
            resolve_plain(&store, &token, ChosenKind::Folder),
            Err(ChosenError::Gone),
            "the token stands for the folder that was picked, not for whatever \
             its name points at now"
        );
    }

    #[test]
    fn the_vet_holds_a_picked_place_to_its_kind() {
        let dir = tempfile::tempdir().unwrap();
        let place = folder(dir.path(), "Eksport");
        let file = place.join("opptak.mp3");
        std::fs::write(&file, b"x").unwrap();

        let vetted = vet_plain(&place, ChosenKind::Folder).unwrap();
        assert_eq!(
            (vetted.place(), vetted.kind()),
            (&*place, ChosenKind::Folder)
        );
        let vetted = vet_plain(&file, ChosenKind::File).unwrap();
        assert_eq!(vetted.place(), file.canonicalize().unwrap());
        assert_eq!(vet_plain(&file, ChosenKind::Folder), Err(ChosenError::Gone));
        assert_eq!(vet_plain(&place, ChosenKind::File), Err(ChosenError::Gone));
        assert_eq!(
            vet_plain(&dir.path().join("finnes-ikke"), ChosenKind::Folder),
            Err(ChosenError::Gone)
        );
    }

    /// A home of its own with the protected folders in it — what the guard
    /// reads from HOME, given to the vet as an argument instead.
    fn home_with_protected_folders(dir: &Path) -> PathBuf {
        let home = folder(dir, "hjem");
        for sub in path_guard::SENSITIVE_HOME_SUBPATHS {
            let protected = home.join(sub);
            std::fs::create_dir_all(&protected).unwrap();
            std::fs::write(protected.join("hemmelig.txt"), b"x").unwrap();
        }
        home
    }

    #[test]
    fn the_vet_refuses_a_protected_folder_and_a_file_in_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = home_with_protected_folders(dir.path());
        std::fs::create_dir_all(home.join("Dokumenter")).unwrap();

        for sub in path_guard::SENSITIVE_HOME_SUBPATHS {
            let protected = home.join(sub);
            assert_eq!(
                vet_for_home(&protected, ChosenKind::Folder, Some(&home)),
                Err(ChosenError::Refused),
                "~/{sub}"
            );
            assert_eq!(
                vet_for_home(
                    &protected.join("hemmelig.txt"),
                    ChosenKind::File,
                    Some(&home)
                ),
                Err(ChosenError::Refused),
                "a file inside ~/{sub}"
            );
            // It is the HOME that decides: the same folder with no home to
            // protect is an ordinary folder.
            assert!(vet_for_home(&protected, ChosenKind::Folder, None).is_ok());
        }
        assert!(vet_for_home(&home.join("Dokumenter"), ChosenKind::Folder, Some(&home)).is_ok());
    }

    #[test]
    fn resolving_applies_the_guard_again() {
        // A place that vetted when it was picked and is protected NOW (the
        // guard's list grew, or a protected folder was made there since) is
        // refused when the token is used.
        let dir = tempfile::tempdir().unwrap();
        let home = home_with_protected_folders(dir.path());
        let ssh = home.join(".ssh");
        let store = ChosenPaths::new();
        let token = mint_for(&store, &ssh, ChosenKind::Folder);
        assert_eq!(
            store.resolve_for_home(&token, ChosenKind::Folder, Some(&home)),
            Err(ChosenError::Refused)
        );
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
        assert_eq!(dialog_answer(Ok(None), "dialog_failed").unwrap(), None);
        let pick = std::env::temp_dir().join("Eksport");
        assert_eq!(
            dialog_answer(Ok(Some(FilePath::Path(pick.clone()))), "dialog_failed").unwrap(),
            Some(pick),
            "a pick is the path, as picked"
        );
        // A dialog that went away without answering is NOT a quiet cancel —
        // and it carries the caller's own code.
        for code in ["dialog_failed", "profile_dialog_failed"] {
            let (tx, rx) = oneshot::channel::<Option<FilePath>>();
            drop(tx);
            match dialog_answer(rx.blocking_recv(), code) {
                Err(AppError::Internal(msg)) => {
                    assert!(msg.starts_with(&format!("{code}:")), "{msg}")
                }
                other => panic!("expected {code}, got {other:?}"),
            }
        }
    }
}
