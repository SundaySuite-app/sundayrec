//! Building the payload — the gather half of the telemetry client.
//!
//! Everything reported here ALREADY EXISTS on the machine, written by earlier
//! phases for the operator's own benefit: E2.1's crash ring, the recorder's
//! telemetry history, the `wake_failure` table, the diagnose findings. This
//! module reads those, projects them through
//! [`sundayrec_core::telemetry`]'s wire types (which is where the privacy
//! guarantee lives) and assembles one [`TelemetryPayload`].
//!
//! ## Watermarks, and why they start at "now"
//!
//! Each source has a watermark: the timestamp of the newest record already
//! reported. A drain takes what is strictly newer and advances it. That makes
//! the drain idempotent and its schedule irrelevant to correctness — running it
//! at startup, on a timer, or twice by accident all produce the same result.
//!
//! The watermarks are initialised to the CURRENT time when consent is granted,
//! which is a deliberate privacy choice rather than an implementation detail. A
//! machine that has been recording for two years has two years of crash records
//! and quality history sitting on disk. Consent given today is consent to share
//! what happens from today; back-filling the archive would technically be
//! answering the same question, but it is not what the person said yes to.
//!
//! ## Nothing here probes anything
//!
//! Building a payload must never run ffmpeg, open a device or touch the network.
//! Diagnose FINDINGS are therefore not produced here — they are cached by
//! `run_diagnostics` when the operator runs a diagnose themselves, and this
//! module only drains that cache. A telemetry system that starts a capture probe
//! to have something to report is a telemetry system that changes what it
//! measures.

use std::path::Path;

use sqlx::SqlitePool;

use sundayrec_core::selftest::RecordingTelemetry;
use sundayrec_core::telemetry::{
    crash_report, finding_report, quality_report, sanitize_language, wake_failure_report,
    CorrectionReport, CounterReport, FindingReport, TelemetryPayload, NIL_INSTALL_ID,
};

use crate::db::store;
use crate::error::AppResult;

/// The `app_setting` key holding the per-source watermarks.
pub const KEY_WATERMARKS: &str = "telemetry.watermarks";

/// The `app_setting` key holding diagnose findings waiting to be reported. A
/// LIST, not a snapshot: two diagnoses between drains are two data points.
pub const KEY_PENDING_FINDINGS: &str = "telemetry.pendingFindings";

/// How many finding rows are cached between drains. Well above the number of
/// distinct `SR-*` codes, so a normal run is never truncated, and far below
/// anything that could bloat the settings row if drains stop happening.
pub const MAX_PENDING_FINDINGS: usize = 60;

/// The newest already-reported record per source, in unix ms (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Watermarks {
    pub crash_at: i64,
    pub quality_at: i64,
    pub wake_at: i64,
}

impl Watermarks {
    /// The starting point when consent is granted: everything up to now is
    /// already-existing local history and is not reported. See the module docs.
    pub fn starting_now(now_ms: i64) -> Self {
        Self {
            crash_at: now_ms,
            quality_at: now_ms,
            wake_at: now_ms,
        }
    }
}

/// Read the watermarks. A missing or malformed row reads as all-zero, which
/// would report the whole archive — so callers only reach this AFTER consent has
/// been granted, and granting writes [`Watermarks::starting_now`] first.
pub async fn watermarks(pool: &SqlitePool) -> AppResult<Watermarks> {
    let raw = store::get_setting(pool, KEY_WATERMARKS).await?;
    Ok(raw
        .and_then(|v| serde_json::from_str::<Watermarks>(&v).ok())
        .unwrap_or_default())
}

/// Persist the watermarks.
pub async fn set_watermarks(pool: &SqlitePool, w: Watermarks) -> AppResult<()> {
    store::set_setting(pool, KEY_WATERMARKS, &serde_json::to_string(&w)?).await
}

/// Cache diagnose findings for the next drain. Called by `run_diagnostics`, and
/// ONLY when consent is active — a diagnose run by someone who has not opted in
/// leaves nothing behind.
pub async fn record_findings(
    pool: &SqlitePool,
    findings: &[sundayrec_core::diagnostics::DiagnosticFinding],
) -> AppResult<()> {
    let mut pending = pending_findings(pool).await?;
    pending.extend(findings.iter().filter_map(finding_report));
    if pending.len() > MAX_PENDING_FINDINGS {
        pending.drain(..pending.len() - MAX_PENDING_FINDINGS);
    }
    store::set_setting(
        pool,
        KEY_PENDING_FINDINGS,
        &serde_json::to_string(&pending)?,
    )
    .await
}

/// The cached findings, oldest first. A malformed row reads as empty.
pub async fn pending_findings(pool: &SqlitePool) -> AppResult<Vec<FindingReport>> {
    let raw = store::get_setting(pool, KEY_PENDING_FINDINGS).await?;
    Ok(raw
        .and_then(|v| serde_json::from_str::<Vec<FindingReport>>(&v).ok())
        .unwrap_or_default())
}

/// Drop the cached findings (after they have been queued).
pub async fn clear_pending_findings(pool: &SqlitePool) -> AppResult<()> {
    store::set_setting(pool, KEY_PENDING_FINDINGS, "[]").await
}

/// Everything a payload build needs that is not in the database.
pub struct GatherContext<'a> {
    /// `<app-data>` — the crash ring and the telemetry history live under it.
    pub app_data_dir: &'a Path,
    /// The running build's version.
    pub app_version: &'a str,
    /// The user's home directory, for the free-text scrubbers.
    pub home: Option<&'a str>,
    /// Unix ms (UTC) now.
    pub now_ms: i64,
    /// The consent scope version this payload is collected under.
    pub consent_version: u32,
    /// The counter snapshot to include (E3.4 fills this; empty means none).
    pub counters: Vec<CounterReport>,
    /// The banded-correction snapshot to include (E8; empty means none).
    ///
    /// Passed in rather than read here for the same reason the counters are:
    /// these are accumulated in memory as corrections are made, not gathered
    /// from disk at build time. `telemetry::corrections`'s module docs give the
    /// argument — the short form is that a correction record has no timestamp,
    /// by design, so there is no watermark that could make a sidecar sweep
    /// idempotent, and it would re-report the same corrections every drain.
    pub corrections: Vec<CorrectionReport>,
}

/// Build the payload for everything newer than `since`, returning it alongside
/// the watermarks a successful enqueue should store.
///
/// Pure with respect to the database: it reads, it never writes. The caller
/// decides whether the result is worth queueing
/// ([`TelemetryPayload::is_empty`]) and only then advances the watermarks.
pub async fn build(
    pool: &SqlitePool,
    ctx: &GatherContext<'_>,
    since: Watermarks,
    install_id: Option<&str>,
) -> AppResult<(TelemetryPayload, Watermarks)> {
    let settings = crate::settings::load(pool).await?;
    let mut next = since;

    let mut payload = TelemetryPayload::new(
        install_id.unwrap_or(NIL_INSTALL_ID),
        ctx.consent_version,
        ctx.app_version,
        ctx.now_ms,
    );
    payload.language = sanitize_language(settings.language.as_deref());
    payload.settings = sundayrec_core::telemetry::WireSettings::from_settings(&settings);
    payload.counters = ctx.counters.clone();
    payload.corrections = ctx.corrections.clone();

    // ── Crashes + supervised restarts (E2.1/E2.2's rings) ────────────────────
    let crash_dir = ctx.app_data_dir.join("crashes");
    let mut records = crate::crash::read_crashes(&crash_dir);
    records.extend(crate::crash::read_restarts(&crash_dir));
    for r in &records {
        let Some(at) = rfc3339_to_millis(&r.timestamp) else {
            continue;
        };
        if at <= since.crash_at {
            continue;
        }
        next.crash_at = next.crash_at.max(at);
        payload.crashes.push(crash_report(
            &r.kind,
            at,
            &r.app_version,
            &r.os,
            &r.message,
            r.location.as_deref(),
            r.task.as_deref(),
            r.backtrace.is_some(),
            ctx.home,
        ));
    }
    payload.crashes.sort_by_key(|c| c.at);

    // ── Recording quality (the recorder's rolling history) ────────────────────
    for t in read_quality_history(ctx.app_data_dir) {
        let Some(at) = rfc3339_to_millis(&t.timestamp) else {
            continue;
        };
        if at <= since.quality_at {
            continue;
        }
        next.quality_at = next.quality_at.max(at);
        payload.quality.push(quality_report(&t, at));
    }
    payload.quality.sort_by_key(|q| q.at);

    // ── Wake failures ────────────────────────────────────────────────────────
    for e in store::list_wake_failures(pool).await? {
        if e.timestamp <= since.wake_at {
            continue;
        }
        next.wake_at = next.wake_at.max(e.timestamp);
        payload.wake_failures.push(wake_failure_report(&e));
    }
    payload.wake_failures.sort_by_key(|w| w.at);

    // ── Diagnose findings (drained from the cache, never probed) ──────────────
    payload.findings = pending_findings(pool).await?;

    payload.truncate_to_caps();
    Ok((payload, next))
}

/// The recorder's rolling telemetry history, newest last. A missing or
/// unreadable file is an empty history, not an error — nothing about telemetry
/// is worth failing a startup for.
fn read_quality_history(app_data_dir: &Path) -> Vec<RecordingTelemetry> {
    std::fs::read_to_string(app_data_dir.join("recording-telemetry-history.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<RecordingTelemetry>>(&s).ok())
        .unwrap_or_default()
}

/// Parse the LOCAL RFC 3339 timestamps the crash ring and the telemetry history
/// write into unix milliseconds (UTC).
///
/// The conversion is the whole point: the local string carries a UTC offset,
/// which narrows an anonymous install towards a timezone. An `i64` of
/// milliseconds says when, and nothing else.
fn rfc3339_to_millis(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::open_pool;
    use sundayrec_core::telemetry::CounterName;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn ctx<'a>(app_data_dir: &'a Path, counters: Vec<CounterReport>) -> GatherContext<'a> {
        GatherContext {
            app_data_dir,
            app_version: "0.10.0",
            home: Some("/Users/kari"),
            now_ms: 1_800_000_000_000,
            consent_version: 1,
            counters,
            corrections: Vec::new(),
        }
    }

    /// Write one crash record into the ring by hand (the real writer is private
    /// to `crash`, and a test must not panic a thread to produce one).
    fn write_crash(app_data_dir: &Path, name: &str, timestamp: &str, message: &str) {
        let dir = app_data_dir.join("crashes");
        std::fs::create_dir_all(&dir).unwrap();
        let json = serde_json::json!({
            "schema": 1,
            "kind": "panic",
            "timestamp": timestamp,
            "appVersion": "0.9.0",
            "os": "macos",
            "arch": "aarch64",
            "thread": "main",
            "message": message,
            "location": "src/recorder/engine.rs:412:9",
            "backtrace": "0: sundayrec::x at /Users/kari/dev/x.rs:9",
            "task": null,
        });
        std::fs::write(dir.join(name), serde_json::to_vec(&json).unwrap()).unwrap();
    }

    fn write_history(app_data_dir: &Path, records: &[RecordingTelemetry]) {
        std::fs::write(
            app_data_dir.join("recording-telemetry-history.json"),
            serde_json::to_vec(records).unwrap(),
        )
        .unwrap();
    }

    fn telemetry_at(timestamp: &str, drops: u64) -> RecordingTelemetry {
        RecordingTelemetry {
            drops,
            duration_sec: 3600.0,
            timestamp: timestamp.to_string(),
            exit_ok: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_fresh_machine_builds_an_empty_payload() {
        let (pool, dir) = temp_pool().await;
        let (p, next) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert!(
            p.is_empty(),
            "nothing has happened, so there is nothing to send"
        );
        assert_eq!(next, Watermarks::default());
        assert_eq!(
            p.install_id, NIL_INSTALL_ID,
            "no install id is invented for a payload that is only being previewed"
        );
    }

    #[tokio::test]
    async fn crashes_and_quality_newer_than_the_watermark_are_collected() {
        let (pool, dir) = temp_pool().await;
        write_crash(
            dir.path(),
            "crash-000000000000001-0000.json",
            "2026-08-01T12:00:00+02:00",
            "gammel krasj",
        );
        write_crash(
            dir.path(),
            "crash-000000000000002-0000.json",
            "2026-08-05T12:00:00+02:00",
            "ny krasj",
        );
        write_history(
            dir.path(),
            &[
                telemetry_at("2026-08-01T13:00:00+02:00", 1),
                telemetry_at("2026-08-05T13:00:00+02:00", 9),
            ],
        );

        let cutoff = rfc3339_to_millis("2026-08-03T00:00:00+02:00").unwrap();
        let since = Watermarks {
            crash_at: cutoff,
            quality_at: cutoff,
            wake_at: cutoff,
        };
        let (p, next) = build(&pool, &ctx(dir.path(), vec![]), since, Some("x"))
            .await
            .unwrap();

        assert_eq!(p.crashes.len(), 1, "only the crash after the watermark");
        assert_eq!(p.crashes[0].message, "ny krasj");
        assert!(p.crashes[0].backtrace_present, "the ring HAS one…");
        let text = serde_json::to_string(&p).unwrap();
        assert!(
            !text.contains("sundayrec::x"),
            "…but the backtrace itself is never on the wire"
        );

        assert_eq!(p.quality.len(), 1);
        assert_eq!(p.quality[0].drops, 9);
        assert!(!p.is_empty());
        assert!(next.crash_at > cutoff && next.quality_at > cutoff);
    }

    #[tokio::test]
    async fn a_drain_is_idempotent_at_its_own_watermark() {
        let (pool, dir) = temp_pool().await;
        write_crash(
            dir.path(),
            "crash-000000000000001-0000.json",
            "2026-08-05T12:00:00+02:00",
            "krasj",
        );
        let (first, next) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert_eq!(first.crashes.len(), 1);

        // Re-running with the advanced watermark reports nothing new — so the
        // drain's SCHEDULE is irrelevant to correctness.
        let (second, after) = build(&pool, &ctx(dir.path(), vec![]), next, None)
            .await
            .unwrap();
        assert!(second.crashes.is_empty());
        assert_eq!(after, next);
    }

    #[tokio::test]
    async fn granting_consent_does_not_back_fill_the_archive() {
        // Two years of local history, consent granted today: the payload is
        // empty, because "yes" applies to what happens next.
        let (pool, dir) = temp_pool().await;
        write_crash(
            dir.path(),
            "crash-000000000000001-0000.json",
            "2024-03-01T12:00:00+01:00",
            "arkivert krasj",
        );
        write_history(dir.path(), &[telemetry_at("2024-03-01T13:00:00+01:00", 5)]);

        let now = 1_800_000_000_000i64;
        let (p, _) = build(
            &pool,
            &ctx(dir.path(), vec![]),
            Watermarks::starting_now(now),
            Some("x"),
        )
        .await
        .unwrap();
        assert!(p.crashes.is_empty());
        assert!(p.quality.is_empty());
        assert!(p.is_empty());
    }

    #[tokio::test]
    async fn wake_failures_are_projected_without_their_labels() {
        use sundayrec_core::wake::{WakeFailureEntry, WakeFailureKind};
        let (pool, dir) = temp_pool().await;
        store::insert_wake_failure(
            &pool,
            &WakeFailureEntry {
                timestamp: 1_799_000_000_000,
                scheduled_at: "2026-08-09T11:00:00+02:00".into(),
                kind: WakeFailureKind::Missed,
                label: "Gudstjeneste Nordstrand".into(),
                reason: Some("no_resume".into()),
                delta_sec: None,
            },
        )
        .await
        .unwrap();

        let (p, next) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert_eq!(p.wake_failures.len(), 1);
        assert_eq!(p.wake_failures[0].reason.as_deref(), Some("no_resume"));
        assert_eq!(next.wake_at, 1_799_000_000_000);
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("Nordstrand"), "{text}");
        assert!(!text.contains("11:00"), "the service time is not reported");
    }

    #[tokio::test]
    async fn findings_are_drained_from_the_cache_not_probed() {
        use sundayrec_core::diagnostics::{DiagnosticFinding, DiagnosticSeverity};
        let (pool, dir) = temp_pool().await;
        // A finding whose DETAIL names a device — the detail must not travel.
        let finding = DiagnosticFinding {
            code: "SR-AUDIO-02".into(),
            severity: DiagnosticSeverity::Warning,
            title: "Valgt lydenhet finnes ikke".into(),
            detail: "«Kari sin Qu-5» ble ikke funnet".into(),
            hint: "Velg enheten på nytt".into(),
        };
        record_findings(&pool, std::slice::from_ref(&finding))
            .await
            .unwrap();

        let (p, _) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert_eq!(p.findings.len(), 1);
        assert_eq!(p.findings[0].code, "SR-AUDIO-02");
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("Qu-5"), "{text}");
        assert!(!text.contains("ble ikke funnet"), "{text}");

        clear_pending_findings(&pool).await.unwrap();
        let (after, _) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert!(after.findings.is_empty());
    }

    #[tokio::test]
    async fn the_finding_cache_is_bounded() {
        use sundayrec_core::diagnostics::{DiagnosticFinding, DiagnosticSeverity};
        let (pool, _d) = temp_pool().await;
        let batch: Vec<DiagnosticFinding> = (0..10)
            .map(|i| DiagnosticFinding {
                code: format!("SR-TEST-{i:02}"),
                severity: DiagnosticSeverity::Info,
                title: String::new(),
                detail: String::new(),
                hint: String::new(),
            })
            .collect();
        for _ in 0..20 {
            record_findings(&pool, &batch).await.unwrap();
        }
        assert_eq!(
            pending_findings(&pool).await.unwrap().len(),
            MAX_PENDING_FINDINGS
        );
    }

    #[tokio::test]
    async fn the_counter_snapshot_is_carried_through() {
        let (pool, dir) = temp_pool().await;
        let counters = vec![CounterReport {
            name: CounterName::EditorOpened,
            value: 4,
        }];
        let (p, _) = build(
            &pool,
            &ctx(dir.path(), counters),
            Watermarks::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(p.counters.len(), 1);
        assert_eq!(p.counters[0].value, 4);
        assert!(!p.is_empty(), "a non-zero counter is worth sending");
    }

    #[tokio::test]
    async fn the_banded_corrections_are_carried_through_and_carry_no_seconds() {
        use sundayrec_core::telemetry::corrections::{
            CorrectionBand, CorrectionDirection, CorrectionKey, CorrectionSignal,
        };
        let (pool, dir) = temp_pool().await;
        let mut c = ctx(dir.path(), vec![]);
        c.corrections = vec![CorrectionReport::new(
            CorrectionKey {
                signal: CorrectionSignal::SermonStart,
                direction: CorrectionDirection::Earlier,
                band: CorrectionBand::From30To60s,
            },
            2,
        )];

        let (p, _) = build(&pool, &c, Watermarks::default(), Some("x"))
            .await
            .unwrap();
        assert_eq!(p.corrections.len(), 1);
        assert_eq!(p.corrections[0].count, 2);
        assert!(
            !p.is_empty(),
            "a person having corrected us is worth sending on its own"
        );

        // What the band is FOR: the movement's size does not travel.
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains("30_60s"), "{text}");
        assert!(!text.contains("deltaSec"), "{text}");
        assert!(!text.contains("startDeltaSec"), "{text}");
    }

    #[tokio::test]
    async fn the_payload_is_capped_even_when_the_sources_are_not() {
        let (pool, dir) = temp_pool().await;
        let history: Vec<RecordingTelemetry> = (0..40)
            .map(|i| telemetry_at(&format!("2026-08-05T12:{i:02}:00+02:00"), i as u64))
            .collect();
        write_history(dir.path(), &history);
        let (p, _) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert_eq!(p.quality.len(), sundayrec_core::telemetry::MAX_QUALITY);
        assert_eq!(
            p.quality.last().unwrap().drops,
            39,
            "the newest records survive the cap"
        );
    }

    #[tokio::test]
    async fn unreadable_sources_are_an_empty_payload_not_an_error() {
        let (pool, dir) = temp_pool().await;
        std::fs::write(
            dir.path().join("recording-telemetry-history.json"),
            b"{not json",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("crashes")).unwrap();
        std::fs::write(dir.path().join("crashes/crash-1.json"), b"{broken").unwrap();
        store::set_setting(&pool, KEY_PENDING_FINDINGS, "nonsense")
            .await
            .unwrap();

        let (p, _) = build(&pool, &ctx(dir.path(), vec![]), Watermarks::default(), None)
            .await
            .unwrap();
        assert!(p.is_empty());
    }

    #[test]
    fn local_timestamps_become_utc_milliseconds() {
        // The same instant in two timezones must produce the same number — that
        // is what dropping the offset means.
        let oslo = rfc3339_to_millis("2026-08-05T14:00:00+02:00").unwrap();
        let utc = rfc3339_to_millis("2026-08-05T12:00:00+00:00").unwrap();
        assert_eq!(oslo, utc);
        assert_eq!(rfc3339_to_millis("not a timestamp"), None);
        assert_eq!(rfc3339_to_millis(""), None);
    }
}
