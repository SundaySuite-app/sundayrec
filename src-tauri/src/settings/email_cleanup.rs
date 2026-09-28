//! The one-time tidy-up after e-mail alerts were removed.
//!
//! SundayRec used to e-mail a volunteer when a recording failed — over the
//! church's own SMTP server, or through the SundaySuite relay. Both are gone:
//! failures are told on the machine, natively, and nowhere else. An install that
//! used either still carries the leftovers, and this module clears them once,
//! at startup:
//!
//!   - the e-mail keys in the settings blob (`emailOnError`, `emailAddress`,
//!     `emailSmtp*`, `emailReceiptEnabled`). The typed [`Settings`] already
//!     ignores them; rewriting the blob makes them actually go away;
//!   - the SMTP password in the OS keychain (`email.smtp_password`);
//!   - the relay's outbox and subscription record — those two are dropped by
//!     migration `0008_drop_notify_outbox.sql`, not here;
//!   - and, for a volunteer who had e-mail alerts switched ON, a pending notice
//!     ([`NOTICE_KEY`]) so the app says once what changed instead of leaving
//!     them to wonder why the mail stopped coming.
//!
//! ## Why the raw blob, and why before anything else
//!
//! Every settings save writes the whole typed struct, which no longer has the
//! e-mail fields — so the FIRST save after the upgrade erases the only evidence
//! that there was anything to clean up. [`run`] therefore reads the stored JSON
//! directly, and `lib.rs` calls it right after the database opens, before the
//! scheduler, the recovery scan or the renderer can save anything.
//!
//! Idempotent without a flag of its own: once the blob is rewritten there are
//! no e-mail keys left, and the next launch finds nothing to do.
//!
//! ## The keychain is touched only when there is something there
//!
//! `secrets`' module docs explain why retired slots were left alone until now: a
//! keychain call can block on an OS authorization prompt. So the deletion runs
//! ONLY on a machine whose settings name an SMTP server or user (a machine that
//! never set SMTP up has no password stored), and it runs in the background —
//! a prompt, if one ever appears, can never hold up launch.

use sqlx::SqlitePool;

use crate::db::store;
use crate::error::AppResult;

use super::SETTINGS_KEY;

/// The `app_setting` key of the "e-mail alerts were removed" notice. The value
/// is [`NOTICE_PENDING`] until the volunteer dismisses the banner, then the row
/// is deleted.
pub const NOTICE_KEY: &str = "notice.email_removed";

/// The value [`NOTICE_KEY`] holds while the banner should show.
pub const NOTICE_PENDING: &str = "pending";

/// Every settings key the e-mail alerts ever stored. Their presence is what
/// marks a blob as "written before the removal".
pub const RETIRED_KEYS: &[&str] = &[
    "emailOnError",
    "emailAddress",
    "emailSmtp",
    "emailSmtpPort",
    "emailSmtpUser",
    "emailSmtpFrom",
    "emailReceiptEnabled",
];

/// What the stored blob says has to happen. Pure — see [`plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CleanupPlan {
    /// The blob still carries e-mail keys: rewrite it without them.
    pub rewrite_settings: bool,
    /// An SMTP server or user was configured, so a password may sit in the
    /// keychain: delete it.
    pub forget_smtp_password: bool,
    /// E-mail alerts were switched ON: tell the volunteer they are gone.
    pub notify_removed: bool,
}

/// Decide the clean-up from the raw settings JSON (`None` = no settings row).
///
/// A blob that does not parse as a JSON object plans nothing: the typed loader
/// already falls back to defaults for it, and a clean-up that guessed at a
/// corrupt blob could only make things worse.
pub fn plan(raw: Option<&str>) -> CleanupPlan {
    let Some(obj) = raw
        .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok())
        .and_then(|v| v.as_object().cloned())
    else {
        return CleanupPlan::default();
    };
    let non_blank = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    };
    CleanupPlan {
        rewrite_settings: RETIRED_KEYS.iter().any(|k| obj.contains_key(*k)),
        forget_smtp_password: non_blank("emailSmtp") || non_blank("emailSmtpUser"),
        notify_removed: obj.get("emailOnError").and_then(|v| v.as_bool()) == Some(true),
    }
}

/// Clear the leftovers in the DATABASE and return the plan, so the caller can
/// do the keychain half off the startup path (see the module docs).
///
/// Order matters: the notice is written BEFORE the blob is rewritten. If the
/// process dies between the two, the next launch still finds the e-mail keys and
/// plans the same notice again (an UPSERT, so still one row) — the other order
/// could lose the notice for good.
pub async fn run(pool: &SqlitePool) -> AppResult<CleanupPlan> {
    let raw = store::get_setting(pool, SETTINGS_KEY).await?;
    let plan = plan(raw.as_deref());
    if plan.notify_removed {
        store::set_setting(pool, NOTICE_KEY, NOTICE_PENDING).await?;
    }
    if plan.rewrite_settings {
        let settings = super::load(pool).await?;
        super::save(pool, settings).await?;
        tracing::info!(
            notice = plan.notify_removed,
            smtp = plan.forget_smtp_password,
            "settings: removed the retired e-mail alert fields"
        );
    }
    Ok(plan)
}

/// Delete the retired SMTP password when `plan` says one may exist. The
/// `delete` seam is injected so the decision is testable without a real
/// keychain (whose calls can block on an OS prompt in a headless run).
pub fn forget_smtp_password_with(
    plan: CleanupPlan,
    delete: impl FnOnce() -> AppResult<()>,
) -> bool {
    if !plan.forget_smtp_password {
        return false;
    }
    match delete() {
        Ok(()) => {
            tracing::info!("settings: removed the retired SMTP password from the keychain");
            true
        }
        Err(e) => {
            tracing::warn!("settings: could not remove the retired SMTP password: {e}");
            false
        }
    }
}

/// The production keychain half of the clean-up: [`forget_smtp_password_with`]
/// over the real `secrets::delete`, on a blocking thread so a keychain prompt
/// can never stall the async runtime or startup.
pub fn forget_smtp_password_in_background(plan: CleanupPlan) {
    if !plan.forget_smtp_password {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        forget_smtp_password_with(plan, || {
            crate::secrets::delete(crate::secrets::SecretProvider::SmtpPassword)
        });
    });
}

/// Whether the "e-mail alerts were removed" banner should show.
pub async fn notice_pending(pool: &SqlitePool) -> AppResult<bool> {
    Ok(store::get_setting(pool, NOTICE_KEY).await?.as_deref() == Some(NOTICE_PENDING))
}

/// The volunteer read the banner: never show it again.
pub async fn dismiss_notice(pool: &SqlitePool) -> AppResult<()> {
    store::delete_setting(pool, NOTICE_KEY).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    /// A v0.20 blob from a church that had SMTP set up and alerts on.
    const SMTP_ON: &str = r#"{
        "churchName": "Domkirken",
        "reminderMinutes": 10,
        "emailOnError": true,
        "emailAddress": "vakt@kirka.no",
        "emailSmtp": "smtp.kirka.no",
        "emailSmtpPort": 587,
        "emailSmtpUser": "vakt",
        "emailSmtpFrom": "",
        "emailReceiptEnabled": false
    }"#;

    /// What every install that ever opened the notify page carries: the
    /// defaults, alerts off, no server.
    const DEFAULTS_ONLY: &str = r#"{
        "churchName": "Domkirken",
        "emailOnError": false,
        "emailAddress": "",
        "emailSmtp": "",
        "emailSmtpPort": 587,
        "emailSmtpUser": "",
        "emailSmtpFrom": "",
        "emailReceiptEnabled": false
    }"#;

    #[test]
    fn a_church_with_smtp_and_alerts_on_gets_all_three() {
        assert_eq!(
            plan(Some(SMTP_ON)),
            CleanupPlan {
                rewrite_settings: true,
                forget_smtp_password: true,
                notify_removed: true,
            }
        );
    }

    #[test]
    fn default_email_fields_are_rewritten_but_touch_neither_keychain_nor_banner() {
        assert_eq!(
            plan(Some(DEFAULTS_ONLY)),
            CleanupPlan {
                rewrite_settings: true,
                forget_smtp_password: false,
                notify_removed: false,
            }
        );
    }

    #[test]
    fn a_relay_user_without_smtp_is_told_but_the_keychain_is_left_alone() {
        // The relay needed no SMTP host — one switch drove both pipes, so the
        // switch alone decides the notice.
        let relay_only = r#"{"emailOnError": true, "emailAddress": "vakt@kirka.no"}"#;
        assert_eq!(
            plan(Some(relay_only)),
            CleanupPlan {
                rewrite_settings: true,
                forget_smtp_password: false,
                notify_removed: true,
            }
        );
    }

    #[test]
    fn a_whitespace_host_is_not_a_configured_server() {
        let blank = r#"{"emailSmtp": "   ", "emailSmtpUser": ""}"#;
        assert!(!plan(Some(blank)).forget_smtp_password);
        let user_only = r#"{"emailSmtpUser": "vakt"}"#;
        assert!(plan(Some(user_only)).forget_smtp_password);
    }

    #[test]
    fn nothing_to_do_plans_nothing() {
        assert_eq!(plan(None), CleanupPlan::default());
        assert_eq!(plan(Some(r#"{"churchName":"X"}"#)), CleanupPlan::default());
        assert_eq!(plan(Some("{not json")), CleanupPlan::default());
        assert_eq!(plan(Some("[1,2]")), CleanupPlan::default());
    }

    #[tokio::test]
    async fn an_upgraded_install_is_cleaned_once_and_keeps_everything_else() {
        let (pool, _d) = temp_pool().await;
        store::set_setting(&pool, SETTINGS_KEY, SMTP_ON)
            .await
            .unwrap();

        let first = run(&pool).await.unwrap();
        assert!(first.rewrite_settings && first.notify_removed && first.forget_smtp_password);

        // The blob no longer carries a single e-mail key…
        let raw = store::get_setting(&pool, SETTINGS_KEY)
            .await
            .unwrap()
            .unwrap();
        let obj: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in RETIRED_KEYS {
            assert!(obj.get(*key).is_none(), "{key} survived the rewrite");
        }
        // …the neighbours survived…
        assert_eq!(obj["churchName"], "Domkirken");
        assert_eq!(obj["reminderMinutes"], 10);
        // …and the notice is waiting.
        assert!(notice_pending(&pool).await.unwrap());

        // The second launch finds nothing to do.
        assert_eq!(run(&pool).await.unwrap(), CleanupPlan::default());
        assert!(
            notice_pending(&pool).await.unwrap(),
            "until it is dismissed"
        );

        dismiss_notice(&pool).await.unwrap();
        assert!(!notice_pending(&pool).await.unwrap());
        // Dismissing twice is harmless.
        dismiss_notice(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn a_fresh_install_has_nothing_to_clean_and_no_notice() {
        let (pool, _d) = temp_pool().await;
        assert_eq!(run(&pool).await.unwrap(), CleanupPlan::default());
        assert!(!notice_pending(&pool).await.unwrap());
        assert!(
            store::get_setting(&pool, SETTINGS_KEY)
                .await
                .unwrap()
                .is_none(),
            "a clean-up with nothing to do writes nothing"
        );
    }

    #[test]
    fn the_keychain_is_asked_only_when_a_server_was_configured() {
        let called = Cell::new(false);
        let cleared = forget_smtp_password_with(plan(Some(DEFAULTS_ONLY)), || {
            called.set(true);
            Ok(())
        });
        assert!(!cleared);
        assert!(!called.get(), "no SMTP host → no keychain call at all");

        let cleared = forget_smtp_password_with(plan(Some(SMTP_ON)), || {
            called.set(true);
            Ok(())
        });
        assert!(cleared);
        assert!(called.get());
    }

    #[test]
    fn a_keychain_that_refuses_is_logged_not_fatal() {
        let cleared = forget_smtp_password_with(plan(Some(SMTP_ON)), || {
            Err(crate::error::AppError::Internal("locked".into()))
        });
        assert!(!cleared);
    }
}
