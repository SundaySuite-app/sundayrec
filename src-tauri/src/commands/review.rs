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

use tauri::State;

use sundayrec_core::integrations::stage::{self, StageManifest};
use sundayrec_core::integrations::{ChapterMarker, ServiceLink};
use sundayrec_core::prep::{self, EpisodePrep, PrepAnalysisSegment, PrepDefaults};
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

async fn load_queue(db: &Db) -> AppResult<Vec<ReviewQueueEntry>> {
    match store::get_setting(&db.pool, REVIEW_QUEUE_KEY).await? {
        Some(json) if !json.is_empty() => Ok(serde_json::from_str(&json).unwrap_or_default()),
        _ => Ok(Vec::new()),
    }
}

async fn save_queue(db: &Db, entries: &[ReviewQueueEntry]) -> AppResult<()> {
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
    Ok(episode)
}

// ── Review queue ──────────────────────────────────────────────────────────

/// The review queue, newest-first, with `ageInDays` filled in.
#[tauri::command]
pub async fn review_queue_list(db: State<'_, Db>) -> AppResult<Vec<ReviewQueueEntry>> {
    let queue = load_queue(&db).await?;
    Ok(review_queue::read_with_age(&queue, now_i64()))
}

/// Mark a queued prep published (kept briefly for the UI toast).
#[tauri::command]
pub async fn review_mark_published(db: State<'_, Db>, id: String) -> AppResult<bool> {
    let mut queue = load_queue(&db).await?;
    let ok = review_queue::mark_published(&mut queue, &id, now_i64());
    if ok {
        save_queue(&db, &queue).await?;
    }
    Ok(ok)
}

/// Mark a queued prep discarded ("ikke publiser denne uka").
#[tauri::command]
pub async fn review_mark_discarded(db: State<'_, Db>, id: String) -> AppResult<bool> {
    let mut queue = load_queue(&db).await?;
    let ok = review_queue::mark_discarded(&mut queue, &id, now_i64());
    if ok {
        save_queue(&db, &queue).await?;
    }
    Ok(ok)
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
