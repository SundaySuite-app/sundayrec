//! The real sender — the first implementation of [`TelemetrySender`].
//!
//! E3 shipped the trait, the outbox and the consent gate with no implementation
//! at all, which was the strongest available version of "nothing sends
//! anywhere". This is the network half, and it is deliberately small: build a
//! request, POST it, turn the status into a decision. Everything interesting —
//! whether to send at all, what to send, when to retry — already happened
//! before `send` is called.
//!
//! ## The status contract
//!
//! The one judgement this file makes is [`classify`], and it exists because the
//! outbox's backoff ladder answers the wrong question for half of the failures.
//! The ladder is for "the church wifi is down": wait, retry, the same bytes
//! succeed later. It is exactly wrong for "this payload is malformed", which is
//! malformed the same way all six times.
//!
//! So the endpoint's status codes carry a second meaning, agreed on both sides:
//!
//! | Status | Means | This client |
//! |---|---|---|
//! | 2xx | stored | drop from the outbox (success) |
//! | 400 | schema violation | **drop, do not retry** |
//! | 401 / 403 | key rejected | **drop, do not retry** |
//! | 413 | over the size cap | **drop, do not retry** |
//! | 408 / 429 | timeout / rate limit | back off and retry |
//! | other 4xx | unrecognised refusal | **drop, do not retry** |
//! | 5xx | endpoint's fault | back off and retry |
//! | transport error | network's fault | back off and retry |
//!
//! The endpoint's half is `sunday-telemetry/src/ingest.ts`, whose 400 responses
//! carry `"retryable": false` and name every failing field, and whose 429 sets
//! `Retry-After`. Both sides cite the other; a contract documented on one side
//! only is a comment.
//!
//! 429 is the interesting entry: the only 4xx that IS transient, and the one a
//! blanket "4xx means stop" rule would get wrong by throwing away a payload the
//! endpoint explicitly asked us to send again later.
//!
//! ## What is NOT here
//!
//! No retry loop, no timer, no consent check. `send` performs exactly one
//! request and returns. The loop is [`super::sender::pump_once`], the schedule
//! is the outbox's `next_attempt`, and the consent gate is
//! `queue::pump_decision` — which runs before this type is even consulted.

use std::time::Duration;

use reqwest::StatusCode;

use super::config::TelemetryEndpoint;
use super::sender::{SendFailure, SendFuture, TelemetrySender};
use crate::util::http_client;

/// The header the endpoint reads the write key from.
pub const WRITE_KEY_HEADER: &str = "x-sundayrec-key";

/// A per-request cap well under the shared client's 120 s.
///
/// A telemetry POST is a couple of kilobytes to an edge worker; if it has not
/// answered in fifteen seconds it is not going to, and the outbox will try again
/// in a minute. The shared client's timeout is sized for a resumable upload
/// chunk on a slow link, which is a different problem.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Posts payloads to the configured endpoint.
pub struct HttpTelemetrySender {
    client: reqwest::Client,
    endpoint: TelemetryEndpoint,
}

impl HttpTelemetrySender {
    /// Build a sender for `endpoint`, reusing the bounded rustls client the
    /// cloud paths use — a bare `Client::new()` has NO timeout, so a server that
    /// accepts the request and never answers would wedge the pump forever.
    pub fn new(endpoint: TelemetryEndpoint) -> Self {
        Self {
            client: http_client(),
            endpoint,
        }
    }

    /// The endpoint this sender was built for.
    pub fn endpoint(&self) -> &TelemetryEndpoint {
        &self.endpoint
    }

    /// Ask the endpoint to delete every row for a retired install id.
    ///
    /// The remote half of "slett dataene mine". `Ok(())` means the endpoint
    /// confirmed — including for an id it has never seen, which it reports as a
    /// success with zero counts rather than a 404, so a machine that retired an
    /// id before ever sending anything does not retry forever.
    pub async fn delete_install(&self, install_id: &str) -> Result<(), SendFailure> {
        let res = self
            .client
            .delete(self.endpoint.delete_url(install_id))
            .header(WRITE_KEY_HEADER, &self.endpoint.write_key)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;

        match res {
            Ok(r) => classify(r.status()),
            Err(e) => Err(SendFailure::Transient(transport_error(&e))),
        }
    }
}

impl TelemetrySender for HttpTelemetrySender {
    fn send<'a>(&'a self, payload_json: &'a str) -> SendFuture<'a> {
        Box::pin(async move {
            let res = self
                .client
                .post(&self.endpoint.ingest_url)
                .header(WRITE_KEY_HEADER, &self.endpoint.write_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(REQUEST_TIMEOUT)
                .body(payload_json.to_string())
                .send()
                .await;

            match res {
                Ok(r) => classify(r.status()),
                Err(e) => Err(SendFailure::Transient(transport_error(&e))),
            }
        })
    }
}

/// Turn one HTTP status into the outbox's decision. See the module table.
///
/// Pure, and separated from the request precisely so it can be exhaustively
/// tested without a socket — the classification is the part with a wrong answer,
/// and the wrong answer is invisible until a deploy starts 4xx-ing the fleet.
pub fn classify(status: StatusCode) -> Result<(), SendFailure> {
    if status.is_success() {
        return Ok(());
    }
    // The two transient 4xx, named before the blanket rule below.
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::REQUEST_TIMEOUT {
        return Err(SendFailure::Transient(format!("endpoint said {status}")));
    }
    if status.is_client_error() {
        // Every other 4xx says this payload is unacceptable. Re-sending the same
        // bytes cannot change that, so the entry is dropped rather than put on a
        // ladder that reaches 24 hours before giving up.
        return Err(SendFailure::Permanent(format!(
            "endpoint rejected the payload: {status}"
        )));
    }
    // 5xx and anything else unrecognised: assume it is ours to wait out. The
    // conservative direction — a payload kept too long costs a queue slot, a
    // payload dropped wrongly is gone.
    Err(SendFailure::Transient(format!("endpoint said {status}")))
}

/// A short, path-free description of a transport failure.
///
/// `reqwest`'s `Display` includes the full URL, which is fine here (the URL is a
/// build constant, not user data) but is noisy in a settings panel. This keeps
/// the CATEGORY, which is the part that helps.
///
/// Shared with the e-mail relay's sender (`crate::notify::relay::sender`), which
/// posts to the same Worker over the same client: a second copy of these four
/// sentences would be two vocabularies for one set of network conditions,
/// visible to the user as two pipes describing the same dead wifi differently.
pub(crate) fn transport_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the endpoint did not answer in time".to_string()
    } else if e.is_connect() {
        "could not reach the endpoint".to_string()
    } else if e.is_request() {
        "the request could not be sent".to_string()
    } else {
        "network error".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_any_2xx() {
        // The endpoint answers 202 Accepted, but 200 must work too — pinning the
        // exact code would make a benign endpoint change look like a rejection.
        for code in [200u16, 201, 202, 204] {
            assert!(
                classify(StatusCode::from_u16(code).unwrap()).is_ok(),
                "{code} should be a success"
            );
        }
    }

    #[test]
    fn a_schema_rejection_is_permanent() {
        // THE case this classification exists for. A 400 six times over 24 hours
        // is six identical rejections of the same bytes.
        let e = classify(StatusCode::BAD_REQUEST).unwrap_err();
        assert!(matches!(e, SendFailure::Permanent(_)), "got {e:?}");
    }

    #[test]
    fn auth_and_size_failures_are_permanent_too() {
        for code in [401u16, 403, 413, 404, 405, 422] {
            let e = classify(StatusCode::from_u16(code).unwrap()).unwrap_err();
            assert!(
                matches!(e, SendFailure::Permanent(_)),
                "{code} should be permanent, got {e:?}"
            );
        }
    }

    #[test]
    fn rate_limiting_is_the_one_transient_4xx() {
        // A blanket "4xx means stop" would throw away a payload the endpoint
        // explicitly asked us to send again later.
        let e = classify(StatusCode::TOO_MANY_REQUESTS).unwrap_err();
        assert!(matches!(e, SendFailure::Transient(_)), "got {e:?}");
        let e = classify(StatusCode::REQUEST_TIMEOUT).unwrap_err();
        assert!(matches!(e, SendFailure::Transient(_)), "got {e:?}");
    }

    #[test]
    fn server_errors_are_transient() {
        for code in [500u16, 502, 503, 504] {
            let e = classify(StatusCode::from_u16(code).unwrap()).unwrap_err();
            assert!(
                matches!(e, SendFailure::Transient(_)),
                "{code} should be transient, got {e:?}"
            );
        }
    }

    #[test]
    fn an_unrecognised_status_is_kept_rather_than_dropped() {
        // A payload kept too long costs a queue slot; a payload dropped wrongly
        // is gone. Unknown non-4xx errs toward keeping.
        let e = classify(StatusCode::from_u16(599).unwrap()).unwrap_err();
        assert!(matches!(e, SendFailure::Transient(_)), "got {e:?}");
    }

    #[test]
    fn the_message_never_carries_a_url() {
        // The last error is shown in the settings panel. It should say what went
        // wrong, not print an endpoint at a user.
        for status in [StatusCode::BAD_REQUEST, StatusCode::TOO_MANY_REQUESTS] {
            let msg = format!("{:?}", classify(status).unwrap_err());
            assert!(!msg.contains("http"), "{msg}");
        }
    }
}

/// Live tests against a running endpoint — `wrangler dev`, or production.
///
/// Ignored by default, because they need a server. What they buy that the
/// socket-level tests cannot: the endpoint's validator is a TypeScript
/// re-statement of a Rust contract, and the only way to know the two agree is to
/// serialise the Rust types and let the real validator judge them. A drift
/// between the two — a renamed field, an enum variant spelled differently — is
/// invisible to every test on either side alone and shows up here as a 400.
///
/// ```sh
/// # against `npx wrangler dev` in sunday-telemetry/
/// SUNDAYREC_TELEMETRY_URL=http://127.0.0.1:8787 \
/// SUNDAYREC_TELEMETRY_KEY=dev-write-key-not-a-real-secret \
///   cargo test -p sundayrec live_endpoint -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_endpoint {
    // The full-pipeline test below holds `counters::test_lock()` — a std `Mutex`
    // — across awaits, exactly as the other telemetry test modules do, and for
    // the same reason: it serialises against process-global counter state.
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use sundayrec_core::diagnostics::DiagnosticSeverity;
    use sundayrec_core::selftest::SelfTestVerdict;
    use sundayrec_core::telemetry::{
        CounterName, CounterReport, CrashKind, CrashReport, FindingReport, QualityReason,
        QualityReport, TelemetryPayload, WakeFailureReport,
    };
    use sundayrec_core::wake::WakeFailureKind;

    fn endpoint() -> TelemetryEndpoint {
        TelemetryEndpoint::resolve()
            .expect("set SUNDAYREC_TELEMETRY_URL and SUNDAYREC_TELEMETRY_KEY — see the module docs")
    }

    /// Every collection populated, every optional present. A minimal payload
    /// would prove only that the header matches.
    fn maximal(install_id: &str) -> TelemetryPayload {
        let now = crate::telemetry::now_ms();
        let mut p = TelemetryPayload::new(install_id, 1, "0.10.0", now);
        p.language = Some("nb".into());
        p.counters = vec![
            CounterReport {
                name: CounterName::RecordingStopped,
                value: 4,
            },
            CounterReport {
                name: CounterName::EditorExportMp3,
                value: 3,
            },
        ];
        p.crashes = vec![CrashReport {
            kind: CrashKind::Panic,
            at: now - 3_600_000,
            app_version: "0.9.9".into(),
            os: sundayrec_core::telemetry::TelemetryOs::Macos,
            message: "called `Option::unwrap()` on a `None` value".into(),
            location: Some("src/recorder/engine.rs:2588:9".into()),
            task: None,
            backtrace_present: true,
        }];
        p.quality = vec![QualityReport {
            at: now - 7_200_000,
            duration_sec: 3600.5,
            expected_sec: 3600.0,
            measured_sec: 3600.0,
            loss_pct: 0.0,
            gap_sec: 0.0,
            drops: 0,
            dups: 0,
            xruns: 0,
            levels_dropped: 0,
            capture_drop_lines: 0,
            msgs_dropped: 0,
            ring_overrun_samples: 0,
            exit_ok: true,
            verdict: Some(SelfTestVerdict::Pass),
            reasons: vec![QualityReason::Clean],
        }];
        p.findings = vec![FindingReport {
            code: "SR-AUDIO-01".into(),
            severity: DiagnosticSeverity::Warning,
        }];
        p.wake_failures = vec![WakeFailureReport {
            kind: WakeFailureKind::Missed,
            at: now - 86_400_000,
            reason: Some("on_battery".into()),
            delta_sec: None,
        }];
        p
    }

    #[tokio::test]
    #[ignore = "needs a running telemetry endpoint"]
    async fn the_rust_types_serialise_to_something_the_validator_accepts() {
        let endpoint = endpoint();
        let sender = HttpTelemetrySender::new(endpoint.clone());
        let install_id = uuid::Uuid::now_v7().to_string();
        let json = serde_json::to_string(&maximal(&install_id)).expect("serialise");
        println!("POST {} ({} bytes)", endpoint.ingest_url, json.len());

        // The whole point: no hand-written fixture, the actual serde output.
        sender
            .send(&json)
            .await
            .expect("the endpoint must accept a payload built from the real types");
        println!("accepted; installId = {install_id}");

        // …and the deletion route must accept the same id back.
        sender
            .delete_install(&install_id)
            .await
            .expect("the endpoint must accept the deletion");
        println!("deleted {install_id}");
    }

    #[tokio::test]
    #[ignore = "needs a running telemetry endpoint"]
    async fn an_unknown_field_is_refused_permanently() {
        // The contract, live: a 4xx that the outbox must not retry.
        let sender = HttpTelemetrySender::new(endpoint());
        let install_id = uuid::Uuid::now_v7().to_string();
        let mut v = serde_json::to_value(maximal(&install_id)).expect("serialise");
        v.as_object_mut()
            .unwrap()
            .insert("churchName".into(), serde_json::json!("Nordstrand kirke"));

        let err = sender
            .send(&v.to_string())
            .await
            .expect_err("an unknown field must be refused");
        println!("refused as expected: {err:?}");
        assert!(
            matches!(err, SendFailure::Permanent(_)),
            "a schema rejection must be permanent, or the outbox will retry it six times: {err:?}"
        );
    }

    /// The whole path, once, with nothing stubbed.
    ///
    /// A real crash record on disk → the real drain builds a payload from it →
    /// the real outbox stores it → the real sender POSTs it → the endpoint
    /// accepts it and the outbox empties. Every other test in this repo cuts
    /// this path somewhere; this one is here to catch the seam that only breaks
    /// when all of it runs together.
    #[tokio::test]
    #[ignore = "needs a running telemetry endpoint"]
    async fn a_crash_on_disk_reaches_the_endpoint() {
        use crate::telemetry::{consent_set, drain_in, payload, queue_store, sender};

        let _guard = crate::telemetry::counters::test_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");

        // Consent, with the watermarks wound back so a record written "an hour
        // ago" counts as new. Granting alone stamps them at NOW, which is right
        // in production and useless for a test that cannot wait an hour.
        consent_set(&pool, true).await.unwrap();
        let since = crate::telemetry::now_ms() - 120 * 60_000;
        payload::set_watermarks(
            &pool,
            payload::Watermarks {
                crash_at: since,
                quality_at: since,
                wake_at: since,
            },
        )
        .await
        .unwrap();

        // A crash record, written exactly as the panic hook writes one.
        let crashes = dir.path().join("crashes");
        std::fs::create_dir_all(&crashes).unwrap();
        let ts = (chrono::Local::now() - chrono::Duration::minutes(60)).to_rfc3339();
        std::fs::write(
            crashes.join("crash-e2e-0000.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 1, "kind": "panic", "timestamp": ts, "appVersion": "0.10.0",
                "os": "macos", "arch": "aarch64", "thread": "main",
                "message": "E4 end-to-end probe", "location": null,
                "backtrace": null, "task": null,
            }))
            .unwrap(),
        )
        .unwrap();

        // Drain: the real builder, the real sanitizers, the real outbox.
        assert!(
            drain_in(&pool, dir.path(), "0.10.0").await.unwrap(),
            "the drain should have queued the crash"
        );
        let queued = queue_store::load_queue(&pool).await.unwrap();
        assert_eq!(queued.len(), 1);
        assert!(queued[0].payload_json.contains("E4 end-to-end probe"));
        let install_id: String = serde_json::from_str::<serde_json::Value>(&queued[0].payload_json)
            .unwrap()["installId"]
            .as_str()
            .unwrap()
            .to_string();
        println!("queued 1 payload for install {install_id}");

        // Send: the real HTTP client against the real endpoint.
        let s = HttpTelemetrySender::new(endpoint());
        assert_eq!(
            sender::pump_once(&pool, &s, crate::telemetry::now_ms())
                .await
                .unwrap(),
            sender::PumpOutcome::Sent
        );
        assert!(
            queue_store::load_queue(&pool).await.unwrap().is_empty(),
            "a delivered payload leaves the outbox"
        );
        println!("delivered; verify the row with:");
        println!(
            "  npx wrangler d1 execute sundayrec-telemetry --local --command \\\n    \"SELECT * FROM event_crashes c JOIN events e ON e.id=c.event_id WHERE e.install_id='{install_id}'\""
        );

        // And the deletion path, so the test leaves nothing behind — unless you
        // are here to look at the row it just wrote, in which case
        // SUNDAYREC_TELEMETRY_KEEP=1 leaves it for the query printed above.
        if std::env::var("SUNDAYREC_TELEMETRY_KEEP").as_deref() == Ok("1") {
            println!("SUNDAYREC_TELEMETRY_KEEP=1 — leaving the row in place");
            return;
        }
        s.delete_install(&install_id).await.expect("delete");
        println!("deleted {install_id}");
    }
}
