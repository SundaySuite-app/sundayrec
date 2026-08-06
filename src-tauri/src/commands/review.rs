//! Episode-prep + review-queue + Stage-import commands (PU-6 P2b) — **INFRA-UNVERIFIED**.
//!
//! The thin IPC layer over the unit-tested `sundayrec_core::{prep, review_queue,
//! integrations::stage}`. The review queue is persisted exactly as the Electron
//! app did — a JSON blob under the `reviewQueue` settings key (no new migration,
//! so this never touches the recording schema). The shell owns the clock + uuid
//! + the JSON (de)serialisation; the decisions are the core's.
//!
//! ## ⚠️ INFRA-UNVERIFIED
//!
//! - [`prep_build_episode`] takes the analysis segments as input rather than
//!   running audio-analysis itself — the ffmpeg/FFT analysis (`audio-analysis.ts`)
//!   is NOT ported yet, so the caller (or a later analysis seam) supplies the
//!   segments. The assembly + status decision ARE the unit-tested core.
//! - [`review_process_reminders`] returns the actions the scheduler should fire;
//!   the actual notify/email/webhook dispatch is left to the existing seams
//!   (PU-1 email, scheduler notifications) and is not wired through here yet.
//!   See docs/NEEDS-RICHARD.md (PU-6).

use tauri::{AppHandle, State};

use sundayrec_core::integrations::stage::{self, StageManifest};
use sundayrec_core::integrations::{ChapterMarker, ServiceLink};
use sundayrec_core::prep::{self, EpisodePrep, PrepAnalysisSegment, PrepDefaults, SuggestedTrim};
use sundayrec_core::review_queue::{self, ReminderAction, ReviewQueueEntry};

use crate::db::store::{self, new_id, now_ms};
use crate::db::Db;
use crate::error::{AppError, AppResult};

/// The settings key the review queue is persisted under (mirrors Electron's
/// `electron-store` `reviewQueue` key).
const REVIEW_QUEUE_KEY: &str = "reviewQueue";

fn now_i64() -> i64 {
    now_ms() as i64
}

// ── Review-queue persistence (JSON blob under a settings key) ───────────────

pub(crate) async fn load_queue(db: &Db) -> AppResult<Vec<ReviewQueueEntry>> {
    match store::get_setting(&db.pool, REVIEW_QUEUE_KEY).await? {
        Some(json) if !json.is_empty() => Ok(serde_json::from_str(&json).unwrap_or_default()),
        _ => Ok(Vec::new()),
    }
}

pub(crate) async fn save_queue(db: &Db, entries: &[ReviewQueueEntry]) -> AppResult<()> {
    // Strip the derived age before persisting (mirrors `writeRaw`).
    let sanitised: Vec<ReviewQueueEntry> = entries
        .iter()
        .cloned()
        .map(|mut e| {
            e.age_in_days = 0.0;
            e
        })
        .collect();
    let json = serde_json::to_string(&sanitised)?;
    store::set_setting(&db.pool, REVIEW_QUEUE_KEY, &json).await
}

// ── Episode prep ────────────────────────────────────────────────────────────

/// Resolve the podcast defaults from settings (master preset + intro/outro). A
/// missing/blank setting falls back to the Electron defaults via [`PrepDefaults`].
async fn prep_defaults(db: &Db) -> AppResult<PrepDefaults> {
    let read = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let master = read(store::get_setting(&db.pool, "podcastDefaultMasterPreset").await?)
        .unwrap_or_else(|| "speech-clear".into());
    let intro = read(store::get_setting(&db.pool, "podcastDefaultIntroPath").await?);
    let outro = read(store::get_setting(&db.pool, "podcastDefaultOutroPath").await?);
    Ok(PrepDefaults {
        master_preset: master,
        intro_path: intro,
        outro_path: outro,
    })
}

/// Build an [`EpisodePrep`] from already-computed analysis segments + the
/// resolved defaults, and add it to the review queue. INFRA-UNVERIFIED: the
/// analysis itself isn't ported; the caller supplies `segments`.
#[tauri::command]
pub async fn prep_build_episode(
    app: AppHandle,
    db: State<'_, Db>,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
) -> AppResult<EpisodePrep> {
    let defaults = prep_defaults(&db).await?;
    let now = now_i64();
    let episode = prep::build_episode_prep(new_id(), recording_path, segments, &defaults, now);

    let queue = load_queue(&db).await?;
    let queue = review_queue::enqueue(queue, episode.clone(), now);
    save_queue(&db, &queue).await?;
    crate::tray_note_review_queue(&app);
    Ok(episode)
}

// ── Review queue ──────────────────────────────────────────────────────────

/// The review queue, newest-first, with `ageInDays` filled in.
#[tauri::command]
pub async fn review_queue_list(db: State<'_, Db>) -> AppResult<Vec<ReviewQueueEntry>> {
    let queue = load_queue(&db).await?;
    Ok(review_queue::read_with_age(&queue, now_i64()))
}

/// How many episodes are genuinely WAITING for a human — the number the tray's
/// "📬 N episoder klare" callout shows. Published/discarded entries linger in the
/// blob for the UI's benefit and must not be counted, or the tray would nag
/// about work that is already done. Never errors: an unreadable queue is 0.
pub async fn pending_review_count(db: &Db) -> u32 {
    let queue = load_queue(db).await.unwrap_or_default();
    queue
        .iter()
        .filter(|e| {
            !matches!(
                e.prep.status,
                sundayrec_core::prep::EpisodePrepStatus::Published
                    | sundayrec_core::prep::EpisodePrepStatus::Discarded
            )
        })
        .count() as u32
}

/// Mark a queued prep published (kept briefly for the UI toast).
#[tauri::command]
pub async fn review_mark_published(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
) -> AppResult<bool> {
    let mut queue = load_queue(&db).await?;
    let ok = review_queue::mark_published(&mut queue, &id, now_i64());
    if ok {
        save_queue(&db, &queue).await?;
        crate::tray_note_review_queue(&app);
    }
    Ok(ok)
}

/// Mark a queued prep discarded ("ikke publiser denne uka").
#[tauri::command]
pub async fn review_mark_discarded(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
) -> AppResult<bool> {
    let mut queue = load_queue(&db).await?;
    let ok = review_queue::mark_discarded(&mut queue, &id, now_i64());
    if ok {
        save_queue(&db, &queue).await?;
        crate::tray_note_review_queue(&app);
    }
    Ok(ok)
}

// ── Review-queue edits (the three field patches the editor pushes back) ─────

/// Load → patch → save → (caller refreshes the tray), atomically enough for a
/// single-writer desktop app: the whole queue is one JSON blob, so a patch that
/// read a stale copy would silently drop a neighbouring entry's edit. Reading
/// immediately before writing keeps the window to a single `await` pair, and
/// every edit goes through here rather than through three hand-rolled copies.
///
/// Returns `false` — WITHOUT writing anything — when `id` is no longer in the
/// queue (published, discarded, or auto-discarded while the editor was open).
/// That `false` is the whole point: the renderer used to assume its edit landed.
///
/// Takes `&Db` rather than `State<Db>` so the tests below can drive it against a
/// real temp database; the commands are the thin `State` wrappers.
async fn patch_prep(db: &Db, id: &str, patch: impl FnOnce(&mut EpisodePrep)) -> AppResult<bool> {
    let mut queue = load_queue(db).await?;
    // `update_entry` guards prep.id/created_at and bumps updated_at for us.
    if !review_queue::update_entry(&mut queue, id, now_i64(), patch) {
        return Ok(false);
    }
    save_queue(db, &queue).await?;
    Ok(true)
}

/// A partial jingle patch from the editor's two intro/outro dropdowns.
///
/// ## The contract (three states per field, not two)
///
/// The editor changes ONE dropdown at a time and posts only that field, so the
/// wire shape has to distinguish three cases — a plain `Option<String>` can only
/// carry two:
///
/// | JS                          | Rust                  | Meaning              |
/// |-----------------------------|-----------------------|----------------------|
/// | `{ outroPath: "/a.mp3" }`   | `Some(Some(path))`    | set it               |
/// | `{ outroPath: null }`       | `Some(None)`          | clear it («Ingen»)   |
/// | `{ }` (key absent)          | `None`                | don't touch it       |
///
/// Serde's plain `Option<Option<T>>` collapses the last two (an explicit `null`
/// deserialises to the outer `None`), which is why each field carries the
/// [`double_option`] reader: `#[serde(default)]` supplies the absent case and
/// the reader wraps everything that IS present in `Some`.
///
/// `undefined` in JS is not a fourth case: `JSON.stringify` drops those keys, so
/// it arrives as "absent".
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JinglesPatch {
    /// Absent = leave the intro alone; `null` = remove it; a path = use it.
    #[serde(default, deserialize_with = "double_option")]
    pub intro_path: Option<Option<String>>,
    /// Absent = leave the outro alone; `null` = remove it; a path = use it.
    #[serde(default, deserialize_with = "double_option")]
    pub outro_path: Option<Option<String>>,
}

/// Deserialise a present field (including an explicit `null`) into
/// `Some(...)`, leaving `#[serde(default)]` to produce `None` for an absent one.
/// The standard serde idiom for "present-but-null" vs "missing".
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use serde::Deserialize as _;
    Option::<T>::deserialize(de).map(Some)
}

/// Whether a trim is a forward, non-negative span. Written as `!(end > start)`
/// rather than `end <= start` so a NaN — which loses every comparison — is
/// rejected instead of silently stored.
fn trim_is_valid(trim: &SuggestedTrim) -> bool {
    trim.start_sec >= 0.0 && trim.end_sec > trim.start_sec
}

/// Store a revised sermon trim on a queued prep.
///
/// `trim` is the renderer's `{ startSec, endSec }`. Rejected outright when it is
/// not a forward, non-negative span (see [`trim_is_valid`]): a stored
/// `endSec <= startSec` would make the next review-mode open produce an empty
/// edit with no visible cause.
#[tauri::command]
pub async fn review_update_trim(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
    trim: SuggestedTrim,
) -> AppResult<bool> {
    if !trim_is_valid(&trim) {
        return Err(AppError::Validation("invalid_trim".into()));
    }
    let changed = patch_prep(&db, &id, |p| p.suggested_trim = Some(trim)).await?;
    if changed {
        crate::tray_note_review_queue(&app);
    }
    Ok(changed)
}

/// Store the mastering preset the operator settled on.
///
/// A BLANK `preset_id` is valid and means "no mastering" — the same empty string
/// the editor's `E.masterPreset` uses — so this deliberately does not reject it.
#[tauri::command]
pub async fn review_update_master_preset(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
    preset_id: String,
) -> AppResult<bool> {
    let changed = patch_prep(&db, &id, |p| p.master_preset = preset_id).await?;
    if changed {
        crate::tray_note_review_queue(&app);
    }
    Ok(changed)
}

/// Store an intro/outro choice. See [`JinglesPatch`] for the absent-vs-null
/// contract: a patch that names neither field is a no-op that still reports
/// whether the entry exists.
#[tauri::command]
pub async fn review_update_jingles(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
    jingles: JinglesPatch,
) -> AppResult<bool> {
    let changed = patch_prep(&db, &id, |p| {
        if let Some(intro) = jingles.intro_path {
            p.intro_path = intro;
        }
        if let Some(outro) = jingles.outro_path {
            p.outro_path = outro;
        }
    })
    .await?;
    if changed {
        crate::tray_note_review_queue(&app);
    }
    Ok(changed)
}

/// Run the reminder timeline over the queue and persist the result, returning
/// the reminder actions the scheduler should fire. INFRA-UNVERIFIED: dispatching
/// each action (notify/email/webhook) is left to the existing seams.
#[tauri::command]
pub async fn review_process_reminders(db: State<'_, Db>) -> AppResult<Vec<ReminderActionDto>> {
    let queue = load_queue(&db).await?;
    let outcome = review_queue::process_reminders(&queue, now_i64());
    if outcome.changed {
        save_queue(&db, &outcome.survivors).await?;
    }
    Ok(outcome.actions.into_iter().map(Into::into).collect())
}

/// A reminder action flattened for the IPC boundary (the core enums don't derive
/// `Serialize`; this is the wire shape).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderActionDto {
    pub id: String,
    /// `notify` | `notify_email` | `notify_email_webhook` |
    /// `notify_email_webhook_warning` | `auto_discard`.
    pub channel: &'static str,
    /// `day1` | `day2` | `day7` | `discard`.
    pub message: &'static str,
}

impl From<ReminderAction> for ReminderActionDto {
    fn from(a: ReminderAction) -> Self {
        use review_queue::{ReminderChannel as C, ReminderMessage as M};
        ReminderActionDto {
            id: a.id,
            channel: match a.channel {
                C::Notify => "notify",
                C::NotifyEmail => "notify_email",
                C::NotifyEmailWebhook => "notify_email_webhook",
                C::NotifyEmailWebhookWarning => "notify_email_webhook_warning",
                C::AutoDiscard => "auto_discard",
            },
            message: match a.message {
                M::Day1 => "day1",
                M::Day2 => "day2",
                M::Day7 => "day7",
                M::Discard => "discard",
            },
        }
    }
}

// ── Stage manifest import ────────────────────────────────────────────────────

/// The result of applying a Stage manifest to a recording.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageApplyResult {
    pub chapters: Vec<ChapterMarker>,
    pub service_link: ServiceLink,
}

/// Parse a SundayStage `service-manifest.json` and map it to chapter markers +
/// a service link, aligned to the recording's start. The fs writes
/// (`.meta.json` chapters + `.service.json` link) are left to the shell's
/// sidecar writer; this returns the mapped data. INFRA-UNVERIFIED.
#[tauri::command]
pub async fn stage_import_manifest(
    manifest_json: String,
    recording_start_ms: i64,
    duration_sec: Option<i64>,
    was_streamed: Option<bool>,
    service_date: Option<String>,
) -> AppResult<StageApplyResult> {
    let manifest: StageManifest = stage::parse_stage_manifest(&manifest_json)
        .ok_or_else(|| AppError::Validation("invalid_manifest".into()))?;
    let chapters = stage::manifest_to_chapters(&manifest, recording_start_ms, duration_sec);
    let service_link = stage::build_service_link(
        &manifest,
        recording_start_ms,
        was_streamed,
        service_date,
        now_i64(),
    );
    Ok(StageApplyResult {
        chapters,
        service_link,
    })
}

#[cfg(test)]
mod tests {
    //! Review-queue persistence over a temp sqlite store. The Tauri commands take
    //! `State<Db>` (not constructible in a unit test), so these exercise the same
    //! load/save seam the commands call, plus the core transitions they thread,
    //! against a real (throwaway) database — no app, no clock prompts.
    use super::*;
    use sundayrec_core::prep::{build_episode_prep, EpisodePrepStatus, PrepDefaults};

    /// A migrated temp-dir database wrapped in a [`Db`] handle.
    async fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (Db::new(pool), dir)
    }

    /// A ready-status episode prep with the given id/path, built through the core
    /// so the fixture matches what `prep_build_episode` produces.
    fn prep(id: &str, path: &str, now: i64) -> EpisodePrep {
        build_episode_prep(
            id.to_string(),
            path.to_string(),
            Vec::new(),
            &PrepDefaults::default(),
            now,
        )
    }

    #[tokio::test]
    async fn load_queue_is_empty_on_a_fresh_store() {
        let (db, _d) = temp_db().await;
        assert!(load_queue(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn save_then_load_round_trips_an_entry() {
        let (db, _d) = temp_db().await;
        let entry = review_queue::ReviewQueueEntry {
            id: "rec-1".into(),
            prep: prep("rec-1", "/rec/a.m4a", 1_000),
            added_at: 1_000,
            reminded: 0,
            age_in_days: 0.0,
        };
        save_queue(&db, std::slice::from_ref(&entry)).await.unwrap();

        let back = load_queue(&db).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, "rec-1");
        assert_eq!(back[0].prep.recording_path, "/rec/a.m4a");
    }

    #[tokio::test]
    async fn save_queue_strips_the_derived_age_before_persisting() {
        let (db, _d) = temp_db().await;
        let entry = review_queue::ReviewQueueEntry {
            id: "rec-1".into(),
            prep: prep("rec-1", "/rec/a.m4a", 1_000),
            added_at: 1_000,
            reminded: 0,
            // A non-zero derived age must NOT survive the write (mirrors writeRaw).
            age_in_days: 9.5,
        };
        save_queue(&db, std::slice::from_ref(&entry)).await.unwrap();
        assert_eq!(load_queue(&db).await.unwrap()[0].age_in_days, 0.0);
    }

    #[tokio::test]
    async fn enqueue_persists_and_dedupes_by_id() {
        let (db, _d) = temp_db().await;
        let q = load_queue(&db).await.unwrap();
        let q = review_queue::enqueue(q, prep("rec-1", "/rec/a.m4a", 1_000), 1_000);
        save_queue(&db, &q).await.unwrap();

        // Re-enqueue the same id with a new path: replaces, never a second row.
        let q = load_queue(&db).await.unwrap();
        let q = review_queue::enqueue(q, prep("rec-1", "/rec/b.m4a", 2_000), 2_000);
        save_queue(&db, &q).await.unwrap();

        let back = load_queue(&db).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].prep.recording_path, "/rec/b.m4a");
    }

    #[tokio::test]
    async fn read_with_age_sorts_newest_first_and_fills_age() {
        let (db, _d) = temp_db().await;
        let mut q = Vec::new();
        q = review_queue::enqueue(q, prep("old", "/rec/old.m4a", 1_000), 1_000);
        q = review_queue::enqueue(q, prep("new", "/rec/new.m4a", 5_000), 5_000);
        save_queue(&db, &q).await.unwrap();

        // now is two days past the newest entry.
        let now = 5_000 + 2 * 24 * 60 * 60 * 1_000;
        let listed = review_queue::read_with_age(&load_queue(&db).await.unwrap(), now);
        assert_eq!(listed[0].id, "new", "newest first");
        assert_eq!(listed[1].id, "old");
        assert!((listed[0].age_in_days - 2.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn mark_published_persists_the_status_transition() {
        let (db, _d) = temp_db().await;
        let q = review_queue::enqueue(Vec::new(), prep("rec-1", "/rec/a.m4a", 1_000), 1_000);
        save_queue(&db, &q).await.unwrap();

        let mut loaded = load_queue(&db).await.unwrap();
        assert!(review_queue::mark_published(&mut loaded, "rec-1", 2_000));
        save_queue(&db, &loaded).await.unwrap();

        let back = load_queue(&db).await.unwrap();
        assert_eq!(back[0].prep.status, EpisodePrepStatus::Published);

        // An unknown id is a no-op (no panic, returns false).
        let mut loaded = load_queue(&db).await.unwrap();
        assert!(!review_queue::mark_published(&mut loaded, "ghost", 3_000));
    }

    #[tokio::test]
    async fn mark_discarded_persists_the_status_transition() {
        let (db, _d) = temp_db().await;
        let q = review_queue::enqueue(Vec::new(), prep("rec-1", "/rec/a.m4a", 1_000), 1_000);
        save_queue(&db, &q).await.unwrap();

        let mut loaded = load_queue(&db).await.unwrap();
        assert!(review_queue::mark_discarded(&mut loaded, "rec-1", 2_000));
        save_queue(&db, &loaded).await.unwrap();

        assert_eq!(
            load_queue(&db).await.unwrap()[0].prep.status,
            EpisodePrepStatus::Discarded
        );
    }

    #[tokio::test]
    async fn load_queue_tolerates_a_corrupt_blob() {
        let (db, _d) = temp_db().await;
        // A non-array / malformed value must degrade to an empty queue, not error
        // (mirrors the `unwrap_or_default` in load_queue).
        store::set_setting(&db.pool, REVIEW_QUEUE_KEY, "not json at all")
            .await
            .unwrap();
        assert!(load_queue(&db).await.unwrap().is_empty());
    }

    // ── The three field patches (P3.1) ──────────────────────────────────────

    /// Seed a one-entry queue and hand back the db.
    async fn seeded_db() -> (Db, tempfile::TempDir) {
        let (db, dir) = temp_db().await;
        let q = review_queue::enqueue(Vec::new(), prep("rec-1", "/rec/a.m4a", 1_000), 1_000);
        save_queue(&db, &q).await.unwrap();
        (db, dir)
    }

    /// The persisted blob, verbatim — so a test can prove that NOTHING was
    /// written, not merely that the visible fields came back the same.
    async fn raw_blob(db: &Db) -> String {
        store::get_setting(&db.pool, REVIEW_QUEUE_KEY)
            .await
            .unwrap()
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn patching_an_unknown_id_reports_false_and_writes_nothing() {
        let (db, _d) = seeded_db().await;
        let before = raw_blob(&db).await;

        let changed = patch_prep(&db, "ghost", |p| p.master_preset = "music-rich".into())
            .await
            .unwrap();

        assert!(!changed, "an entry that is gone must not report success");
        // The whole point of the early return: a miss must not rewrite the blob
        // (which would bump nothing visibly but churn the store on every stray
        // dropdown change from an editor left open on a published episode).
        assert_eq!(raw_blob(&db).await, before);
    }

    #[tokio::test]
    async fn a_trim_patch_round_trips_and_bumps_updated_at() {
        let (db, _d) = seeded_db().await;
        let changed = patch_prep(&db, "rec-1", |p| {
            p.suggested_trim = Some(SuggestedTrim {
                start_sec: 12.5,
                end_sec: 1_800.0,
            })
        })
        .await
        .unwrap();
        assert!(changed);

        let back = &load_queue(&db).await.unwrap()[0].prep;
        let trim = back.suggested_trim.as_ref().expect("trim stored");
        assert_eq!(trim.start_sec, 12.5);
        assert_eq!(trim.end_sec, 1_800.0);
        // update_entry stamps the clock; created_at is immutable.
        assert!(back.updated_at >= back.created_at);
    }

    #[test]
    fn an_inverted_or_nan_trim_is_rejected_before_it_reaches_the_store() {
        assert!(trim_is_valid(&SuggestedTrim {
            start_sec: 0.0,
            end_sec: 1.0
        }));
        // Zero-length and backwards spans would produce an empty edit later.
        assert!(!trim_is_valid(&SuggestedTrim {
            start_sec: 10.0,
            end_sec: 10.0
        }));
        assert!(!trim_is_valid(&SuggestedTrim {
            start_sec: 20.0,
            end_sec: 10.0
        }));
        assert!(!trim_is_valid(&SuggestedTrim {
            start_sec: -1.0,
            end_sec: 10.0
        }));
        // NaN loses every comparison — the `!(end > start)` shape is what
        // catches it, so pin it.
        assert!(!trim_is_valid(&SuggestedTrim {
            start_sec: 0.0,
            end_sec: f64::NAN
        }));
        assert!(!trim_is_valid(&SuggestedTrim {
            start_sec: f64::NAN,
            end_sec: 10.0
        }));
    }

    #[tokio::test]
    async fn a_master_preset_patch_round_trips_including_the_blank_none() {
        let (db, _d) = seeded_db().await;
        assert!(
            patch_prep(&db, "rec-1", |p| p.master_preset = "music-rich".into())
                .await
                .unwrap()
        );
        assert_eq!(
            load_queue(&db).await.unwrap()[0].prep.master_preset,
            "music-rich"
        );

        // Blank is a real choice ("no mastering"), not a missing value.
        assert!(
            patch_prep(&db, "rec-1", |p| p.master_preset = String::new())
                .await
                .unwrap()
        );
        assert_eq!(load_queue(&db).await.unwrap()[0].prep.master_preset, "");
    }

    #[test]
    fn the_jingles_patch_tells_absent_from_null_from_a_value() {
        // The whole reason for the double Option. A plain Option<String> would
        // read the middle case as the first and silently ignore «Ingen».
        let absent: JinglesPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.intro_path, None);
        assert_eq!(absent.outro_path, None);

        let cleared: JinglesPatch = serde_json::from_str(r#"{"introPath":null}"#).unwrap();
        assert_eq!(cleared.intro_path, Some(None));
        assert_eq!(cleared.outro_path, None, "the untouched field stays absent");

        let set: JinglesPatch = serde_json::from_str(r#"{"outroPath":"/j/outro.mp3"}"#).unwrap();
        assert_eq!(set.outro_path, Some(Some("/j/outro.mp3".to_string())));
        assert_eq!(set.intro_path, None);
    }

    #[tokio::test]
    async fn a_partial_jingles_patch_leaves_the_other_jingle_alone() {
        let (db, _d) = seeded_db().await;
        // Start with both set, the way `prep_build_episode` would from defaults.
        patch_prep(&db, "rec-1", |p| {
            p.intro_path = Some("/j/intro.mp3".into());
            p.outro_path = Some("/j/outro.mp3".into());
        })
        .await
        .unwrap();

        // The editor's outro dropdown posts ONLY the outro — exactly the payload
        // that used to be swallowed by the `async () => true` stub.
        let patch: JinglesPatch = serde_json::from_str(r#"{"outroPath":null}"#).unwrap();
        let changed = patch_prep(&db, "rec-1", |p| {
            if let Some(intro) = patch.intro_path {
                p.intro_path = intro;
            }
            if let Some(outro) = patch.outro_path {
                p.outro_path = outro;
            }
        })
        .await
        .unwrap();
        assert!(changed);

        let back = &load_queue(&db).await.unwrap()[0].prep;
        assert_eq!(back.outro_path, None, "the named field was cleared");
        assert_eq!(
            back.intro_path.as_deref(),
            Some("/j/intro.mp3"),
            "the absent field must survive untouched"
        );
    }

    #[tokio::test]
    async fn reminder_action_dto_maps_every_channel_and_message() {
        use review_queue::{ReminderAction, ReminderChannel as C, ReminderMessage as M};
        let cases = [
            (C::Notify, M::Day1, "notify", "day1"),
            (C::NotifyEmail, M::Day2, "notify_email", "day2"),
            (
                C::NotifyEmailWebhook,
                M::Day7,
                "notify_email_webhook",
                "day7",
            ),
            (
                C::NotifyEmailWebhookWarning,
                M::Day7,
                "notify_email_webhook_warning",
                "day7",
            ),
            (C::AutoDiscard, M::Discard, "auto_discard", "discard"),
        ];
        for (channel, message, want_channel, want_message) in cases {
            let dto: ReminderActionDto = ReminderAction {
                id: "x".into(),
                channel,
                message,
            }
            .into();
            assert_eq!(dto.channel, want_channel);
            assert_eq!(dto.message, want_message);
            assert_eq!(dto.id, "x");
        }
    }
}
