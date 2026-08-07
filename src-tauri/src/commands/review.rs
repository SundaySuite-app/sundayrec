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
//!   running audio-analysis itself. The analysis IS ported now
//!   (`sundayrec_core::audio_analysis`, driven by `crate::editor::segments`), and
//!   E8 connected the two: every content-analysis pass that actually runs offers
//!   its recording to the queue through [`build_and_enqueue`]. Until then this
//!   command had no callers at all and the queue could only ever be empty — it
//!   worked perfectly and was never asked anything. The assembly + status
//!   decision ARE the unit-tested core.
//! - [`review_process_reminders`] returns the actions the queue's timeline wants
//!   fired, for a renderer that asks. It is no longer the only path: the hourly
//!   [`crate::notify::reminders`] tick runs the same core ladder and dispatches
//!   each action through the Phase-2 notify seam, so the reminders happen
//!   whether or not anybody has the app in front of them. NETWORK-UNVERIFIED
//!   below the decision, as ever.

use tauri::{AppHandle, State};

use sundayrec_core::integrations::stage::{self, StageManifest};
use sundayrec_core::integrations::{ChapterMarker, ServiceLink};
use sundayrec_core::prep::{self, EpisodePrep, PrepAnalysisSegment, PrepDefaults, SuggestedTrim};
use sundayrec_core::review_queue::{self, ReminderAction, ReviewQueueEntry};
use sundayrec_core::trim_feedback::{self, TrimDeltas};

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
///
/// **Path policy: [`PathPolicy::ReadOnlyMedia`]** over [`MEDIA_EXTENSIONS`].
/// The path is PERSISTED into the review-queue blob and later handed to the
/// mastering/export/publish chain, so an unguarded value is a stored capability
/// that outlives the call. Media-only + must exist is the narrowest rule that
/// still admits every episode (this is always a finished recording). Not
/// root-scoped: an episode may legitimately be prepped from a recording the
/// operator has already moved to an archive volume, and unlike cloud backup
/// nothing here leaves the machine on its own.
#[tauri::command]
pub async fn prep_build_episode(
    app: AppHandle,
    db: State<'_, Db>,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
) -> AppResult<EpisodePrep> {
    build_and_enqueue(&app, &db, recording_path, segments).await
}

/// The body of [`prep_build_episode`], reachable from inside the shell as well
/// as over IPC — [`crate::commands::editor::editor_segments`] offers every fresh
/// analysis pass to the queue through here, which is what makes the queue
/// populate at all (nothing had ever called the command).
///
/// **Idempotent on `recording_path`**, and that is the load-bearing part. The
/// same service arrives here repeatedly by design — a re-open that misses the
/// segments cache, an explicit «Analyser opptak», the next launch — and
/// [`review_queue::enqueue`] dedups by the prep's uuid, which is freshly minted
/// each time and therefore never matches. So the queue is asked whether it
/// already knows this FILE first, and an entry that exists is returned
/// unchanged: no second row, no reset of `added_at` (which would restart the
/// reminder ladder), and no resurrection of something the operator already
/// published or discarded.
///
/// Takes `&Db` rather than `State<Db>` so the non-command caller can pass a
/// plain reference; the command above is the thin `State` wrapper.
pub(crate) async fn build_and_enqueue(
    app: &AppHandle,
    db: &Db,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
) -> AppResult<EpisodePrep> {
    let (episode, added) = build_and_enqueue_inner(db, recording_path, segments).await?;
    // Nothing changed on a duplicate, so the tray has nothing new to say.
    if added {
        crate::tray_note_review_queue(app);
    }
    Ok(episode)
}

/// Like [`build_and_enqueue`], but silently declines a file the app did not
/// record. Returns the entry when there is one, `None` when the file was passed
/// over.
///
/// The distinction exists because the automatic caller is the editor's content
/// analysis, and the editor opens whatever the operator points it at — a
/// borrowed sermon, a jingle, last year's concert. Those are not episodes of
/// this church's service, and a review queue that fills with them is a queue
/// nobody trusts. A recording the app itself made has a `recording` history row;
/// nothing else does.
///
/// The explicit command has no such gate on purpose: an operator (or an
/// importer) asking for a specific file to be prepped has said what they want,
/// and this is a check on a guess, not on a request.
pub(crate) async fn build_and_enqueue_if_recorded(
    app: &AppHandle,
    db: &Db,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
) -> AppResult<Option<EpisodePrep>> {
    let Some((episode, added)) = enqueue_if_recorded_inner(db, recording_path, segments).await?
    else {
        return Ok(None);
    };
    if added {
        crate::tray_note_review_queue(app);
    }
    Ok(Some(episode))
}

/// The `AppHandle`-free half of [`build_and_enqueue_if_recorded`], so the gate
/// itself is testable. `None` = not a file this app recorded.
async fn enqueue_if_recorded_inner(
    db: &Db,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
) -> AppResult<Option<(EpisodePrep, bool)>> {
    if !store::recording_exists_for_path(&db.pool, &recording_path).await? {
        return Ok(None);
    }
    build_and_enqueue_inner(db, recording_path, segments)
        .await
        .map(Some)
}

/// The queue half of [`build_and_enqueue`], without the `AppHandle` the tray
/// note needs — an `AppHandle` cannot be constructed in a unit test, and the
/// idempotency rule above is exactly the part that has to be tested. Returns
/// whether a NEW entry was written (`false` = the recording was already known).
async fn build_and_enqueue_inner(
    db: &Db,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
) -> AppResult<(EpisodePrep, bool)> {
    crate::commands::path_guard::check(
        &recording_path,
        crate::commands::path_guard::PathPolicy::ReadOnlyMedia(
            crate::commands::path_guard::MEDIA_EXTENSIONS,
        ),
    )?;

    let queue = load_queue(db).await?;
    if let Some(existing) = review_queue::find_by_recording_path(&queue, &recording_path) {
        return Ok((existing.prep.clone(), false));
    }

    let defaults = prep_defaults(db).await?;
    let now = now_i64();
    let episode = prep::build_episode_prep(new_id(), recording_path, segments, &defaults, now);

    let queue = review_queue::enqueue(queue, episode.clone(), now);
    save_queue(db, &queue).await?;
    Ok((episode, true))
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
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::ReviewPublished);
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
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::ReviewDiscarded);
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

/// Store a revised sermon trim on a queued prep, and record how far it moved
/// from what the analysis proposed.
///
/// `trim` is the renderer's `{ startSec, endSec }`. Rejected outright when it is
/// not a forward, non-negative span (see [`trim_is_valid`]): a stored
/// `endSec <= startSec` would make the next review-mode open produce an empty
/// edit with no visible cause.
///
/// ## Why the deltas are measured against the SEGMENTS, not `suggested_trim`
///
/// The obvious anchor is the entry's current `suggested_trim` — and it is the
/// wrong one, because this very command overwrites it. The first adjustment
/// would measure correctly; a second would measure against the operator's own
/// first adjustment and report a near-zero correction of a detector that was
/// never consulted. `analysis_segments` is immutable for the life of the entry,
/// so re-deriving the proposal from it gives the same answer every time,
/// however often the operator changes their mind.
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

    // Read the proposal inside the patch closure: `patch_prep` already holds the
    // entry there, and a separate load would race its own write.
    let mut feedback: Option<(String, TrimDeltas)> = None;
    let changed = patch_prep(&db, &id, |p| {
        let proposed = proposed_trim(p);
        if let Some(deltas) = trim_feedback::trim_deltas(proposed, trim) {
            feedback = Some((p.recording_path.clone(), deltas));
        }
        p.suggested_trim = Some(trim);
    })
    .await?;

    if changed {
        // An operator who opened review and published without touching the
        // boundaries taught us nothing about them — and a corpus where the
        // untouched majority outvotes the corrections would tune the detector
        // toward whatever it already does. Those deltas are nonetheless handed
        // over rather than filtered out here: an operator who dragged a boundary
        // BACK onto the proposal looks identical from this side, and the
        // difference — withdrawing the adjustment recorded earlier — can only be
        // seen by the layer that holds the file. Neither case is stored.
        if let Some((path, deltas)) = feedback {
            crate::learning::record_trim_deltas(&path, deltas);
        }
        crate::tray_note_review_queue(&app);
    }
    Ok(changed)
}

/// Re-derive the sermon span the analysis originally proposed for this prep,
/// from the segments it was built from. `None` when the detector found no
/// sermon block — there is then no boundary for an operator to have corrected.
fn proposed_trim(p: &EpisodePrep) -> Option<SuggestedTrim> {
    let duration_sec = prep::derive_duration_sec(&p.analysis_segments);
    prep::find_sermon_segment(&p.analysis_segments, duration_sec).map(|s| SuggestedTrim {
        start_sec: s.start_sec,
        end_sec: s.end_sec,
    })
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
/// the reminder actions it produced.
///
/// The app does NOT rely on this being called: [`crate::notify::reminders`]
/// runs the same core ladder on its own hourly timer and dispatches what comes
/// back. This command remains for a renderer that wants to force a sweep — and
/// is safe to call alongside the tick, because `process_reminders` is idempotent
/// within a threshold window (an action fires only when `reminded` crosses a NEW
/// rung, and the bump is persisted here too).
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

    // ── Automatic population: idempotent on the recording (E8) ──────────────

    /// A real media file the path guard will accept, plus the tempdir keeping
    /// it alive. `ReadOnlyMedia` requires a media extension AND existence, so a
    /// bare string path would fail the guard before the queue logic runs.
    fn media_file(dir: &tempfile::TempDir, name: &str) -> String {
        let p = dir.path().join(name);
        std::fs::write(&p, b"not really audio").unwrap();
        p.to_string_lossy().into_owned()
    }

    /// One 30-minute speech block starting 6 minutes in — a segment list the
    /// sermon heuristic actually accepts, so the prep carries a real proposal.
    fn sermon_segments() -> Vec<PrepAnalysisSegment> {
        vec![PrepAnalysisSegment {
            start_sec: 360.0,
            end_sec: 2160.0,
            duration_sec: 1800.0,
            kind: sundayrec_core::prep::SegmentType::Speech,
            confidence: 0.9,
            avg_rms_db: -20.0,
            label: String::new(),
        }]
    }

    #[tokio::test]
    async fn a_fresh_analysis_puts_the_recording_in_the_queue() {
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");

        let (episode, added) = build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        assert!(added);
        assert_eq!(episode.recording_path, path);
        assert_eq!(load_queue(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn re_analysing_the_same_recording_does_not_queue_it_twice() {
        // «Analyser opptak» forces a second pass over a file already queued.
        // `enqueue`'s id-dedup cannot catch this — the second prep has a new
        // uuid — so the path guard above it is what keeps the queue honest.
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");

        let (first, _) = build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        let (second, added) = build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();

        assert!(!added, "the second pass must not add a row");
        assert_eq!(load_queue(&db).await.unwrap().len(), 1);
        assert_eq!(
            second.id, first.id,
            "the caller must get the entry that already exists, not a new one"
        );
    }

    #[tokio::test]
    async fn re_analysis_does_not_restart_the_reminder_ladder() {
        // `added_at` drives the 24 h → 48 h → 7 d → auto-discard timeline. A
        // re-analysis that reset it would let an episode dodge its reminders
        // indefinitely, one editor visit at a time.
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");

        build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        let added_at = load_queue(&db).await.unwrap()[0].added_at;

        build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        assert_eq!(load_queue(&db).await.unwrap()[0].added_at, added_at);
    }

    #[tokio::test]
    async fn a_published_recording_does_not_come_back() {
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");

        let (episode, _) = build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        let mut q = load_queue(&db).await.unwrap();
        assert!(review_queue::mark_published(&mut q, &episode.id, 2_000));
        save_queue(&db, &q).await.unwrap();

        let (_, added) = build_and_enqueue_inner(&db, path, sermon_segments())
            .await
            .unwrap();
        assert!(!added, "a published episode must not be re-queued");
        let back = load_queue(&db).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].prep.status, EpisodePrepStatus::Published);
    }

    #[tokio::test]
    async fn a_discarded_recording_does_not_come_back() {
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");

        let (episode, _) = build_and_enqueue_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        let mut q = load_queue(&db).await.unwrap();
        assert!(review_queue::mark_discarded(&mut q, &episode.id, 2_000));
        save_queue(&db, &q).await.unwrap();

        let (_, added) = build_and_enqueue_inner(&db, path, sermon_segments())
            .await
            .unwrap();
        assert!(!added, "a discarded episode must not be re-queued");
        let back = load_queue(&db).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].prep.status, EpisodePrepStatus::Discarded);
    }

    #[tokio::test]
    async fn two_different_recordings_both_get_in() {
        let (db, d) = temp_db().await;
        let a = media_file(&d, "morning.m4a");
        let b = media_file(&d, "evening.m4a");

        build_and_enqueue_inner(&db, a, sermon_segments())
            .await
            .unwrap();
        build_and_enqueue_inner(&db, b, sermon_segments())
            .await
            .unwrap();
        assert_eq!(load_queue(&db).await.unwrap().len(), 2);
    }

    /// Register `path` as something this app recorded, so the gate lets it in.
    async fn as_recorded(db: &Db, path: &str) {
        store::insert_recording(
            &db.pool,
            store::RecordingRow {
                id: String::new(),
                file_path: path.to_string(),
                device_name: None,
                started_at: 0.0,
                duration_ms: None,
                byte_size: None,
                created_at: 0.0,
                note: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_file_the_app_never_recorded_is_passed_over() {
        // The editor opens borrowed sermons, jingles and last year's concert.
        // None of those is an episode of this church's service.
        let (db, d) = temp_db().await;
        let path = media_file(&d, "someone-elses-sermon.m4a");

        let got = enqueue_if_recorded_inner(&db, path, sermon_segments())
            .await
            .unwrap();
        assert!(got.is_none());
        assert!(load_queue(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_file_the_app_recorded_gets_in() {
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");
        as_recorded(&db, &path).await;

        let got = enqueue_if_recorded_inner(&db, path, sermon_segments())
            .await
            .unwrap();
        assert!(matches!(got, Some((_, true))));
        assert_eq!(load_queue(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_gate_is_still_idempotent_on_re_analysis() {
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");
        as_recorded(&db, &path).await;

        enqueue_if_recorded_inner(&db, path.clone(), sermon_segments())
            .await
            .unwrap();
        let second = enqueue_if_recorded_inner(&db, path, sermon_segments())
            .await
            .unwrap();
        assert!(
            matches!(second, Some((_, false))),
            "a re-analysis must report the existing entry, not a new one"
        );
        assert_eq!(load_queue(&db).await.unwrap().len(), 1);
    }

    // ── Trim feedback: deltas against the analysis, not the last edit ────────

    #[tokio::test]
    async fn the_proposal_is_re_derived_from_the_segments_every_time() {
        // The regression this guards: measuring against `suggested_trim` would
        // make a SECOND adjustment read as a correction of the operator's own
        // first one, and the detector — the thing being tuned — would never
        // appear in the numbers again.
        let (db, d) = temp_db().await;
        let path = media_file(&d, "service.m4a");
        let (episode, _) = build_and_enqueue_inner(&db, path, sermon_segments())
            .await
            .unwrap();

        let proposed = proposed_trim(&episode).expect("these segments yield a sermon");
        assert_eq!(proposed.start_sec, 360.0);

        // Simulate the first adjustment having already been stored.
        let moved = SuggestedTrim {
            start_sec: 400.0,
            end_sec: 2160.0,
        };
        patch_prep(&db, &episode.id, |p| p.suggested_trim = Some(moved))
            .await
            .unwrap();

        let after = load_queue(&db).await.unwrap()[0].prep.clone();
        assert_eq!(after.suggested_trim, Some(moved));
        assert_eq!(
            proposed_trim(&after),
            Some(proposed),
            "the proposal must survive the operator overwriting the trim"
        );
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
