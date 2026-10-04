//! Defense-in-depth validation of renderer-supplied filesystem paths.
//!
//! The editor IPC commands receive raw `input_path`/`media_path` strings from
//! the webview and hand them to ffmpeg/fs. The CSP already locks the renderer
//! down, but a compromised webview could still call these commands with any
//! path the process can read. This guard rejects the obviously hostile cases —
//! relative paths, `..` traversal, non-files, and the same sensitive
//! dot-directories the `assetProtocol` scope in `tauri.conf.json` denies —
//! before the seam touches the filesystem.
//!
//! The guards validate and return `()`; the ORIGINAL string is what flows on to
//! ffmpeg/fs, so behaviour for legitimate paths is byte-for-byte unchanged
//! (canonicalisation is only used for the checks, which also catches symlink
//! escapes into a denied directory).
//!
//! ## The policy vocabulary (E1.2)
//!
//! Every command that takes a path names WHICH of these it is, in its own
//! doc-comment, via [`PathPolicy`]:
//!
//! | policy | means | typical caller |
//! |---|---|---|
//! | [`PathPolicy::UserChosenRead`] | absolute, exists, not protected | a file the user picked in a native OPEN dialog |
//! | [`PathPolicy::UserChosenWrite`] | absolute, no `..`, not protected, target may not exist | a destination the user picked in a native SAVE dialog |
//! | [`PathPolicy::RecordingsRooted`] | must resolve INSIDE the configured save folder | anything a REMOTE party (a deep link) can name, and anything that leaves the machine |
//!
//! The tension the table resolves: settings export/import legitimately targets
//! anywhere the user pointed a native dialog at, so pinning it to the recordings
//! folder would break a real flow for no gain — the dialog IS the authorisation.
//! But only a dialog the PROCESS opened: a path the renderer passes is just a
//! claim that one was shown, and these guards judge it only against the
//! protected folders. So the settings profile no longer takes a path at all —
//! `commands::settings` opens the dialog in Rust and holds the answer to the
//! `UserChosen*` policy as defence in depth (finding A1). The commands that
//! still take a dialog's path from the renderer are listed in `SECURITY.md`.
//! A deep link, by contrast, carries no user intent at all, so it gets the
//! narrowest policy plus an explicit confirmation (see `commands::deeplink`).
//!
//! (Until v0.15 a fourth policy, `ReadOnlyMedia` — `UserChosenRead` plus an
//! extension allowlist — guarded `whisper_transcribe`, the one command that
//! decoded whatever it was pointed at. It left with its only caller; the
//! allowlist idea is in git if a future command hands a user file to another
//! process.)

use crate::error::{AppError, AppResult};
use std::path::{Component, Path, PathBuf};

/// Home-relative locations an IPC path must never resolve into. Mirrors the
/// `assetProtocol.scope.deny` list in `tauri.conf.json` — keep the two in sync.
/// Also the list `recordings_open::vet_new_save_folder` keeps the recordings
/// folder out of.
pub(crate) const SENSITIVE_HOME_SUBPATHS: &[&str] =
    &[".ssh", ".aws", ".gnupg", ".netrc", ".config/gh"];

/// The user's home directory as the environment names it (`HOME`, or
/// `USERPROFILE` on Windows), not canonicalised.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

// ── Comparing two spellings of one file ─────────────────────────────────────
//
// `canonicalize` resolves symlinks and `..`, but it does not make every
// spelling of a file the same string. Two checks therefore meet in the
// comparisons below, each covering what the other cannot:
//
// - a comparison KEY ([`compare_key`]) — string work, so it also judges paths
//   that do not exist yet (a save folder the recorder will create). It folds
//   the spellings the platforms are known to have: Windows' verbatim prefix,
//   case on the case-insensitive file systems, and macOS' firmlinks.
// - file IDENTITY ([`identity`], device + inode, unix only) — spelling-blind,
//   so it also catches an alias nobody listed (another firmlink, a bind mount),
//   but only for what EXISTS: a `~/.aws` that is not there yet has no inode.
//
// Neither alone is enough: the key cannot know every alias, and identity
// cannot judge the future. The deny list and the save-folder vet use both.

/// Whether this build's comparisons fold case. Windows' and macOS' default
/// file systems are case-insensitive; Linux' are not.
pub(crate) const FOLD_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// Fold Windows' verbatim prefixes away: `\\?\C:\x` → `C:\x` and
/// `\\?\UNC\server\share\x` → `\\server\share\x`. `std::fs::canonicalize`
/// returns the verbatim form on Windows; the file manager and a path typed by a
/// person use the plain one. Pure string work, so it is tested on every OS.
pub(crate) fn strip_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return rest.to_string();
        }
    }
    s.to_string()
}

/// The data volume's mount point on macOS 10.15+, lower-cased (it is compared
/// after the case fold). Every firmlink in `/usr/share/firmlinks` — `/Users`,
/// `/private`, `/Volumes`, `/Applications`, `/usr/local` … — joins a path on
/// the read-only system volume to the same path below this one, and
/// `canonicalize` keeps whichever spelling it was given:
/// `/System/Volumes/Data/Users/kari/.ssh` stays that, and IS `~/.ssh`.
#[cfg(target_os = "macos")]
const MACOS_DATA_VOLUME: &str = "/system/volumes/data";

/// Fold the data-volume spelling onto the firmlinked one: `/System/Volumes/Data`
/// → `/`, `/System/Volumes/Data/Users/kari` → `/Users/kari`. Applied to every
/// path below it, not only to the listed firmlinks: for a firmlinked root the
/// two ARE the same file, and for any other the system-volume twin is a sealed,
/// read-only path no recording or protected folder lives in — so the fold can
/// only ever join what is one file.
#[cfg(target_os = "macos")]
fn fold_macos_data_volume(key: String) -> String {
    match key.strip_prefix(MACOS_DATA_VOLUME) {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        _ => key,
    }
}

/// The form every path comparison in the guards is made in. `canonical` must
/// already be canonical; this adds the verbatim fold, the case fold where the
/// default file system is case-insensitive, and (macOS) the firmlink fold.
///
/// The case fold is `to_lowercase` plus one letter: `ſ` (U+017F LATIN SMALL
/// LETTER LONG S). It is already lower case, so `to_lowercase` keeps it, but
/// APFS' case folding — and NTFS' upcase table — treat it as `s`: `~/.ſsh` IS
/// `~/.ssh` there. It is the only letter whose fold lands on ASCII that
/// `to_lowercase` misses (the Kelvin sign, the other one, already lowers to
/// `k`), and ASCII is what every protected name is spelled in.
///
/// A path that is not valid UTF-8 is compared byte-for-byte rather than
/// lossily, so two different names can never fold into one key.
pub(crate) fn compare_key(canonical: &Path) -> PathBuf {
    let Some(s) = canonical.to_str() else {
        return canonical.to_path_buf();
    };
    let plain = strip_verbatim(s);
    let folded = if FOLD_CASE {
        plain.to_lowercase().replace('ſ', "s")
    } else {
        plain
    };
    #[cfg(target_os = "macos")]
    let folded = fold_macos_data_volume(folded);
    PathBuf::from(folded)
}

/// An existing path's identity — device and inode, whatever it is called.
/// `None` for a path that does not exist, and always off unix (std has no
/// stable file index on Windows; there the key carries the check alone).
#[cfg(unix)]
pub(crate) fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
pub(crate) fn identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn deny_sensitive(canonical: &Path) -> AppResult<()> {
    deny_sensitive_for_home(canonical, home_dir().as_deref())
}

/// [`deny_sensitive`] with the home folder passed in, so the tests can give it
/// one without touching the process environment other tests read.
fn deny_sensitive_for_home(canonical: &Path, home: Option<&Path>) -> AppResult<()> {
    let Some(home) = home else {
        return Ok(());
    };
    // Canonicalise home too so the prefix comparison is apples-to-apples.
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    deny_sensitive_under(canonical, &home)
}

/// Refuse a path inside one of the protected home locations — by comparison
/// key (any spelling the key folds, existing or not) or by identity (any
/// spelling at all, for the locations that exist). `canonical` may also be a
/// path whose missing tail was appended to a canonical ancestor
/// ([`resolved_with_missing_tail`]).
pub(crate) fn deny_sensitive_under(canonical: &Path, home: &Path) -> AppResult<()> {
    let key = compare_key(canonical);
    // The identities of `canonical` and every folder above it — stat'ed once,
    // and only if some protected location exists to compare them with.
    let mut ancestor_ids: Option<Vec<(u64, u64)>> = None;
    for sub in SENSITIVE_HOME_SUBPATHS {
        let protected = home.join(sub);
        let by_key = key.starts_with(compare_key(&protected));
        let by_identity = !by_key
            && identity(&protected).is_some_and(|id| {
                ancestor_ids
                    .get_or_insert_with(|| canonical.ancestors().filter_map(identity).collect())
                    .contains(&id)
            });
        if by_key || by_identity {
            return Err(AppError::Validation(format!(
                "path resolves into a protected directory (~/{sub})"
            )));
        }
    }
    Ok(())
}

/// Whether `path` is `home` or a folder above it — by key, or by identity
/// against every existing folder from `home` up to its root. `path` must be
/// canonical (or [`resolved_with_missing_tail`]); `home` canonical.
pub(crate) fn holds_home(path: &Path, home: &Path) -> bool {
    if compare_key(home).starts_with(compare_key(path)) {
        return true;
    }
    let Some(id) = identity(path) else {
        return false;
    };
    home.ancestors().any(|a| identity(a) == Some(id))
}

fn require_absolute(raw: &str) -> AppResult<&Path> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(AppError::Validation(format!(
            "path must be absolute: {raw}"
        )));
    }
    Ok(path)
}

/// Validate a renderer-supplied path that must name an existing file
/// (recordings, intro/outro clips). Canonicalises (resolving symlinks and
/// `..`) and rejects anything under a protected directory.
pub fn checked_input_file(raw: &str) -> AppResult<()> {
    checked_input_file_for_home(raw, home_dir().as_deref())
}

/// [`checked_input_file`] with the home folder passed in (see
/// [`deny_sensitive_for_home`]).
fn checked_input_file_for_home(raw: &str, home: Option<&Path>) -> AppResult<()> {
    let path = require_absolute(raw)?;
    let canonical = path
        .canonicalize()
        .map_err(|e| AppError::Validation(format!("cannot resolve path {raw}: {e}")))?;
    if !canonical.is_file() {
        return Err(AppError::Validation(format!("not a file: {raw}")));
    }
    deny_sensitive_for_home(&canonical, home)
}

/// Validate a renderer-supplied path whose target may not exist yet (sidecar
/// stems, export outputs, sweep folders). `..` components are rejected
/// outright (the non-existing tail can't be canonicalised, so traversal there
/// would otherwise go unseen); the deepest existing ancestor is canonicalised
/// and checked against the deny list.
pub fn checked_path(raw: &str) -> AppResult<()> {
    let path = require_absolute(raw)?;
    reject_traversal(path, raw)?;
    let canonical = deepest_existing_canonical(path, raw)?;
    deny_sensitive(&canonical)
}

/// Reject a path carrying `..` components. Split out so the root-scoping guard
/// can rely on the same rule: with `..` gone, a path's non-existent tail can
/// only ever go DEEPER than its deepest existing ancestor, which is what makes
/// [`checked_under_root`] sound for targets that do not exist yet.
fn reject_traversal(path: &Path, raw: &str) -> AppResult<()> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AppError::Validation(format!(
            "path must not contain '..': {raw}"
        )));
    }
    Ok(())
}

/// Canonicalise `path`, walking up to its deepest EXISTING ancestor when the
/// target itself does not exist yet. Resolving through `canonicalize` is what
/// makes a symlink escape visible: a link inside the save folder pointing at
/// `/etc/passwd` canonicalises to `/etc/passwd`, which no longer starts with
/// the save folder.
fn deepest_existing_canonical(path: &Path, raw: &str) -> AppResult<PathBuf> {
    let mut probe = path;
    loop {
        match probe.canonicalize() {
            Ok(c) => return Ok(c),
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent,
                _ => {
                    return Err(AppError::Validation(format!(
                        "cannot resolve any ancestor of path: {raw}"
                    )))
                }
            },
        }
    }
}

/// Where `path` WILL resolve once its missing tail has been created: the
/// deepest EXISTING ancestor canonicalised, with the components below it
/// appended as written. `None` when not even a root resolves (a drive or share
/// that is not there).
///
/// [`deepest_existing_canonical`] stops at the ancestor, which is enough for a
/// prefix check against a folder that exists — but not for a deny list whose
/// entries may not exist YET: with no `~/.aws`, `~/.aws/opptak` resolves only
/// as far as `~`, and the folder the recorder would then create is `~/.aws`.
/// Appending the tail is sound only for an absolute, `..`-free path (the caller
/// checks both) whose missing components are truly ABSENT: a name that does
/// not exist at all cannot redirect anywhere, so the tail can only go DEEPER
/// than the ancestor it hangs from. A component that exists but does not
/// canonicalise is not absent — a dangling symlink, or macOS' `/.vol/<dev>/<ino>`
/// spelling (which names `~/.ssh` as well as anything, and which `realpath`
/// refuses) — so it answers `None` rather than being treated as new.
pub(crate) fn resolved_with_missing_tail(path: &Path) -> Option<PathBuf> {
    let mut probe = path;
    let mut tail = Vec::new();
    loop {
        if let Ok(canonical) = probe.canonicalize() {
            let mut resolved = canonical;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            return Some(resolved);
        }
        if probe.symlink_metadata().is_ok() {
            return None;
        }
        tail.push(probe.file_name()?);
        probe = probe.parent()?;
    }
}

/// Validate a path that must resolve INSIDE `root` (the configured save
/// folder). The target itself may not exist yet — a sidecar written beside a
/// recording is the motivating case — but `..` is rejected and the deepest
/// existing ancestor must canonicalise under the canonical `root`, so neither
/// traversal nor a symlink can point the write outside.
///
/// A `root` that does not exist is a hard reject: nothing can be inside it, and
/// silently degrading to "allow" would turn a first-run misconfiguration into an
/// open door.
pub fn checked_under_root(raw: &str, root: &Path) -> AppResult<()> {
    let path = require_absolute(raw)?;
    reject_traversal(path, raw)?;
    let canonical_root = root.canonicalize().map_err(|e| {
        AppError::Validation(format!(
            "cannot resolve the save folder {}: {e}",
            root.display()
        ))
    })?;
    let canonical = deepest_existing_canonical(path, raw)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(AppError::Validation(format!(
            "path is outside the save folder ({}): {raw}",
            canonical_root.display()
        )));
    }
    deny_sensitive(&canonical)
}

/// Which rule a command's path parameter is held to. Every guarded command
/// names one in its doc-comment so the policy is reviewable without reading the
/// body — see the table in the module docs for the reasoning behind each.
#[derive(Debug, Clone, Copy)]
pub enum PathPolicy<'a> {
    /// An existing file the user picked in a native OPEN dialog.
    UserChosenRead,
    /// A destination the user picked in a native SAVE dialog (may not exist).
    UserChosenWrite,
    /// Must resolve inside the configured save folder.
    RecordingsRooted(&'a Path),
}

/// Apply a [`PathPolicy`] to `raw`.
pub fn check(raw: &str, policy: PathPolicy<'_>) -> AppResult<()> {
    match policy {
        PathPolicy::UserChosenRead => checked_input_file(raw),
        PathPolicy::UserChosenWrite => checked_path(raw),
        PathPolicy::RecordingsRooted(root) => checked_under_root(raw, root),
    }
}

/// The effective recordings root: the configured `save_folder`, or the default
/// `<Documents>/SundayRec` — via [`crate::save_folder::resolve`], the same
/// resolver the recorder itself uses, so a root-scoped guard and the recorder
/// can never disagree about where recordings live. Errs (fail CLOSED) when no
/// root can be resolved — the pre-R3 fallback was a RELATIVE `./SundayRec`,
/// which would have scoped the guard to the process working directory.
pub async fn recordings_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &crate::db::Db,
) -> crate::error::AppResult<PathBuf> {
    let settings = crate::settings::load(&db.pool).await.unwrap_or_default();
    crate::save_folder::resolve(app, settings.save_folder.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_validation(result: AppResult<()>) {
        match result {
            Err(AppError::Validation(_)) => {}
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert_validation(checked_input_file("relative/file.mp3"));
        assert_validation(checked_path("relative/file.mp3"));
    }

    #[test]
    fn missing_input_file_is_rejected() {
        assert_validation(checked_input_file("/definitely/not/a/real/file.mp3"));
    }

    #[test]
    fn directory_is_not_an_input_file() {
        let dir = std::env::temp_dir();
        assert_validation(checked_input_file(dir.to_str().unwrap()));
    }

    #[test]
    fn existing_file_passes() {
        let dir = std::env::temp_dir().join("sundayrec-path-guard-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ok.mp3");
        std::fs::write(&file, b"x").unwrap();
        checked_input_file(file.to_str().unwrap()).unwrap();
        checked_path(file.to_str().unwrap()).unwrap();
    }

    #[test]
    fn nonexistent_target_with_existing_ancestor_passes() {
        let path = std::env::temp_dir().join("sundayrec-path-guard-test/new-dir/out.mp3");
        checked_path(path.to_str().unwrap()).unwrap();
    }

    // ── comparison keys and identity ─────────────────────────────────────────

    #[test]
    fn verbatim_prefixes_fold_to_the_plain_spelling() {
        assert_eq!(strip_verbatim(r"\\?\C:\Users\a\x.mp3"), r"C:\Users\a\x.mp3");
        assert_eq!(
            strip_verbatim(r"\\?\UNC\nas\opptak\x.mp3"),
            r"\\nas\opptak\x.mp3"
        );
        // A verbatim path that is not a drive path is left alone rather than
        // guessed at (a volume GUID path has no plain spelling).
        assert_eq!(
            strip_verbatim(r"\\?\Volume{0000}\x.mp3"),
            r"\\?\Volume{0000}\x.mp3"
        );
        assert_eq!(strip_verbatim("/Users/a/x.mp3"), "/Users/a/x.mp3");
    }

    #[test]
    fn keys_fold_case_exactly_where_the_file_system_does() {
        let upper = compare_key(Path::new("/Users/A/SundayRec/X.mp3"));
        let lower = compare_key(Path::new("/users/a/sundayrec/x.mp3"));
        assert_eq!(upper == lower, FOLD_CASE);
        // `ſ` folds to `s` on APFS and NTFS; `to_lowercase` alone keeps it.
        let long_s = compare_key(Path::new("/Users/a/.ſsh"));
        let plain = compare_key(Path::new("/Users/a/.ssh"));
        assert_eq!(long_s == plain, FOLD_CASE);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_data_volume_spelling_folds_onto_the_firmlinked_one() {
        for (data, firm) in [
            ("/System/Volumes/Data/Users/kari/.ssh", "/Users/kari/.ssh"),
            ("/System/Volumes/Data/Users/kari", "/Users/kari"),
            ("/System/Volumes/Data/Users", "/Users"),
            ("/System/Volumes/Data", "/"),
            ("/SYSTEM/Volumes/data/private/var/x", "/private/var/x"),
        ] {
            assert_eq!(compare_key(Path::new(data)), compare_key(Path::new(firm)));
        }
        // A sibling that merely shares the prefix is not folded.
        assert_eq!(
            compare_key(Path::new("/System/Volumes/DataX/Users")),
            PathBuf::from("/system/volumes/datax/users")
        );
    }

    /// A fake home under the temp dir with the protected `.ssh` and a
    /// `.config/gh/config.yml`, canonicalised.
    fn fake_home_with_secrets() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("kantor");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join(".config").join("gh")).unwrap();
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        std::fs::write(home.join(".config").join("gh").join("config.yml"), b"x").unwrap();
        std::fs::write(home.join("Documents").join("preken.mp3"), b"x").unwrap();
        let home = home.canonicalize().unwrap();
        (dir, home)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_firmlink_spelling_of_a_protected_file_is_refused() {
        // The temp dir lives under `/private`, a firmlink, so the fake home has
        // a second spelling below `/System/Volumes/Data` — which
        // `canonicalize` keeps as given. Found in review of #308: the gh
        // config file got through `checked_input_file` this way.
        let (_dir, home) = fake_home_with_secrets();
        let data = |p: &Path| format!("/System/Volumes/Data{}", p.to_str().unwrap());
        let gh = home.join(".config").join("gh").join("config.yml");
        assert_eq!(
            Path::new(&data(&gh)).canonicalize().unwrap(),
            PathBuf::from(data(&gh)),
            "the premise: canonicalize keeps the data-volume spelling"
        );
        assert_validation(checked_input_file_for_home(&data(&gh), Some(&home)));
        // …and through the plain spelling, as before.
        assert_validation(checked_input_file_for_home(
            gh.to_str().unwrap(),
            Some(&home),
        ));
        // A file that is not protected passes under either spelling.
        let ok = home.join("Documents").join("preken.mp3");
        checked_input_file_for_home(&data(&ok), Some(&home)).unwrap();
        checked_input_file_for_home(ok.to_str().unwrap(), Some(&home)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn identity_catches_a_spelling_the_key_cannot_know() {
        // A hard link IS the protected file under an unrelated name — no
        // string fold can see that, its inode can.
        let (_dir, home) = fake_home_with_secrets();
        let netrc = home.join(".netrc");
        std::fs::write(&netrc, b"machine x").unwrap();
        let alias = home.join("Documents").join("innocent.txt");
        std::fs::hard_link(&netrc, &alias).unwrap();
        assert_validation(deny_sensitive_under(&alias, &home));
        assert_validation(checked_input_file_for_home(
            alias.to_str().unwrap(),
            Some(&home),
        ));
        // A different file with the same kind of name is fine.
        let other = home.join("Documents").join("other.txt");
        std::fs::write(&other, b"x").unwrap();
        deny_sensitive_under(&other, &home).unwrap();
    }

    #[test]
    fn the_home_and_its_parents_hold_the_home_and_nothing_else_does() {
        let (dir, home) = fake_home_with_secrets();
        assert!(holds_home(&home, &home));
        assert!(holds_home(home.parent().unwrap(), &home));
        assert!(holds_home(&dir.path().canonicalize().unwrap(), &home));
        assert!(!holds_home(&home.join("Documents"), &home));
        assert!(!holds_home(&home.join("Documents").join("new"), &home));
        // A sibling of the home is not above it.
        let sibling = dir.path().canonicalize().unwrap().join("kantor2");
        assert!(!holds_home(&sibling, &home));
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_name_that_does_not_resolve_is_not_a_missing_tail() {
        // A dangling symlink exists (it has metadata) but does not
        // canonicalise. Appended as if it were new, its target would be
        // judged by the LINK's name — and the recorder would create the
        // target. So it answers `None`.
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path().join("not-there-yet"), &link).unwrap();
        assert_eq!(resolved_with_missing_tail(&link.join("Opptak")), None);
        assert_eq!(resolved_with_missing_tail(&link), None);
    }

    #[test]
    fn a_missing_tail_is_appended_to_the_canonical_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("Documents");
        std::fs::create_dir_all(&existing).unwrap();
        let canonical = existing.canonicalize().unwrap();
        // Existing: the canonical path itself.
        assert_eq!(
            resolved_with_missing_tail(&existing),
            Some(canonical.clone())
        );
        // Missing: the canonical ancestor plus the tail as written — not just
        // the ancestor, which is what `deepest_existing_canonical` answers.
        let missing = existing.join("SundayRec").join("2026");
        assert_eq!(
            resolved_with_missing_tail(&missing),
            Some(canonical.join("SundayRec").join("2026"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_tail_hangs_from_where_a_symlink_points() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            resolved_with_missing_tail(&link.join("new")),
            Some(real.canonicalize().unwrap().join("new"))
        );
    }

    #[test]
    fn dotdot_is_rejected_for_lenient_paths() {
        let path = std::env::temp_dir().join("x/../secret");
        assert_validation(checked_path(path.to_str().unwrap()));
    }

    // ── E1.2: root scoping ───────────────────────────────────────────────────

    #[test]
    fn under_root_accepts_inside_and_rejects_outside() {
        let root = std::env::temp_dir().join("sundayrec-root-test/recordings");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("2026-08-06.mp3");
        std::fs::write(&inside, b"x").unwrap();
        checked_under_root(inside.to_str().unwrap(), &root).unwrap();
        // A sidecar that does not exist yet is still inside.
        let sidecar = root.join("nested/2026-08-06.transcript.json");
        checked_under_root(sidecar.to_str().unwrap(), &root).unwrap();

        // A sibling directory is not inside, even though it shares a prefix.
        let sibling = std::env::temp_dir().join("sundayrec-root-test/recordings-evil/x.mp3");
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, b"x").unwrap();
        assert_validation(checked_under_root(sibling.to_str().unwrap(), &root));
        // And neither is an absolute path somewhere else entirely.
        assert_validation(checked_under_root("/etc/passwd", &root));
    }

    #[test]
    fn under_root_rejects_traversal_and_relative_paths() {
        let root = std::env::temp_dir().join("sundayrec-root-test/recordings");
        std::fs::create_dir_all(&root).unwrap();
        let escape = format!("{}/../../etc/passwd", root.to_str().unwrap());
        assert_validation(checked_under_root(&escape, &root));
        assert_validation(checked_under_root("relative.mp3", &root));
    }

    #[cfg(unix)]
    #[test]
    fn under_root_rejects_a_symlink_that_escapes() {
        // The case canonicalisation exists for: a link INSIDE the save folder
        // whose target is not. A lexical prefix check would wave this through.
        let root = std::env::temp_dir().join("sundayrec-root-symlink/recordings");
        std::fs::create_dir_all(&root).unwrap();
        let outside = std::env::temp_dir().join("sundayrec-root-symlink/outside.mp3");
        std::fs::write(&outside, b"x").unwrap();
        let link = root.join("innocent.mp3");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_validation(checked_under_root(link.to_str().unwrap(), &root));
    }

    #[test]
    fn a_missing_save_folder_fails_closed() {
        // Degrading to "allow" on a first-run/unmounted save folder would turn a
        // misconfiguration into an open door.
        let root = std::env::temp_dir().join("sundayrec-root-that-does-not-exist");
        let _ = std::fs::remove_dir_all(&root);
        assert_validation(checked_under_root("/tmp/anything.mp3", &root));
    }

    #[test]
    fn the_policy_dispatcher_matches_its_named_guard() {
        let dir = std::env::temp_dir().join("sundayrec-policy-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("service.mp3");
        std::fs::write(&file, b"x").unwrap();
        let raw = file.to_str().unwrap();
        check(raw, PathPolicy::UserChosenRead).unwrap();
        check(raw, PathPolicy::UserChosenWrite).unwrap();
        check(raw, PathPolicy::RecordingsRooted(&dir)).unwrap();
        assert_validation(check("relative.mp3", PathPolicy::UserChosenRead));
    }

    #[test]
    fn sensitive_home_subpaths_are_denied() {
        let home = Path::new("/home/example");
        for sub in [".ssh", ".aws", ".gnupg", ".netrc", ".config/gh"] {
            let target = home.join(sub).join("leaf");
            assert_validation(deny_sensitive_under(&target, home));
        }
        // Siblings that merely share a prefix are fine.
        deny_sensitive_under(&home.join(".sshfs/mount"), home).unwrap();
        deny_sensitive_under(&home.join("Recordings/service.mp3"), home).unwrap();
    }

    // ── E1.7: SENSITIVE_HOME_SUBPATHS ↔ tauri.conf.json deny list ──────────────

    #[test]
    fn every_sensitive_home_subpath_has_a_matching_asset_scope_deny_entry() {
        // SENSITIVE_HOME_SUBPATHS (above) and tauri.conf.json's
        // app.security.assetProtocol.scope.deny are kept in sync BY HAND — this
        // is the tripwire. If it fails, add the missing entry to
        // src-tauri/tauri.conf.json's assetProtocol.scope.deny (as
        // "$HOME/<subpath>/**", or "$HOME/<subpath>" for a single file like
        // .netrc) to match SENSITIVE_HOME_SUBPATHS in this file.
        let conf_json = include_str!("../../tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(conf_json).expect("tauri.conf.json must be valid JSON");
        let deny = conf["app"]["security"]["assetProtocol"]["scope"]["deny"]
            .as_array()
            .expect("app.security.assetProtocol.scope.deny must be a JSON array");
        // Normalize each entry the same way regardless of shape: drop the
        // "$HOME/" prefix and an optional trailing "/**" glob, so
        // "$HOME/.ssh/**" and "$HOME/.netrc" both compare against the bare
        // ".ssh" / ".netrc" that SENSITIVE_HOME_SUBPATHS uses.
        let deny_normalized: Vec<&str> = deny
            .iter()
            .map(|v| {
                let s = v.as_str().expect("deny entries must be strings");
                let s = s.strip_prefix("$HOME/").unwrap_or(s);
                s.strip_suffix("/**").unwrap_or(s)
            })
            .collect();

        for sub in SENSITIVE_HOME_SUBPATHS {
            assert!(
                deny_normalized.contains(sub),
                "SENSITIVE_HOME_SUBPATHS (src-tauri/src/commands/path_guard.rs) \
                 has {sub:?} but tauri.conf.json's \
                 app.security.assetProtocol.scope.deny has no matching entry \
                 — update src-tauri/tauri.conf.json to keep the two lists in sync"
            );
        }
    }
}
