//! Client for remote MCP servers over Streamable HTTP (`PLAN_NEXUS_RO.md`,
//! step 3: Nexus as an MCP client; remote servers only — local servers run
//! foreign code on this PC and stay off). Only what Nexus uses is
//! implemented: listing tools and calling them.
//!
//! Protocol: revision 2026-07-28 (stateless; version, client info and
//! capabilities travel in each request's `_meta`, mirrored in headers),
//! with the documented fallback to the session-based revision 2025-11-25
//! (`initialize`, `Mcp-Session-Id`) for servers that do not speak it yet.
//! Checked against modelcontextprotocol.io on 2026-10-17.
//!
//! The server is untrusted. Its address must be public HTTPS (the same host
//! rule as the web read, pinned against DNS rebinding), redirects are not
//! followed, cookies are not kept, responses are bounded in size and time,
//! a key only ever travels in the `Authorization` header, and everything
//! the server says (tool descriptions, schemas, annotations, results) is
//! data for the user and the model — never instructions or permissions.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use reqwest::Url;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{Value, json};

use crate::ApiKey;
use crate::sse::SseParser;

pub const MODERN_VERSION: &str = "2026-07-28";
pub const LEGACY_VERSION: &str = "2025-11-25";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 10;
const MAX_TOOLS: usize = 200;
const MAX_NAME_CHARS: usize = 128;
const MAX_DESCRIPTION_CHARS: usize = 4000;
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
/// Text handed back to the model per call, like a web read.
pub const MAX_RESULT_CHARS: usize = 20_000;

// JSON-RPC errors a modern (2026-07-28) server answers with; any other
// failure of a modern request means the server is from the session era.
const HEADER_MISMATCH: i64 = -32020;
const MISSING_CAPABILITY: i64 = -32021;
const UNSUPPORTED_VERSION: i64 = -32022;
const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum McpError {
    #[error("the server address must be an https:// URL without a user name or password")]
    InvalidUrl,
    #[error("{0:?} does not resolve to a public address")]
    DisallowedHost(String),
    #[error("the server refused the key or needs one (status {0})")]
    Unauthenticated(u16),
    #[error("the server answered with status {status}: {message}")]
    Http { status: u16, message: String },
    #[error("the server did not answer in time")]
    Timeout,
    #[error("could not reach the server: {0}")]
    Transport(String),
    #[error("the server's answer is not valid MCP: {0}")]
    Protocol(String),
    #[error("the server reported an error: {0}")]
    Rpc(String),
    #[error("the tool asked for input Nexus cannot give it")]
    InputRequired,
}

/// A tool as the server describes it, bounded and checked. Tools that
/// break the protocol's rules are left out of a listing, with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    /// The server's own hints (`readOnlyHint`, `destructiveHint`, ...):
    /// untrusted, shown to the user, never a permission.
    pub annotations: Value,
}

impl McpTool {
    /// What the server says about whether the tool only reads.
    pub fn read_only_hint(&self) -> Option<bool> {
        self.annotations
            .get("readOnlyHint")
            .and_then(Value::as_bool)
    }
}

/// A tool call's outcome, as text for the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub truncated: bool,
    /// The tool itself reported a failure (`isError`), as text the model
    /// can use; not a protocol failure.
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub tools: Vec<McpTool>,
    /// Tools left out, with why (bad name, oversized schema, ...).
    pub rejected: Vec<String>,
}

#[derive(Debug, Clone)]
enum Era {
    Unknown,
    Modern,
    Legacy {
        version: String,
        session: Option<String>,
    },
}

pub struct McpClient {
    url: Url,
    token: Option<ApiKey>,
    /// Tests only: lets a loopback `http://` server stand in for a real one.
    allow_loopback: bool,
    era: Mutex<Era>,
    next_id: AtomicU64,
}

impl McpClient {
    pub fn new(url: &str, token: Option<ApiKey>, allow_loopback: bool) -> Result<Self, McpError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            url: checked_url(url, allow_loopback)?,
            token,
            allow_loopback,
            era: Mutex::new(Era::Unknown),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    /// Every tool the server offers (bounded), in its order.
    pub async fn list_tools(&self) -> Result<Listing, McpError> {
        let mut listing = Listing {
            tools: Vec::new(),
            rejected: Vec::new(),
        };
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.request("tools/list", params, Vec::new()).await?;
            let tools = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| McpError::Protocol("tools/list without a tools array".into()))?;
            for raw in tools {
                if listing.tools.len() >= MAX_TOOLS {
                    listing.rejected.push(format!(
                        "more than {MAX_TOOLS} tools; the rest are left out"
                    ));
                    return Ok(listing);
                }
                match parse_tool(raw) {
                    Ok(tool) => listing.tools.push(tool),
                    Err(reason) => listing.rejected.push(reason),
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(listing);
            }
        }
        listing.rejected.push(format!(
            "more than {MAX_PAGES} pages of tools; the rest are left out"
        ));
        Ok(listing)
    }

    /// Calls `tool` with `arguments`; the tool's own failure comes back as
    /// output with `is_error`, a protocol failure as an error.
    pub async fn call_tool(
        &self,
        tool: &McpTool,
        arguments: Value,
    ) -> Result<ToolOutput, McpError> {
        let mut headers = vec![("Mcp-Name".to_string(), header_value(&tool.name))];
        for (path, name) in header_params(&tool.input_schema).map_err(McpError::Protocol)? {
            if let Some(value) = value_at(&arguments, &path) {
                headers.push((format!("Mcp-Param-{name}"), header_value(&value)));
            }
        }
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool.name, "arguments": arguments }),
                headers,
            )
            .await?;
        if result.get("resultType").and_then(Value::as_str) == Some("input_required") {
            return Err(McpError::InputRequired);
        }
        Ok(tool_output(&result))
    }

    /// One request, in whichever era the server speaks (found on first use).
    async fn request(
        &self,
        method: &str,
        params: Value,
        extra: Vec<(String, String)>,
    ) -> Result<Value, McpError> {
        let era = self.era.lock().unwrap().clone();
        match era {
            Era::Unknown | Era::Modern => match self.modern(method, params.clone(), &extra).await {
                Ok(result) => {
                    *self.era.lock().unwrap() = Era::Modern;
                    Ok(result)
                }
                Err(Fallback::Legacy) if matches!(era, Era::Unknown) => {
                    tracing::info!(server = %self.url, "MCP server speaks the session era; using {LEGACY_VERSION}");
                    self.initialize().await?;
                    self.legacy(method, params).await
                }
                Err(Fallback::Legacy) => Err(McpError::Protocol(
                    "the server stopped answering modern requests".into(),
                )),
                Err(Fallback::Error(e)) => Err(e),
            },
            Era::Legacy { .. } => self.legacy(method, params).await,
        }
    }

    async fn modern(
        &self,
        method: &str,
        mut params: Value,
        extra: &[(String, String)],
    ) -> Result<Value, Fallback> {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
            "io.modelcontextprotocol/clientInfo": client_info(),
            "io.modelcontextprotocol/clientCapabilities": {},
        });
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut headers = vec![
            (
                "MCP-Protocol-Version".to_string(),
                MODERN_VERSION.to_string(),
            ),
            ("Mcp-Method".to_string(), method.to_string()),
        ];
        headers.extend(extra.iter().cloned());
        let reply = self.post(&body, &headers).await.map_err(Fallback::Error)?;
        match reply.status {
            401 | 403 => Err(Fallback::Error(McpError::Unauthenticated(reply.status))),
            200..=299 => {
                let message = reply.response(id).map_err(Fallback::Error)?;
                match rpc_error(&message) {
                    Some((code, _))
                        if code == UNSUPPORTED_VERSION && legacy_supported(&message) =>
                    {
                        Err(Fallback::Legacy)
                    }
                    Some((_, text)) => Err(Fallback::Error(McpError::Rpc(text))),
                    None => result_of(message).map_err(Fallback::Error),
                }
            }
            400..=499 => match reply.response(id).ok().as_ref().and_then(rpc_error) {
                Some((code, text))
                    if [HEADER_MISMATCH, MISSING_CAPABILITY, METHOD_NOT_FOUND].contains(&code) =>
                {
                    Err(Fallback::Error(McpError::Rpc(text)))
                }
                Some((code, _)) if code == UNSUPPORTED_VERSION => {
                    // A modern server that only speaks other versions:
                    // the session era is the one Nexus can offer instead.
                    Err(Fallback::Legacy)
                }
                _ => Err(Fallback::Legacy),
            },
            status => Err(Fallback::Error(McpError::Http {
                status,
                message: reply.snippet(),
            })),
        }
    }

    /// The session-era handshake: `initialize`, then `notifications/initialized`.
    async fn initialize(&self) -> Result<(), McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0", "id": id, "method": "initialize",
            "params": { "protocolVersion": LEGACY_VERSION, "capabilities": {}, "clientInfo": client_info() },
        });
        let reply = self.post(&body, &[]).await?;
        check_status(&reply)?;
        let session = reply.session.clone();
        let message = reply.response(id)?;
        if let Some((_, text)) = rpc_error(&message) {
            return Err(McpError::Rpc(text));
        }
        let version = result_of(message)?
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LEGACY_VERSION)
            .to_string();
        *self.era.lock().unwrap() = Era::Legacy {
            version: version.clone(),
            session: session.clone(),
        };
        let mut headers = vec![("MCP-Protocol-Version".to_string(), version)];
        if let Some(s) = session {
            headers.push(("Mcp-Session-Id".to_string(), s));
        }
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let reply = self.post(&note, &headers).await?;
        check_status(&reply)
    }

    async fn legacy(&self, method: &str, params: Value) -> Result<Value, McpError> {
        for attempt in 0..2 {
            let (version, session) = match self.era.lock().unwrap().clone() {
                Era::Legacy { version, session } => (version, session),
                _ => return Err(McpError::Protocol("no session".into())),
            };
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
            let mut headers = vec![("MCP-Protocol-Version".to_string(), version)];
            if let Some(s) = &session {
                headers.push(("Mcp-Session-Id".to_string(), s.clone()));
            }
            let reply = self.post(&body, &headers).await?;
            // An expired session is answered with 404: start a new one once.
            if reply.status == 404 && session.is_some() && attempt == 0 {
                self.initialize().await?;
                continue;
            }
            check_status(&reply)?;
            let message = reply.response(id)?;
            if let Some((_, text)) = rpc_error(&message) {
                return Err(McpError::Rpc(text));
            }
            return result_of(message);
        }
        Err(McpError::Protocol(
            "the session could not be renewed".into(),
        ))
    }

    async fn post(&self, body: &Value, headers: &[(String, String)]) -> Result<Reply, McpError> {
        let host = self.url.host_str().ok_or(McpError::InvalidUrl)?.to_string();
        let addr = resolve(&self.url, self.allow_loopback).await?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .resolve(&host, addr)
            .build()
            .map_err(|e| McpError::Transport(e.to_string()))?;
        let mut request = client
            .post(self.url.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, concat!("nexus-mcp/", env!("CARGO_PKG_VERSION")))
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(token) = &self.token {
            request = request.header(AUTHORIZATION, format!("Bearer {}", token.expose()));
        }
        let wanted = body.get("id").and_then(Value::as_u64);
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let response = request.send().await.map_err(transport)?;
            Reply::read(response, wanted).await
        })
        .await
        .map_err(|_| McpError::Timeout)?
    }
}

enum Fallback {
    Legacy,
    Error(McpError),
}

/// An HTTP answer: the JSON-RPC message for our request (read from a JSON
/// body or picked out of an SSE stream, other events ignored) or the raw
/// text of a failure.
struct Reply {
    status: u16,
    session: Option<String>,
    message: Option<Value>,
    text: String,
}

impl Reply {
    async fn read(response: reqwest::Response, wanted: Option<u64>) -> Result<Self, McpError> {
        let status = response.status().as_u16();
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .filter(|s| {
                !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
            })
            .map(str::to_string);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if content_type.starts_with("text/event-stream") {
            let message = read_stream(response, wanted).await?;
            return Ok(Self {
                status,
                session,
                message,
                text: String::new(),
            });
        }
        let bytes = read_bounded(response).await?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let message = serde_json::from_str::<Value>(&text).ok();
        Ok(Self {
            status,
            session,
            message,
            text,
        })
    }

    /// The JSON-RPC response to request `id`.
    fn response(&self, id: u64) -> Result<Value, McpError> {
        let message = self
            .message
            .clone()
            .ok_or_else(|| McpError::Protocol("no JSON-RPC response in the answer".into()))?;
        // An error without an id answers the request it came back on.
        if message.get("id").and_then(Value::as_u64) != Some(id) && message.get("error").is_none() {
            return Err(McpError::Protocol(
                "the answer is for another request".into(),
            ));
        }
        Ok(message)
    }

    fn snippet(&self) -> String {
        let text = self
            .message
            .as_ref()
            .and_then(rpc_error)
            .map(|(_, t)| t)
            .unwrap_or_else(|| self.text.clone());
        text.chars().take(300).collect()
    }
}

fn check_status(reply: &Reply) -> Result<(), McpError> {
    match reply.status {
        200..=299 => Ok(()),
        401 | 403 => Err(McpError::Unauthenticated(reply.status)),
        status => Err(McpError::Http {
            status,
            message: reply.snippet(),
        }),
    }
}

async fn read_bounded(response: reqwest::Response) -> Result<Vec<u8>, McpError> {
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(McpError::Protocol(format!(
                "the answer exceeded {MAX_BODY_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads SSE events until the response to `wanted` arrives; progress and
/// log notifications (and any request a session-era server sends) are
/// ignored. Dropping the stream afterwards closes it.
async fn read_stream(
    response: reqwest::Response,
    wanted: Option<u64>,
) -> Result<Option<Value>, McpError> {
    let mut parser = SseParser::new(MAX_EVENT_BYTES);
    let mut received = 0usize;
    let mut chunks = response.bytes_stream();
    let pick = |data: &str| -> Option<Value> {
        let value: Value = serde_json::from_str(data).ok()?;
        let is_response = value.get("result").is_some() || value.get("error").is_some();
        let ours = wanted.is_none() || value.get("id").and_then(Value::as_u64) == wanted;
        (is_response && ours).then_some(value)
    };
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        received += chunk.len();
        if received > MAX_BODY_BYTES {
            return Err(McpError::Protocol(format!(
                "the answer exceeded {MAX_BODY_BYTES} bytes"
            )));
        }
        for data in parser.push(&chunk).map_err(McpError::Protocol)? {
            if let Some(found) = pick(&data) {
                return Ok(Some(found));
            }
        }
    }
    Ok(parser
        .finish()
        .map_err(McpError::Protocol)?
        .and_then(|d| pick(&d)))
}

fn transport(err: reqwest::Error) -> McpError {
    if err.is_timeout() {
        McpError::Timeout
    } else {
        McpError::Transport(err.to_string())
    }
}

fn client_info() -> Value {
    json!({ "name": "nexus", "version": env!("CARGO_PKG_VERSION") })
}

fn rpc_error(message: &Value) -> Option<(i64, String)> {
    let error = message.get("error")?;
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let text: String = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .chars()
        .take(300)
        .collect();
    Some((code, text))
}

fn legacy_supported(message: &Value) -> bool {
    message["error"]["data"]["supported"]
        .as_array()
        .is_some_and(|v| v.iter().any(|s| s.as_str() == Some(LEGACY_VERSION)))
}

fn result_of(message: Value) -> Result<Value, McpError> {
    message
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("a response without a result".into()))
}

/// `https://` only (plus loopback `http://` for tests), no credentials in
/// the URL, no fragment.
fn checked_url(url: &str, allow_loopback: bool) -> Result<Url, McpError> {
    let parsed = Url::parse(url.trim()).map_err(|_| McpError::InvalidUrl)?;
    let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let scheme_ok =
        parsed.scheme() == "https" || (allow_loopback && loopback && parsed.scheme() == "http");
    if !scheme_ok
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || url.len() > 2000
    {
        return Err(McpError::InvalidUrl);
    }
    // An address written as an IP is refused now; a name is checked when it
    // is resolved, before every connection.
    if let Some(ip) = parsed
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|h| h.parse::<IpAddr>().ok())
        && crate::web::is_disallowed(ip)
        && !(allow_loopback && ip.is_loopback())
    {
        return Err(McpError::DisallowedHost(ip.to_string()));
    }
    Ok(parsed)
}

/// Resolves the host and refuses it unless every address is public (the
/// web read's rule); the connection is pinned to the address checked here.
async fn resolve(url: &Url, allow_loopback: bool) -> Result<SocketAddr, McpError> {
    let host = url.host_str().ok_or(McpError::InvalidUrl)?;
    let port = url.port_or_known_default().ok_or(McpError::InvalidUrl)?;
    let lookup = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((lookup, port))
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?
        .collect();
    let allowed = |a: &SocketAddr| {
        !crate::web::is_disallowed(a.ip()) || (allow_loopback && a.ip().is_loopback())
    };
    if addrs.is_empty() || !addrs.iter().all(allowed) {
        return Err(McpError::DisallowedHost(host.to_string()));
    }
    Ok(addrs[0])
}

fn parse_tool(raw: &Value) -> Result<McpTool, String> {
    let name = raw
        .get("name")
        .and_then(Value::as_str)
        .ok_or("a tool without a name")?
        .to_string();
    let label: String = name.chars().take(60).collect();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(format!("{label:?}: the name breaks MCP's naming rules"));
    }
    let input_schema = raw
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object" }));
    if !input_schema.is_object() || input_schema.to_string().len() > MAX_SCHEMA_BYTES {
        return Err(format!(
            "{label}: the parameter schema is missing or larger than {MAX_SCHEMA_BYTES} bytes"
        ));
    }
    header_params(&input_schema).map_err(|e| format!("{label}: {e}"))?;
    let description: String = raw
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(MAX_DESCRIPTION_CHARS)
        .collect();
    let title = raw
        .get("title")
        .or_else(|| raw.get("annotations").and_then(|a| a.get("title")))
        .and_then(Value::as_str)
        .map(|t| t.chars().take(200).collect());
    let annotations = raw
        .get("annotations")
        .filter(|a| a.is_object() && a.to_string().len() <= 4096)
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(McpTool {
        name,
        title,
        description,
        input_schema,
        annotations,
    })
}

/// The tool parameters to mirror into `Mcp-Param-{name}` headers, checked
/// against the spec's rules (a definition that breaks them is rejected).
pub fn header_params(schema: &Value) -> Result<Vec<(Vec<String>, String)>, String> {
    let mut found = Vec::new();
    walk_headers(schema, &mut Vec::new(), true, &mut found)?;
    let mut seen = std::collections::HashSet::new();
    for (_, name) in &found {
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("x-mcp-header {name:?} is used twice"));
        }
    }
    Ok(found)
}

fn walk_headers(
    schema: &Value,
    path: &mut Vec<String>,
    reachable: bool,
    found: &mut Vec<(Vec<String>, String)>,
) -> Result<(), String> {
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    if let Some(header) = object.get("x-mcp-header") {
        let name = header.as_str().unwrap_or("");
        let token = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
        let primitive = matches!(
            object.get("type").and_then(Value::as_str),
            Some("string" | "integer" | "boolean")
        );
        if !reachable || path.is_empty() || !token || !primitive {
            return Err(format!("x-mcp-header {name:?} breaks the header rules"));
        }
        found.push((path.clone(), name.to_string()));
    }
    for (key, value) in object {
        if key == "properties" {
            if let Some(props) = value.as_object() {
                for (prop, sub) in props {
                    path.push(prop.clone());
                    walk_headers(sub, path, reachable, found)?;
                    path.pop();
                }
            }
        } else if value.is_object() || value.is_array() {
            // Anything not a chain of `properties` (items, oneOf, $defs, ...)
            // may not carry a header annotation.
            for sub in value
                .as_array()
                .map(|a| a.iter().collect())
                .unwrap_or_else(|| vec![value])
            {
                walk_headers(sub, &mut Vec::new(), false, found)?;
            }
        }
    }
    Ok(())
}

fn value_at(arguments: &Value, path: &[String]) -> Option<String> {
    let mut current = arguments;
    for key in path {
        current = current.get(key)?;
    }
    match current {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        _ => None,
    }
}

/// A header value as the spec requires: as is when it is plain visible
/// ASCII, otherwise Base64 in the `=?base64?…?=` sentinel.
pub fn header_value(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| (0x20..=0x7e).contains(&b) || b == b'\t')
        && !value.starts_with([' ', '\t'])
        && !value.ends_with([' ', '\t'])
        && !(value.starts_with("=?base64?") && value.ends_with("?="));
    if plain {
        value.to_string()
    } else {
        format!("=?base64?{}?=", base64(value.as_bytes()))
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The result as text: text content as is, structured content as JSON when
/// there is no text, media and binary resources named but not included.
fn tool_output(result: &Value) -> ToolOutput {
    let mut pieces: Vec<String> = Vec::new();
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let piece = match item.get("type").and_then(Value::as_str) {
            Some("text") => item.get("text").and_then(Value::as_str).map(str::to_string),
            Some("image") => Some("[an image was returned; Nexus does not pass images on]".into()),
            Some("audio") => Some("[audio was returned; Nexus does not pass audio on]".into()),
            Some("resource_link") => Some(format!(
                "[link: {} {}]",
                item.get("name").and_then(Value::as_str).unwrap_or(""),
                item.get("uri").and_then(Value::as_str).unwrap_or("")
            )),
            Some("resource") => {
                let resource = item.get("resource").cloned().unwrap_or_default();
                let uri = resource.get("uri").and_then(Value::as_str).unwrap_or("");
                Some(match resource.get("text").and_then(Value::as_str) {
                    Some(text) => format!("[{uri}]\n{text}"),
                    None => format!("[a binary resource was returned: {uri}]"),
                })
            }
            _ => None,
        };
        pieces.extend(piece);
    }
    if pieces.iter().all(|p| p.trim().is_empty())
        && let Some(structured) = result.get("structuredContent")
    {
        pieces = vec![structured.to_string()];
    }
    let joined = pieces.join("\n\n");
    let truncated = joined.chars().count() > MAX_RESULT_CHARS;
    ToolOutput {
        text: if truncated {
            joined.chars().take(MAX_RESULT_CHARS).collect()
        } else {
            joined
        },
        truncated,
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_values_follow_the_spec_encoding() {
        // Examples from the spec's table (2026-07-28, Streamable HTTP).
        assert_eq!(header_value("us-west1"), "us-west1");
        assert_eq!(
            header_value("Hello, 世界"),
            "=?base64?SGVsbG8sIOS4lueVjA==?="
        );
        assert_eq!(header_value(" padded "), "=?base64?IHBhZGRlZCA=?=");
        assert_eq!(header_value("line1\nline2"), "=?base64?bGluZTEKbGluZTI=?=");
        assert_eq!(
            header_value("=?base64?literal?="),
            "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="
        );
    }

    #[test]
    fn header_parameters_are_taken_only_from_property_chains() {
        let ok = json!({"type": "object", "properties": {
            "region": {"type": "string", "x-mcp-header": "Region"},
            "opts": {"type": "object", "properties": {"tenant": {"type": "integer", "x-mcp-header": "Tenant"}}}
        }});
        let params = header_params(&ok).unwrap();
        assert!(params.contains(&(vec!["region".into()], "Region".into())));
        assert!(params.contains(&(vec!["opts".into(), "tenant".into()], "Tenant".into())));
        let args = json!({"region": "eu", "opts": {"tenant": 7}});
        assert_eq!(
            value_at(&args, &["opts".into(), "tenant".into()]).as_deref(),
            Some("7")
        );

        for bad in [
            json!({"type": "object", "properties": {"n": {"type": "number", "x-mcp-header": "N"}}}),
            json!({"type": "object", "properties": {"l": {"type": "array", "items": {"type": "string", "x-mcp-header": "L"}}}}),
            json!({"type": "object", "properties": {"a": {"type": "string", "x-mcp-header": "Bad Name"}}}),
            json!({"type": "object", "properties": {
                "a": {"type": "string", "x-mcp-header": "Same"}, "b": {"type": "string", "x-mcp-header": "same"}}}),
        ] {
            assert!(header_params(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tools_breaking_the_rules_are_left_out_with_a_reason() {
        assert!(
            parse_tool(&json!({"name": "has space", "inputSchema": {"type": "object"}})).is_err()
        );
        assert!(parse_tool(&json!({"name": "x".repeat(129)})).is_err());
        let huge = json!({"name": "big", "inputSchema": {"type": "object", "description": "y".repeat(MAX_SCHEMA_BYTES)}});
        assert!(parse_tool(&huge).is_err());
        let tool = parse_tool(&json!({
            "name": "search", "description": "d".repeat(MAX_DESCRIPTION_CHARS + 50),
            "annotations": {"readOnlyHint": true, "title": "Search"}
        }))
        .unwrap();
        assert_eq!(tool.description.chars().count(), MAX_DESCRIPTION_CHARS);
        assert_eq!(tool.title.as_deref(), Some("Search"));
        assert_eq!(tool.read_only_hint(), Some(true));
        assert_eq!(tool.input_schema, json!({"type": "object"}));
    }

    #[test]
    fn results_become_bounded_text() {
        let out = tool_output(&json!({"content": [
            {"type": "text", "text": "first"},
            {"type": "image", "data": "AAAA", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "doc://a", "text": "body"}},
            {"type": "resource_link", "uri": "https://x/y", "name": "y"}
        ], "isError": true}));
        assert!(out.is_error);
        assert!(out.text.starts_with("first"));
        assert!(out.text.contains("does not pass images on"));
        assert!(out.text.contains("[doc://a]\nbody"));
        assert!(
            !out.text.contains("AAAA"),
            "media bytes never reach the model"
        );
        let structured = tool_output(&json!({"content": [], "structuredContent": {"n": 1}}));
        assert_eq!(structured.text, r#"{"n":1}"#);
        let long = tool_output(
            &json!({"content": [{"type": "text", "text": "z".repeat(MAX_RESULT_CHARS + 10)}]}),
        );
        assert!(long.truncated);
        assert_eq!(long.text.chars().count(), MAX_RESULT_CHARS);
    }

    #[test]
    fn only_public_https_addresses_are_accepted() {
        for bad in [
            "http://example.com/mcp",
            "ftp://example.com/mcp",
            "https://user:pass@example.com/mcp",
            "https://example.com/mcp#frag",
            "not a url",
        ] {
            assert_eq!(
                checked_url(bad, false).unwrap_err(),
                McpError::InvalidUrl,
                "{bad}"
            );
        }
        assert!(checked_url("https://learn.microsoft.com/api/mcp", false).is_ok());
        assert!(
            checked_url("http://127.0.0.1:9/mcp", false).is_err(),
            "loopback only in tests"
        );
        assert!(checked_url("http://127.0.0.1:9/mcp", true).is_ok());
        for private in [
            "https://10.0.0.5/mcp",
            "https://192.168.1.1/mcp",
            "https://169.254.169.254/latest",
            "https://127.0.0.1/mcp",
            "https://0x7f.1/mcp",
            "https://[::1]/mcp",
            "https://[fd00::1]/mcp",
        ] {
            assert!(
                matches!(
                    checked_url(private, false),
                    Err(McpError::DisallowedHost(_))
                ),
                "{private}"
            );
        }
    }
}
