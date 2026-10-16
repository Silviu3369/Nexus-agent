//! What every provider adapter shares: an HTTPS client that never follows
//! redirects (a key must not reach another origin), bounded reads, an
//! endpoint allow-list (the provider's own origin or a loopback test server),
//! and a bounded Server-Sent Events loop.

use std::time::Duration;

use futures::StreamExt;
use reqwest::header::RETRY_AFTER;

use crate::ProviderError;
use crate::sse::SseParser;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const MAX_EVENT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_ERROR_BYTES: usize = 64 * 1024;
pub(crate) const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// Whole model listing, all pages; stays below the RPC deadline of the
/// calls that wait for it (`nexus_client::PROVIDER_CALL_DEADLINE`).
pub(crate) const LIST_BUDGET: Duration = Duration::from_secs(30);

pub(crate) fn client() -> Result<reqwest::Client, ProviderError> {
    // reqwest is built without a bundled crypto provider; use ring.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("nexus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ProviderError::InvalidConfiguration(e.to_string()))
}

/// `base` must be exactly `production` or a loopback test server
/// (`http://127.0.0.1:<port>`, optionally with a path); anything else is
/// refused.
pub(crate) fn allowed_base_url(base: &str, production: &str) -> Result<String, ProviderError> {
    let base = base.trim_end_matches('/');
    let loopback = base.strip_prefix("http://127.0.0.1:").is_some_and(|rest| {
        let port = rest.split('/').next().unwrap_or("");
        !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
    });
    if base == production || loopback {
        Ok(base.to_string())
    } else {
        Err(ProviderError::InvalidConfiguration(format!(
            "endpoint {base:?} is not allowed; only {production} (or a loopback test server)"
        )))
    }
}

/// A model id sent in a request body (OpenAI, Anthropic); only a safe
/// character set. Gemini, which puts it in the URL path, is stricter.
pub(crate) fn checked_body_model_id(model: &str) -> Result<&str, ProviderError> {
    let id = model.strip_prefix("models/").unwrap_or(model);
    let valid = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if valid {
        Ok(id)
    } else {
        Err(ProviderError::InvalidConfiguration(format!(
            "invalid model id {model:?}"
        )))
    }
}

pub(crate) fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

pub(crate) async fn read_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        if body.len() + chunk.len() > limit {
            return Err(ProviderError::InvalidResponse(format!(
                "body exceeded {limit} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) fn transport(err: reqwest::Error) -> ProviderError {
    if err.is_timeout() {
        ProviderError::Timeout
    } else {
        // reqwest's message may include the URL; it never contains the key.
        ProviderError::Transport(err.to_string())
    }
}

/// Maps an HTTP failure the same way for every provider: the key is wrong,
/// the quota is used up, the request is refused, or the provider is down
/// (5xx, and 529 "overloaded") — only the last is worth trying again.
pub(crate) fn status_error(
    status: u16,
    message: String,
    retry_after: Option<Duration>,
) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthenticated(message),
        429 => ProviderError::RateLimited { retry_after },
        400..=499 => ProviderError::Rejected { status, message },
        _ => ProviderError::Unavailable { status, message },
    }
}

/// Reads an SSE response to its end, handing each event's `data` to
/// `on_data`; bounded per event and in total.
pub(crate) async fn read_sse(
    response: reqwest::Response,
    mut on_data: impl FnMut(&str) -> Result<(), ProviderError>,
) -> Result<(), ProviderError> {
    let mut parser = SseParser::new(MAX_EVENT_BYTES);
    let mut received = 0usize;
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        received += chunk.len();
        if received > MAX_STREAM_BYTES {
            return Err(ProviderError::InvalidResponse(format!(
                "response exceeded {MAX_STREAM_BYTES} bytes"
            )));
        }
        for data in parser
            .push(&chunk)
            .map_err(ProviderError::InvalidResponse)?
        {
            on_data(&data)?;
        }
    }
    if let Some(data) = parser.finish().map_err(ProviderError::InvalidResponse)? {
        on_data(&data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROD: &str = "https://api.example.com";

    #[test]
    fn only_the_provider_origin_or_loopback_is_allowed() {
        assert!(allowed_base_url(PROD, PROD).is_ok());
        assert!(allowed_base_url("http://127.0.0.1:8080", PROD).is_ok());
        assert!(allowed_base_url("http://127.0.0.1:8080/anthropic", PROD).is_ok());
        for bad in [
            "http://api.example.com",
            "https://evil.example.com",
            "https://api.example.com.evil.com",
            "http://127.0.0.1.evil.com:80",
            "http://localhost:8080",
            "http://127.0.0.1:",
            "http://127.0.0.1:/x",
        ] {
            assert!(
                allowed_base_url(bad, PROD).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn model_ids_cannot_inject() {
        assert_eq!(
            checked_body_model_id("models/gemini-flash").unwrap(),
            "gemini-flash"
        );
        assert!(checked_body_model_id("gpt-6.1-sol").is_ok());
        assert!(checked_body_model_id("ft:gpt-5:org:x").is_ok());
        for bad in [
            "../secret",
            "a/b",
            "m?key=x",
            "m#x",
            "",
            "-x",
            "m\"x",
            "m x",
        ] {
            assert!(
                checked_body_model_id(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn statuses_map_to_stable_errors() {
        assert!(matches!(
            status_error(401, "x".into(), None),
            ProviderError::Unauthenticated(_)
        ));
        assert!(matches!(
            status_error(429, "x".into(), None),
            ProviderError::RateLimited { .. }
        ));
        assert!(matches!(
            status_error(402, "x".into(), None),
            ProviderError::Rejected { status: 402, .. }
        ));
        assert!(matches!(
            status_error(503, "x".into(), None),
            ProviderError::Unavailable { status: 503, .. }
        ));
        assert!(matches!(
            status_error(529, "x".into(), None),
            ProviderError::Unavailable { status: 529, .. }
        ));
    }
}
