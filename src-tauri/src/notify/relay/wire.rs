//! The request bodies, and the three ways to put one in the outbox.
//!
//! Here rather than in `crate::commands::notify_relay` because the commands are
//! not the only caller. A2 queues subscribe / unsubscribe / test rows from a
//! button press; A3 queues failure, missed and receipt rows from the dispatcher,
//! which is not an IPC path at all. If the shapes lived beside the buttons, the
//! dispatcher would have to either import from `commands` — backwards, the thin
//! IPC layer is supposed to depend on the module, not the other way round — or
//! re-state them. Re-stating them is the one that actually hurts: a field spelled
//! differently in two places is a 400, and a 400 is a PERMANENT drop, so the
//! alert would not be retried, it would be gone.
//!
//! ## Why these types are not the ones the templates produce
//!
//! [`sundayrec_core::email::RelayMessage`] also carries `kind`, and the
//! endpoint's validator rejects unknown fields. So the `message` object on the
//! wire is [`WireMessage`] — subject, text, html, nothing else — and the kind
//! goes where each route expects it: at the top level of a send body, and
//! nowhere at all in a subscribe body, where the mail is a confirmation by
//! definition.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use sundayrec_core::email::{RelayMessage, RelayMessageKind};
use sundayrec_core::relay::{RelayEntry, RelayKind, RelayStatus};

use super::store;
use crate::db::store as db_store;
use crate::error::{AppError, AppResult};
use crate::util::now_ms;

/// The `message` object every request body carries.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireMessage<'a> {
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
}

impl<'a> From<&'a RelayMessage> for WireMessage<'a> {
    fn from(m: &'a RelayMessage) -> Self {
        Self {
            subject: &m.subject,
            text: &m.text,
            html: &m.html,
        }
    }
}

/// `POST /v1/apps/sundayrec/notify/subscribe`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscribeBody<'a> {
    pub sub_id: &'a str,
    pub address: &'a str,
    pub lang: &'a str,
    pub confirm_hash: &'a str,
    pub unsub_hash: &'a str,
    pub message: WireMessage<'a>,
}

/// `POST /v1/apps/sundayrec/notify/send`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendBody<'a> {
    pub sub_id: &'a str,
    pub kind: RelayMessageKind,
    pub message: WireMessage<'a>,
}

/// The body of an `unsubscribe` row.
///
/// ONE type with both halves, written by the queueing side and read by the
/// sender, which needs the id to build the `DELETE` URL. Two structs for one
/// body would be two things that can disagree — and the row is read back after
/// the local record it came from has been deleted, so there would be nothing
/// left to check it against.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnsubscribeBody<'a> {
    #[serde(borrow)]
    pub sub_id: std::borrow::Cow<'a, str>,
}

/// A queued row, with the fields every kind shares filled in one place.
fn queued_row(
    kind: RelayKind,
    event: Option<RelayMessageKind>,
    dedup_key: String,
    body: String,
) -> RelayEntry {
    let now = now_ms();
    RelayEntry {
        id: db_store::new_id(),
        created_at: now,
        kind,
        event,
        dedup_key,
        payload_json: body,
        attempts: 0,
        next_attempt: now,
        last_error: None,
        status: RelayStatus::Pending,
    }
}

/// Refuse a message the endpoint's validator would refuse.
///
/// A 400 is a PERMANENT drop, so queueing an over-long message would spend a
/// queue slot to learn what [`RelayMessage::fits`] already knows — and lose the
/// alert. Whatever produced it is a renderer bug or an absurd input, and both
/// belong in the log rather than in a badge the user can neither cause nor fix.
pub(crate) fn guard_size(message: &RelayMessage) -> AppResult<()> {
    if message.fits() {
        return Ok(());
    }
    tracing::error!(
        kind = message.kind.as_str(),
        subject_chars = message.subject.chars().count(),
        text_chars = message.text.chars().count(),
        html_chars = message.html.chars().count(),
        "relay: a rendered message does not fit the endpoint's limits and was NOT queued"
    );
    Err(AppError::Internal(
        "the rendered message does not fit the endpoint's limits".into(),
    ))
}

/// Queue one subscribe request. Returns whether a row was added.
pub(crate) async fn queue_subscribe(
    pool: &SqlitePool,
    body: &SubscribeBody<'_>,
    message: &RelayMessage,
) -> AppResult<bool> {
    guard_size(message)?;
    // The dedup key carries the confirmation token's HASH, not the sub id: a
    // "send it again" is a genuinely different request (a new token) and must
    // queue its own row, while the outbox retrying the same one must not.
    let dedup_key = format!("subscribe:{}", body.confirm_hash);
    store::insert_capped(
        pool,
        &queued_row(
            RelayKind::Subscribe,
            None,
            dedup_key,
            serde_json::to_string(body)?,
        ),
    )
    .await
}

/// Queue one already-rendered notification. Returns whether a row was added —
/// `false` means an identical one was already waiting, which is the answer the
/// caller wants when two observers report the same event.
///
/// THE door A3's dispatcher uses. It takes a rendered [`RelayMessage`] because
/// the client renders and the endpoint forwards: what is stored is the mail as
/// it will be sent, so a row queued by version N is sent unchanged by N+1.
pub(crate) async fn queue_send(
    pool: &SqlitePool,
    sub_id: &str,
    kind: RelayMessageKind,
    message: &RelayMessage,
    dedup_key: String,
) -> AppResult<bool> {
    guard_size(message)?;
    let body = serde_json::to_string(&SendBody {
        sub_id,
        kind,
        message: message.into(),
    })?;
    store::insert_capped(
        pool,
        &queued_row(RelayKind::Send, Some(kind), dedup_key, body),
    )
    .await
}

/// Queue the request to forget this address.
///
/// ⚠️ Does NOT touch the local subscription record, and must not: the pump is
/// gated on that record, so clearing it here would strand the row this function
/// just wrote and the endpoint would go on sending. The pump clears it once the
/// row has left — see [`super::store::subscription_clear`].
pub(crate) async fn queue_unsubscribe(pool: &SqlitePool, sub_id: &str) -> AppResult<bool> {
    let body = serde_json::to_string(&UnsubscribeBody {
        sub_id: sub_id.into(),
    })?;
    store::insert_capped(
        pool,
        &queued_row(
            RelayKind::Unsubscribe,
            None,
            format!("unsubscribe:{sub_id}"),
            body,
        ),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_message(kind: RelayMessageKind) -> RelayMessage {
        RelayMessage {
            kind,
            subject: "Emne".into(),
            text: "Tekst".into(),
            html: "<p>Tekst</p>".into(),
        }
    }

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = db_store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    #[test]
    fn a_message_on_the_wire_carries_three_fields_and_no_kind() {
        // THE trap this type exists for: `RelayMessage` also serialises
        // `kind`, and the endpoint rejects unknown fields with a 400 — which
        // the outbox reads as permanent and DROPS. The alert would be gone,
        // not retried.
        let m = a_message(RelayMessageKind::Failure);
        let json = serde_json::to_string(&WireMessage::from(&m)).expect("serialise");
        assert_eq!(
            json,
            r#"{"subject":"Emne","text":"Tekst","html":"<p>Tekst</p>"}"#
        );
        assert!(
            serde_json::to_string(&m).unwrap().contains("\"kind\""),
            "and the difference is real — the source type does carry one"
        );
    }

    #[test]
    fn the_subscribe_body_is_the_shape_the_endpoint_validates() {
        let m = a_message(RelayMessageKind::Confirm);
        let json = serde_json::to_string(&SubscribeBody {
            sub_id: "sub-1",
            address: "ola@kirka.no",
            lang: "no",
            confirm_hash: &"a".repeat(64),
            unsub_hash: &"b".repeat(64),
            message: (&m).into(),
        })
        .expect("serialise");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "address",
                "confirmHash",
                "lang",
                "message",
                "subId",
                "unsubHash"
            ],
            "camelCase, and no field the validator has not been told about — an unknown \
             one is a 400, which the outbox drops rather than retries"
        );
        assert_eq!(v["message"].as_object().unwrap().len(), 3);
    }

    #[test]
    fn the_send_body_names_the_kind_at_the_top_level() {
        let m = a_message(RelayMessageKind::Test);
        let json = serde_json::to_string(&SendBody {
            sub_id: "sub-1",
            kind: RelayMessageKind::Test,
            message: (&m).into(),
        })
        .expect("serialise");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(v["kind"], "test", "one of the endpoint's five closed words");
        assert!(v["message"].get("kind").is_none());
        for kind in RelayMessageKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().into())
            );
        }
    }

    #[test]
    fn the_unsubscribe_body_round_trips_through_the_row_it_is_stored_in() {
        // Written by the button, read by the sender AFTER the local record is
        // gone — so this round-trip is the only thing checking the two halves
        // against each other.
        let json = serde_json::to_string(&UnsubscribeBody {
            sub_id: "sub-1".into(),
        })
        .expect("serialise");
        assert_eq!(json, r#"{"subId":"sub-1"}"#);
        let back: UnsubscribeBody = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.sub_id, "sub-1");
    }

    #[test]
    fn an_oversized_message_is_refused_rather_than_queued() {
        let mut m = a_message(RelayMessageKind::Failure);
        assert!(guard_size(&m).is_ok());
        m.subject = "æ".repeat(sundayrec_core::email::SUBJECT_MAX_CHARS + 1);
        assert!(guard_size(&m).is_err());
        m = a_message(RelayMessageKind::Failure);
        m.html = String::new();
        assert!(
            guard_size(&m).is_err(),
            "both body parts are required — a text-only mail reads as bulk"
        );
    }

    #[tokio::test]
    async fn the_send_door_absorbs_a_second_observer_of_the_same_event() {
        // What A3's dispatcher relies on: `check_missed` runs at startup and
        // after every wake, and one missed Sunday must not become two mails.
        let (pool, _d) = temp_pool().await;
        let m = a_message(RelayMessageKind::Missed);
        let key = "missed:2026-09-06T11:00".to_string();
        assert!(
            queue_send(&pool, "sub-1", RelayMessageKind::Missed, &m, key.clone())
                .await
                .unwrap()
        );
        assert!(
            !queue_send(&pool, "sub-1", RelayMessageKind::Missed, &m, key)
                .await
                .unwrap(),
            "the same occurrence must not queue twice"
        );
        let q = store::load_queue(&pool).await.unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, RelayKind::Send);
        assert_eq!(q[0].event, Some(RelayMessageKind::Missed));
    }

    #[tokio::test]
    async fn an_oversized_alert_never_reaches_the_queue() {
        let (pool, _d) = temp_pool().await;
        let mut m = a_message(RelayMessageKind::Failure);
        m.text = String::new();
        assert!(
            queue_send(&pool, "sub-1", RelayMessageKind::Failure, &m, "k".into())
                .await
                .is_err()
        );
        assert!(store::load_queue(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn queueing_an_unsubscribe_leaves_the_local_record_alone() {
        // The trap, at the door rather than at the store: whoever writes this
        // row must not also clear the record it is gated on.
        let (pool, _d) = temp_pool().await;
        store::subscription_set(
            &pool,
            &super::super::RelaySubscription {
                sub_id: "sub-1".into(),
                address: "ola@kirka.no".into(),
                state: super::super::RelaySubscriptionState::Confirmed,
                enrolled_at: 0,
                confirmed_at: None,
                last_checked: None,
                unsub_token: "u".repeat(64),
            },
        )
        .await
        .unwrap();
        assert!(queue_unsubscribe(&pool, "sub-1").await.unwrap());
        assert!(
            store::subscription_get(&pool).await.unwrap().is_some(),
            "clearing it here would strand the row that was just written"
        );
        // …and queueing it twice is absorbed by the dedup index.
        assert!(!queue_unsubscribe(&pool, "sub-1").await.unwrap());
    }
}
