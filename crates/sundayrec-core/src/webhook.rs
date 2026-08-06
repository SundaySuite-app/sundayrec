//! Webhook-notification decisions — pure, GUI-free, network-free.
//!
//! Ports the Electron `src/main/webhook.ts` (the behavioural spec). That module
//! interleaved the *decisions* (URL validation, Slack/Discord detection, the
//! human-readable message shaping, the generic-vs-chat payload choice) with the
//! actual `fetch` POST. Here we keep ONLY the deterministic parts so the body the
//! shell sends is unit-tested without a network: the `src-tauri` `email` seam
//! builds the [`WebhookPayload`], calls [`build_webhook_body`], and POSTs it
//! (NETWORK-UNVERIFIED behind the `email` feature).

use serde::{Deserialize, Serialize};

/// Severity of a webhook notification. Mirrors the Electron `'warn' | 'error'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookSeverity {
    Warn,
    Error,
}

/// The structured notification the generic-JSON branch sends. Mirrors the
/// Electron `WebhookPayload` field-for-field (the `app` is always `SundayRec`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookPayload {
    pub app: String,
    pub church: String,
    pub severity: WebhookSeverity,
    pub category: String,
    pub message: String,
    pub timestamp: String,
}

impl WebhookPayload {
    /// A test payload (the "Send test" button). Mirrors the Electron `test-webhook`
    /// handler's payload: `severity: warn`, `category: device`, a Norwegian body.
    pub fn test(church: &str, timestamp: &str) -> Self {
        Self {
            app: "SundayRec".into(),
            church: if church.is_empty() {
                "untitled".into()
            } else {
                church.into()
            },
            severity: WebhookSeverity::Warn,
            category: "device".into(),
            message: "Test fra SundayRec — webhook fungerer.".into(),
            timestamp: timestamp.into(),
        }
    }
}

/// Whether a webhook URL is well-formed enough to POST to. Mirrors the Electron
/// `/^https?:\/\//i` guard — an empty or non-http(s) URL is rejected (the send
/// then no-ops with `false`).
///
/// SYNTAX ONLY. Whether we may actually send to it is [`webhook_gate`], which
/// adds the SSRF policy (see below).
pub fn is_valid_webhook_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

// ─────────────────────────────────────────────────────────────────────────────
//   SSRF policy (E1.4)
// ─────────────────────────────────────────────────────────────────────────────
//
// The webhook URL is fully user-controlled and the POST's response is DISCARDED
// — a textbook blind SSRF. Point it at `http://192.168.1.1/reboot` or at a cloud
// metadata endpoint and SundayRec becomes the attacker's HTTP client inside the
// church LAN, with the machine's own network position.
//
// The naive fix (block every private address) is wrong for THIS app: churches
// legitimately webhook LAN devices — a control panel in the tech booth, a Home
// Assistant box that turns on the "ON AIR" light. So the owner policy is
//
//     block private / loopback / link-local BY DEFAULT,
//     with a per-URL opt-in the operator grants explicitly.
//
// This module owns the CLASSIFICATION (pure, no DNS — a hostname is judged by
// name); `src-tauri`'s `notify` seam owns the RESOLUTION and re-checks the
// resolved IPs on every send, so a DNS-rebinding host that answers "public" once
// and "192.168.x" a minute later is caught on the next POST rather than trusted
// forever.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Where a webhook host actually lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostClass {
    /// A routable internet address. The normal case (Slack, Discord, Teams).
    Public,
    /// `127.0.0.0/8`, `::1`, or a name that always means "this machine".
    Loopback,
    /// RFC1918 / CGNAT / IPv6 ULA, or a name that conventionally means the LAN.
    Private,
    /// `169.254.0.0/16` or `fe80::/10` — includes the cloud metadata endpoint
    /// `169.254.169.254`, which is the single most-abused SSRF target there is.
    LinkLocal,
    /// Not a URL we can extract a host from at all.
    Invalid,
}

impl HostClass {
    /// Whether this class is "inside" — the set the default policy blocks.
    pub fn is_local(self) -> bool {
        matches!(
            self,
            HostClass::Loopback | HostClass::Private | HostClass::LinkLocal
        )
    }

    /// Stable label for logs and the block reason.
    pub fn as_str(self) -> &'static str {
        match self {
            HostClass::Public => "public",
            HostClass::Loopback => "loopback",
            HostClass::Private => "private",
            HostClass::LinkLocal => "link-local",
            HostClass::Invalid => "invalid",
        }
    }
}

/// Extract the host from an `http(s)://` URL: everything between the scheme and
/// the first `/`, `?` or `#`, with userinfo and the port stripped. Lowercased.
/// An IPv6 literal keeps its brackets off but its colons intact.
pub fn host_of(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        // `user:pass@host` — the host is what comes after the LAST `@`, so
        // `https://evil.com@127.0.0.1/` classifies on 127.0.0.1, not evil.com.
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    // `[::1]:8080` → `::1`; `host:8080` → `host`.
    let host = if let Some(end) = authority.strip_prefix('[').and_then(|r| r.find(']')) {
        &authority[1..=end]
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Classify a resolved IP address. This is the AUTHORITATIVE check — the shell
/// runs it on every address DNS returns, immediately before each POST.
pub fn classify_ip(ip: IpAddr) -> HostClass {
    match ip {
        IpAddr::V4(v4) => classify_ipv4(v4),
        IpAddr::V6(v6) => classify_ipv6(v6),
    }
}

fn classify_ipv4(ip: Ipv4Addr) -> HostClass {
    let [a, b, ..] = ip.octets();
    if ip.is_loopback() {
        return HostClass::Loopback;
    }
    if ip.is_link_local() {
        // 169.254.0.0/16 — cloud metadata lives here.
        return HostClass::LinkLocal;
    }
    if ip.is_private()
        // 100.64.0.0/10 — carrier-grade NAT, also what Tailscale hands out.
        || (a == 100 && (64..=127).contains(&b))
        // 0.0.0.0/8 — "this network"; 0.0.0.0 resolves to localhost on Linux.
        || a == 0
        || ip.is_broadcast()
        || ip.is_multicast()
    {
        return HostClass::Private;
    }
    HostClass::Public
}

fn classify_ipv6(ip: Ipv6Addr) -> HostClass {
    if ip.is_loopback() || ip.is_unspecified() {
        return HostClass::Loopback;
    }
    // An IPv4-mapped address (::ffff:127.0.0.1) must classify as its IPv4 self,
    // or it is a trivial bypass of every rule above.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_ipv4(v4);
    }
    let segments = ip.segments();
    // fe80::/10 — link-local unicast.
    if (segments[0] & 0xffc0) == 0xfe80 {
        return HostClass::LinkLocal;
    }
    // fc00::/7 — unique local addresses (the IPv6 RFC1918).
    if (segments[0] & 0xfe00) == 0xfc00 {
        return HostClass::Private;
    }
    // ff00::/8 — multicast.
    if (segments[0] & 0xff00) == 0xff00 {
        return HostClass::Private;
    }
    HostClass::Public
}

/// mDNS. `printer.local` is resolved over link-local multicast and can name
/// nothing outside the broadcast domain — so it IS link-local, by definition.
const LINK_LOCAL_NAME_SUFFIXES: &[&str] = &[".local"];

/// Suffixes that conventionally name a machine on the LAN (but are resolved by
/// an ordinary local DNS server rather than mDNS).
const PRIVATE_NAME_SUFFIXES: &[&str] = &[".lan", ".internal", ".home", ".home.arpa", ".intranet"];

/// Names that always mean "this machine".
const LOOPBACK_NAMES: &[&str] = &["localhost", "localhost.localdomain", "ip6-localhost"];

/// Classify a host from a URL WITHOUT resolving it: an IP literal is judged
/// exactly, a hostname by convention.
///
/// A hostname that is not obviously local classifies as [`HostClass::Public`] —
/// which is a claim about the NAME, not about where it points. The shell must
/// still resolve it (see the module comment); this pre-filter exists so the
/// settings UI can ask the confirmation question at the moment the operator
/// types the URL, with no network round-trip.
pub fn classify_host(host: &str) -> HostClass {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return HostClass::Invalid;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return classify_ip(ip);
    }
    if LOOPBACK_NAMES.contains(&host.as_str()) {
        return HostClass::Loopback;
    }
    if LINK_LOCAL_NAME_SUFFIXES.iter().any(|s| host.ends_with(s)) {
        return HostClass::LinkLocal;
    }
    if PRIVATE_NAME_SUFFIXES.iter().any(|s| host.ends_with(s)) {
        return HostClass::Private;
    }
    // A single label with no dot is a LAN short-name (`mixer`, `nas`) — a
    // public host always has a registrable domain.
    if !host.contains('.') {
        return HostClass::Private;
    }
    HostClass::Public
}

/// Classify a whole URL by its host.
pub fn classify_url(url: &str) -> HostClass {
    match host_of(url) {
        Some(host) => classify_host(&host),
        None => HostClass::Invalid,
    }
}

/// Why a webhook must not be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookBlock {
    /// Empty, or not `http(s)://…` with a host.
    InvalidUrl,
    /// A local address without the operator's per-URL opt-in.
    LocalNotAllowed(HostClass),
    /// Plaintext `http://` to a PUBLIC host — the payload carries the church's
    /// name and what just failed, and would cross the open internet in the
    /// clear. Plaintext to a LAN device is a different question and is allowed
    /// once the opt-in is granted: most control panels speak only http.
    InsecureTransport,
}

impl WebhookBlock {
    /// Stable code for logs / the test-button result.
    pub fn code(self) -> &'static str {
        match self {
            WebhookBlock::InvalidUrl => "invalid_url",
            WebhookBlock::LocalNotAllowed(_) => "local_not_allowed",
            WebhookBlock::InsecureTransport => "insecure_transport",
        }
    }
}

/// The NAME-level half of the SSRF policy: may we even try this URL?
///
/// `allow_local` is `settings.webhook_allow_local`, which the settings UI sets
/// only after the operator has confirmed a LAN address out loud. Pure — the
/// resolved-IP half runs in the shell on every send.
pub fn webhook_gate(url: &str, allow_local: bool) -> Result<HostClass, WebhookBlock> {
    if !is_valid_webhook_url(url) {
        return Err(WebhookBlock::InvalidUrl);
    }
    let class = classify_url(url);
    if class == HostClass::Invalid {
        return Err(WebhookBlock::InvalidUrl);
    }
    if class.is_local() && !allow_local {
        return Err(WebhookBlock::LocalNotAllowed(class));
    }
    let is_plaintext = url.trim().to_ascii_lowercase().starts_with("http://");
    if is_plaintext && !class.is_local() {
        return Err(WebhookBlock::InsecureTransport);
    }
    Ok(class)
}

/// Whether a webhook may be sent, for the routing matrix (which wants a bool).
pub fn may_send_webhook(url: &str, allow_local: bool) -> bool {
    webhook_gate(url, allow_local).is_ok()
}

/// Whether `url` is a Slack or Discord incoming-webhook endpoint, which take a
/// `{text|content}` chat payload rather than our structured JSON. Mirrors the
/// Electron `/hooks\.slack\.com|discord(app)?\.com\/api\/webhooks/i` test.
pub fn is_chat_webhook(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("hooks.slack.com")
        || lower.contains("discord.com/api/webhooks")
        || lower.contains("discordapp.com/api/webhooks")
}

/// The human-readable one-liner Slack/Discord render. Mirrors the Electron
/// template: a severity glyph + `*SundayRec* (church)`, the `[category] message`,
/// and the italicised timestamp.
pub fn human_message(p: &WebhookPayload) -> String {
    let glyph = match p.severity {
        WebhookSeverity::Error => "⚠️",
        WebhookSeverity::Warn => "ℹ️",
    };
    let church = if p.church.is_empty() {
        "untitled"
    } else {
        &p.church
    };
    format!(
        "{glyph} *SundayRec* ({church})\n[{}] {}\n_{}_",
        p.category, p.message, p.timestamp
    )
}

/// Build the JSON request body for `url` + `payload`. For Slack/Discord URLs this
/// is `{"text": …, "content": …}` (a single body that satisfies either); for any
/// other URL it's the structured payload serialised as-is. Returns `None` when
/// the URL is invalid (the caller then reports the `no_url`/invalid no-op).
pub fn build_webhook_body(url: &str, payload: &WebhookPayload) -> Option<String> {
    if !is_valid_webhook_url(url) {
        return None;
    }
    if is_chat_webhook(url) {
        let human = human_message(payload);
        // Both Slack (`text`) and Discord (`content`) keys present → one body
        // works for either. Serialised manually so the escaping is correct.
        Some(serde_json::json!({ "text": human, "content": human }).to_string())
    } else {
        serde_json::to_string(payload).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> WebhookPayload {
        WebhookPayload::test("Vår Frelsers", "2026-06-01T10:00:00Z")
    }

    #[test]
    fn url_validation() {
        assert!(is_valid_webhook_url("https://example.com/hook"));
        assert!(is_valid_webhook_url("HTTP://example.com"));
        assert!(!is_valid_webhook_url(""));
        assert!(!is_valid_webhook_url("ftp://example.com"));
        assert!(!is_valid_webhook_url("example.com"));
    }

    #[test]
    fn chat_detection() {
        assert!(is_chat_webhook("https://hooks.slack.com/services/T/B/X"));
        assert!(is_chat_webhook("https://discord.com/api/webhooks/1/abc"));
        assert!(is_chat_webhook("https://discordapp.com/api/webhooks/1/abc"));
        assert!(!is_chat_webhook("https://example.com/generic"));
    }

    #[test]
    fn human_message_shape() {
        let h = human_message(&payload());
        assert!(h.contains("*SundayRec* (Vår Frelsers)"));
        assert!(h.contains("[device] Test fra SundayRec"));
        assert!(h.contains("_2026-06-01T10:00:00Z_"));
        assert!(h.starts_with("ℹ️")); // warn glyph
    }

    #[test]
    fn human_message_error_glyph() {
        let mut p = payload();
        p.severity = WebhookSeverity::Error;
        assert!(human_message(&p).starts_with("⚠️"));
    }

    #[test]
    fn body_generic_is_structured_json() {
        let body = build_webhook_body("https://example.com/hook", &payload()).unwrap();
        assert!(body.contains("\"app\":\"SundayRec\""));
        assert!(body.contains("\"category\":\"device\""));
        assert!(body.contains("\"severity\":\"warn\""));
        // not the chat shape
        assert!(!body.contains("\"text\""));
    }

    #[test]
    fn body_chat_has_text_and_content() {
        let body =
            build_webhook_body("https://hooks.slack.com/services/T/B/X", &payload()).unwrap();
        assert!(body.contains("\"text\""));
        assert!(body.contains("\"content\""));
        assert!(body.contains("SundayRec"));
    }

    #[test]
    fn body_none_for_invalid_url() {
        assert!(build_webhook_body("", &payload()).is_none());
        assert!(build_webhook_body("not-a-url", &payload()).is_none());
    }

    #[test]
    fn test_payload_defaults_church() {
        let p = WebhookPayload::test("", "t");
        assert_eq!(p.church, "untitled");
        assert_eq!(p.app, "SundayRec");
    }

    // ── E1.4: SSRF classification ────────────────────────────────────────────

    #[test]
    fn ipv4_classification_table() {
        let cases: &[(&str, HostClass)] = &[
            // Loopback
            ("127.0.0.1", HostClass::Loopback),
            ("127.1.2.3", HostClass::Loopback),
            // RFC1918
            ("10.0.0.1", HostClass::Private),
            ("10.255.255.255", HostClass::Private),
            ("172.16.0.1", HostClass::Private),
            ("172.31.255.254", HostClass::Private),
            ("192.168.1.50", HostClass::Private),
            // CGNAT / Tailscale
            ("100.64.0.1", HostClass::Private),
            ("100.127.255.255", HostClass::Private),
            // "this network"
            ("0.0.0.0", HostClass::Private),
            // Link-local, including the cloud metadata endpoint
            ("169.254.1.1", HostClass::LinkLocal),
            ("169.254.169.254", HostClass::LinkLocal),
            // Public — including the addresses that merely LOOK private
            ("1.1.1.1", HostClass::Public),
            ("8.8.8.8", HostClass::Public),
            ("172.15.0.1", HostClass::Public),
            ("172.32.0.1", HostClass::Public),
            ("192.167.1.1", HostClass::Public),
            ("100.63.255.255", HostClass::Public),
            ("100.128.0.1", HostClass::Public),
            ("11.0.0.1", HostClass::Public),
        ];
        for (ip, want) in cases {
            assert_eq!(classify_host(ip), *want, "{ip}");
        }
    }

    #[test]
    fn ipv6_classification_table() {
        let cases: &[(&str, HostClass)] = &[
            ("::1", HostClass::Loopback),
            ("::", HostClass::Loopback),
            // ULA (fc00::/7)
            ("fc00::1", HostClass::Private),
            ("fd12:3456:789a::1", HostClass::Private),
            // Link-local (fe80::/10)
            ("fe80::1", HostClass::LinkLocal),
            ("febf::1", HostClass::LinkLocal),
            // Multicast
            ("ff02::1", HostClass::Private),
            // Public
            ("2606:4700:4700::1111", HostClass::Public),
            ("2001:4860:4860::8888", HostClass::Public),
        ];
        for (ip, want) in cases {
            assert_eq!(classify_host(ip), *want, "{ip}");
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_cannot_smuggle_a_private_address() {
        // ::ffff:127.0.0.1 is the classic bypass: an IPv6 literal that IS an
        // IPv4 loopback.
        assert_eq!(classify_host("::ffff:127.0.0.1"), HostClass::Loopback);
        assert_eq!(classify_host("::ffff:192.168.0.5"), HostClass::Private);
        assert_eq!(
            classify_host("::ffff:169.254.169.254"),
            HostClass::LinkLocal
        );
    }

    #[test]
    fn hostname_classification_is_by_convention() {
        assert_eq!(classify_host("localhost"), HostClass::Loopback);
        assert_eq!(classify_host("LOCALHOST"), HostClass::Loopback);
        assert_eq!(classify_host("printer.local"), HostClass::LinkLocal);
        assert_eq!(classify_host("nas.lan"), HostClass::Private);
        assert_eq!(classify_host("api.internal"), HostClass::Private);
        assert_eq!(classify_host("hass.home.arpa"), HostClass::Private);
        // A bare label is a LAN short-name; a real host has a domain.
        assert_eq!(classify_host("mixer"), HostClass::Private);
        assert_eq!(classify_host("hooks.slack.com"), HostClass::Public);
        // A trailing root dot must not defeat the suffix match.
        assert_eq!(classify_host("printer.local."), HostClass::LinkLocal);
        assert_eq!(classify_host(""), HostClass::Invalid);
        // …and a name that merely CONTAINS a local word is not local.
        assert_eq!(classify_host("localhost.example.com"), HostClass::Public);
        assert_eq!(classify_host("mylocal.com"), HostClass::Public);
    }

    #[test]
    fn host_extraction_handles_ports_userinfo_and_ipv6_literals() {
        assert_eq!(
            host_of("https://example.com/hook").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("http://192.168.1.5:8123/api").as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(host_of("https://[::1]:9000/x").as_deref(), Some("::1"));
        assert_eq!(
            host_of("https://example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("https://example.com?a=1").as_deref(),
            Some("example.com")
        );
        // The userinfo trick: the REAL host is after the last `@`.
        assert_eq!(
            host_of("https://hooks.slack.com@127.0.0.1/x").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(host_of("ftp://example.com"), None);
        assert_eq!(host_of("https://"), None);
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn the_userinfo_trick_is_classified_on_the_real_host() {
        // Without the rsplit('@'), this reads as a public Slack URL and posts
        // straight at loopback.
        assert_eq!(
            classify_url("https://hooks.slack.com@127.0.0.1/services/T/B/X"),
            HostClass::Loopback
        );
    }

    // ── E1.4: the gate + the allow flag ──────────────────────────────────────

    #[test]
    fn a_public_https_webhook_is_allowed_either_way() {
        for allow in [false, true] {
            assert_eq!(
                webhook_gate("https://hooks.slack.com/services/T/B/X", allow),
                Ok(HostClass::Public)
            );
        }
    }

    #[test]
    fn a_lan_webhook_needs_the_explicit_opt_in() {
        // Default: refused, with the class named so the log says WHY.
        assert_eq!(
            webhook_gate("http://192.168.1.50/hook", false),
            Err(WebhookBlock::LocalNotAllowed(HostClass::Private))
        );
        assert_eq!(
            webhook_gate("http://localhost:9000/hook", false),
            Err(WebhookBlock::LocalNotAllowed(HostClass::Loopback))
        );
        assert_eq!(
            webhook_gate("http://169.254.169.254/latest/meta-data/", false),
            Err(WebhookBlock::LocalNotAllowed(HostClass::LinkLocal))
        );
        // Opted in: allowed, INCLUDING over plaintext — a church control panel
        // or Home Assistant box usually speaks only http, and the traffic never
        // leaves the building.
        assert_eq!(
            webhook_gate("http://192.168.1.50/hook", true),
            Ok(HostClass::Private)
        );
        assert_eq!(
            webhook_gate("http://hass.local:8123/api/webhook/abc", true),
            Ok(HostClass::LinkLocal)
        );
    }

    #[test]
    fn plaintext_to_a_public_host_is_refused_even_with_the_flag() {
        // The payload carries the church's name and what just failed. The
        // allow-flag is about REACHING the LAN, not about sending church data
        // across the internet in the clear.
        assert_eq!(
            webhook_gate("http://example.com/hook", true),
            Err(WebhookBlock::InsecureTransport)
        );
        assert_eq!(
            webhook_gate("http://example.com/hook", false),
            Err(WebhookBlock::InsecureTransport)
        );
    }

    #[test]
    fn junk_urls_are_invalid_not_merely_local() {
        for url in ["", "   ", "example.com", "ftp://example.com", "https://"] {
            assert_eq!(
                webhook_gate(url, true),
                Err(WebhookBlock::InvalidUrl),
                "{url}"
            );
        }
    }

    #[test]
    fn may_send_matches_the_gate() {
        assert!(may_send_webhook("https://example.com/h", false));
        assert!(!may_send_webhook("http://10.0.0.1/h", false));
        assert!(may_send_webhook("http://10.0.0.1/h", true));
    }

    #[test]
    fn block_codes_are_stable_and_distinct() {
        let codes = [
            WebhookBlock::InvalidUrl.code(),
            WebhookBlock::LocalNotAllowed(HostClass::Private).code(),
            WebhookBlock::InsecureTransport.code(),
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn is_local_covers_exactly_the_inside_classes() {
        assert!(HostClass::Loopback.is_local());
        assert!(HostClass::Private.is_local());
        assert!(HostClass::LinkLocal.is_local());
        assert!(!HostClass::Public.is_local());
        assert!(!HostClass::Invalid.is_local());
    }
}
