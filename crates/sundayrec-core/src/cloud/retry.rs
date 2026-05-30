//! Retry classification + backoff — the deterministic core of `withRetry`.
//!
//! Ported from `src/main/cloud/http-util.ts`. The Electron `withRetry` mixed the
//! actual `sleep`/`fetch`/token-refresh side effects with two pure decisions:
//! *is this error worth retrying* and *how long until the next attempt*. We keep
//! only those decisions; the `src-tauri` shell owns the timer, the `reqwest`
//! call, and the random jitter it adds on top of [`backoff_delay_ms`].

use chrono::DateTime;

/// Is this error transient (worth retrying)? Mirrors `isTransient` in
/// `http-util.ts`:
///   - HTTP `408`, `429`, or any `5xx` → retry,
///   - network error codes (`ENOTFOUND`, `ECONNRESET`, `ETIMEDOUT`, `EAI_AGAIN`,
///     `ECONNREFUSED`, `UND_ERR_SOCKET`) → retry,
///   - an undici-style `"fetch failed"` message → retry,
///   - everything else (incl. 4xx auth/client errors apart from 408/429) → no.
///
/// `status` wins when present, exactly as the JS version returns early on a set
/// `.status`. `code` and `msg` are the lower-level fallbacks.
pub fn is_transient(status: Option<u16>, code: Option<&str>, msg: &str) -> bool {
    if let Some(s) = status {
        return s == 408 || s == 429 || (500..600).contains(&s);
    }
    if let Some(code) = code {
        if matches!(
            code,
            "ENOTFOUND"
                | "ECONNRESET"
                | "ETIMEDOUT"
                | "EAI_AGAIN"
                | "ECONNREFUSED"
                | "UND_ERR_SOCKET"
        ) {
            return true;
        }
    }
    msg.contains("fetch failed")
}

/// Parse an RFC 7231 `Retry-After` header into milliseconds, returning `0` when
/// the header is missing or malformed (the caller then falls back to
/// [`backoff_delay_ms`]). Mirrors `parseRetryAfter` in `http-util.ts`: numeric
/// `delta-seconds` or an HTTP-date. `now_ms` is injected so HTTP-date math stays
/// deterministic (the JS code used `Date.now()`).
pub fn parse_retry_after(header: Option<&str>, now_ms: i64) -> i64 {
    let Some(raw) = header else { return 0 };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return 0;
    }
    // delta-seconds (may be fractional, like the JS `Number(trimmed)`).
    if let Ok(secs) = trimmed.parse::<f64>() {
        if secs.is_finite() && secs >= 0.0 {
            return (secs * 1000.0).round() as i64;
        }
    }
    // HTTP-date (IMF-fixdate uses "GMT"; normalise to a numeric offset so
    // chrono's RFC 2822 parser accepts it).
    let normalised = trimmed
        .strip_suffix(" GMT")
        .map(|d| format!("{d} +0000"))
        .unwrap_or_else(|| trimmed.to_string());
    if let Ok(dt) = DateTime::parse_from_rfc2822(&normalised) {
        return (dt.timestamp_millis() - now_ms).max(0);
    }
    0
}

/// The largest plain-exponential backoff delay (before jitter). Matches the
/// `Math.min(30_000, …)` cap in `withRetry`.
pub const MAX_BACKOFF_MS: u64 = 30_000;
/// The cap applied to a server-suggested `Retry-After` — anything longer means
/// the service is effectively down for us (`Math.min(60_000, retryAfterMs)`).
pub const MAX_RETRY_AFTER_MS: u64 = 60_000;

/// Compute the delay before the next attempt, *without* jitter (the shell adds
/// `Math.random() * 500`). `attempt` is 1-based. Mirrors `withRetry`:
///   - if the server suggested a positive `Retry-After`, honour it capped at
///     [`MAX_RETRY_AFTER_MS`];
///   - otherwise exponential backoff `base * 2^(attempt-1)` capped at
///     [`MAX_BACKOFF_MS`].
pub fn backoff_delay_ms(attempt: u32, base_ms: u64, retry_after_ms: Option<u64>) -> u64 {
    if let Some(ra) = retry_after_ms.filter(|&ms| ms > 0) {
        return ra.min(MAX_RETRY_AFTER_MS);
    }
    let exp = base_ms.saturating_mul(
        1u64.checked_shl(attempt.saturating_sub(1))
            .unwrap_or(u64::MAX),
    );
    exp.min(MAX_BACKOFF_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_by_status() {
        assert!(is_transient(Some(408), None, ""));
        assert!(is_transient(Some(429), None, ""));
        assert!(is_transient(Some(500), None, ""));
        assert!(is_transient(Some(503), None, ""));
        assert!(!is_transient(Some(400), None, ""));
        assert!(!is_transient(Some(401), None, ""));
        assert!(!is_transient(Some(404), None, ""));
        // status wins even when a transient code is also present
        assert!(!is_transient(Some(403), Some("ECONNRESET"), "fetch failed"));
    }

    #[test]
    fn transient_by_code_and_message() {
        assert!(is_transient(None, Some("ECONNRESET"), ""));
        assert!(is_transient(None, Some("ETIMEDOUT"), ""));
        assert!(is_transient(None, Some("UND_ERR_SOCKET"), ""));
        assert!(!is_transient(None, Some("EPERM"), ""));
        assert!(is_transient(None, None, "TypeError: fetch failed"));
        assert!(!is_transient(None, None, "some other error"));
    }

    #[test]
    fn retry_after_numeric_seconds() {
        assert_eq!(parse_retry_after(Some("120"), 0), 120_000);
        assert_eq!(parse_retry_after(Some("0.5"), 0), 500);
        assert_eq!(parse_retry_after(Some("  30  "), 0), 30_000);
    }

    #[test]
    fn retry_after_http_date_relative_to_now() {
        // 1994-11-06 08:49:37 GMT = 784111777 s since epoch.
        let target_ms = 784_111_777_000_i64;
        let now = target_ms - 10_000; // 10 s before
        assert_eq!(
            parse_retry_after(Some("Sun, 06 Nov 1994 08:49:37 GMT"), now),
            10_000
        );
        // A date already in the past clamps to 0.
        assert_eq!(
            parse_retry_after(Some("Sun, 06 Nov 1994 08:49:37 GMT"), target_ms + 5_000),
            0
        );
    }

    #[test]
    fn retry_after_missing_or_malformed_is_zero() {
        assert_eq!(parse_retry_after(None, 0), 0);
        assert_eq!(parse_retry_after(Some(""), 0), 0);
        assert_eq!(parse_retry_after(Some("not-a-date"), 0), 0);
        assert_eq!(parse_retry_after(Some("-5"), 0), 0);
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_delay_ms(1, 1000, None), 1000);
        assert_eq!(backoff_delay_ms(2, 1000, None), 2000);
        assert_eq!(backoff_delay_ms(3, 1000, None), 4000);
        assert_eq!(backoff_delay_ms(4, 1000, None), 8000);
        // capped at 30 s
        assert_eq!(backoff_delay_ms(10, 1000, None), MAX_BACKOFF_MS);
    }

    #[test]
    fn backoff_honours_retry_after_capped_at_60s() {
        assert_eq!(backoff_delay_ms(1, 1000, Some(5_000)), 5_000);
        assert_eq!(backoff_delay_ms(1, 1000, Some(120_000)), MAX_RETRY_AFTER_MS);
        // zero retry-after falls back to exponential
        assert_eq!(backoff_delay_ms(2, 1000, Some(0)), 2000);
    }
}
