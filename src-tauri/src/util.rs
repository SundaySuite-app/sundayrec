//! Small cross-cutting helpers shared across the shell modules.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use sundayrec_core::ffmpeg::Platform;

/// The bundle identifier Tauri names the app-data directory after. Read from
/// `tauri.conf.json` at COMPILE time (`include_str!`) so the two can never drift
/// — the same trick `path_guard`'s asset-scope tripwire uses.
fn bundle_identifier() -> Option<String> {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).ok()?;
    conf.get("identifier")?.as_str().map(str::to_string)
}

/// The OS app-data directory — resolved WITHOUT a Tauri app handle.
///
/// `tauri::PathResolver::app_data_dir()` is `dirs::data_dir()?.join(identifier)`;
/// this mirrors that computation, because the two earliest observability seams
/// (the panic hook and the file-log writer) both have to be armed in `run()`,
/// before any `AppHandle` exists — and a crash between process start and
/// `setup()` is exactly the crash you most want a record of.
///
/// `setup` verifies the two agree ([`crate::crash::verify_dir_matches`]), so a
/// future change in Tauri's rule shows up as a warning rather than as records
/// quietly written somewhere nobody looks.
pub fn app_data_dir() -> Option<PathBuf> {
    let identifier = bundle_identifier()?;
    Some(platform_data_dir()?.join(identifier))
}

/// `dirs::data_dir()`, hand-rolled for the three desktop targets so no new
/// dependency is pulled in for two env-var lookups.
fn platform_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
}

/// The OS LOCAL app-data directory — resolved WITHOUT a Tauri app handle.
///
/// Mirrors `tauri::PathResolver::app_local_data_dir()`
/// (`dirs::data_local_dir()?.join(identifier)`) — the same relationship
/// [`app_data_dir`] has to `app.path().app_data_dir()`, and for the same
/// reason: [`crate::logfile::init`] has to be armed before any `AppHandle`
/// exists.
///
/// Differs from [`app_data_dir`] ONLY on Windows (F2-W10): `%LOCALAPPDATA%`
/// rather than the ROAMING `%APPDATA%`. Two things this app writes
/// continuously while a service runs — the pre-roll engine's rolling capture
/// segments (`lib.rs` setup) and the file log ([`crate::logfile::init`]) —
/// have no business being roamed at all, and a roaming profile is exactly the
/// kind of location a sync client can lock a file that is still growing, or
/// simply make slow: a domain's roaming-profile share, or a personal OneDrive
/// a user has pointed at their whole profile by hand (see
/// `sundayrec_core::preflight::looks_like_onedrive` for the save-folder half
/// of that same problem, F2-W9). `%LOCALAPPDATA%` is never roamed or synced
/// by Windows itself.
///
/// On macOS and Linux this returns the EXACT same path as [`app_data_dir`] —
/// [`platform_local_data_dir`] falls through to [`platform_data_dir`] there,
/// so the two cannot drift apart by editing one and forgetting the other. Any
/// caller switching from [`app_data_dir`] to this function therefore changes
/// NOTHING on those two platforms; see
/// `app_local_data_dir_is_the_same_dir_as_app_data_dir_off_windows` below.
pub fn app_local_data_dir() -> Option<PathBuf> {
    let identifier = bundle_identifier()?;
    Some(platform_local_data_dir()?.join(identifier))
}

/// `dirs::data_local_dir()`, hand-rolled — see [`platform_data_dir`], which
/// this function literally IS off Windows.
fn platform_local_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        platform_data_dir()
    }
}

// ── One-time move to local app-data (F2-W10) ─────────────────────────────────

/// What a caller switching a sub-path from [`app_data_dir`] to
/// [`app_local_data_dir`] should do about data already sitting at the OLD
/// (roaming) location.
///
/// A pure decision over the two facts that matter — kept separate from the
/// actual `rename` in [`move_once_best_effort`] so the three cases are a table
/// a test can drive without touching a filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateAction {
    /// Nothing to move: either nothing has ever written to the old path, or a
    /// previous launch already moved it. The common case on every launch but
    /// the one right after this upgrade.
    Nothing,
    /// Move `old` to `new`: the upgrade case — a previous version left data at
    /// the old roaming path, and the new local path is still untouched.
    Move,
    /// Both exist. Most likely two versions of the app have run on this
    /// machine, or a previous move got only partway before a crash. Leave
    /// both alone rather than guess which one to keep — silently merging or
    /// overwriting could destroy whichever turns out to matter, and the
    /// caller can carry on writing to `new` either way.
    LeaveBoth,
}

/// [`MigrateAction`] from whether the old and new directories currently
/// exist.
pub fn plan_one_time_move(old_exists: bool, new_exists: bool) -> MigrateAction {
    match (old_exists, new_exists) {
        (false, _) => MigrateAction::Nothing,
        (true, false) => MigrateAction::Move,
        (true, true) => MigrateAction::LeaveBoth,
    }
}

/// Carry out [`plan_one_time_move`]'s decision for `old` → `new`, best-effort.
///
/// Never fails the caller: a permissions error, a cross-device rename, or a
/// previous move that got only partway all just log a warning and leave `old`
/// exactly where it was. Worst case, the move never happens and old data sits
/// unused at the roaming path forever — precisely as it always did before
/// F2-W10 introduced a local path to move it to.
pub fn move_once_best_effort(old: &Path, new: &Path) {
    match plan_one_time_move(old.is_dir(), new.is_dir()) {
        MigrateAction::Nothing => {}
        MigrateAction::LeaveBoth => {
            tracing::warn!(
                old = %old.display(),
                new = %new.display(),
                "F2-W10: both the old (roaming) and new (local) directory exist — leaving both, not merging"
            );
        }
        MigrateAction::Move => {
            if let Some(parent) = new.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    tracing::warn!(
                        dir = %parent.display(),
                        "F2-W10: could not create the local app-data directory — leaving the old one in place"
                    );
                    return;
                }
            }
            match std::fs::rename(old, new) {
                Ok(()) => {
                    tracing::info!(
                        old = %old.display(),
                        new = %new.display(),
                        "F2-W10: moved to local app-data"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        old = %old.display(),
                        new = %new.display(),
                        "F2-W10: one-time move to local app-data failed, leaving the old directory in place: {e}"
                    );
                }
            }
        }
    }
}

/// The platform we're running on, mapped to the core [`Platform`] enum. A
/// compile-time `cfg!` check, consolidated here so the recorder, preroll, and
/// preview seams stop each carrying an identical copy.
pub fn detect_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

/// A `reqwest` client with bounded connect + per-request timeouts. A bare
/// `Client::new()` has NO timeout, so a half-open TCP connection or a server that
/// accepts the request then never responds (a token refresh, a telemetry POST)
/// would hang the calling task forever — wedging a background worker or blocking
/// a UI command. The connect timeout fails fast on a dead host; the request
/// timeout caps a stalled response. (Lived in the cloud-backup module until that
/// feature was removed; the Sunday Account + telemetry paths still need it.)
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        // A builder failure (no TLS backend) is a build/config error, not a
        // runtime input — fall back to the default client rather than panicking.
        .unwrap_or_else(|e| {
            tracing::warn!("http client builder failed ({e}); using default");
            reqwest::Client::new()
        })
}

/// Unix milliseconds as i64 — the timestamp convention every shell-side clock
/// read shares (alert throttle, account session freshness).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Lock a [`Mutex`], recovering its inner value if a previous holder panicked
/// rather than propagating the poison.
///
/// Every mutex in this crate guards plain bookkeeping (a status snapshot, an
/// `Option<JoinHandle>`, a counter) — never an invariant a panic could leave
/// half-broken. So taking the poisoned inner guard is correct, and strictly safer
/// than `.lock().expect(...)`: a single panicked thread must not cascade into a
/// crash on every later lock — least of all mid-recording, the worst possible
/// moment. On the happy path this is identical to `.lock().unwrap()`.
///
/// Consolidated here so the ~9 modules that need it stop each carrying their own
/// copy.
pub fn lock_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ── Child processes (no console windows on Windows) ──────────────────────────

/// The `CreateProcess` flag that gives a console child NO console of its own.
///
/// `winapi`/`windows-sys` spell it `CREATE_NO_WINDOW`; the literal is used here
/// so no dependency is pulled in for one constant, and so the value is visible
/// at the only place it is applied.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A [`tokio::process::Command`] that will not open a console window.
///
/// **Why this exists.** `main.rs` carries `windows_subsystem = "windows"`: the
/// app is a GUI process with NO console attached. When such a process starts a
/// CONSOLE subsystem child — every one of ours is: `ffmpeg.exe`, `ffprobe.exe`,
/// `powershell.exe`, `powercfg.exe` — Windows has nowhere to put the child's
/// stdio, so it ALLOCATES A NEW, VISIBLE console for it. The operator sees a
/// black window: one per device enumeration, one that stands for the minutes a
/// delivery transcode takes, and — with video — one that stands for the whole
/// service. That last one is not cosmetic: a volunteer who closes it sends
/// ffmpeg `CTRL_CLOSE_EVENT`, ffmpeg exits, and the recording dies.
///
/// `CREATE_NO_WINDOW` suppresses that allocation. It is deliberately NOT
/// `DETACHED_PROCESS`: the child must still inherit our piped stdio (the
/// recorder reads ffmpeg's stderr line-by-line for progress and
/// `silencedetect`, and writes `q` to its stdin for a graceful stop), and it
/// must still belong to our Job Object so [`crate::platform`]'s kill-on-close
/// guarantee keeps holding.
///
/// **Use this for EVERY child process** — including ones behind `#[cfg(unix)]`
/// or `#[cfg(target_os = "macos")]`. Off Windows the helper is the identity
/// function, so routing a `pgrep` through it costs nothing, and a rule with no
/// exceptions is a rule the `hidden_command_ratchet` test can enforce. An
/// exception list is where the next raw `Command::new` would hide.
///
/// The two `cfg` blocks (rather than one `let` plus a conditional call) are
/// there so neither lane earns a warning: off Windows a `let mut` that is never
/// mutated is an `unused_mut`, and `let x = …; x` is clippy's `let_and_return`.
pub fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[cfg(windows)]
    {
        // `creation_flags` is an INHERENT method on tokio's `Command` under
        // `cfg(windows)` — NOT the `std::os::windows::process::CommandExt`
        // trait the std twin below needs. Importing that trait here would earn
        // an unused-import warning, which `-D warnings` turns red.
        let mut cmd = tokio::process::Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        tokio::process::Command::new(program)
    }
}

/// [`hidden_command`]'s synchronous twin, for the one-shot probes and the
/// detached helpers that have no reason to carry the async machinery.
///
/// Same contract, same flag, same "use it everywhere" rule; see
/// [`hidden_command`] for why the flag is needed and why it is not
/// `DETACHED_PROCESS`.
pub fn hidden_std_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(program)
    }
}

// ── Hidden directories (Windows) ──────────────────────────────────────────────

/// Mark `dir` hidden in Windows Explorer (F2-W6).
///
/// A leading `.` hides a folder on macOS (Finder) for free, but it is just an
/// ordinary character to Windows — `.sundayrec-capture-<id>` (the live capture
/// folder) and `.sundayrec-trash` (the Papirkurv) sit there in plain sight in
/// Explorer. A volunteer poking around the save folder mid-service can find —
/// and "tidy away" — the WAV/MKV fragments a live recording is still writing,
/// or mistake the Papirkurv for stray junk and delete what was meant to be
/// recoverable. Setting the real `FILE_ATTRIBUTE_HIDDEN` bit closes that gap
/// the same way Explorer's own "Hidden items" folders behave.
///
/// Call this right after `create_dir_all` creates the directory — the
/// attribute is a property of the directory ENTRY, so it must already exist.
/// Best-effort and silent to the caller: odd ACLs or a network share that
/// rejects the attribute only logs a warning. The directory still does its
/// actual job either way (a live recording, a moved-to-trash file); it just
/// stays visible, which is exactly today's behaviour — never a reason to fail
/// a recording or a delete.
pub fn hide_dir_on_windows(dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::Storage::FileSystem::{
            SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN,
        };

        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer, valid and unchanged
        // for the duration of this call — everything `SetFileAttributesW`
        // requires of its pointer argument.
        let ok = unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            tracing::warn!(dir = %dir.display(), "could not mark directory hidden: {err}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
    }
}

// ── Atomic file writes ──────────────────────────────────────────────────────

/// The scratch file [`write_atomic`] lands in before the rename.
///
/// The name is DERIVED from the target (`manifest.json` → `manifest.json.tmp`)
/// rather than randomised, and that is the deliberate half: a process that dies
/// between the write and the rename leaves at most ONE stray file per target,
/// which the next write reuses. A unique temp name would instead accumulate one
/// corpse per crash in a directory nothing prunes.
///
/// `with_extension` would REPLACE `.json`, so the suffix is appended to the raw
/// `OsString` instead.
fn temp_beside(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// Write `bytes` to `path` so that a reader — or a power cut — never sees half
/// of them.
///
/// `std::fs::write` truncates first and fills afterwards: every millisecond in
/// between, the file on disk IS the truncated one. For a file the app treats as
/// a record of what exists (the Papirkurv manifest, a crash record, the
/// telemetry snapshot) that window is the difference between "the previous
/// answer" and "no answer at all". So: write a scratch file beside the target,
/// `fsync` it, then `rename` over the target — a rename within one directory is
/// atomic on every filesystem the app ships on, so the target is only ever the
/// old file or the new one.
///
/// **The `fsync` is not decoration.** `rename` orders the DIRECTORY entry, not
/// the data blocks behind it: without the sync a crash can leave the new name
/// pointing at a block of zeros, which is precisely the "manifest is there but
/// unreadable" state this helper exists to prevent. It costs one flush of a few
/// kilobytes; every caller writes small files, none from a capture path.
/// (On macOS this is `fsync`, not `F_FULLFSYNC` — it hands the bytes to the
/// drive without forcing its cache, which is the trade every database on this
/// platform makes too.)
///
/// The scratch file is removed when either step fails, so a failing disk does
/// not also litter.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_beside(path);

    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// [`write_atomic`] for a caller already inside an async task, with the same
/// contract (same temp name, same `fsync`, same cleanup).
///
/// Its own function rather than `spawn_blocking(write_atomic)`: the one caller
/// is the crash-recovery manifest, written once per segment from the recorder's
/// session loop, and handing that to the blocking pool would put a thread hop
/// in the middle of the recording path to save four lines.
pub async fn write_atomic_async(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = temp_beside(path);

    let write = async {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(bytes).await?;
        f.sync_all().await
    };
    if let Err(e) = write.await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn lock_recover_returns_inner_after_poison() {
        // A poisoned mutex must still hand back its inner guard so one panicked
        // thread can't crash every later lock.
        let m = Arc::new(Mutex::new(1u8));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.lock().is_err(), "precondition: the mutex is poisoned");
        *lock_recover(&m) = 42;
        assert_eq!(*lock_recover(&m), 42);
    }

    #[test]
    fn the_bundle_identifier_is_read_from_the_real_tauri_conf() {
        // If the identifier ever moves or is renamed, the crash ring + file log
        // would silently start writing to `<data>/` instead of
        // `<data>/no.sundayrec.app/` — beside every OTHER app's data.
        let id = bundle_identifier().expect("tauri.conf.json must carry an identifier");
        assert!(id.contains('.'), "a bundle identifier is reverse-DNS: {id}");
        assert!(!id.trim().is_empty());
    }

    #[test]
    fn the_app_data_dir_is_the_platform_dir_joined_with_the_identifier() {
        // The invariant Tauri's own resolver holds
        // (`dirs::data_dir()?.join(identifier)`), asserted against ours.
        let Some(dir) = app_data_dir() else {
            return; // no HOME/APPDATA in this environment — nothing to compare
        };
        let id = bundle_identifier().unwrap();
        assert_eq!(dir.file_name().unwrap().to_string_lossy(), id);
        assert_eq!(dir.parent().unwrap(), platform_data_dir().unwrap());
        assert!(dir.is_absolute(), "{}", dir.display());
    }

    // ── app_local_data_dir (F2-W10) ─────────────────────────────────────────

    #[test]
    fn the_app_local_data_dir_is_the_local_platform_dir_joined_with_the_identifier() {
        let Some(dir) = app_local_data_dir() else {
            return; // no HOME/APPDATA/LOCALAPPDATA in this environment
        };
        let id = bundle_identifier().unwrap();
        assert_eq!(dir.file_name().unwrap().to_string_lossy(), id);
        assert_eq!(dir.parent().unwrap(), platform_local_data_dir().unwrap());
        assert!(dir.is_absolute(), "{}", dir.display());
    }

    /// F2-W10's pin: macOS (and Linux) have no roaming/local split at all, so
    /// the switch away from [`app_data_dir`] must be a complete no-op there —
    /// anyone relying on the OLD location keeps finding their files in
    /// exactly the same place, with nothing to migrate.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn app_local_data_dir_is_the_same_dir_as_app_data_dir_off_windows() {
        assert_eq!(app_local_data_dir(), app_data_dir());
    }

    /// The Windows half of the same pin: `%LOCALAPPDATA%` and `%APPDATA%` are
    /// real, DIFFERENT special folders there, which is the entire point of
    /// F2-W10 — a test that only checked "doesn't panic" would pass even if
    /// this had quietly become another copy of [`platform_data_dir`].
    #[cfg(target_os = "windows")]
    #[test]
    fn app_local_data_dir_differs_from_app_data_dir_on_windows() {
        let local = app_local_data_dir().expect("LOCALAPPDATA must be set on windows-latest");
        let roaming = app_data_dir().expect("APPDATA must be set on windows-latest");
        assert_ne!(
            local, roaming,
            "F2-W10: local and roaming app-data must not be the same directory"
        );
    }

    // ── plan_one_time_move / move_once_best_effort (F2-W10) ────────────────

    #[test]
    fn plan_one_time_move_covers_all_three_cases() {
        assert_eq!(plan_one_time_move(false, false), MigrateAction::Nothing);
        assert_eq!(plan_one_time_move(false, true), MigrateAction::Nothing);
        assert_eq!(plan_one_time_move(true, false), MigrateAction::Move);
        assert_eq!(plan_one_time_move(true, true), MigrateAction::LeaveBoth);
    }

    #[test]
    fn move_once_best_effort_moves_the_old_directory_to_the_new_path() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old/logs");
        let new = root.path().join("new/logs");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("sundayrec.log"), b"hello").unwrap();

        move_once_best_effort(&old, &new);

        assert!(!old.exists(), "the old directory must be gone after a move");
        assert_eq!(
            std::fs::read(new.join("sundayrec.log")).unwrap(),
            b"hello",
            "the file's contents must survive the move"
        );
    }

    #[test]
    fn move_once_best_effort_does_nothing_when_there_is_no_old_directory() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old/logs");
        let new = root.path().join("new/logs");

        move_once_best_effort(&old, &new);

        assert!(!new.exists(), "nothing to move must not invent a directory");
    }

    #[test]
    fn move_once_best_effort_leaves_both_directories_alone_when_both_exist() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old/logs");
        let new = root.path().join("new/logs");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("old.log"), b"old").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("new.log"), b"new").unwrap();

        move_once_best_effort(&old, &new);

        assert_eq!(std::fs::read(old.join("old.log")).unwrap(), b"old");
        assert_eq!(std::fs::read(new.join("new.log")).unwrap(), b"new");
    }

    #[test]
    fn detect_platform_matches_the_build_target() {
        let p = detect_platform();
        if cfg!(target_os = "windows") {
            assert_eq!(p, Platform::Windows);
        } else if cfg!(target_os = "macos") {
            assert_eq!(p, Platform::MacOS);
        } else {
            assert_eq!(p, Platform::Linux);
        }
    }

    // ── write_atomic ────────────────────────────────────────────────────────

    /// Every `.tmp` left in `dir`. The whole point of the helper is that this
    /// is empty once it returns.
    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn an_atomic_write_lands_the_bytes_and_leaves_no_scratch_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        write_atomic(&path, b"{\"entries\":[]}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"entries\":[]}");
        assert!(
            leftovers(dir.path()).is_empty(),
            "atomic write left scratch"
        );
    }

    #[test]
    fn a_second_write_replaces_the_first_without_a_window_of_nothing() {
        // The regression this helper exists for: `fs::write` truncates first,
        // so a reader (or a power cut) between truncate and fill sees an EMPTY
        // file where a whole one used to be.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        write_atomic(&path, b"first, and long enough to be truncated").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn the_scratch_file_sits_beside_the_target_and_keeps_its_extension() {
        // `with_extension(".tmp")` would turn `manifest.json` into
        // `manifest.tmp` — a different file, in the same directory, that a
        // suffix-based sweep would not recognise as scratch.
        let tmp = temp_beside(Path::new("/a/b/manifest.json"));
        assert_eq!(tmp, PathBuf::from("/a/b/manifest.json.tmp"));
    }

    #[test]
    fn an_atomic_write_creates_the_directory_it_was_pointed_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/last-recording.json");
        write_atomic(&path, b"{}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }

    #[test]
    fn a_write_that_cannot_land_leaves_the_previous_file_whole() {
        // A rename onto a DIRECTORY fails on every platform. The old answer
        // must survive a failed new one — that is the entire contract.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("occupied");
        std::fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, b"nope").is_err());
        assert!(path.is_dir(), "the existing entry survived");
        assert!(
            leftovers(dir.path()).is_empty(),
            "a failed write must not litter"
        );
    }

    #[tokio::test]
    async fn the_async_twin_holds_the_same_contract() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        write_atomic_async(&path, b"first").await.unwrap();
        write_atomic_async(&path, b"2").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"2");
        assert!(leftovers(dir.path()).is_empty());
    }

    // ── hide_dir_on_windows (F2-W6) ──────────────────────────────────────────

    /// Real Explorer visibility, not a mock: `windows-check` runs this on
    /// `windows-latest`, so the assertion below is the actual bit a real
    /// Explorer window reads, via the same std API that reads it.
    #[cfg(windows)]
    #[test]
    fn hide_dir_on_windows_sets_the_real_hidden_attribute() {
        use std::os::windows::fs::MetadataExt;

        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".sundayrec-capture-1700000000000");
        std::fs::create_dir_all(&target).unwrap();
        let before = std::fs::metadata(&target).unwrap().file_attributes();
        assert_eq!(
            before & FILE_ATTRIBUTE_HIDDEN,
            0,
            "precondition: a freshly created directory must not already be hidden"
        );

        hide_dir_on_windows(&target);

        let after = std::fs::metadata(&target).unwrap().file_attributes();
        assert_ne!(
            after & FILE_ATTRIBUTE_HIDDEN,
            0,
            "FILE_ATTRIBUTE_HIDDEN was not set after hide_dir_on_windows"
        );
    }

    /// Off Windows the helper is the identity function — it must not touch the
    /// filesystem at all (there is nothing to touch: no attribute bit exists),
    /// so calling it on a directory that does not even exist must not panic or
    /// error.
    #[cfg(not(windows))]
    #[test]
    fn hide_dir_on_windows_is_a_no_op_off_windows() {
        hide_dir_on_windows(Path::new("/does/not/exist"));
    }
}
