//! The graduated low-disk observer — a warning while there is still time.
//!
//! The recorder engine has always had a low-disk guard, and it is a good one:
//! below 500 MB (audio) / 4 GB (video) plus a finalise reserve it STOPS the take
//! gracefully, so the container is closed and the file plays. That guard is
//! untouched by this module and must stay that way — it is the last line of
//! defence and it is hardware-verified.
//!
//! But it is also the FIRST thing the operator hears. A volunteer whose disk
//! filled during the sermon gets no warning and then an ended recording. This
//! observer sits well above those numbers (2 GB audio / 8 GB video, see
//! [`sundayrec_core::notify::disk_warn_threshold_bytes`]) and says something
//! while deleting one old service would still fix it.
//!
//! ## Shape
//!
//! Purely observational, exactly like the tray's state wiring: it subscribes to
//! `recording://started` and `recording://state`, which the engine already
//! emits, and polls `fs4::available_space` on a 60 s tick. No engine code is
//! called, no capture path is touched, and the poll only runs while a take is
//! live. One warning per recording session — a disk hovering at the threshold
//! must not produce a toast every minute for an hour.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Listener, Manager};

use sundayrec_core::notify::{code, should_warn_low_disk, BackendWarning};
use sundayrec_core::preflight::video_active;
use sundayrec_core::recorder::RecorderState;

use crate::db::Db;

/// How often free space is sampled while recording. Slow enough to be free,
/// frequent enough that the gap between "getting low" and the engine's
/// terminal stop is many samples wide.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Subscribe the observer to the recorder's lifecycle events. Call once, from
/// `setup`.
pub fn wire(app: &AppHandle) {
    use crate::recorder::engine::{STARTED_EVENT, STATE_EVENT};

    // Shared between the two listeners and the polling task:
    //   `active`  — a take is live (poll) / is not (idle out)
    //   `warned`  — this session has already said its piece
    //   `running` — an observer task is up, so a second start does not spawn a
    //               second one
    let active = Arc::new(AtomicBool::new(false));
    let warned = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(false));

    let handle = app.clone();
    let (a, w, r) = (
        Arc::clone(&active),
        Arc::clone(&warned),
        Arc::clone(&running),
    );
    app.listen(STARTED_EVENT, move |_| {
        a.store(true, Ordering::SeqCst);
        // A new take re-arms the warning: the operator deserves to be told again
        // for THIS recording, even if the last one already complained.
        w.store(false, Ordering::SeqCst);
        if r.swap(true, Ordering::SeqCst) {
            return; // an observer is already up
        }
        let app = handle.clone();
        let (a, w, r) = (Arc::clone(&a), Arc::clone(&w), Arc::clone(&r));
        tauri::async_runtime::spawn(async move { observe(app, a, w, r).await });
    });

    let a = Arc::clone(&active);
    app.listen(STATE_EVENT, move |ev| {
        // `RecorderStatePayload { state, … }`. An unparseable payload leaves the
        // flag alone — the observer idling one extra minute is harmless; missing
        // a stop and polling forever would not be.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(ev.payload()) else {
            return;
        };
        let Some(state) = value.get("state") else {
            return;
        };
        let Ok(state) = serde_json::from_value::<RecorderState>(state.clone()) else {
            return;
        };
        if !state.is_active() {
            a.store(false, Ordering::SeqCst);
        }
    });
}

/// Poll free space while a take is live. Exits when the take ends.
async fn observe(
    app: AppHandle,
    active: Arc<AtomicBool>,
    warned: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
) {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        if !active.load(Ordering::SeqCst) {
            running.store(false, Ordering::SeqCst);
            // A take that started while we were deciding to exit saw
            // `running == true` and declined to spawn a replacement. Claim the
            // flag back rather than leaving that session unwatched.
            if active.load(Ordering::SeqCst) && !running.swap(true, Ordering::SeqCst) {
                continue;
            }
            return;
        }

        if warned.load(Ordering::SeqCst) {
            continue; // already said it for this session; keep the loop cheap
        }

        let Some(db) = app.try_state::<Db>() else {
            continue;
        };
        let settings = crate::settings::load(&db.pool).await.unwrap_or_default();
        let Some(folder) = save_folder(&app, settings.save_folder.as_deref()) else {
            continue;
        };
        let Ok(free) = fs4::available_space(&folder) else {
            continue; // a volume that won't report is not a volume that is full
        };

        let video = video_active(&settings);
        if should_warn_low_disk(free, video, warned.load(Ordering::SeqCst)) {
            warned.store(true, Ordering::SeqCst);
            let gb = free as f64 / 1_073_741_824.0;
            crate::notify::warn(
                &app,
                BackendWarning::warn(code::DISK_LOW)
                    .msg(format!(
                        "Det begynner å bli lite plass på disken — {gb:.1} GB ledig. \
                         Opptaket stopper av seg selv hvis den blir full."
                    ))
                    .param("freeBytes", free.to_string()),
            );
        }
    }
}

/// The volume to measure: the configured save folder when it exists, else the
/// Documents directory. Same fallback as the `get_disk_space` command — a
/// configured folder that has been unplugged would otherwise make the probe
/// fail rather than report the disk the recording is actually landing on.
fn save_folder(app: &AppHandle, configured: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(folder) = configured.map(str::trim).filter(|f| !f.is_empty()) {
        let path = std::path::PathBuf::from(folder);
        if path.exists() {
            return Some(path);
        }
    }
    app.path().document_dir().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_poll_is_frequent_enough_to_precede_the_terminal_stop() {
        // The engine's own guard polls far faster and stops the take; this one
        // only has to fire at least a few times between "getting low" and
        // "stopped", so a minute is generous on both sides.
        assert!(POLL_INTERVAL >= Duration::from_secs(30));
        assert!(POLL_INTERVAL <= Duration::from_secs(120));
    }

    #[test]
    fn only_a_working_session_is_polled() {
        // The predicate the STATE_EVENT listener idles out on. `Stopping` is
        // deliberately NOT live here (unlike the tray's badge): a take that is
        // finalising cannot be helped by a disk warning.
        assert!(RecorderState::Recording.is_active());
        assert!(RecorderState::Preparing.is_active());
        assert!(RecorderState::Reconnecting.is_active());
        assert!(!RecorderState::Stopping.is_active());
        assert!(!RecorderState::Stopped.is_active());
        assert!(!RecorderState::Failed.is_active());
        assert!(!RecorderState::Idle.is_active());
    }

    #[test]
    fn the_state_payload_the_engine_emits_still_parses_into_a_state() {
        // The listener digs `state` out of the JSON and deserialises it. A serde
        // rename on `RecorderState` would silently strand the observer polling
        // forever after a take ends.
        let payload = serde_json::json!({ "state": "recording", "reconnectCount": 0 });
        let state: RecorderState =
            serde_json::from_value(payload.get("state").expect("state field").clone())
                .expect("the emitted state string must parse");
        assert!(state.is_active());
    }
}
