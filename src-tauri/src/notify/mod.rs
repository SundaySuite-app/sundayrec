//! The notification DISPATCH seam — one place a failure or a warning is told to
//! everybody who should hear it.
//!
//! ## Why this module exists
//!
//! Before it, each source of trouble picked its own audience by hand, and the
//! picks were wrong in ways nobody could see from any single file:
//!
//!   - the recorder's terminal `recording://error` reached the tray badge and
//!     the renderer, and nothing else — the "send me an e-mail when the
//!     recording fails" setting had **no caller at all**, in any build;
//!   - the scheduler's three start failures fired a native notification only,
//!     so an operator who had gone home learned nothing.
//!
//! Everything now goes through [`dispatch_failure`] (terminal failures →
//! native + e-mail, over SMTP or the relay) or [`warn`] (degradations → an
//! in-app toast). Which channels actually fire is the unit-tested
//! [`sundayrec_core::notify`] matrix, not an `if` written at the call site. (A
//! third leg, a chat webhook, lived here until the sharing cluster was removed —
//! it is in git if it is ever wanted.)
//!
//! [`dispatch_receipt`] is the one dispatch that is NOT about trouble: a
//! scheduled recording finished, and the volunteer who was not in the building
//! would like to know. It has its own matrix ([`plan_receipt`]) because the
//! failure table's first row is "always fire a native notification", which is
//! exactly wrong for news the person watching the app just watched happen.
//!
//! ## Featureless on purpose
//!
//! This module compiles in EVERY build. The `email` feature (in `default`, and
//! in both release feature lists) gates only the send itself: with
//! `--no-default-features` the routing still runs, the matrix is told the
//! feature is absent, and the e-mail leg is planned `false` — a clean
//! degradation to native only rather than a compile error or a silent lie.
//!
//! ## Observational, never invasive
//!
//! Nothing here is called from a capture path. Recorder failures arrive on the
//! events the engine ALREADY emits (`app.listen`, exactly like the tray does);
//! the warning sources are back-off branches and post-hoc skips. The engine's
//! hardware-verified start/stop code has no idea this module exists.
//!
//! ## ⚠️ NETWORK-UNVERIFIED
//!
//! Behind the feature, the SMTP send does real I/O. The decisions above it are
//! pure and tested; the wire is provable only against a real server — see
//! docs/SMOKE-TEST.md.

use chrono::{DateTime, Local};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Listener, Manager};

use sundayrec_core::email::{
    render_error, render_missed, render_receipt, MailLang, MissedOccurrence, RelayMessage,
    RelayMessageKind, RenderedEmail,
};
use sundayrec_core::notify::{
    alert_church, alert_date_format, alert_person, plan_failure, plan_receipt, BackendWarning,
    FailureRouting, FailureSource, ReceiptRouting,
};
use sundayrec_core::relay::{seen_decision, SeenScope};
use sundayrec_core::settings::Settings;
use sundayrec_core::telemetry::{sanitize_free_text, MESSAGE_MAX_CHARS};

use crate::db::Db;
use crate::error::AppResult;
use crate::util::now_ms;

pub mod disk;
/// The e-mail relay (A2): outbox, pump, endpoint and the local subscription
/// record. A3 wired the legs that FEED it — [`dispatch_failure`] gathers the
/// five relay facts beside the two SMTP ones, and [`dispatch_receipt`] is a
/// relay-only leg of its own.
pub mod relay;

pub use sundayrec_core::notify::code;

use relay::config::RelayEndpoint;
use relay::{store as relay_store, wire, RelaySubscription};

/// The event the renderer's `backend-warning` channel is mapped to. Follows the
/// `scheme://name` convention every other Rust-emitted event uses
/// (`recording://…`, `scheduler://…`, `tray://…`).
pub const WARNING_EVENT: &str = "backend://warning";

/// The stable code a [`FailureSource::Missed`] dispatch carries. Not one of
/// [`code`]'s renderer-facing warning codes: those name a live degradation the
/// toast localises, and this names the absence of a recording, which reaches the
/// operator as a native notification and a mail rather than a toast.
pub const CODE_SCHEDULED_MISSED: &str = "scheduled_missed";

/// One scheduled occurrence behind a [`FailureSource::Missed`] dispatch.
///
/// Two time fields, deliberately. [`Self::at`] is the machine's — the ISO-like
/// local string `check_missed` already produces, and half of the durable
/// `notify_seen` key. [`Self::date`] is the human's, formatted in the mail
/// language. Keeping them apart is what stops a volunteer who switches the app
/// from Norwegian to English from being told about the same missed Sunday twice,
/// which is exactly what a ledger keyed on the printed date would do.
#[derive(Debug, Clone)]
pub struct MissedSlot {
    /// ISO-like local start (`YYYY-MM-DDTHH:MM:SS`).
    pub at: String,
    /// The schedule's own name for the slot ("Ukentlig opptak (11:00–13:00)").
    pub label: String,
    /// The already-localized human date/time the mail prints.
    pub date: String,
}

impl MissedSlot {
    /// This occurrence's row in the `notify_seen` ledger, under
    /// [`SeenScope::Missed`].
    ///
    /// The label is HASHED rather than spelled out: a special recording's name
    /// is something a person typed ("Bryllup Kari og Ola"), and a ledger that
    /// keeps names is a second place a name lives for no gain — the key only
    /// ever has to be compared with itself. `at` stays legible because a
    /// timestamp is the one part somebody debugging this actually needs to read.
    pub fn seen_key(&self) -> String {
        format!("{}:{}", self.at, short_hash(&self.label))
    }
}

/// Everything [`dispatch_failure`] needs to know about a failure.
#[derive(Debug, Clone)]
pub struct FailureCtx {
    /// Stable machine code (the recorder's `RecordingEvent::code`, or one of the
    /// scheduler's). Logged with the dispatch so a device drop-out can be told
    /// from a disk stop at a glance.
    pub code: String,
    /// The human sentence. This is ALSO the native notification body and the
    /// e-mail's error line, which is why the scheduler's existing wording is
    /// passed through verbatim rather than re-derived here.
    pub message: String,
    /// Which half of the app failed.
    pub source: FailureSource,
    /// When it happened (local time) — the e-mail's human date.
    pub occurred_at: DateTime<Local>,
    /// The occurrences behind a [`FailureSource::Missed`] dispatch, OLDEST
    /// FIRST. Empty for every other source.
    ///
    /// Why a field on the context rather than a second dispatch function: the
    /// native leg, the SMTP leg and the throttle are identical to a failure's,
    /// and the news is the same news ("Sunday was not recorded"). Only the
    /// relay's rendering differs — [`render_missed`] writes one mail listing
    /// however many Sundays the sweep found, which [`render_error`] has no shape
    /// for.
    pub missed: Vec<MissedSlot>,
}

impl FailureCtx {
    /// A failure that just happened.
    pub fn now(code: impl Into<String>, message: impl Into<String>, source: FailureSource) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            source,
            occurred_at: Local::now(),
            missed: Vec::new(),
        }
    }

    /// Attach the occurrences a [`FailureSource::Missed`] dispatch is about.
    pub fn with_missed(mut self, missed: Vec<MissedSlot>) -> Self {
        self.missed = missed;
        self
    }
}

/// Sixteen hex digits of SHA-256 — enough that two different labels colliding
/// is not a thing that happens, short enough that a `notify_seen` key stays
/// readable in a `sqlite3` session.
fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(s.as_bytes())
        .iter()
        .take(8)
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The `notify_seen` key for a repeatable failure: the code plus a hash of the
/// exact sentence.
///
/// The same pair [`sundayrec_core::email::AlertGate`] throttles SMTP on, minus
/// the recipient — which is implicit here, because a machine has at most one
/// relay subscription. The two therefore suppress the same repeats, one in RAM
/// for the ten-minute window and one in a table that survives a restart.
fn failure_seen_key(code: &str, message: &str) -> String {
    format!("{code}:{}", short_hash(message))
}

/// Fire a native OS notification. The one channel no setting can silence: the
/// person standing at the machine is the only one who can still save the
/// service. Previously private to the scheduler — the recorder had no way to
/// reach it at all.
///
/// Generic over the runtime because the tray and the quit path are too: the
/// tray's «Avslutt» and the app menu's Quit both reach the notification through
/// `crate::window`, and a concrete `AppHandle` there would force the runtime
/// parameter out of those call sites for no gain.
pub fn native<R: tauri::Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notify: native notification failed: {e}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Failures
// ─────────────────────────────────────────────────────────────────────────────

/// Subscribe the dispatcher to the failure events the app ALREADY emits.
///
/// Exactly the seam the tray uses (`tray::wire_state_sources` listens to this
/// same [`ERROR_EVENT`](crate::recorder::engine::ERROR_EVENT) six lines of code
/// away): `app.listen` is observational, so the recorder's hardware-verified
/// capture and stop code is not touched, not even by one line. That matters more
/// here than anywhere — every regression this app has shipped in the recorder
/// came from editing the capture path for a reason that turned out to be a
/// reporting reason.
///
/// The scheduler's failures do NOT arrive here: they are not events, they are
/// return values, so those three call sites invoke [`dispatch_failure`] directly.
///
/// Call once, from `setup`.
pub fn wire_failure_sources(app: &AppHandle) {
    use crate::recorder::engine::{RecordingEvent, RecordingFinished, ERROR_EVENT, FINISHED_EVENT};

    let handle = app.clone();
    app.listen(ERROR_EVENT, move |ev| {
        let Ok(e) = serde_json::from_str::<RecordingEvent>(ev.payload()) else {
            tracing::warn!("notify: unparseable {ERROR_EVENT} payload — no alert sent");
            return;
        };
        // The listener callback is synchronous and runs on the event loop; the
        // dispatch does database + network I/O, so it has to leave immediately.
        let app = handle.clone();
        tauri::async_runtime::spawn(async move {
            dispatch_failure(
                &app,
                FailureCtx::now(e.code, e.message, FailureSource::Recording),
            )
            .await;
        });
    });

    // The receipt rides the SAME observational seam, one event over. The engine
    // already emits `recording://finished` for the record→edit hand-off; A3 adds
    // a second listener to it rather than a hook inside the engine, so the
    // hardware-verified stop path is not touched by one line — the rule this
    // module's header states, and the reason the failure leg was built this way.
    let handle = app.clone();
    app.listen(FINISHED_EVENT, move |ev| {
        let Ok(f) = serde_json::from_str::<RecordingFinished>(ev.payload()) else {
            tracing::warn!("notify: unparseable {FINISHED_EVENT} payload — no receipt sent");
            return;
        };
        let app = handle.clone();
        tauri::async_runtime::spawn(async move {
            dispatch_receipt(&app, &f.file_path).await;
        });
    });

    // The graduated low-disk observer rides on the same observational seam.
    disk::wire(app);

    tracing::info!("notify: failure dispatch wired to {ERROR_EVENT}, receipts to {FINISHED_EVENT}");
}

/// Tell everybody who should hear about a terminal failure.
///
/// Always fires the native notification; adds the e-mail alert when the settings
/// ask for one AND this build can send AND a transport exists AND the throttle
/// gate hasn't already sent this exact alert in the last ten minutes. The
/// channel choice itself is [`sundayrec_core::notify::plan_failure`].
///
/// Never returns an error: an alert path that can itself fail loudly is a second
/// failure on top of the first. Every leg logs and moves on.
pub async fn dispatch_failure(app: &AppHandle, ctx: FailureCtx) {
    // 1. The native leg first — it needs nothing but the app handle, so it still
    //    fires if the database is unreachable (which is itself a failure mode
    //    the operator would want to hear about).
    native(app, "SundayRec", &ctx.message);

    // A recording that DIED cannot also have finished: drop the scheduler's
    // marker so the receipt leg has nothing to report. Only for the recorder's
    // own terminal error — a scheduler failure means the recording never started
    // (there is no marker), and a `Missed` sweep may have just late-started one
    // (whose marker must survive).
    if ctx.source == FailureSource::Recording {
        if let Some(marker) = app.try_state::<crate::scheduler::ScheduledRunMarker>() {
            if marker.take().is_some() {
                tracing::info!("notify: the scheduled run failed — no receipt will be sent");
            }
        }
    }

    let Some(db) = app.try_state::<Db>() else {
        tracing::warn!(
            code = %ctx.code,
            "notify: no database yet — failure alerted natively only"
        );
        return;
    };
    let settings = crate::settings::load(&db.pool).await.unwrap_or_default();

    // 2. Ask the throttle gate BEFORE planning, so "already mailed" is part of
    //    the tested matrix rather than a surprise deep inside the send.
    let recipient = settings.email_address.trim().to_string();
    let throttled = email_throttled(app, &recipient, &ctx.message);
    let transport_ready = email_transport_ready(&settings, &recipient);

    // 3. …and the relay's five, gathered right beside them so the matrix decides
    //    on one complete picture. The subscription record, the durable ledger and
    //    the compiled-in endpoint are three reads that cost nothing on a machine
    //    that never enrolled: `subscription_get` returns `None` and everything
    //    downstream shuts.
    let scrubbed = scrub(&ctx.message);
    let (scope, seen_key) = failure_ledger_key(&ctx, &scrubbed);
    let facts = RelayFacts::gather(&db.pool, scope, &seen_key).await;

    let plan = plan_failure(&FailureRouting {
        email_on_error: settings.email_on_error,
        email_recipient: &recipient,
        email_feature_built: cfg!(feature = "email"),
        email_transport_ready: transport_ready,
        email_throttled: throttled,
        relay_enrolled: facts.gate.enrolled,
        relay_confirmed: facts.gate.confirmed,
        relay_suppressed: facts.gate.suppressed,
        relay_throttled: facts.throttled,
        relay_endpoint_built: facts.endpoint.is_some(),
    });

    tracing::info!(
        code = %ctx.code,
        source = %ctx.source.as_str(),
        email = plan.email,
        relay = plan.relay,
        "notify: dispatching failure"
    );

    if plan.email {
        send_failure_email(app, &settings, &recipient, &ctx).await;
    }
    if plan.relay {
        // The matrix planned the leg on `enrolled && endpoint_built`, so both
        // halves are present; the `let else` is the compiler's price for saying
        // that in types the matrix does not carry.
        let (Some(sub), Some(endpoint)) = (facts.subscription.as_ref(), facts.endpoint.as_ref())
        else {
            tracing::warn!("notify: the relay leg was planned without a subscription — skipped");
            return;
        };
        let lang = MailLang::from_code(settings.language.as_deref());
        let church = scrub(alert_church(&settings.church_name));
        let person = scrub(&alert_person(&settings.responsible_person, &sub.address));

        // ONE rendering choice, on the source. A missed sweep lists occurrences;
        // everything else has a single error line.
        let occurrences: Vec<MissedOccurrence> = ctx
            .missed
            .iter()
            .map(|m| MissedOccurrence {
                label: m.label.as_str(),
                date: m.date.as_str(),
            })
            .collect();
        let rendered = if occurrences.is_empty() {
            let date = ctx.occurred_at.format(alert_date_format(lang)).to_string();
            render_error(lang, &church, &person, &date, &scrubbed)
        } else {
            // `None` is unreachable — the slice is non-empty — but a renderer
            // that returns an Option is one that has told us "nothing to say",
            // and unwrapping that in an alert path is how a panic gets shipped.
            let Some(r) = render_missed(lang, &church, &person, &occurrences) else {
                return;
            };
            r
        };

        let dedup_key = failure_dedup_key(&ctx, &scrubbed);
        let kind = if occurrences.is_empty() {
            RelayMessageKind::Failure
        } else {
            RelayMessageKind::Missed
        };
        if let Err(e) = send_relay_message(
            &db.pool, sub, endpoint, kind, lang, rendered, scope, &seen_key, dedup_key,
        )
        .await
        {
            tracing::warn!("notify: could not queue the relay alert: {e}");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The relay leg
// ─────────────────────────────────────────────────────────────────────────────

/// The relay's five facts for ONE dispatch, read together.
struct RelayFacts {
    /// The local subscription record, or `None` on a machine that never
    /// enrolled.
    subscription: Option<RelaySubscription>,
    /// Its gate — enrolled / confirmed / suppressed — with the no-record default
    /// already applied.
    gate: sundayrec_core::relay::RelayGate,
    /// The durable `notify_seen` verdict for this exact event.
    throttled: bool,
    /// The endpoint this build can reach, or `None` when the URL or the write
    /// key was never configured.
    endpoint: Option<RelayEndpoint>,
}

impl RelayFacts {
    /// Read all five. Never fails: a database error here must not turn one
    /// failure into two, so an unreadable record reads as "not enrolled" and an
    /// unreadable ledger reads as "not yet said" — quiet in the first case, one
    /// possible repeat in the second, and a repeat is the better of those.
    async fn gather(pool: &SqlitePool, scope: SeenScope, key: &str) -> Self {
        let subscription = relay_store::subscription_get(pool)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("notify: could not read the relay subscription: {e}");
                None
            });
        let last_seen = relay_store::seen_get(pool, scope, key)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("notify: could not read the notify_seen ledger: {e}");
                None
            });
        Self {
            gate: relay::gate_of(subscription.as_ref()),
            subscription,
            throttled: seen_decision(scope, last_seen, now_ms()),
            endpoint: RelayEndpoint::resolve(),
        }
    }
}

/// Which ledger row this dispatch is keyed on.
///
/// A missed sweep keys on its OLDEST occurrence, under [`SeenScope::Missed`] —
/// the same key `check_missed` filters on and stamps afterwards, so the two
/// cannot disagree about whether a Sunday has been reported. Everything else is
/// a repeatable failure under [`SeenScope::Failure`], where the ledger is a
/// ten-minute window rather than a full stop.
fn failure_ledger_key(ctx: &FailureCtx, scrubbed_message: &str) -> (SeenScope, String) {
    match ctx.missed.first() {
        Some(oldest) => (SeenScope::Missed, oldest.seen_key()),
        None => (
            SeenScope::Failure,
            failure_seen_key(&ctx.code, scrubbed_message),
        ),
    }
}

/// The outbox's unique key for this alert — the belt to the ledger's braces.
///
/// STABLE for a given event, not stamped with a clock: two observers racing to
/// report the same failure must produce one row, and the unique index is the
/// only thing that holds when both read the ledger before either writes it. A
/// repeat that is genuinely new ten minutes later still queues, because the
/// delivered row has left the table by then.
fn failure_dedup_key(ctx: &FailureCtx, scrubbed_message: &str) -> String {
    match ctx.missed.first() {
        // Every occurrence in the sweep, so a sweep that finds a SECOND missed
        // Sunday is a different mail from the one that found only the first.
        Some(oldest) => format!(
            "missed:{}:{}",
            oldest.at,
            short_hash(
                &ctx.missed
                    .iter()
                    .map(MissedSlot::seen_key)
                    .collect::<Vec<_>>()
                    .join("|")
            )
        ),
        None => format!("failure:{}", failure_seen_key(&ctx.code, scrubbed_message)),
    }
}

/// Scrub free text on its way into a relay mail.
///
/// Exactly what `email::tests::relay_bodies_pass_the_endpoints_own_validator`
/// proves is enough: the endpoint runs its `ABSOLUTE_PATH_RE` over subject, text
/// AND html and answers `400 unscrubbed_path`, which the outbox reads as
/// permanent and DROPS — so an unscrubbed church name would not delay an alert,
/// it would delete it. The scrub happens BEFORE rendering, on every free-text
/// input, because a template can interpolate its argument into three places and
/// only one of them is easy to remember.
fn scrub(raw: &str) -> String {
    sanitize_free_text(
        raw,
        crate::telemetry::home_dir().as_deref(),
        MESSAGE_MAX_CHARS,
    )
}

/// Render's last mile: footer, size guard, queue, ledger, doorbell.
///
/// The ledger is stamped when the row is QUEUED, not when it is delivered — the
/// second observer of a missed Sunday looks before the first row has left, and
/// a sighting recorded on delivery would be a sighting recorded too late.
#[allow(clippy::too_many_arguments)]
async fn send_relay_message(
    pool: &SqlitePool,
    sub: &RelaySubscription,
    endpoint: &RelayEndpoint,
    kind: RelayMessageKind,
    lang: MailLang,
    rendered: RenderedEmail,
    scope: SeenScope,
    seen_key: &str,
    dedup_key: String,
) -> AppResult<()> {
    let message = RelayMessage::new(
        kind,
        lang,
        rendered,
        &endpoint.unsubscribe_url(&sub.sub_id, &sub.unsub_token),
    );
    let queued = wire::queue_send(pool, &sub.sub_id, kind, &message, dedup_key).await?;
    // Stamped whether or not a row was added: `false` means an identical one is
    // already waiting, which is the same "we have said this" the ledger records.
    relay_store::seen_mark(pool, scope, seen_key, now_ms()).await?;
    if queued {
        relay::sender::kick();
    }
    tracing::info!(
        kind = kind.as_str(),
        queued,
        "notify: relay message handed to the outbox"
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//   Receipts
// ─────────────────────────────────────────────────────────────────────────────

/// Tell the volunteer that the SCHEDULED recording they were not there for
/// finished.
///
/// Three gates, and the first is not in the matrix because the matrix cannot see
/// it: **the recording has to have been a scheduled one**. The marker
/// ([`crate::scheduler::ScheduledRunMarker`]) is stamped by the scheduler after
/// a successful start and consumed here, so a volunteer who pressed Start and
/// watched the app finish gets nothing — they were standing there.
///
/// Taking the marker is unconditional and happens FIRST, before the settings are
/// even read. A marker left behind by a run whose receipt was switched off would
/// be claimed by the next MANUAL recording, which is precisely the mail this
/// function exists not to send.
pub async fn dispatch_receipt(app: &AppHandle, file_path: &str) {
    let Some(marker) = app.try_state::<crate::scheduler::ScheduledRunMarker>() else {
        return;
    };
    let Some(run) = marker.take() else {
        tracing::debug!("notify: a manual recording finished — no receipt");
        return;
    };
    let Some(db) = app.try_state::<Db>() else {
        return;
    };
    let settings = crate::settings::load(&db.pool).await.unwrap_or_default();

    // The run's own moment plus its slot — one row per finished scheduled
    // recording, which under `SeenScope::Receipt` is once and for all.
    let seen_key = format!(
        "{}:{}",
        run.started_at.format("%Y-%m-%dT%H:%M:%S"),
        short_hash(&run.slot)
    );
    let facts = RelayFacts::gather(&db.pool, SeenScope::Receipt, &seen_key).await;
    let plan = plan_receipt(&ReceiptRouting {
        receipt_enabled: settings.email_receipt_enabled,
        relay_enrolled: facts.gate.enrolled,
        relay_confirmed: facts.gate.confirmed,
        relay_suppressed: facts.gate.suppressed,
        relay_throttled: facts.throttled,
        relay_endpoint_built: facts.endpoint.is_some(),
    });
    tracing::info!(relay = plan.relay, "notify: dispatching receipt");
    if !plan.relay {
        return;
    }
    let (Some(sub), Some(endpoint)) = (facts.subscription.as_ref(), facts.endpoint.as_ref()) else {
        return;
    };

    let lang = MailLang::from_code(settings.language.as_deref());
    let church = scrub(alert_church(&settings.church_name));
    let person = scrub(&alert_person(&settings.responsible_person, &sub.address));
    let slot = scrub(&run.slot);
    let started = run.started_at.format(alert_date_format(lang)).to_string();
    let duration_secs = (Local::now() - run.started_at).num_seconds().max(0) as u64;

    // `file_path` is handed over WHOLE and deliberately unscrubbed:
    // `render_receipt` reduces it to its basename itself, which is the stronger
    // guarantee — a folder that is never assembled cannot leak, where a scrubber
    // leaves the tail of any file name containing a space.
    let rendered = render_receipt(
        lang,
        &church,
        &person,
        &slot,
        &started,
        duration_secs,
        file_path,
    );
    if let Err(e) = send_relay_message(
        &db.pool,
        sub,
        endpoint,
        RelayMessageKind::Receipt,
        lang,
        rendered,
        SeenScope::Receipt,
        &seen_key,
        format!("receipt:{seen_key}"),
    )
    .await
    {
        tracing::warn!("notify: could not queue the receipt: {e}");
    }
}

/// Whether the [`crate::email::AlertGate`] would suppress this alert as a repeat.
/// `false` in a build without the feature (there is no gate, and the matrix will
/// have switched the e-mail leg off anyway).
#[cfg_attr(not(feature = "email"), allow(unused_variables))]
fn email_throttled(app: &AppHandle, recipient: &str, error_message: &str) -> bool {
    #[cfg(feature = "email")]
    {
        use sundayrec_core::email::AlertDecision;
        let Some(gate) = app.try_state::<crate::email::AlertGateState>() else {
            // Not managed (shouldn't happen — lib.rs manages it) — better to
            // attempt the send than to silently swallow the first real alert.
            tracing::warn!("notify: AlertGateState not managed; skipping the throttle check");
            return false;
        };
        matches!(
            gate.decide(recipient, error_message, crate::util::now_ms()),
            AlertDecision::Throttled
        )
    }
    #[cfg(not(feature = "email"))]
    {
        false
    }
}

/// Whether a mail transport could be assembled from the saved configuration:
/// an SMTP host WITH a password (typed once into the keychain) and a resolvable
/// `From:`. Without one there is nothing to send *with*, however willing the
/// settings are — and reporting that up front keeps the matrix honest instead of
/// making the failure surface as a cryptic transport error.
#[cfg_attr(not(feature = "email"), allow(unused_variables))]
fn email_transport_ready(settings: &Settings, recipient: &str) -> bool {
    #[cfg(feature = "email")]
    {
        build_transport(settings, recipient).is_some()
    }
    #[cfg(not(feature = "email"))]
    {
        false
    }
}

/// Assemble the transport the unattended alert will use. Mirrors
/// `email_send_test`'s resolution exactly — the same password precedence
/// ([`crate::commands::email::resolve_smtp_password`], with NO request-side
/// value: an unattended alert has nobody to type one) and the same `From:`
/// precedence ([`crate::commands::email::resolve_from_address`]) — so "Send
/// test" working is a real prediction that the 3 a.m. alert will work too.
///
/// A blank SMTP host means no transport at all.
#[cfg(feature = "email")]
fn build_transport(settings: &Settings, recipient: &str) -> Option<crate::email::Transport> {
    use crate::commands::email::{resolve_from_address, resolve_smtp_password};
    use crate::email::Transport;
    use crate::secrets::SecretProvider;

    let host = settings.email_smtp.trim();
    if host.is_empty() {
        return None;
    }
    let user = Some(settings.email_smtp_user.clone()).filter(|u| !u.trim().is_empty());
    // No request-side password: nobody is at the keyboard. The keychain is
    // the only source, which is exactly why P1 added the write path.
    let pass = resolve_smtp_password(None, crate::secrets::get(SecretProvider::SmtpPassword))?;
    let from = resolve_from_address(Some(&settings.email_smtp_from), user.as_deref(), recipient)?;
    Some(Transport::Smtp {
        host: host.to_string(),
        port: settings.email_smtp_port.clamp(1, 65_535) as u16,
        user,
        pass,
        from,
    })
}

/// Send the failure alert. Compiled out (a no-op) without the `email` feature —
/// the matrix will never plan the leg in such a build, and this keeps the module
/// itself featureless.
#[cfg_attr(not(feature = "email"), allow(unused_variables))]
async fn send_failure_email(
    app: &AppHandle,
    settings: &Settings,
    recipient: &str,
    ctx: &FailureCtx,
) {
    #[cfg(feature = "email")]
    {
        let Some(gate) = app.try_state::<crate::email::AlertGateState>() else {
            tracing::warn!("notify: AlertGateState not managed; e-mail alert skipped");
            return;
        };
        let Some(transport) = build_transport(settings, recipient) else {
            // `email_transport_ready` already said yes; a race with the user
            // clearing the keychain is the only way here.
            tracing::warn!("notify: no mail transport at send time; e-mail alert skipped");
            return;
        };

        let lang = MailLang::from_code(settings.language.as_deref());
        let date = ctx.occurred_at.format(alert_date_format(lang)).to_string();
        let person = alert_person(&settings.responsible_person, recipient);

        match crate::email::send_error_alert(
            &gate,
            &transport,
            recipient,
            settings.language.as_deref(),
            alert_church(&settings.church_name),
            &person,
            &date,
            &ctx.message,
        )
        .await
        {
            Ok(true) => tracing::info!("notify: failure alert e-mailed"),
            // The gate re-decides inside `send_error_alert`; a `false` here just
            // means the window closed between our check and the send.
            Ok(false) => tracing::info!("notify: e-mail alert suppressed by the throttle gate"),
            Err(e) => tracing::warn!("notify: e-mail alert failed: {e}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Warnings
// ─────────────────────────────────────────────────────────────────────────────

/// Raise a live backend warning: log it and emit it to the renderer (which
/// localises on [`BackendWarning::code`] and toasts it).
///
/// Synchronous by design so it can be called from anywhere — including the
/// `&mut`-heavy back-off branches of the pre-roll loop. The event goes out
/// immediately, because a warning must never make the thing it is warning
/// about slower.
pub fn warn(app: &AppHandle, w: BackendWarning) {
    tracing::warn!(code = %w.code, msg = ?w.msg, "notify: backend warning");
    if let Err(e) = app.emit(WARNING_EVENT, &w) {
        tracing::warn!("notify: could not emit {WARNING_EVENT}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warning_event_follows_the_projects_scheme_convention() {
        // The renderer's EVENT_MAP maps its legacy `backend-warning` channel to
        // exactly this string; a rename here without one there re-creates the
        // very "live consumer, no emitter" gap this phase closed.
        assert_eq!(WARNING_EVENT, "backend://warning");
        assert!(WARNING_EVENT.contains("://"));
    }

    #[test]
    fn a_failure_context_timestamps_itself() {
        let before = Local::now();
        let ctx = FailureCtx::now(
            "device_disconnected",
            "Enheten forsvant",
            FailureSource::Recording,
        );
        assert_eq!(ctx.code, "device_disconnected");
        assert_eq!(ctx.message, "Enheten forsvant");
        assert_eq!(ctx.source, FailureSource::Recording);
        assert!(ctx.occurred_at >= before);
    }

    /// The listener registered by [`wire_failure_sources`] sees JSON, not the
    /// struct the engine emitted. A serde rename, an added `#[serde(rename_all)]`
    /// or a wrapper on either side would turn every recorder failure back into
    /// no alert at all — silently, because a failed parse is exactly what "no
    /// recorder failures happened" looks like. Pin the round-trip.
    #[test]
    fn the_listener_parses_exactly_what_the_engine_emits() {
        use crate::recorder::engine::{RecordingEvent, ERROR_EVENT};

        // What `engine::emit_error` puts on the wire, verbatim.
        let emitted = serde_json::to_string(&RecordingEvent {
            code: "device_disconnected".into(),
            message: "Lydenheten forsvant under opptak.".into(),
        })
        .expect("the engine's payload must serialise");

        // What the listener does with it.
        let parsed: RecordingEvent =
            serde_json::from_str(&emitted).expect("the listener must be able to parse it");
        let ctx = FailureCtx::now(parsed.code, parsed.message, FailureSource::Recording);

        assert_eq!(ctx.code, "device_disconnected");
        assert_eq!(ctx.message, "Lydenheten forsvant under opptak.");
        assert_eq!(ctx.source, FailureSource::Recording);
        // And the event we subscribe to is the terminal one, not the warning.
        assert_eq!(ERROR_EVENT, "recording://error");
    }

    /// A default (never-configured) settings row must never report a ready
    /// transport — in EITHER build. Without the feature the answer is a constant
    /// `false` (the degradation path); with it, the SMTP branch bails on the
    /// blank host before it can reach the keychain.
    ///
    /// Deliberately does not assert the positive case: a `true` needs a real
    /// keychain entry, and an unauthorised keychain read BLOCKS on an OS prompt
    /// (see `secrets::tests`) — it would hang the headless gate.
    #[test]
    fn an_unconfigured_settings_row_has_no_transport_to_send_with() {
        let settings = Settings::default();
        assert!(!email_transport_ready(&settings, "vakt@kirka.no"));

        // …and the matrix therefore plans the e-mail leg off even with the
        // switch on and a recipient set — this is the whole degradation story.
        let plan = plan_failure(&FailureRouting {
            email_on_error: true,
            email_recipient: "vakt@kirka.no",
            email_feature_built: cfg!(feature = "email"),
            email_transport_ready: email_transport_ready(&settings, "vakt@kirka.no"),
            email_throttled: false,
            ..FailureRouting::default()
        });
        assert!(!plan.email);
        assert!(!plan.relay, "and no relay subscription to fall back on");
        assert!(plan.native, "the native leg survives every degradation");
    }

    // ── A3: the relay leg ────────────────────────────────────────────────────

    use relay::RelaySubscriptionState;
    use sundayrec_core::email::ALERT_THROTTLE_MS;
    use sundayrec_core::relay::RelayKind;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn an_endpoint() -> RelayEndpoint {
        RelayEndpoint::normalize(
            Some("https://notify.sundaysuite.app".into()),
            Some("write-key".into()),
        )
        .expect("a well-formed endpoint")
    }

    fn a_subscription(state: RelaySubscriptionState) -> RelaySubscription {
        RelaySubscription {
            sub_id: "018f3a2b-7c4d-7e1f-9a2b-3c4d5e6f7a8b".into(),
            address: "frivillig@kirka.no".into(),
            state,
            enrolled_at: 1_000,
            confirmed_at: Some(2_000),
            last_checked: None,
            unsub_token: "u".repeat(64),
        }
    }

    async fn enrol(pool: &SqlitePool, state: RelaySubscriptionState) -> RelaySubscription {
        let sub = a_subscription(state);
        relay_store::subscription_set(pool, &sub)
            .await
            .expect("store the record");
        sub
    }

    /// A failure ctx with the missed occurrences attached, as `check_missed`
    /// builds one.
    fn missed_ctx(slots: Vec<MissedSlot>) -> FailureCtx {
        FailureCtx::now(
            CODE_SCHEDULED_MISSED,
            "Planlagt opptak ble ikke gjort.",
            FailureSource::Missed,
        )
        .with_missed(slots)
    }

    fn a_slot(at: &str, label: &str) -> MissedSlot {
        MissedSlot {
            at: at.into(),
            label: label.into(),
            date: "06.09.2026 11:00".into(),
        }
    }

    /// The whole relay leg with no subscription: three reads, one verdict, and
    /// nothing that could reach a socket.
    ///
    /// This is A2's "panic sender behind a shut gate" restated at the DISPATCH,
    /// which is where the leg is now decided. A machine that never enrolled must
    /// not queue a row — not because the pump would refuse to send it (it would),
    /// but because a row that exists is a row somebody has to reason about.
    #[tokio::test]
    async fn without_a_subscription_the_relay_leg_is_dark() {
        let (pool, _d) = temp_pool().await;
        let facts = RelayFacts::gather(&pool, SeenScope::Failure, "device_missing:abc").await;
        assert!(!facts.gate.enrolled);
        assert!(facts.subscription.is_none());
        assert!(!facts.throttled, "an empty ledger has said nothing");

        let plan = plan_failure(&FailureRouting {
            email_on_error: true,
            relay_enrolled: facts.gate.enrolled,
            relay_confirmed: facts.gate.confirmed,
            relay_suppressed: facts.gate.suppressed,
            relay_throttled: facts.throttled,
            // Even with an endpoint compiled in, which is the harder case.
            relay_endpoint_built: true,
            ..FailureRouting::default()
        });
        assert!(!plan.relay);
        assert!(plan.native, "and the operator is still told");
        assert!(
            relay_store::load_queue(&pool).await.unwrap().is_empty(),
            "no subscription, no row"
        );
    }

    /// An UNCONFIRMED address is enrolled and still gets nothing. The double
    /// opt-in as a property of the dispatch, not only of the pump.
    #[tokio::test]
    async fn an_unconfirmed_address_never_reaches_the_relay_leg() {
        let (pool, _d) = temp_pool().await;
        enrol(&pool, RelaySubscriptionState::Pending).await;
        let facts = RelayFacts::gather(&pool, SeenScope::Failure, "k").await;
        assert!(facts.gate.enrolled && !facts.gate.confirmed);
        assert!(
            !plan_failure(&FailureRouting {
                email_on_error: true,
                relay_enrolled: facts.gate.enrolled,
                relay_confirmed: facts.gate.confirmed,
                relay_endpoint_built: true,
                ..FailureRouting::default()
            })
            .relay
        );
    }

    /// THE throttle, end to end through the door: the same failure twice inside
    /// the ten-minute window is ONE queued row.
    ///
    /// Two independent mechanisms have to hold for that, and this asserts both —
    /// the durable ledger (which the matrix reads, so the second dispatch is
    /// never planned) and the outbox's unique dedup key (which holds even if two
    /// observers read the ledger before either wrote it).
    #[tokio::test]
    async fn the_same_failure_twice_in_ten_minutes_is_one_queued_row() {
        let (pool, _d) = temp_pool().await;
        let sub = enrol(&pool, RelaySubscriptionState::Confirmed).await;
        let endpoint = an_endpoint();
        let ctx = FailureCtx::now(
            "device_disconnected",
            "Lydenheten forsvant under opptak.",
            FailureSource::Recording,
        );
        let scrubbed = scrub(&ctx.message);
        let (scope, key) = failure_ledger_key(&ctx, &scrubbed);
        assert_eq!(scope, SeenScope::Failure);

        for _ in 0..2 {
            let rendered =
                render_error(MailLang::No, "Kirka", "Ola", "06.09.2026 11:00", &scrubbed);
            send_relay_message(
                &pool,
                &sub,
                &endpoint,
                RelayMessageKind::Failure,
                MailLang::No,
                rendered,
                scope,
                &key,
                failure_dedup_key(&ctx, &scrubbed),
            )
            .await
            .expect("the door accepts it");
        }
        let q = relay_store::load_queue(&pool).await.unwrap();
        assert_eq!(q.len(), 1, "the unique dedup key absorbed the second");
        assert_eq!(q[0].kind, RelayKind::Send);

        // …and the matrix would not even have planned the second, because the
        // ledger now says this exact failure went out a moment ago.
        let facts = RelayFacts::gather(&pool, scope, &key).await;
        assert!(facts.throttled);
        assert!(
            !plan_failure(&FailureRouting {
                email_on_error: true,
                relay_enrolled: facts.gate.enrolled,
                relay_confirmed: facts.gate.confirmed,
                relay_throttled: facts.throttled,
                relay_endpoint_built: true,
                ..FailureRouting::default()
            })
            .relay
        );

        // The window is a window, not a full stop: the same drop-out next Sunday
        // is news again.
        let seen = relay_store::seen_get(&pool, scope, &key).await.unwrap();
        assert!(!seen_decision(
            scope,
            seen,
            now_ms() + ALERT_THROTTLE_MS + 1
        ));
    }

    /// A missed sweep keys on the ledger row `check_missed` filters and stamps,
    /// so the two halves cannot disagree about whether a Sunday was reported.
    #[test]
    fn a_missed_dispatch_is_keyed_on_its_oldest_occurrence() {
        let oldest = a_slot("2026-09-06T11:00:00", "Ukentlig opptak (11:00–13:00)");
        let ctx = missed_ctx(vec![
            oldest.clone(),
            a_slot("2026-09-06T19:00:00", "Kveldsmesse"),
        ]);
        let (scope, key) = failure_ledger_key(&ctx, "irrelevant");
        assert_eq!(scope, SeenScope::Missed);
        assert_eq!(key, oldest.seen_key());
        assert!(
            key.starts_with("2026-09-06T11:00:00:"),
            "the machine's timestamp stays legible: {key}"
        );
        assert!(
            !key.contains("Kveldsmesse") && !key.contains("Ukentlig"),
            "a name somebody typed is hashed, not stored: {key}"
        );

        // A sweep that finds a SECOND Sunday is a different mail from the one
        // that found only the first — so the outbox must not absorb it.
        let one = missed_ctx(vec![oldest.clone()]);
        assert_ne!(
            failure_dedup_key(&one, ""),
            failure_dedup_key(&ctx, ""),
            "one occurrence and two are not the same message"
        );
    }

    /// The failure ledger key is the AlertGate's pair with the recipient dropped
    /// — implicit, because a machine has at most one subscription.
    #[test]
    fn the_failure_key_separates_codes_and_sentences() {
        assert_ne!(
            failure_seen_key("device_missing", "Enheten forsvant"),
            failure_seen_key("disk_full", "Enheten forsvant")
        );
        assert_ne!(
            failure_seen_key("device_missing", "Enheten forsvant"),
            failure_seen_key("device_missing", "Disken er full")
        );
        assert_eq!(
            failure_seen_key("device_missing", "Enheten forsvant"),
            failure_seen_key("device_missing", "Enheten forsvant"),
            "and the same failure is the same key, or nothing throttles"
        );
    }

    /// A relay alert is built from SCRUBBED text — the endpoint answers an
    /// unscrubbed path with a 400, which the outbox drops permanently. The
    /// per-language proof is `email::tests::relay_bodies_pass_…`; this pins that
    /// THIS call site actually applies it.
    #[test]
    fn the_dispatch_scrubs_before_it_renders() {
        let raw = "kunne ikke åpne /Users/kari/Opptak/gudstjeneste.wav";
        let cleaned = scrub(raw);
        assert!(!cleaned.contains("/Users/kari"), "{cleaned}");
        assert!(cleaned.contains("<path"), "{cleaned}");
        assert_eq!(scrub(&cleaned), cleaned, "and scrubbing twice is a no-op");
    }

    /// The receipt names the FILE and never the folder — `render_receipt`
    /// reduces the path itself, and this proves the reduction survives the
    /// footer, the wire body and the row it is stored in.
    #[tokio::test]
    async fn a_receipt_carries_the_basename_and_never_the_path() {
        let (pool, _d) = temp_pool().await;
        let sub = enrol(&pool, RelaySubscriptionState::Confirmed).await;
        let path = "/Users/kari/Opptak/gudstjeneste 6. september.wav";
        let rendered = render_receipt(
            MailLang::No,
            "Kirka",
            "Ola",
            "Ukentlig opptak (11:00–13:00)",
            "06.09.2026 11:00",
            4_980,
            path,
        );
        send_relay_message(
            &pool,
            &sub,
            &an_endpoint(),
            RelayMessageKind::Receipt,
            MailLang::No,
            rendered,
            SeenScope::Receipt,
            "2026-09-06T11:00:00:abc",
            "receipt:2026-09-06T11:00:00:abc".into(),
        )
        .await
        .expect("queued");

        let q = relay_store::load_queue(&pool).await.unwrap();
        assert_eq!(q.len(), 1);
        let body = &q[0].payload_json;
        assert!(
            body.contains("gudstjeneste 6. september.wav"),
            "the volunteer has to be able to find the file: {body}"
        );
        // …and the folder it sat in is not the app's to hand over. Both halves
        // of the path, because the endpoint's validator would reject either and
        // answer 400 — which the outbox drops without a retry.
        assert!(!body.contains("/Users"), "{body}");
        assert!(!body.contains("/Opptak"), "{body}");
        assert_eq!(q[0].event, Some(RelayMessageKind::Receipt));
    }

    /// A receipt is once and for all, unlike a failure's ten-minute window. The
    /// same finished recording reported twice is one mail.
    #[tokio::test]
    async fn a_receipt_is_recorded_once_and_for_all() {
        let (pool, _d) = temp_pool().await;
        let key = "2026-09-06T11:00:00:abc";
        relay_store::seen_mark(&pool, SeenScope::Receipt, key, now_ms())
            .await
            .unwrap();
        let facts = RelayFacts::gather(&pool, SeenScope::Receipt, key).await;
        assert!(facts.throttled);
        // …and a year later it is still throttled, which is what "full stop"
        // means: the moment cannot happen twice.
        let seen = relay_store::seen_get(&pool, SeenScope::Receipt, key)
            .await
            .unwrap();
        assert!(seen_decision(
            SeenScope::Receipt,
            seen,
            now_ms() + 365 * 24 * 60 * 60 * 1_000
        ));

        assert!(
            !plan_receipt(&ReceiptRouting {
                receipt_enabled: true,
                relay_enrolled: true,
                relay_confirmed: true,
                relay_throttled: facts.throttled,
                relay_endpoint_built: true,
                ..ReceiptRouting::default()
            })
            .relay
        );
    }

    /// The receipt's own switch, on its own. Off by default and independent of
    /// `email_on_error`: "tell me when something broke" and "tell me every
    /// Sunday that nothing broke" are different appetites.
    #[test]
    fn a_receipt_needs_its_own_switch() {
        let confirmed = ReceiptRouting {
            receipt_enabled: false,
            relay_enrolled: true,
            relay_confirmed: true,
            relay_suppressed: false,
            relay_throttled: false,
            relay_endpoint_built: true,
        };
        assert!(!plan_receipt(&confirmed).relay, "default off");
        assert!(
            plan_receipt(&ReceiptRouting {
                receipt_enabled: true,
                ..confirmed
            })
            .relay
        );
        assert!(
            !Settings::default().email_receipt_enabled,
            "…and the field it reads defaults off"
        );
    }

    /// ⚠️ The behaviour change, stated as a test: a missed occurrence fires the
    /// NATIVE notification whatever the relay is doing.
    ///
    /// Before A3 a missed Sunday produced an event to a renderer that might not
    /// be running, and nothing else. Now the operator standing at the machine is
    /// told — and that leg does not depend on an endpoint, a subscription, a
    /// confirmation or a network.
    #[test]
    fn a_missed_recording_reaches_the_operator_even_with_the_relay_unreachable() {
        for (enrolled, confirmed, suppressed, endpoint) in [
            (false, false, false, false),
            (true, false, false, true),
            (true, true, true, true),
            (true, true, false, false),
        ] {
            let plan = plan_failure(&FailureRouting {
                email_on_error: true,
                relay_enrolled: enrolled,
                relay_confirmed: confirmed,
                relay_suppressed: suppressed,
                relay_endpoint_built: endpoint,
                ..FailureRouting::default()
            });
            assert!(
                plan.native,
                "native survives every degradation ({enrolled}/{confirmed}/{suppressed}/{endpoint})"
            );
            assert!(!plan.relay);
            assert!(!plan.email, "and no SMTP transport was configured either");
        }
    }

    /// One failure never produces two e-mails, whichever leg carries it. The
    /// truth table is pinned in the core; this pins that the DISPATCH still
    /// obeys it once both sets of facts come from real reads.
    #[tokio::test]
    async fn a_configured_smtp_server_wins_and_the_relay_stands_down() {
        let (pool, _d) = temp_pool().await;
        enrol(&pool, RelaySubscriptionState::Confirmed).await;
        let facts = RelayFacts::gather(&pool, SeenScope::Failure, "k").await;
        assert!(facts.gate.confirmed, "a relay leg that COULD have fired");

        let plan = plan_failure(&FailureRouting {
            email_on_error: true,
            email_recipient: "vakt@kirka.no",
            email_feature_built: true,
            email_transport_ready: true,
            email_throttled: false,
            relay_enrolled: facts.gate.enrolled,
            relay_confirmed: facts.gate.confirmed,
            relay_suppressed: facts.gate.suppressed,
            relay_throttled: facts.throttled,
            relay_endpoint_built: true,
        });
        assert!(plan.email && !plan.relay);
        assert!(!(plan.email && plan.relay), "the invariant, restated here");
    }
}
