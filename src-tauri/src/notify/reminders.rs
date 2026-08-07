//! The review-queue reminder TICK — the wall clock the queue's timeline never had.
//!
//! ## What was missing
//!
//! `sundayrec_core::review_queue::process_reminders` is a complete, unit-tested
//! escalation ladder: nudge after 24 h, again with a webhook after 48 h, again
//! with an in-app warning after 7 d, auto-discard at 14 d. It was reachable only
//! through [`review_process_reminders`](crate::commands::review::review_process_reminders),
//! a command with **zero TS callers**. Nothing in the app ever called the
//! function, so an episode nobody reviewed sat in the queue in perfect silence
//! until it silently deleted itself a fortnight later.
//!
//! This module is the missing half: a lightweight timer that runs the ladder and
//! hands each returned action to the Phase-2 dispatch seam.
//!
//! ## Deliberately not the scheduler
//!
//! The scheduler's supervisor owns recording start/stop on the wall clock and is
//! the most safety-critical loop in the app. A reminder is neither urgent nor
//! recording-related; entangling the two would put mail and HTTP inside the loop
//! that has to fire a start at 10:59:59. This is its own task with one timer.
//!
//! ## Quiet by construction
//!
//! An empty queue — the normal state of a machine between Sundays — returns
//! before the settings row is read and without logging anything. The only
//! wake-up is the hourly timer itself.
//!
//! ## Persist first, then speak
//!
//! The bumped `reminded` counters are saved BEFORE any dispatch. A crash between
//! the two costs one reminder; the other order costs a reminder every hour,
//! forever, for the same episode.
//!
//! ## ⚠️ NETWORK-UNVERIFIED
//!
//! The mail + webhook legs are the same sockets `dispatch_failure` uses (see the
//! module header there); the decisions above them are pure and tested.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use sundayrec_core::email::{
    reminder_age_text, render_auto_discarded, render_reminder, MailLang, RenderedEmail,
};
use sundayrec_core::notify::{alert_church, code, plan_failure, BackendWarning, FailureRouting};
use sundayrec_core::review_queue::{self, ReminderAction, ReviewQueueEntry, AUTO_DISCARD_DAYS};
use sundayrec_core::settings::Settings;
use sundayrec_core::webhook::{WebhookPayload, WebhookSeverity};

use crate::commands::review::{load_queue, save_queue};
use crate::db::Db;

/// How long after startup the first sweep runs. Long enough to be behind the
/// recorder's crash recovery and the first window paint; short enough that a
/// machine woken on Sunday morning, used, and shut down again still gets one.
const FIRST_TICK_DELAY: Duration = Duration::from_secs(45);

/// The gap between sweeps. One hour, exactly as the Electron build's
/// `setInterval` — the thresholds are measured in days, so a finer clock would
/// buy nothing and cost wake-ups.
const TICK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The webhook `category` for a reminder, so a chat channel can tell "an episode
/// is waiting" from "the recorder died" without reading the sentence.
const WEBHOOK_CATEGORY: &str = "review_reminder";

/// Start the reminder tick. Call once, from `setup`, after the database is
/// managed. The task lives for the process lifetime and holds nothing but the
/// app handle.
pub fn spawn(app: AppHandle) {
    // E2.2: supervised. A dead reminder tick is silent by construction — the
    // ladder it drives is the thing that would have spoken — so an episode
    // nobody reviewed would sit unmentioned until it auto-discarded itself.
    let tick_app = app.clone();
    crate::supervise::supervised_spawn(
        app,
        "notify::reminders",
        crate::supervise::TaskAlert {
            title: "SundayRec — påminnelser stoppet",
            body: "Påminnelsene om episoder som venter på gjennomgang har en vedvarende \
                   feil. Start appen på nytt; vedvarer det, kjør Diagnose under \
                   Innstillinger → Lyd.",
        },
        move || {
            let app = tick_app.clone();
            async move {
                tokio::time::sleep(FIRST_TICK_DELAY).await;
                loop {
                    tick(&app).await;
                    tokio::time::sleep(TICK_INTERVAL).await;
                }
            }
        },
    );
    tracing::info!(
        "notify: review-queue reminders armed (every {} min)",
        TICK_INTERVAL.as_secs() / 60
    );
}

/// One sweep: read the queue, run the pure ladder, persist, dispatch.
async fn tick(app: &AppHandle) {
    let Some(db) = app.try_state::<Db>() else {
        return; // pre-setup; the next tick will find it
    };
    let queue = match load_queue(&db).await {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("review reminders: could not read the queue: {e}");
            return;
        }
    };
    // The quiet path: no queue, no work, no log line, no settings read.
    if queue.is_empty() {
        return;
    }

    let now = crate::db::store::now_ms() as i64;
    let outcome = review_queue::process_reminders(&queue, now);

    if outcome.changed {
        if let Err(e) = save_queue(&db, &outcome.survivors).await {
            // Dispatching what we could not remember dispatching would re-send
            // the same reminder every hour until someone reviewed the episode.
            tracing::warn!("review reminders: could not persist the queue ({e}) — not dispatching");
            return;
        }
        crate::tray_note_review_queue(app);
    }
    if outcome.actions.is_empty() {
        return;
    }

    let settings = crate::settings::load(&db.pool).await.unwrap_or_default();
    tracing::info!(
        actions = outcome.actions.len(),
        "review reminders: dispatching"
    );
    for action in &outcome.actions {
        // `queue` is the pre-sweep snapshot on purpose: an auto-discarded entry
        // is already gone from `survivors`, and its name is what the notice says.
        dispatch(app, &settings, &queue, action, now).await;
    }
}

/// Route ONE reminder action to the channels its rung asks for.
async fn dispatch(
    app: &AppHandle,
    settings: &Settings,
    snapshot: &[ReviewQueueEntry],
    action: &ReminderAction,
    now: i64,
) {
    let entry = snapshot.iter().find(|e| e.id == action.id);
    // A queue entry has no title until a human gives it one, so the file name is
    // what the recipient will actually recognise it by.
    let label = entry
        .map(|e| episode_label(&e.prep.recording_path))
        .unwrap_or_else(|| action.id.clone());
    let age_days = entry
        .map(|e| review_queue::age_in_days(e.added_at, now).round() as i64)
        .unwrap_or(0);
    let lang = MailLang::from_code(settings.language.as_deref());
    let church = alert_church(&settings.church_name);

    // 14 days: the entry is already dropped from the saved queue. Nothing to
    // remind about — just say what happened, natively, once.
    if action.channel.is_auto_discard() {
        super::native(
            app,
            "SundayRec",
            &format!(
                "{label}: {}",
                render_auto_discarded(lang, AUTO_DISCARD_DAYS)
            ),
        );
        tracing::info!(id = %action.id, "review reminders: auto-discarded after 14 days");
        return;
    }

    let rendered = render_reminder(lang, church, &label, age_days);

    // The native leg fires on every rung, unconditionally — same rule as a
    // failure: the person at the machine is never the one we skip.
    super::native(
        app,
        &rendered.subject,
        &format!("{label} — {}", reminder_age_text(lang, age_days)),
    );

    // Reuse the FAILURE matrix for the parts of it that are about PERMISSION
    // rather than about failures: did the user ask for mail at all, is there an
    // address, can this build send, is a transport actually configured, is the
    // webhook URL valid. The throttle input is `false` because the queue's own
    // persisted `reminded` counter is the throttle here (see `send_rendered`).
    // The rung then decides which of the permitted legs it wants.
    let recipient = settings.email_address.trim().to_string();
    let permitted = plan_failure(&FailureRouting {
        email_on_error: settings.email_on_error,
        email_recipient: &recipient,
        email_feature_built: cfg!(feature = "email"),
        email_transport_ready: super::email_transport_ready(settings, &recipient),
        email_throttled: false,
        webhook_url: &settings.webhook_url,
        webhook_allow_local: settings.webhook_allow_local,
    });

    if action.channel.wants_email() && permitted.email {
        send_reminder_email(settings, &recipient, &rendered).await;
    }

    if action.channel.wants_webhook() && permitted.webhook {
        let payload = WebhookPayload {
            app: "SundayRec".into(),
            church: settings.church_name.clone(),
            severity: WebhookSeverity::Warn,
            category: WEBHOOK_CATEGORY.into(),
            message: rendered.text.clone(),
            timestamp: chrono::Local::now().to_rfc3339(),
        };
        super::post_webhook(
            &settings.webhook_url,
            settings.webhook_allow_local,
            &payload,
        )
        .await;
    }

    if action.channel.wants_warning() {
        // The event only — NOT `notify::warn`, which would POST the webhook a
        // second time whenever `webhook_on_warning` is on. This rung has already
        // sent it above, because its ladder says webhook, not "webhook if the
        // user also opted warnings in".
        super::emit_warning_event(
            app,
            &BackendWarning::warn(code::REVIEW_OVERDUE)
                .msg(format!(
                    "En episode har ventet i {age_days} dager på gjennomgang: {label}"
                ))
                .param("episode", label.clone())
                .param("days", age_days.to_string()),
        );
    }
}

/// Send the rendered reminder. Compiled to a no-op without the `email` feature —
/// the matrix never permits the leg in such a build, and this keeps the module
/// itself featureless, exactly like [`super::send_failure_email`].
#[cfg_attr(not(feature = "email"), allow(unused_variables))]
async fn send_reminder_email(settings: &Settings, recipient: &str, rendered: &RenderedEmail) {
    #[cfg(feature = "email")]
    {
        let Some(transport) = super::build_transport(settings, recipient) else {
            // `email_transport_ready` said yes a moment ago; only a race with
            // the user clearing the keychain gets here.
            tracing::warn!("review reminders: no mail transport at send time");
            return;
        };
        match crate::email::send_rendered(&transport, recipient, rendered).await {
            Ok(()) => tracing::info!("review reminders: reminder e-mailed"),
            Err(e) => tracing::warn!("review reminders: reminder e-mail failed: {e}"),
        }
    }
}

/// The recording's file name, without directories. Kept to the last path
/// component on both separators: a Windows path arriving on macOS (a queue blob
/// copied between machines) must not print in full.
fn episode_label(recording_path: &str) -> String {
    recording_path
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(recording_path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::review_queue::ReminderChannel;

    #[test]
    fn the_episode_label_is_the_file_name_on_either_separator() {
        assert_eq!(episode_label("/rec/2026-08-02.m4a"), "2026-08-02.m4a");
        assert_eq!(episode_label(r"C:\Opptak\2026-08-02.m4a"), "2026-08-02.m4a");
        // Degenerate inputs must still say something rather than nothing.
        assert_eq!(episode_label("bare-name.m4a"), "bare-name.m4a");
        assert_eq!(episode_label("/trailing/"), "/trailing/");
        assert_eq!(episode_label(""), "");
    }

    #[test]
    fn the_tick_interval_is_an_hour_and_the_first_sweep_is_not_at_boot() {
        // The thresholds are days; a finer clock buys nothing and costs wake-ups.
        assert_eq!(TICK_INTERVAL, Duration::from_secs(3600));
        // …and the first sweep waits, so it is never racing crash recovery or
        // the first paint for the database.
        assert!(FIRST_TICK_DELAY >= Duration::from_secs(30));
        assert!(FIRST_TICK_DELAY < TICK_INTERVAL);
    }

    /// The permission matrix is the failure one; what makes a reminder a
    /// reminder is the RUNG's answer, ANDed with it. Pin both halves of that
    /// conjunction, since only their product is ever executed.
    #[test]
    fn a_rung_can_only_narrow_what_the_settings_permit() {
        let permitted = plan_failure(&FailureRouting {
            email_on_error: true,
            email_recipient: "vakt@kirka.no",
            email_feature_built: true,
            email_transport_ready: true,
            email_throttled: false,
            webhook_url: "https://hooks.slack.com/services/T/B/X",
            webhook_allow_local: false,
        });
        assert!(permitted.email && permitted.webhook);

        // Day 1 wants mail but not the chat channel, even with a valid URL.
        assert!(ReminderChannel::NotifyEmail.wants_email() && permitted.email);
        assert!(!(ReminderChannel::NotifyEmail.wants_webhook() && permitted.webhook));
        // Day 2 widens to the webhook.
        assert!(ReminderChannel::NotifyEmailWebhook.wants_webhook() && permitted.webhook);

        // And with e-mail switched off in settings, no rung can send one.
        let no_mail = plan_failure(&FailureRouting {
            email_on_error: false,
            email_recipient: "vakt@kirka.no",
            email_feature_built: true,
            email_transport_ready: true,
            email_throttled: false,
            webhook_url: "",
            webhook_allow_local: false,
        });
        assert!(!(ReminderChannel::NotifyEmailWebhookWarning.wants_email() && no_mail.email));
    }

    /// The one code this module emits must be in `code::ALL`, or the renderer's
    /// key table (checked against that list) can never localise it.
    #[test]
    fn the_overdue_warning_code_is_registered() {
        assert!(code::ALL.contains(&code::REVIEW_OVERDUE));
        assert_eq!(code::REVIEW_OVERDUE, "review_overdue");
    }
}
