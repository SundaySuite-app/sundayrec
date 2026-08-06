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
//!     so an operator who had gone home learned nothing;
//!   - the webhook existed solely as a "send test" button.
//!
//! Everything now goes through [`dispatch_failure`] (terminal failures →
//! native + e-mail + webhook) or [`warn`] (degradations → an in-app toast +
//! optionally the webhook). Which channels actually fire is the unit-tested
//! [`sundayrec_core::notify`] matrix, not an `if` written at the call site.
//!
//! ## Featureless on purpose
//!
//! This module compiles in EVERY build. The `email` feature (in `default`, and
//! in both release feature lists) gates only the send itself: with
//! `--no-default-features` the routing still runs, the matrix is told the
//! feature is absent, and the e-mail leg is planned `false` — a clean
//! degradation to native + webhook rather than a compile error or a silent lie.
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
//! The webhook POST and (behind the feature) the SMTP/Gmail send do real I/O.
//! The decisions above them are pure and tested; the wire is provable only
//! against a real server — see docs/SMOKE-TEST.md.

use chrono::{DateTime, Local};
use tauri::{AppHandle, Emitter, Listener, Manager};

use sundayrec_core::notify::{
    plan_failure, plan_warning, BackendWarning, FailureRouting, FailureSource, WarningRouting,
};
use sundayrec_core::settings::Settings;
use sundayrec_core::webhook::{build_webhook_body, WebhookPayload, WebhookSeverity};

use crate::db::Db;

pub mod disk;
pub mod reminders;

pub use sundayrec_core::notify::code;

/// The event the renderer's `backend-warning` channel is mapped to. Follows the
/// `scheme://name` convention every other Rust-emitted event uses
/// (`recording://…`, `scheduler://…`, `tray://…`).
pub const WARNING_EVENT: &str = "backend://warning";

/// How long a webhook POST may take before we stop waiting. An unreachable chat
/// server must not hold a failure path open — the alert has already gone out
/// natively by the time this runs.
const WEBHOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Everything [`dispatch_failure`] needs to know about a failure.
#[derive(Debug, Clone)]
pub struct FailureCtx {
    /// Stable machine code (the recorder's `RecordingEvent::code`, or one of the
    /// scheduler's). Used as the webhook `category` so an operator scanning a
    /// chat channel can tell a device drop-out from a disk stop at a glance.
    pub code: String,
    /// The human sentence. This is ALSO the native notification body and the
    /// e-mail's error line, which is why the scheduler's existing wording is
    /// passed through verbatim rather than re-derived here.
    pub message: String,
    /// Which half of the app failed.
    pub source: FailureSource,
    /// When it happened (local time) — the e-mail's human date and the
    /// webhook's timestamp.
    pub occurred_at: DateTime<Local>,
}

impl FailureCtx {
    /// A failure that just happened.
    pub fn now(code: impl Into<String>, message: impl Into<String>, source: FailureSource) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            source,
            occurred_at: Local::now(),
        }
    }
}

/// Fire a native OS notification. The one channel no setting can silence: the
/// person standing at the machine is the only one who can still save the
/// service. Previously private to the scheduler — the recorder had no way to
/// reach it at all.
pub fn native(app: &AppHandle, title: &str, body: &str) {
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
    use crate::recorder::engine::{RecordingEvent, ERROR_EVENT};

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

    // The graduated low-disk observer rides on the same observational seam.
    disk::wire(app);

    tracing::info!("notify: failure dispatch wired to {ERROR_EVENT}");
}

/// Tell everybody who should hear about a terminal failure.
///
/// Always fires the native notification; adds the e-mail alert when the settings
/// ask for one AND this build can send AND a transport exists AND the throttle
/// gate hasn't already sent this exact alert in the last ten minutes; adds a
/// webhook POST when a valid URL is configured. The channel choice itself is
/// [`sundayrec_core::notify::plan_failure`].
///
/// Never returns an error: an alert path that can itself fail loudly is a second
/// failure on top of the first. Every leg logs and moves on.
pub async fn dispatch_failure(app: &AppHandle, ctx: FailureCtx) {
    // 1. The native leg first — it needs nothing but the app handle, so it still
    //    fires if the database is unreachable (which is itself a failure mode
    //    the operator would want to hear about).
    native(app, "SundayRec", &ctx.message);

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

    let plan = plan_failure(&FailureRouting {
        email_on_error: settings.email_on_error,
        email_recipient: &recipient,
        email_feature_built: cfg!(feature = "email"),
        email_transport_ready: transport_ready,
        email_throttled: throttled,
        webhook_url: &settings.webhook_url,
    });

    tracing::info!(
        code = %ctx.code,
        source = %ctx.source.as_str(),
        email = plan.email,
        webhook = plan.webhook,
        "notify: dispatching failure"
    );

    if plan.email {
        send_failure_email(app, &settings, &recipient, &ctx).await;
    }

    if plan.webhook {
        let payload = WebhookPayload {
            app: "SundayRec".into(),
            church: settings.church_name.clone(),
            severity: WebhookSeverity::Error,
            category: ctx.code.clone(),
            message: ctx.message.clone(),
            timestamp: ctx.occurred_at.to_rfc3339(),
        };
        post_webhook(&settings.webhook_url, &payload).await;
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
            gate.decide(recipient, error_message, crate::cloud::now_ms()),
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
/// `From:`, or a configured Google OAuth client WITH a stored Gmail refresh
/// token. Without one there is nothing to send *with*, however willing the
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
/// SMTP wins when a host is configured (the settings doc's rule: "blank = use
/// the Gmail transport instead"); Gmail requires BOTH a resolvable OAuth client
/// and a stored refresh token.
#[cfg(feature = "email")]
fn build_transport(settings: &Settings, recipient: &str) -> Option<crate::email::Transport> {
    use crate::commands::email::{resolve_from_address, resolve_smtp_password};
    use crate::email::Transport;
    use crate::secrets::SecretProvider;

    let host = settings.email_smtp.trim();
    if !host.is_empty() {
        let user = Some(settings.email_smtp_user.clone()).filter(|u| !u.trim().is_empty());
        // No request-side password: nobody is at the keyboard. The keychain is
        // the only source, which is exactly why P1 added the write path.
        let pass = resolve_smtp_password(None, crate::secrets::get(SecretProvider::SmtpPassword))?;
        let from =
            resolve_from_address(Some(&settings.email_smtp_from), user.as_deref(), recipient)?;
        return Some(Transport::Smtp {
            host: host.to_string(),
            port: settings.email_smtp_port.clamp(1, 65_535) as u16,
            user,
            pass,
            from,
        });
    }

    let config = crate::cloud::config::GoogleOAuthConfig::resolve()?;
    if !crate::secrets::has(SecretProvider::Gmail) {
        return None;
    }
    Some(Transport::Gmail { config })
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
        use sundayrec_core::email::MailLang;
        use sundayrec_core::notify::{alert_church, alert_date_format, alert_person};

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

/// Raise a live backend warning: emit it to the renderer (which localises on
/// [`BackendWarning::code`] and toasts it) and, when the user widened the
/// webhook to warnings, POST it too.
///
/// Synchronous by design so it can be called from anywhere — including the
/// `&mut`-heavy back-off branches of the pre-roll loop and the cloud worker.
/// The event goes out immediately; the settings read + POST run in a detached
/// task, because a warning must never make the thing it is warning about slower.
pub fn warn(app: &AppHandle, w: BackendWarning) {
    emit_warning_event(app, &w);

    let Some(db) = app.try_state::<Db>() else {
        return; // pre-setup: the event above is all we can do
    };
    let pool = db.pool.clone();
    tauri::async_runtime::spawn(async move {
        let settings = crate::settings::load(&pool).await.unwrap_or_default();
        let plan = plan_warning(&WarningRouting {
            webhook_on_warning: settings.webhook_on_warning,
            webhook_url: &settings.webhook_url,
        });
        if !plan.webhook {
            return;
        }
        let payload = WebhookPayload {
            app: "SundayRec".into(),
            church: settings.church_name.clone(),
            severity: w.severity.to_webhook(),
            category: w.code.clone(),
            message: w
                .msg
                .clone()
                .unwrap_or_else(|| format!("SundayRec: {}", w.code)),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        post_webhook(&settings.webhook_url, &payload).await;
    });
}

/// The renderer half of [`warn`], on its own: log it and put it on the wire.
///
/// Extracted (not changed — `warn` calls this and behaves exactly as before) for
/// the one caller that must NOT take the webhook half with it: a day-7 review
/// reminder POSTs the webhook from its own rung, and routing the toast through
/// `warn` as well would deliver the same message to the same chat channel twice
/// whenever `webhook_on_warning` happens to be on.
pub(crate) fn emit_warning_event(app: &AppHandle, w: &BackendWarning) {
    tracing::warn!(code = %w.code, msg = ?w.msg, "notify: backend warning");
    if let Err(e) = app.emit(WARNING_EVENT, w) {
        tracing::warn!("notify: could not emit {WARNING_EVENT}: {e}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The webhook side effect
// ─────────────────────────────────────────────────────────────────────────────

/// POST `payload` to `url`. The body shaping (URL validation, Slack/Discord
/// detection, structured-vs-chat choice) is the unit-tested
/// [`sundayrec_core::webhook`]; this is only the socket. Bounded, and never
/// propagates: a webhook that is down is not a reason for a failure alert to
/// become two failures. NETWORK-UNVERIFIED.
async fn post_webhook(url: &str, payload: &WebhookPayload) {
    let Some(body) = build_webhook_body(url, payload) else {
        return; // invalid URL — the plan already filtered these, belt and braces
    };
    let result = reqwest::Client::new()
        .post(url)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(WEBHOOK_TIMEOUT)
        .send()
        .await;
    match result {
        Ok(r) if r.status().is_success() => tracing::info!("notify: webhook delivered"),
        Ok(r) => tracing::warn!("notify: webhook returned {}", r.status()),
        Err(e) => tracing::warn!("notify: webhook POST failed: {e}"),
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

    #[test]
    fn the_webhook_timeout_is_bounded_and_short() {
        // The alert has already gone out natively by the time this runs; a hung
        // chat server must not keep the failure path open behind it.
        assert!(WEBHOOK_TIMEOUT <= std::time::Duration::from_secs(15));
    }

    /// A default (never-configured) settings row must never report a ready
    /// transport — in EITHER build. Without the feature the answer is a constant
    /// `false` (the degradation path); with it, the SMTP branch bails on the
    /// blank host before it can reach the keychain, and the Gmail branch needs a
    /// resolvable OAuth client, which a test binary has no reason to carry.
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
            webhook_url: "",
        });
        assert!(!plan.email);
        assert!(plan.native, "the native leg survives every degradation");
    }
}
