//! Read-only web fetch: the first tool the model can call without the
//! user's per-call approval, because it has no effect — it only reads a
//! public page (`PLAN_NEXUS_RO.md`, the agent-loop slice approved
//! 2026-10-15). GET only, no cookies sent, no JavaScript executed, blocked
//! against private/loopback/link-local targets (SSRF — `ARHITECTURA_NEXUS_RO.md`
//! §8: tool side effects are policy too, and a read has none, but the
//! *destination* still needs a rule). Fetched text is data, never
//! instructions — the chat system instruction says so explicitly.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::Url;
use reqwest::header::{CONTENT_TYPE, LOCATION, USER_AGENT};

const MAX_REDIRECTS: u8 = 5;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 20_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq)]
pub struct FetchedPage {
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FetchError {
    #[error("only http:// and https:// URLs can be fetched")]
    UnsupportedScheme,
    #[error("could not parse the URL")]
    InvalidUrl,
    #[error("{0:?} does not resolve to a public address")]
    DisallowedHost(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("the response exceeded {MAX_BODY_BYTES} bytes")]
    TooLarge,
    #[error("the response is not a readable text document ({0})")]
    NotText(String),
    #[error("the request timed out")]
    Timeout,
    #[error("could not reach the host: {0}")]
    Transport(String),
    #[error("the server answered with status {0}")]
    Status(u16),
}

/// Fetches `url`, following up to [`MAX_REDIRECTS`] redirects (each
/// re-checked against the same host rule), and returns its readable text.
pub async fn fetch_url(url: &str) -> Result<FetchedPage, FetchError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut target = checked_scheme(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let addr = resolve_allowed(&target).await?;
        let client = client_pinned_to(target.host_str().unwrap_or_default(), addr)?;
        let response = send(&client, &target).await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::Status(response.status().as_u16()))?;
            target = checked_scheme(
                target
                    .join(location)
                    .map_err(|_| FetchError::InvalidUrl)?
                    .as_str(),
            )?;
            continue;
        }
        let final_url = target.to_string();
        if !response.status().is_success() {
            return Err(FetchError::Status(response.status().as_u16()));
        }
        return extract(final_url, response).await;
    }
    Err(FetchError::TooManyRedirects)
}

fn checked_scheme(url: &str) -> Result<Url, FetchError> {
    let parsed = Url::parse(url).map_err(|_| FetchError::InvalidUrl)?;
    if parsed.scheme() == "http" || parsed.scheme() == "https" {
        Ok(parsed)
    } else {
        Err(FetchError::UnsupportedScheme)
    }
}

/// Resolves the target's host and refuses it unless every address it
/// resolves to is public — pinning the connection to the address checked
/// here, so a second lookup at connect time (DNS rebinding) cannot swap it.
async fn resolve_allowed(target: &Url) -> Result<SocketAddr, FetchError> {
    let host = target.host_str().ok_or(FetchError::InvalidUrl)?;
    let port = target
        .port_or_known_default()
        .ok_or(FetchError::InvalidUrl)?;
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| FetchError::Transport(e.to_string()))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| is_disallowed(a.ip())) {
        return Err(FetchError::DisallowedHost(host.to_string()));
    }
    Ok(addrs[0])
}

pub(crate) fn is_disallowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_disallowed_v4(v4),
        IpAddr::V6(v6) => is_disallowed_v6(v6),
    }
}

fn is_disallowed_v4(v4: Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local() // includes the 169.254.169.254 metadata address
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]) // 100.64.0.0/10, carrier-grade NAT
}

fn is_disallowed_v6(v6: Ipv6Addr) -> bool {
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7, unique local
        || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10, link-local
        || v6.to_ipv4_mapped().is_some_and(is_disallowed_v4)
}

fn client_pinned_to(host: &str, addr: SocketAddr) -> Result<reqwest::Client, FetchError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .resolve(host, addr)
        .build()
        .map_err(|e| FetchError::Transport(e.to_string()))
}

async fn send(client: &reqwest::Client, target: &Url) -> Result<reqwest::Response, FetchError> {
    tokio::time::timeout(
        TOTAL_TIMEOUT,
        client
            .get(target.clone())
            .header(
                USER_AGENT,
                concat!("nexus-fetch/", env!("CARGO_PKG_VERSION")),
            )
            .send(),
    )
    .await
    .map_err(|_| FetchError::Timeout)?
    .map_err(transport)
}

fn transport(err: reqwest::Error) -> FetchError {
    if err.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Transport(err.to_string())
    }
}

enum ContentKind {
    Html,
    PlainText,
}

fn content_kind(content_type: &str) -> Option<ContentKind> {
    let media = content_type.split(';').next().unwrap_or("").trim();
    match media {
        "text/html" | "application/xhtml+xml" => Some(ContentKind::Html),
        "application/json" => Some(ContentKind::PlainText),
        _ if media.starts_with("text/") => Some(ContentKind::PlainText),
        _ => None,
    }
}

/// Shared by the public entry point (which only ever calls it on an
/// already host-checked response) and the tests, which exercise it
/// directly against an ordinary local server.
async fn extract(
    final_url: String,
    response: reqwest::Response,
) -> Result<FetchedPage, FetchError> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Some(kind) = content_kind(&content_type) else {
        return Err(FetchError::NotText(content_type));
    };
    let body = read_limited(response).await?;
    let raw = String::from_utf8_lossy(&body).into_owned();
    let text = match kind {
        ContentKind::Html => strip_html(&raw),
        ContentKind::PlainText => raw,
    };
    let char_count = text.chars().count();
    let truncated = char_count > MAX_TEXT_CHARS;
    let text = if truncated {
        text.chars().take(MAX_TEXT_CHARS).collect()
    } else {
        text
    };
    Ok(FetchedPage {
        final_url,
        status,
        content_type,
        text,
        truncated,
    })
}

async fn read_limited(response: reqwest::Response) -> Result<Vec<u8>, FetchError> {
    use futures::StreamExt;
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Deliberately simple: drops `<script>`/`<style>` bodies, every other tag,
/// decodes the handful of entities real pages actually use, and collapses
/// whitespace. Not a readability extractor — good enough for a model to
/// read, not for a human-facing reader view.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut chars = html.char_indices().peekable();
    let mut skip_until: Option<&'static str> = None;
    let bytes = html.as_bytes();

    while let Some((i, c)) = chars.next() {
        if let Some(closing) = skip_until {
            if html[i..].to_ascii_lowercase().starts_with(closing) {
                for _ in 0..closing.len() - 1 {
                    chars.next();
                }
                skip_until = None;
            }
            continue;
        }
        if c == '<' {
            let rest = &html[i..];
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("<script") {
                skip_until = Some("</script>");
            } else if lower.starts_with("<style") {
                skip_until = Some("</style>");
            }
            // Skip to the matching '>' of this tag.
            let mut j = i;
            while j < bytes.len() && bytes[j] != b'>' {
                j += 1;
            }
            while let Some(&(k, _)) = chars.peek() {
                if k > j {
                    break;
                }
                chars.next();
            }
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    let decoded = decode_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let semi = tail.find(';').filter(|&p| p <= 10);
        match semi {
            Some(p) => {
                let entity = &tail[1..p];
                let replacement = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "nbsp" => Some(' '),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    _ if entity.starts_with('#') => {
                        entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                match replacement {
                    Some(c) => {
                        out.push(c);
                        rest = &tail[p + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &tail[1..];
                    }
                }
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // --- pure SSRF-gate tests: no network, exhaustive on the IP ranges ---

    #[test]
    fn loopback_and_private_v4_ranges_are_disallowed() {
        for ip in [
            "127.0.0.1",
            "127.255.255.255",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata
            "169.254.0.1",
            "100.64.0.1", // carrier-grade NAT
            "0.0.0.0",
            "224.0.0.1", // multicast
            "255.255.255.255",
        ] {
            assert!(
                is_disallowed(ip.parse().unwrap()),
                "{ip} must be disallowed"
            );
        }
    }

    #[test]
    fn public_v4_addresses_are_allowed() {
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            assert!(!is_disallowed(ip.parse().unwrap()), "{ip} must be allowed");
        }
    }

    #[test]
    fn loopback_and_private_v6_ranges_are_disallowed() {
        for ip in ["::1", "fe80::1", "fc00::1", "fd12:3456::1", "::"] {
            assert!(
                is_disallowed(ip.parse().unwrap()),
                "{ip} must be disallowed"
            );
        }
    }

    #[test]
    fn public_v6_addresses_are_allowed() {
        assert!(!is_disallowed("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn only_http_and_https_schemes_are_accepted() {
        assert!(checked_scheme("https://example.com").is_ok());
        assert!(checked_scheme("http://example.com").is_ok());
        for bad in [
            "ftp://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert!(checked_scheme(bad).is_err(), "{bad} must be refused");
        }
    }

    // --- the public entry point refuses a loopback target before any I/O ---

    #[tokio::test]
    async fn fetching_a_loopback_target_is_refused_without_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accept = tokio::spawn(async move {
            // If fetch_url ever connects here, this returns instead of
            // hanging forever; the test fails on its result, not a timeout.
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1];
            let _ = socket.read(&mut buf).await;
        });
        let result = fetch_url(&format!("http://127.0.0.1:{port}/")).await;
        assert!(
            matches!(result, Err(FetchError::DisallowedHost(_))),
            "{result:?}"
        );
        accept.abort();
    }

    // --- download + extraction, against a local server the gate never sees ---

    async fn serve_once(body_head: &str, payload: &[u8]) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let head = body_head.to_string();
        let payload = payload.to_vec();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await; // drain the request
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&payload).await.unwrap();
            let _ = socket.shutdown().await;
        });
        (format!("http://127.0.0.1:{port}/"), handle)
    }

    async fn direct_extract(url: &str) -> Result<FetchedPage, FetchError> {
        // Mirrors fetch_url's download step but without the SSRF gate, since
        // the gate itself is proven separately above and loopback can never
        // pass it.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let target = Url::parse(url).unwrap();
        let response = send(&client, &target).await?;
        if !response.status().is_success() {
            return Err(FetchError::Status(response.status().as_u16()));
        }
        extract(target.to_string(), response).await
    }

    #[tokio::test]
    async fn html_is_stripped_to_readable_text() {
        // The title's own text is kept (it is useful context, unlike
        // script/style bodies, which must never reach the model).
        let body = b"<html><head><title>A Page</title><style>body{color:red}</style></head><body><h1>Hello</h1><script>evil()</script><p>World &amp; friends</p></body></html>";
        let (url, _server) = serve_once(
            &format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body,
        )
        .await;
        let page = direct_extract(&url).await.unwrap();
        assert_eq!(page.text, "A Page Hello World & friends");
        assert!(!page.truncated);
        assert!(
            !page.text.contains("color:red"),
            "style content must never reach the model: {}",
            page.text
        );
        assert!(
            !page.text.contains("evil()"),
            "script content must never reach the model: {}",
            page.text
        );
    }

    #[tokio::test]
    async fn plain_text_passes_through_unchanged() {
        let body = b"line one\nline two";
        let (url, _server) = serve_once(
            &format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body,
        )
        .await;
        let page = direct_extract(&url).await.unwrap();
        assert_eq!(page.text, "line one\nline two");
    }

    #[tokio::test]
    async fn binary_content_is_refused_not_passed_through() {
        let body = [0xFFu8, 0xD8, 0xFF, 0xE0];
        let (url, _server) = serve_once(
            "HTTP/1.1 200 OK\r\ncontent-type: image/jpeg\r\ncontent-length: 4\r\nconnection: close\r\n\r\n",
            &body,
        )
        .await;
        let result = direct_extract(&url).await;
        assert!(matches!(result, Err(FetchError::NotText(_))), "{result:?}");
    }

    #[tokio::test]
    async fn a_response_over_the_limit_is_refused() {
        let body = vec![b'a'; MAX_BODY_BYTES + 1];
        let (url, _server) = serve_once(
            &format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            &body,
        )
        .await;
        let result = direct_extract(&url).await;
        assert_eq!(result, Err(FetchError::TooLarge));
    }

    #[tokio::test]
    async fn long_text_is_truncated_not_rejected() {
        let body = "word ".repeat(MAX_TEXT_CHARS);
        let (url, _server) = serve_once(
            &format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body.as_bytes(),
        )
        .await;
        let page = direct_extract(&url).await.unwrap();
        assert!(page.truncated);
        assert_eq!(page.text.chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn entity_decoding_covers_the_common_set() {
        // decode_entities only swaps entities for characters; collapsing the
        // resulting whitespace is strip_html's job, so &nbsp; here still
        // shows as its own space next to the literal one before it.
        assert_eq!(
            decode_entities("A &amp; B &lt;tag&gt; &quot;q&quot; &#39;s&#39; &#x26;&nbsp;end"),
            "A & B <tag> \"q\" 's' & end"
        );
        // An unterminated or unknown entity is left as literal text, not dropped.
        assert_eq!(decode_entities("5 &notareal; done"), "5 &notareal; done");
        assert_eq!(decode_entities("a & b"), "a & b");
    }
}
