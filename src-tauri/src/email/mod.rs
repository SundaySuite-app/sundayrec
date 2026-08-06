//! Email-alert plumbing (PU-1 P2b) — **NETWORK-UNVERIFIED**, `email` feature (IN `default`).
//!
//! The impure half of the error/test mailer. Every *decision* — the localized
//! templates, the throttle/dedup gate, the RFC 2822 message + base64url
//! assembly — lives in the unit-tested [`sundayrec_core::email`]. This module
//! only performs the single side effect each path needs:
//!   - the Gmail-API send (a `reqwest` POST of the base64url raw message), reusing
//!     the cloud OAuth refresh-token machinery exactly like `oauth_flow.rs` does,
//!   - the SMTP send (a `lettre` async transport).
//!
//! Mirrors the Electron `src/main/mailer.ts` `sendError`/`sendTest` two-path
//! design (prefer Gmail OAuth when connected, fall back to SMTP). Whether to
//! send at all is the core [`AlertGate`]'s call; the shell holds one gate in
//! managed state and records a successful dispatch.
//!
//! ## ⚠️ NETWORK-UNVERIFIED
//!
//! The Gmail POST and the SMTP handshake are wired + compile under
//! `--features email` (now part of `default` AND of both release feature lists),
//! but the wire behaviour against a real provider (a real token, a reachable
//! SMTP server, deliverability) is only provable on a real account + network —
//! see docs/SMOKE-TEST.md. A `--no-default-features` build excludes this module
//! entirely and the commands return `feature_disabled`.

use std::sync::Mutex;

use sundayrec_core::cloud::{oauth, CloudService, GOOGLE_TOKEN_URL};
use sundayrec_core::email::{
    base64url, build_mime, render_error, render_test, AlertDecision, AlertGate, MailLang,
    RenderedEmail,
};

use crate::cloud::config::GoogleOAuthConfig;
use crate::cloud::{now_ms, secret_provider_for};
use crate::error::{AppError, AppResult};
use crate::util::lock_recover;

/// Gmail `messages.send` endpoint (base64url raw message in the JSON body).
const GMAIL_SEND_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages/send";

/// Opt-in that lets [`send_via_smtp`] speak **plaintext** instead of TLS, so the
/// integration test below can point it at a `127.0.0.1` socket. See
/// [`plaintext_test_transport`].
const PLAINTEXT_TEST_ENV: &str = "SUNDAYREC_SMTP_PLAINTEXT_TEST";

/// Whether to build a cleartext SMTP transport. BOTH halves are load-bearing:
///
/// - the env var makes it explicit and off by default, and
/// - `cfg!(debug_assertions)` means a SHIPPED (release) build folds this to a
///   constant `false`. An env var that downgrades TLS is a credential-leak
///   primitive — anything that can set `SUNDAYREC_SMTP_PLAINTEXT_TEST=1` in the
///   app's environment could otherwise make a real alert send the SMTP password
///   in the clear to a real server. In a release binary there is no such switch.
fn plaintext_test_transport() -> bool {
    cfg!(debug_assertions) && std::env::var(PLAINTEXT_TEST_ENV).is_ok_and(|v| v == "1")
}

/// Which transport to use for a send. Mirrors `mailer.ts`'s "prefer Gmail OAuth
/// when connected, else SMTP" choice — the caller resolves it from settings +
/// the keychain.
pub enum Transport {
    /// Send via the Gmail API using the stored Gmail OAuth refresh token.
    Gmail { config: GoogleOAuthConfig },
    /// Send via an SMTP server. `pass` is the app-password / SMTP secret.
    Smtp {
        host: String,
        port: u16,
        user: Option<String>,
        pass: String,
        from: String,
    },
}

/// Managed-state wrapper around the pure [`AlertGate`] so the Tauri layer can
/// share one throttle window across the app's lifetime.
#[derive(Default)]
pub struct AlertGateState {
    inner: Mutex<AlertGate>,
}

impl AlertGateState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide whether to send (delegates to the core gate at `now_ms`).
    pub fn decide(&self, recipient: &str, error_message: &str, now: i64) -> AlertDecision {
        lock_recover(&self.inner).decide(recipient, error_message, now)
    }

    /// Record a dispatched alert, opening the throttle window.
    pub fn record_sent(&self, recipient: &str, error_message: &str, now: i64) {
        lock_recover(&self.inner).record_sent(recipient, error_message, now)
    }
}

/// Send a recording-error alert to `recipient`, gated by [`AlertGateState`].
/// Returns `Ok(true)` when a mail was dispatched, `Ok(false)` when the gate
/// suppressed it (no recipient / throttled). `date` is the already-localized
/// human date string (the caller formats it; see [`MailLang::locale`]).
#[allow(clippy::too_many_arguments)]
pub async fn send_error_alert(
    gate: &AlertGateState,
    transport: &Transport,
    recipient: &str,
    lang_code: Option<&str>,
    church: &str,
    person: &str,
    date: &str,
    error_message: &str,
) -> AppResult<bool> {
    let now = now_ms();
    let decided = match gate.decide(recipient, error_message, now) {
        AlertDecision::Send { recipient } => recipient,
        AlertDecision::NoRecipient | AlertDecision::Throttled => return Ok(false),
    };
    let lang = MailLang::from_code(lang_code);
    let rendered = render_error(lang, church, person, date, error_message);
    dispatch(transport, &decided, &rendered).await?;
    gate.record_sent(&decided, error_message, now);
    Ok(true)
}

/// Send a message the caller has already rendered — the review-reminder path
/// ([`sundayrec_core::email::render_reminder`]).
///
/// Ungated on purpose, and the reason matters: the [`AlertGate`] exists to stop
/// a *flapping* recorder from mailing the same error forty times, which is a
/// hazard a reminder does not have. The review queue's own `reminded` counter is
/// the throttle — it is bumped and PERSISTED before this is ever called, so each
/// rung mails exactly once even if the app restarts mid-send. Putting the gate
/// in front of this as well would silently swallow two different episodes'
/// reminders that happen to fall in the same ten minutes.
pub async fn send_rendered(
    transport: &Transport,
    recipient: &str,
    rendered: &RenderedEmail,
) -> AppResult<()> {
    let recipient = recipient.trim();
    if recipient.is_empty() {
        return Err(AppError::Validation("no_config".into()));
    }
    dispatch(transport, recipient, rendered).await
}

/// Send a localized "email works" test message to `recipient`. Ungated (the user
/// explicitly asked to test). Errors if `recipient` is blank.
pub async fn send_test(
    transport: &Transport,
    recipient: &str,
    lang_code: Option<&str>,
) -> AppResult<()> {
    let recipient = recipient.trim();
    if recipient.is_empty() {
        return Err(AppError::Validation("no_config".into()));
    }
    let rendered = render_test(MailLang::from_code(lang_code));
    dispatch(transport, recipient, &rendered).await
}

/// Perform the single side effect for the chosen transport.
async fn dispatch(
    transport: &Transport,
    recipient: &str,
    rendered: &RenderedEmail,
) -> AppResult<()> {
    match transport {
        Transport::Gmail { config } => send_via_gmail(config, recipient, rendered).await,
        Transport::Smtp {
            host,
            port,
            user,
            pass,
            from,
        } => {
            send_via_smtp(
                host,
                *port,
                user.as_deref(),
                pass,
                from,
                recipient,
                rendered,
            )
            .await
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Gmail API path (reuses the cloud OAuth refresh machinery)
// ─────────────────────────────────────────────────────────────────────────────

/// Send via the Gmail API. Mints a fresh access token from the stored Gmail
/// refresh token (same flow as the cloud worker), assembles the raw RFC 2822
/// message via the core, base64url-encodes it, and POSTs it.
async fn send_via_gmail(
    config: &GoogleOAuthConfig,
    recipient: &str,
    rendered: &RenderedEmail,
) -> AppResult<()> {
    let token = gmail_access_token(config).await?;
    // The Gmail account address would normally come from the stored identity;
    // `me` is Gmail's documented alias for the authenticated user as sender.
    let from = "\"SundayRec\" <me>";
    let seed = now_ms().to_string();
    let mime = build_mime(
        from,
        recipient,
        &rendered.subject,
        &rendered.text,
        &rendered.html,
        &seed,
    );
    let raw = base64url(mime.as_bytes());

    // Bounded client: a hung Gmail endpoint must not block the error-alert path.
    let client = crate::cloud::http_client();
    let resp = client
        .post(GMAIL_SEND_URL)
        .bearer_auth(&token)
        .json(&serde_json::json!({ "raw": raw }))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("gmail send request: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "gmail send failed: {status} {}",
            text.chars().take(200).collect::<String>()
        )));
    }
    Ok(())
}

/// Exchange the stored Gmail refresh token for an access token. Same shape as
/// `cloud::worker::access_token` (kept local so the email feature is
/// self-contained and doesn't widen the worker's visibility).
async fn gmail_access_token(config: &GoogleOAuthConfig) -> AppResult<String> {
    let refresh = crate::secrets::get(secret_provider_for(CloudService::Gmail))
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| AppError::Internal("gmail-not-authenticated".into()))?;
    let body =
        oauth::build_refresh_body(&config.client_id, config.client_secret.as_deref(), &refresh);
    let client = crate::cloud::http_client();
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("gmail token request: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // Cap what we surface: an error page can be arbitrarily large, and the
        // raw body has no business in logs beyond a diagnostic snippet.
        let snippet: String = text.chars().take(300).collect();
        return Err(AppError::Internal(format!(
            "gmail token refresh {status}: {snippet}"
        )));
    }
    oauth::parse_token_response(&text, now_ms())
        .map(|t| t.access_token)
        .map_err(|e| AppError::Internal(format!("gmail token parse: {e:?}")))
}

// ─────────────────────────────────────────────────────────────────────────────
//   SMTP path (lettre)
// ─────────────────────────────────────────────────────────────────────────────

/// Send via SMTP using `lettre`'s async tokio transport. Mirrors the
/// `mailer.ts` nodemailer config (587 STARTTLS / 465 implicit TLS, optional
/// auth). Both plaintext + HTML parts are attached when HTML is present.
async fn send_via_smtp(
    host: &str,
    port: u16,
    user: Option<&str>,
    pass: &str,
    from: &str,
    recipient: &str,
    rendered: &RenderedEmail,
) -> AppResult<()> {
    use lettre::message::{header::ContentType, MultiPart, SinglePart};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let from_addr = format!("SundayRec <{from}>");
    let from_mailbox = from_addr
        .parse::<lettre::message::Mailbox>()
        .map_err(|e| AppError::Validation(format!("from address: {e}")))?;
    let to_mailbox = recipient
        .parse::<lettre::message::Mailbox>()
        .map_err(|e| AppError::Validation(format!("to address: {e}")))?;

    // `Message::builder()` is consumed by `body`/`multipart`, so build one per
    // branch from the already-parsed mailboxes.
    let base = || {
        Message::builder()
            .from(from_mailbox.clone())
            .to(to_mailbox.clone())
            .subject(&rendered.subject)
    };

    let message = if rendered.html.is_empty() {
        base()
            .body(rendered.text.clone())
            .map_err(|e| AppError::Internal(format!("smtp body: {e}")))?
    } else {
        base()
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(rendered.text.clone()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(rendered.html.clone()),
                    ),
            )
            .map_err(|e| AppError::Internal(format!("smtp multipart: {e}")))?
    };

    // 465 → implicit TLS, otherwise STARTTLS on the submission port. The third
    // branch is cleartext and exists ONLY for the localhost integration test —
    // it is unreachable in a release build (see `plaintext_test_transport`).
    let mut transport_builder = if plaintext_test_transport() {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
    } else if port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(host)
            .map_err(|e| AppError::Internal(format!("smtp relay: {e}")))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            .map_err(|e| AppError::Internal(format!("smtp starttls: {e}")))?
    }
    .port(port);

    if let Some(user) = user {
        transport_builder =
            transport_builder.credentials(Credentials::new(user.to_string(), pass.to_string()));
    }

    transport_builder
        .build()
        .send(message)
        .await
        .map_err(|e| AppError::Internal(format!("smtp send: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Everything a fake SMTP session saw: the command dialog (envelope) and the
    /// `DATA` payload (headers + body), kept apart so an assertion says which
    /// half it is really testing.
    struct SmtpCapture {
        /// `EHLO` / `MAIL FROM` / `RCPT TO` / `DATA` / `QUIT`, one per line.
        dialog: String,
        /// Everything between `DATA` and the lone `.` terminator.
        payload: String,
    }

    /// A single-session SMTP server on an ephemeral loopback port, speaking the
    /// smallest dialect that satisfies lettre: `220` greeting, `250` to
    /// EHLO/MAIL/RCPT, `354` to DATA, `250` after the `.` terminator, `221` to
    /// QUIT. It advertises NO extensions, so no STARTTLS is offered and no AUTH
    /// mechanism is negotiated — the client sends the message in the clear and
    /// we can read exactly what SundayRec put on the wire.
    ///
    /// Returns the port plus a handle that resolves once the session ends.
    async fn spawn_fake_smtp() -> (u16, tokio::task::JoinHandle<SmtpCapture>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();

        let handle = tokio::spawn(async move {
            let mut cap = SmtpCapture {
                dialog: String::new(),
                payload: String::new(),
            };
            let Ok((stream, _)) = listener.accept().await else {
                return cap;
            };
            let (rx, mut tx) = stream.into_split();
            let mut reader = BufReader::new(rx);
            let _ = tx.write_all(b"220 localhost SundayRec test SMTP\r\n").await;

            let mut in_data = false;
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if in_data {
                    if line.trim_end_matches(['\r', '\n']) == "." {
                        in_data = false;
                        let _ = tx.write_all(b"250 2.0.0 Ok: queued\r\n").await;
                        continue;
                    }
                    // Undo RFC 5321 dot-stuffing so the payload is the message
                    // exactly as lettre encoded it.
                    cap.payload
                        .push_str(line.strip_prefix('.').unwrap_or(&line));
                    continue;
                }
                cap.dialog.push_str(&line);
                let verb = line.trim_end().to_ascii_uppercase();
                if verb.starts_with("DATA") {
                    in_data = true;
                    let _ = tx
                        .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                        .await;
                } else if verb.starts_with("QUIT") {
                    let _ = tx.write_all(b"221 2.0.0 Bye\r\n").await;
                    break;
                } else {
                    // EHLO/HELO/MAIL/RCPT and anything else: a bare 250 with no
                    // capability lines.
                    let _ = tx.write_all(b"250 2.0.0 Ok\r\n").await;
                }
            }
            cap
        });

        (port, handle)
    }

    /// The SMTP transport, end to end, against a real socket: a real
    /// [`send_error_alert`] → [`AlertGate`] → [`render_error`] → lettre →
    /// TCP → our fake server, with the delivered bytes read back.
    ///
    /// This is the only test that proves the *send* half at all. Everything
    /// above it (templates, throttle, MIME) was already unit-tested pure, and
    /// everything below it (TLS, a real provider) still needs a live account —
    /// but "does a rendered Norwegian alert actually leave the process and
    /// arrive intact" was, until now, only ever verified by hand.
    ///
    /// Runs in the DEFAULT build since `email` joined `default`.
    // The env override is process-global, so it is serialised on the ONE
    // suite-wide lock (see `media::ffmpeg::tests::ENV_LOCK` — two modules with
    // private locks do not exclude each other). Held across the send, which is
    // why `await_holding_lock` is silenced: the only alternative is letting a
    // parallel test's `remove_var` land mid-handshake and downgrade this send
    // into a TLS attempt against a plaintext socket.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn smtp_send_puts_the_rendered_norwegian_alert_on_a_real_socket() {
        if !cfg!(debug_assertions) {
            eprintln!("SKIP: the plaintext test transport is compiled out of release builds");
            return;
        }
        let (port, server) = spawn_fake_smtp().await;

        let gate = AlertGateState::new();
        let transport = Transport::Smtp {
            host: "127.0.0.1".into(),
            port,
            // No credentials: the fake server advertises no AUTH mechanism, and
            // an anonymous submission keeps the dialog to the parts we assert.
            user: None,
            pass: String::new(),
            from: "opptak@testkirken.no".into(),
        };

        let sent = {
            let _guard = crate::media::ffmpeg::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe { std::env::set_var(PLAINTEXT_TEST_ENV, "1") };
            let result = send_error_alert(
                &gate,
                &transport,
                "vakt@testkirken.no",
                Some("no"),
                "Testkirken",
                "Ola",
                "3. august 2026",
                "Ingen lyd fra mikseren",
            )
            .await;
            unsafe { std::env::remove_var(PLAINTEXT_TEST_ENV) };
            result
        };
        assert!(
            sent.expect("the send should reach the fake server"),
            "the gate should have allowed the first alert"
        );

        let cap = tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .expect("the fake SMTP session should finish")
            .expect("the server task should not panic");

        // The envelope — proves the resolved From:/recipient reached the wire.
        assert!(
            cap.dialog.contains("MAIL FROM:<opptak@testkirken.no>"),
            "envelope sender missing from dialog:\n{}",
            cap.dialog
        );
        assert!(
            cap.dialog.contains("RCPT TO:<vakt@testkirken.no>"),
            "envelope recipient missing from dialog:\n{}",
            cap.dialog
        );

        // The message — headers, then the NORWEGIAN template markers. The
        // subject carries "⚠️" and an em dash, so lettre RFC 2047-encodes it;
        // asserting the encoding is present is the honest check (the pure
        // `encode_subject_rfc2047` tests cover the content).
        let body = &cap.payload;
        assert!(
            body.contains("From: SundayRec <opptak@testkirken.no>"),
            "From header missing:\n{body}"
        );
        assert!(
            body.contains("To: vakt@testkirken.no"),
            "To header missing:\n{body}"
        );
        assert!(
            body.contains("Subject: =?utf-8?"),
            "subject should be RFC 2047-encoded (it starts with ⚠️):\n{body}"
        );
        // Norwegian, not English: the greeting and sign-off are pure ASCII, so
        // they survive the transfer encoding verbatim on their own short lines.
        assert!(body.contains("Hei Ola,"), "greeting missing:\n{body}");
        assert!(
            body.contains("Hilsen SundayRec"),
            "sign-off missing:\n{body}"
        );
        assert!(
            body.contains("Ingen lyd fra mikseren"),
            "the actual error text missing:\n{body}"
        );
        // Both alternatives were attached (render_error fills text AND html).
        assert!(
            body.contains("multipart/alternative"),
            "expected a text+html alternative:\n{body}"
        );
    }

    #[test]
    fn gate_state_threads_through_to_the_core() {
        let gate = AlertGateState::new();
        // No recipient → suppressed.
        assert!(matches!(
            gate.decide("", "err", 0),
            AlertDecision::NoRecipient
        ));
        // First real send allowed, then throttled until the window passes.
        assert!(matches!(
            gate.decide("a@b.no", "err", 1000),
            AlertDecision::Send { .. }
        ));
        gate.record_sent("a@b.no", "err", 1000);
        assert!(matches!(
            gate.decide(
                "a@b.no",
                "err",
                1000 + sundayrec_core::email::ALERT_THROTTLE_MS - 1
            ),
            AlertDecision::Throttled
        ));
    }
}
