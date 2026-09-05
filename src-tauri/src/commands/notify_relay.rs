//! The e-mail relay's IPC surface: enrol, resend, unsubscribe, test, report.
//!
//! Five commands over `crate::notify::relay`. Each of them does the same two
//! things in the same order — render what has to be rendered, queue a row
//! through `relay::wire` — and then rings the pump's doorbell. None of them
//! performs a request itself: a button that waits on a socket is a button that
//! hangs on a church network, and the outbox already knows how to be patient.
//!
//! What is HERE rather than in `relay::wire` is what only a button needs: the
//! address the renderer typed, the tokens an enrolment mints, and the DTO the
//! panel reads. The request bodies live next to the outbox, because A3's
//! dispatcher queues rows too and it is not an IPC path at all.
//!
//! ## Path policy
//!
//! None of these takes a path and none can be made to touch one. The only
//! storage they reach is the `app_setting` bag and the two tables from migration
//! 0006, both addressed by constants inside the process — so, as with
//! `commands::telemetry`, they do not appear in `commands::path_ratchet`'s
//! lists, and adding a path-shaped parameter would fail that ratchet until
//! somebody classified it.
//!
//! ## Trust boundary
//!
//! One value crosses from the renderer: the address. It is normalised and
//! validated here ([`normalize_address`]) before it can reach a template, a
//! queue row or the wire — and the endpoint validates again, because a client
//! check is a convenience and never a boundary.
//!
//! ## Why the tokens are minted on this machine
//!
//! Thirty-two random bytes each, and only their SHA-256 is ever sent. A dump of
//! the endpoint's database therefore cannot confirm a subscription or
//! unsubscribe anybody — the values that would do so exist here and in one
//! e-mail. It also makes enrolment IDEMPOTENT: the same body can be posted
//! again by the outbox after a timeout without minting a second identity, which
//! is what lets a sign-up survive a dropped connection at all.
//!
//! Featureless, deliberately. The relay is HTTP, not SMTP: it works in a
//! `--no-default-features` build, and gating it on the `email` feature would
//! make the settings panel's promise true only in some builds.

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;
use ts_rs::TS;

use sundayrec_core::email::{
    render_confirm, render_test, MailLang, RelayMessage, RelayMessageKind,
};
use sundayrec_core::notify::{alert_church, alert_person};

use crate::db::{store, Db};
use crate::error::{AppError, AppResult};
use crate::notify::relay::config::RelayEndpoint;
use crate::notify::relay::wire::{self, SubscribeBody};
use crate::notify::relay::{
    sender, store as relay_store, RelaySubscription, RelaySubscriptionState,
};
use crate::util::now_ms;

/// What the settings panel needs to render the relay card — and nothing more.
///
/// No token, no hash, no sub id. The panel's job is to show a state, an address
/// and how many messages are waiting; handing the webview a credential it has no
/// use for would be a credential in one more place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "RelaySubscriptionStatus.ts")]
#[serde(rename_all = "camelCase")]
pub struct RelaySubscriptionStatus {
    /// Whether this build has a relay endpoint at all. `false` means the URL or
    /// the write key was missing at compile time, and the panel should say so
    /// rather than offering a button that queues rows nothing will ever send.
    pub endpoint_built: bool,
    /// `None` when this machine has never enrolled an address.
    pub state: Option<RelaySubscriptionState>,
    /// The enrolled address, echoed back so the panel does not have to remember
    /// what was typed in a previous session.
    pub address: Option<String>,
    // Unix ms. Typed as `number` rather than ts-rs's default `bigint` for an
    // i64, matching `TelemetryQueueStatus::oldest_at`: serde puts a JSON NUMBER
    // on the wire, so a `bigint` annotation would describe a value the webview
    // never receives — and any arithmetic the panel did with it would throw.
    #[ts(type = "number | null")]
    pub enrolled_at: Option<i64>,
    #[ts(type = "number | null")]
    pub confirmed_at: Option<i64>,
    /// How many messages are waiting in the outbox.
    pub queued: u32,
}

impl RelaySubscriptionStatus {
    async fn read(pool: &sqlx::SqlitePool) -> AppResult<Self> {
        let sub = relay_store::subscription_get(pool).await?;
        Ok(Self {
            endpoint_built: RelayEndpoint::resolve().is_some(),
            state: sub.as_ref().map(|s| s.state),
            address: sub.as_ref().map(|s| s.address.clone()),
            enrolled_at: sub.as_ref().map(|s| s.enrolled_at),
            confirmed_at: sub.as_ref().and_then(|s| s.confirmed_at),
            queued: relay_store::queued_count(pool).await?,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The address
// ─────────────────────────────────────────────────────────────────────────────

/// The stable snake code for "that is not an address we can send to" — the one
/// the renderer branches on via `errorCode()`, exactly as
/// [`crate::commands::email::NO_CONFIG_SMTP_HOST`] does for SMTP. A CODE, not
/// prose: the human half must stay free to be reworded in seven languages.
pub const INVALID_ADDRESS: &str = "relay_invalid_address";

/// The stable snake code for "this build has no relay endpoint".
pub const NO_ENDPOINT: &str = "relay_no_endpoint";

/// The stable snake code for "there is no confirmed subscription to use".
pub const NOT_CONFIRMED: &str = "relay_not_confirmed";

/// Trim an address and check that it could plausibly be delivered to.
///
/// Deliberately NOT an RFC 5322 parser. The only question worth answering here
/// is "is this a typo the user should fix before we spend a confirmation mail on
/// it" — and the endpoint validates independently, with the mail provider behind
/// it as the real authority. What this catches is the whole population of real
/// mistakes: an empty field, a missing `@`, a domain with no dot, stray spaces,
/// two addresses pasted at once.
///
/// Lower-cased on purpose. The local part is technically case-sensitive; no mail
/// provider anybody uses treats it that way, and folding means "Ola@Kirka.no"
/// and "ola@kirka.no" cannot become two subscriptions to one inbox — which the
/// endpoint's per-address cap would then count separately.
pub fn normalize_address(raw: &str) -> Option<String> {
    let a = raw.trim();
    if a.is_empty() || a.len() > 254 || a.chars().any(char::is_whitespace) {
        return None;
    }
    let mut parts = a.split('@');
    let local = parts.next()?;
    let domain = parts.next()?;
    if parts.next().is_some() || local.is_empty() || domain.len() < 3 {
        return None;
    }
    // A domain with a dot, and nothing empty on either side of it.
    let (host, tld) = domain.rsplit_once('.')?;
    if host.is_empty() || tld.len() < 2 || domain.starts_with('.') || domain.ends_with('.') {
        return None;
    }
    Some(a.to_lowercase())
}

// ─────────────────────────────────────────────────────────────────────────────
//   Tokens
// ─────────────────────────────────────────────────────────────────────────────

/// How many random bytes a confirmation / unsubscribe token carries.
///
/// Thirty-two, i.e. 256 bits, which is not a guess: the token IS the
/// authorisation — anybody who has it can confirm the subscription or cancel it
/// — and it travels in a URL that lives in an inbox and in mail-server logs.
/// There is no rate limit that makes a short one safe, and no cost to a long
/// one.
const TOKEN_BYTES: usize = 32;

/// A fresh token as lowercase hex, from the OS CSPRNG.
///
/// Falls back to nothing: `getrandom` failing means the operating system cannot
/// produce randomness, and inventing some from a clock would produce a token
/// that LOOKS like the real thing while being guessable. The caller turns
/// `None` into a refusal.
fn mint_token() -> Option<String> {
    let mut buf = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut buf).ok()?;
    Some(to_hex(&buf))
}

/// The SHA-256 of a token, lowercase hex — the only form that ever leaves this
/// machine. Matches the endpoint's `^[0-9a-f]{64}$`.
fn hash_token(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

/// Lowercase hex. Four lines rather than a dependency, and pinned by a test —
/// the digits are the wire format the endpoint's regex matches.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The endpoint for this build, or the stable "no endpoint" refusal.
fn endpoint() -> AppResult<RelayEndpoint> {
    RelayEndpoint::resolve().ok_or_else(|| {
        AppError::Validation(format!(
            "{NO_ENDPOINT}: this build has no relay endpoint configured"
        ))
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//   The commands
// ─────────────────────────────────────────────────────────────────────────────

/// What the panel should show: the local record, plus whether this build could
/// use it.
///
/// Reads only. The freshness of `state` is the pump's business — it polls the
/// endpoint every fifteen minutes while a subscription is pending, and A5's page
/// asks again when it opens.
#[tauri::command]
pub async fn relay_status(db: State<'_, Db>) -> AppResult<RelaySubscriptionStatus> {
    RelaySubscriptionStatus::read(&db.pool).await
}

/// Enrol an address: mint an identity, render the confirmation mail, queue it.
///
/// The order matters and is the whole design. NOTHING is sent from this call —
/// the row goes in the outbox and the pump takes it from there, so a church
/// network that is down at the moment somebody presses the button costs a minute
/// rather than the sign-up. The local record is written in `pending` state
/// immediately, because it is what starts the pump ([`sender::should_run`]) and
/// what the panel reads back.
///
/// Re-enrolling replaces the record. That is the honest reading of "the user
/// typed a different address and pressed confirm": the old subscription's rows
/// are already queued with their own sub id and will still be delivered, and the
/// endpoint's own per-address caps are what stop this being a way to send mail
/// to strangers.
#[tauri::command]
pub async fn relay_subscribe(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    address: String,
) -> AppResult<RelaySubscriptionStatus> {
    let endpoint = endpoint()?;
    let address = normalize_address(&address).ok_or_else(|| {
        AppError::Validation(format!("{INVALID_ADDRESS}: not a usable e-mail address"))
    })?;

    let sub_id = store::new_id();
    let unsub_token = mint_token().ok_or_else(|| {
        AppError::Internal("the operating system could not produce a random token".into())
    })?;

    // The RECORD first, then the row — and the order is load-bearing, because
    // the two failure modes are not symmetric. This way round, a failure at the
    // second write leaves a `pending` record with nothing queued, which «Send
    // på nytt» fixes with the SAME sub id. The other way round leaves a queued
    // row with no record: the pump is shut, so it waits — and the next
    // successful enrolment opens the gate and sends BOTH, one of them a
    // confirmation for an identity this machine cannot confirm or unsubscribe.
    relay_store::subscription_set(
        &db.pool,
        &RelaySubscription {
            sub_id: sub_id.clone(),
            address: address.clone(),
            state: RelaySubscriptionState::Pending,
            enrolled_at: now_ms(),
            confirmed_at: None,
            last_checked: None,
            unsub_token: unsub_token.clone(),
        },
    )
    .await?;
    queue_confirmation(&db.pool, &endpoint, &sub_id, &address, &unsub_token).await?;

    // Start the pump if it is not running: without this, a user who enrols
    // mid-session has their confirmation mail sit in the outbox until the next
    // launch, which is indistinguishable from the feature being broken.
    sender::maybe_spawn(&app, &db.pool).await;
    sender::kick();
    RelaySubscriptionStatus::read(&db.pool).await
}

/// "Send it again" — a NEW confirmation token and a new subscribe row.
///
/// New, not the old one repeated, and for two reasons. The first is that the old
/// token was not kept ([`RelaySubscription`] documents why). The second is that
/// re-sending a live token would mean a link in an inbox somebody has since
/// stopped trusting stays valid for its full seven days no matter how many times
/// "send again" was pressed; minting replaces it.
///
/// The endpoint has a cooldown of its own (ten minutes). Pressing this inside it
/// gets a 429, which the outbox backs off and retries rather than dropping — so
/// an impatient click costs a wait, never a lost sign-up.
#[tauri::command]
pub async fn relay_resend(db: State<'_, Db>) -> AppResult<RelaySubscriptionStatus> {
    let endpoint = endpoint()?;
    let sub = relay_store::subscription_get(&db.pool)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!("{NOT_CONFIRMED}: no address is enrolled here"))
        })?;

    queue_confirmation(
        &db.pool,
        &endpoint,
        &sub.sub_id,
        &sub.address,
        &sub.unsub_token,
    )
    .await?;
    sender::kick();
    RelaySubscriptionStatus::read(&db.pool).await
}

/// Queue one subscribe row: mint the confirmation token, render the mail in the
/// user's language, check it against the endpoint's own limits, store it.
///
/// Shared by enrolment and re-send so the two cannot diverge — a resend that
/// rendered a different mail from the original would be a second first
/// impression.
async fn queue_confirmation(
    pool: &sqlx::SqlitePool,
    endpoint: &RelayEndpoint,
    sub_id: &str,
    address: &str,
    unsub_token: &str,
) -> AppResult<()> {
    let settings = crate::settings::load(pool).await.unwrap_or_default();
    let lang = MailLang::from_code(settings.language.as_deref());

    let confirm_token = mint_token().ok_or_else(|| {
        AppError::Internal("the operating system could not produce a random token".into())
    })?;

    let rendered = render_confirm(
        lang,
        alert_church(&settings.church_name),
        &alert_person(&settings.responsible_person, address),
        &endpoint.confirm_url(sub_id, &confirm_token),
    );
    // The confirmation is the one mail with no unsubscribe footer — there is no
    // subscription yet to leave. `RelayMessage::new` owns that rule; the empty
    // URL is what it ignores for this kind.
    let message = RelayMessage::new(RelayMessageKind::Confirm, lang, rendered, "");

    wire::queue_subscribe(
        pool,
        &SubscribeBody {
            sub_id,
            address,
            lang: lang.as_code(),
            confirm_hash: &hash_token(&confirm_token),
            unsub_hash: &hash_token(unsub_token),
            message: (&message).into(),
        },
        &message,
    )
    .await?;
    Ok(())
}

/// Ask the endpoint to forget this address.
///
/// ⚠️ The local record is NOT deleted here, and that is not an omission. The
/// pump is gated on the record existing, so clearing it now would strand the
/// very row this call queues — and the endpoint, never having heard anything,
/// would go on sending. The pump clears it when the row has left. See
/// `crate::notify::relay::store::subscription_clear` and
/// `sundayrec_core::relay::RelayGate`.
///
/// The panel therefore keeps showing a subscription for a moment after the
/// click. That is honest: it is still subscribed until the endpoint says
/// otherwise.
#[tauri::command]
pub async fn relay_unsubscribe(db: State<'_, Db>) -> AppResult<RelaySubscriptionStatus> {
    let sub = relay_store::subscription_get(&db.pool)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!("{NOT_CONFIRMED}: no address is enrolled here"))
        })?;

    wire::queue_unsubscribe(&db.pool, &sub.sub_id).await?;
    sender::kick();
    RelaySubscriptionStatus::read(&db.pool).await
}

/// "E-post virker" — queue the localized test message.
///
/// Requires a CONFIRMED subscription, because a test that could reach an
/// unconfirmed address would be a hole straight through the double opt-in: mail
/// to somebody who has not said yes, sent by a button labelled as harmless. The
/// pump's gate would refuse the row anyway; refusing here means the user gets a
/// sentence instead of a message that silently never leaves.
#[tauri::command]
pub async fn relay_send_test(db: State<'_, Db>) -> AppResult<()> {
    let endpoint = endpoint()?;
    let sub = relay_store::subscription_get(&db.pool)
        .await?
        .filter(|s| s.state == RelaySubscriptionState::Confirmed)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "{NOT_CONFIRMED}: confirm the address before sending a test"
            ))
        })?;

    let settings = crate::settings::load(&db.pool).await.unwrap_or_default();
    let lang = MailLang::from_code(settings.language.as_deref());
    let message = RelayMessage::new(
        RelayMessageKind::Test,
        lang,
        render_test(lang),
        &endpoint.unsubscribe_url(&sub.sub_id, &sub.unsub_token),
    );
    wire::queue_send(
        &db.pool,
        &sub.sub_id,
        RelayMessageKind::Test,
        &message,
        // Stamped, so pressing the button twice sends twice — which is what a
        // test button is for. Every other kind keys on the OCCURRENCE instead,
        // because two observers of one failure are one e-mail.
        format!("test:{}", now_ms()),
    )
    .await?;
    sender::kick();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── The address ──────────────────────────────────────────────────────────

    #[test]
    fn a_usable_address_is_trimmed_and_folded() {
        assert_eq!(
            normalize_address("  Ola@Kirka.NO "),
            Some("ola@kirka.no".into()),
            "folding means one inbox cannot become two subscriptions"
        );
        assert_eq!(
            normalize_address("frivillig+varsel@sundaysuite.app"),
            Some("frivillig+varsel@sundaysuite.app".into()),
            "plus-addressing is how somebody tests without a second inbox"
        );
        assert_eq!(
            normalize_address("a@b.co"),
            Some("a@b.co".into()),
            "short but real"
        );
    }

    #[test]
    fn the_typos_that_would_waste_a_confirmation_mail_are_refused() {
        for bad in [
            "",
            "   ",
            "ola",                    // no @
            "ola@",                   // no domain
            "@kirka.no",              // no local part
            "ola@kirka",              // no dot
            "ola@.no",                // empty host
            "ola@kirka.",             // empty tld
            "ola@kirka.n",            // one-letter tld
            "ola@@kirka.no",          // two @
            "ola kirka@kirka.no",     // a space
            "ola@kirka.no, per@x.no", // two addresses pasted at once
        ] {
            assert_eq!(normalize_address(bad), None, "{bad:?} should be refused");
        }
        // …and something absurdly long, which the endpoint caps too.
        let long = format!("{}@kirka.no", "a".repeat(300));
        assert_eq!(normalize_address(&long), None);
    }

    #[test]
    fn the_error_codes_are_bare_snake_tokens_the_renderer_can_extract() {
        // The general-page.ts seam: the wire message must LEAD with the code so
        // `errorCode()`'s `/^[a-z][a-z0-9_]*/` finds it. The prose after it is
        // free to be reworded in seven languages; the prefix is not.
        for code in [INVALID_ADDRESS, NO_ENDPOINT, NOT_CONFIRMED] {
            assert!(
                code.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{code}"
            );
            let err = AppError::Validation(format!("{code}: something human"));
            assert!(err.to_string().starts_with(&format!("validation: {code}")));
        }
    }

    // ── Tokens ───────────────────────────────────────────────────────────────

    #[test]
    fn a_token_is_sixty_four_hex_digits_and_never_the_same_twice() {
        let a = mint_token().expect("the OS must have randomness");
        let b = mint_token().expect("the OS must have randomness");
        assert_eq!(a.len(), TOKEN_BYTES * 2);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        assert_ne!(a, b, "a token that repeated would be no token at all");
    }

    #[test]
    fn only_the_hash_matches_what_the_endpoint_stores() {
        // The endpoint's column is `^[0-9a-f]{64}$`, and it never sees the
        // token — so a dump of it cannot confirm a subscription.
        let token = "0123456789abcdef".repeat(4);
        let h = hash_token(&token);
        assert_eq!(h.len(), 64);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        assert_ne!(h, token);
        assert_eq!(h, hash_token(&token), "and it is a function, not a nonce");
        // The known-answer test, so a swapped digest would be caught rather
        // than merely producing different-looking hex.
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    // ── The queued rows ──────────────────────────────────────────────────────

    use sundayrec_core::relay::RelayKind;

    async fn temp_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn test_endpoint() -> RelayEndpoint {
        RelayEndpoint::normalize(
            Some("https://notify.example".into()),
            Some("test-key".into()),
        )
        .expect("valid")
    }

    #[tokio::test]
    async fn enrolling_queues_a_confirmation_whose_link_carries_the_token_and_not_the_hash() {
        let (pool, _d) = temp_pool().await;
        queue_confirmation(
            &pool,
            &test_endpoint(),
            "sub-1",
            "ola@kirka.no",
            &"u".repeat(64),
        )
        .await
        .unwrap();

        let q = relay_store::load_queue(&pool).await.unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, RelayKind::Subscribe);
        assert_eq!(
            q[0].event, None,
            "a subscribe row carries no mail of its own"
        );

        let body: serde_json::Value = serde_json::from_str(&q[0].payload_json).unwrap();
        let text = body["message"]["text"].as_str().expect("a plaintext part");
        let confirm_hash = body["confirmHash"].as_str().unwrap();
        assert_eq!(confirm_hash.len(), 64);
        assert!(
            !text.contains(confirm_hash),
            "the mail must carry the TOKEN; the endpoint gets only the hash"
        );
        // The link in the mail hashes to exactly what was sent — the two halves
        // of the double opt-in, checked against each other.
        let token = text
            .split("/v1/notify/confirm/sub-1/")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("a confirmation link");
        assert_eq!(hash_token(token), confirm_hash);
        assert_eq!(body["address"], "ola@kirka.no");
        assert_eq!(body["lang"], "no");
    }

    #[tokio::test]
    async fn a_resend_is_a_new_row_and_a_retry_is_not() {
        let (pool, _d) = temp_pool().await;
        let ep = test_endpoint();
        queue_confirmation(&pool, &ep, "sub-1", "ola@kirka.no", &"u".repeat(64))
            .await
            .unwrap();
        queue_confirmation(&pool, &ep, "sub-1", "ola@kirka.no", &"u".repeat(64))
            .await
            .unwrap();
        let q = relay_store::load_queue(&pool).await.unwrap();
        assert_eq!(
            q.len(),
            2,
            "a new token is a genuinely different request and needs its own row"
        );
        assert_ne!(q[0].dedup_key, q[1].dedup_key);

        // …while the SAME row queued twice is the collision the index absorbs.
        let again = q[0].clone();
        let mut twin = again.clone();
        twin.id = "other".into();
        assert!(!relay_store::insert_capped(&pool, &twin).await.unwrap());
        assert_eq!(relay_store::load_queue(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_status_reports_an_empty_machine_without_inventing_a_subscription() {
        let (pool, _d) = temp_pool().await;
        let s = RelaySubscriptionStatus::read(&pool).await.unwrap();
        assert_eq!(s.state, None);
        assert_eq!(s.address, None);
        assert_eq!(s.queued, 0);

        relay_store::subscription_set(
            &pool,
            &RelaySubscription {
                sub_id: "sub-1".into(),
                address: "ola@kirka.no".into(),
                state: RelaySubscriptionState::Confirmed,
                enrolled_at: 10,
                confirmed_at: Some(20),
                last_checked: Some(30),
                unsub_token: "u".repeat(64),
            },
        )
        .await
        .unwrap();
        let s = RelaySubscriptionStatus::read(&pool).await.unwrap();
        assert_eq!(s.state, Some(RelaySubscriptionState::Confirmed));
        assert_eq!(s.address.as_deref(), Some("ola@kirka.no"));
        assert_eq!(s.enrolled_at, Some(10));
        assert_eq!(s.confirmed_at, Some(20));

        // The DTO carries nothing the panel has no use for.
        let json = serde_json::to_string(&s).expect("serialise");
        assert!(!json.contains("unsubToken"), "{json}");
        assert!(!json.contains("subId"), "{json}");
        assert!(!json.contains(&"u".repeat(64)), "{json}");
    }
}
