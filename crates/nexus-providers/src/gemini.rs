//! Gemini API adapter (REST `v1beta`, checked against ai.google.dev on
//! 2026-10-15). The key travels only in the `x-goog-api-key` header — the
//! official examples put it in the URL, which Nexus refuses — redirects are
//! disabled so the key never reaches another origin, and the endpoint is
//! fixed to Google's (or a loopback test server).

use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sse::SseParser;
use crate::{
    ApiKey, FinishReason, Message, ModelInfo, ProviderError, ProviderEvent, ProviderRequest, Role,
    Usage,
};

pub const PRODUCTION_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const API_VERSION: &str = "v1beta";
const KEY_HEADER: &str = "x-goog-api-key";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// Whole model listing, all pages; stays below the RPC deadline of the
/// calls that wait for it (`nexus_client::PROVIDER_CALL_DEADLINE`).
const LIST_BUDGET: Duration = Duration::from_secs(30);
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODEL_PAGES: usize = 10;

pub struct GeminiClient {
    http: reqwest::Client,
    base_url: String,
    key: ApiKey,
}

impl GeminiClient {
    /// `base_url` must be [`PRODUCTION_BASE_URL`] or a loopback test server
    /// (`http://127.0.0.1:<port>`); anything else is refused.
    pub fn new(key: ApiKey, base_url: &str) -> Result<Self, ProviderError> {
        let base_url = allowed_base_url(base_url)?;
        // reqwest is built without a bundled crypto provider; use ring.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("nexus/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::InvalidConfiguration(e.to_string()))?;
        Ok(Self {
            http,
            base_url,
            key,
        })
    }

    /// Models that support `generateContent`, as the API reports them.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        tokio::time::timeout(LIST_BUDGET, self.list_model_pages())
            .await
            .map_err(|_| ProviderError::Timeout)?
    }

    async fn list_model_pages(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut models = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..MAX_MODEL_PAGES {
            let mut url = reqwest::Url::parse(&format!("{}/{API_VERSION}/models", self.base_url))
                .map_err(|e| ProviderError::InvalidConfiguration(e.to_string()))?;
            url.query_pairs_mut().append_pair("pageSize", "200");
            if let Some(token) = &page_token {
                url.query_pairs_mut().append_pair("pageToken", token);
            }
            let response = self
                .http
                .get(url)
                .header(KEY_HEADER, self.key.expose())
                .timeout(LIST_TIMEOUT)
                .send()
                .await
                .map_err(transport)?;
            if !response.status().is_success() {
                return Err(self.error_from(response).await);
            }
            let body = read_limited(response, MAX_BODY_BYTES).await?;
            let page: ModelPage = serde_json::from_slice(&body)
                .map_err(|e| ProviderError::InvalidResponse(format!("model list: {e}")))?;
            models.extend(
                page.models
                    .into_iter()
                    .filter(|m| {
                        m.supported_generation_methods
                            .iter()
                            .any(|g| g == "generateContent")
                    })
                    .map(|m| ModelInfo {
                        id: m
                            .name
                            .strip_prefix("models/")
                            .unwrap_or(&m.name)
                            .to_string(),
                        display_name: m.display_name.unwrap_or_default(),
                        input_token_limit: m.input_token_limit,
                        output_token_limit: m.output_token_limit,
                    }),
            );
            match page.next_page_token.filter(|t| !t.is_empty()) {
                Some(next) => page_token = Some(next),
                None => break,
            }
        }
        Ok(models)
    }

    /// Streams one generation. Events are delivered as they arrive; the
    /// whole call is bounded by `request.deadline`. Dropping the future
    /// cancels it (the connection is closed).
    pub async fn stream(
        &self,
        request: &ProviderRequest,
        mut on_event: impl FnMut(ProviderEvent) + Send,
    ) -> Result<(), ProviderError> {
        let model = checked_model_id(&request.model)?;
        let url = format!(
            "{}/{API_VERSION}/models/{model}:streamGenerateContent?alt=sse",
            self.base_url
        );
        let body = request_body(request);
        let call = async {
            let response = self
                .http
                .post(url)
                .header(KEY_HEADER, self.key.expose())
                .header(CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .await
                .map_err(transport)?;
            if !response.status().is_success() {
                return Err(self.error_from(response).await);
            }

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
                    handle_chunk(&data, &mut on_event)?;
                }
            }
            if let Some(data) = parser.finish().map_err(ProviderError::InvalidResponse)? {
                handle_chunk(&data, &mut on_event)?;
            }
            Ok(())
        };
        tokio::time::timeout(request.deadline, call)
            .await
            .unwrap_or(Err(ProviderError::Timeout))
    }

    async fn error_from(&self, response: reqwest::Response) -> ProviderError {
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = read_limited(response, 64 * 1024).await.unwrap_or_default();
        let (message, api_key_invalid) = serde_json::from_slice::<ErrorEnvelope>(&body)
            .map(|e| {
                let invalid = e
                    .error
                    .details
                    .iter()
                    .any(|d| d.reason.as_deref() == Some("API_KEY_INVALID"));
                (e.error.message, invalid)
            })
            .unwrap_or_else(|_| {
                (
                    String::from_utf8_lossy(&body[..body.len().min(300)]).into_owned(),
                    false,
                )
            });
        // Defence in depth: a provider message must never carry our key out.
        let message = message.replace(self.key.expose(), "<redacted>");
        match status {
            401 | 403 => ProviderError::Unauthenticated(message),
            // Google answers a malformed/unknown key with 400 INVALID_ARGUMENT,
            // not 401; its structured reason still says this is about the key.
            400 if api_key_invalid => ProviderError::Unauthenticated(message),
            429 => ProviderError::RateLimited { retry_after },
            400..=499 => ProviderError::Rejected { status, message },
            _ => ProviderError::Unavailable { status, message },
        }
    }
}

fn allowed_base_url(base: &str) -> Result<String, ProviderError> {
    let base = base.trim_end_matches('/');
    let loopback = base
        .strip_prefix("http://127.0.0.1:")
        .is_some_and(|port| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()));
    if base == PRODUCTION_BASE_URL || loopback {
        Ok(base.to_string())
    } else {
        Err(ProviderError::InvalidConfiguration(format!(
            "endpoint {base:?} is not allowed; only {PRODUCTION_BASE_URL} (or a loopback test server)"
        )))
    }
}

/// The model id goes into the URL path, so only a safe character set.
fn checked_model_id(model: &str) -> Result<&str, ProviderError> {
    let id = model.strip_prefix("models/").unwrap_or(model);
    let valid = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if valid {
        Ok(id)
    } else {
        Err(ProviderError::InvalidConfiguration(format!(
            "invalid model id {model:?}"
        )))
    }
}

/// Best-effort major-version read from the model id (`gemini-3.8-flash` ->
/// 3, `gemini-2.5-flash` -> 2). An id that does not parse is treated as
/// pre-3, so it never gets a generationConfig field it might reject.
fn is_gemini_3_or_later(model: &str) -> bool {
    model
        .strip_prefix("gemini-")
        .and_then(|rest| rest.split(['.', '-']).next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 3)
}

fn handle_chunk(data: &str, on_event: &mut impl FnMut(ProviderEvent)) -> Result<(), ProviderError> {
    let chunk: GenerateResponse = serde_json::from_str(data)
        .map_err(|e| ProviderError::InvalidResponse(format!("stream chunk: {e}")))?;
    if let Some(reason) = chunk.prompt_feedback.and_then(|f| f.block_reason) {
        return Err(ProviderError::Blocked(reason));
    }
    if let Some(candidate) = chunk.candidates.into_iter().next() {
        for part in candidate.content.map(|c| c.parts).unwrap_or_default() {
            if part.thought == Some(true) {
                continue; // reasoning traces are not part of the answer
            }
            if let Some(text) = part.text.filter(|t| !t.is_empty()) {
                on_event(ProviderEvent::TextDelta(text));
            }
            if let Some(call) = part.function_call {
                on_event(ProviderEvent::ToolProposal {
                    name: call.name,
                    arguments: call.args,
                });
            }
        }
        if let Some(reason) = candidate.finish_reason {
            if let Some(usage) = &chunk.usage_metadata {
                on_event(ProviderEvent::Usage(usage.into()));
            }
            on_event(ProviderEvent::Finished(match reason.as_str() {
                "STOP" => FinishReason::Stop,
                "MAX_TOKENS" => FinishReason::MaxTokens,
                "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "SPII" => FinishReason::Safety,
                other => FinishReason::Other(other.to_string()),
            }));
        }
    }
    Ok(())
}

fn request_body(request: &ProviderRequest) -> Value {
    let contents: Vec<Value> = request
        .messages
        .iter()
        .map(|Message { role, parts }| {
            let parts: Vec<Value> = parts
                .iter()
                .map(|p| match p {
                    crate::Part::Text(text) => serde_json::json!({ "text": text }),
                    crate::Part::FunctionCall { name, arguments } => {
                        serde_json::json!({ "functionCall": { "name": name, "args": arguments } })
                    }
                    crate::Part::FunctionResponse { name, response } => {
                        serde_json::json!({ "functionResponse": { "name": name, "response": response } })
                    }
                })
                .collect();
            serde_json::json!({
                "role": match role { Role::User => "user", Role::Model => "model" },
                "parts": parts,
            })
        })
        .collect();
    let mut generation_config = serde_json::json!({ "maxOutputTokens": request.max_output_tokens });
    // Gemini 3.x models think by default (confirmed against ai.google.dev,
    // 2026-10-15: "medium" unless set otherwise); the user chooses the
    // level (speed vs. reasoning quality) per connection. 2.x models use a
    // different field (thinkingBudget) with no evidence of the same
    // default-latency issue, so they are left alone rather than guessed at.
    if let Some(level) = request.thinking_level
        && is_gemini_3_or_later(&request.model)
    {
        generation_config["thinkingConfig"] =
            serde_json::json!({ "thinkingLevel": level.as_str() });
    }
    let mut body = serde_json::json!({
        "contents": contents,
        "generationConfig": generation_config,
    });
    if !request.system_instruction.is_empty() {
        body["systemInstruction"] =
            serde_json::json!({ "parts": [{ "text": request.system_instruction }] });
    }
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|t| serde_json::json!({ "name": t.name, "description": t.description, "parameters": t.parameters }))
            .collect();
        body["tools"] = serde_json::json!([{ "functionDeclarations": declarations }]);
    }
    body
}

async fn read_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ProviderError> {
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

fn transport(err: reqwest::Error) -> ProviderError {
    if err.is_timeout() {
        ProviderError::Timeout
    } else {
        // reqwest's message may include the URL; it never contains the key.
        ProviderError::Transport(err.to_string())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelPage {
    #[serde(default)]
    models: Vec<ApiModel>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiModel {
    name: String,
    display_name: Option<String>,
    input_token_limit: Option<u32>,
    output_token_limit: Option<u32>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    prompt_feedback: Option<PromptFeedback>,
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<Content>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    text: Option<String>,
    function_call: Option<FunctionCall>,
    thought: Option<bool>,
}

#[derive(Deserialize)]
struct FunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptFeedback {
    block_reason: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

impl From<&UsageMetadata> for Usage {
    fn from(u: &UsageMetadata) -> Self {
        Usage {
            prompt_tokens: u.prompt_token_count.unwrap_or(0),
            output_tokens: u.candidates_token_count.unwrap_or(0),
            total_tokens: u.total_token_count.unwrap_or(0),
        }
    }
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
    #[serde(default)]
    details: Vec<ErrorDetail>,
}

/// Only the `ErrorInfo` detail (if present) carries `reason`; other detail
/// types (e.g. `LocalizedMessage`) are skipped by leaving it `None`.
#[derive(Deserialize)]
struct ErrorDetail {
    reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_google_or_loopback_endpoints_are_allowed() {
        assert!(allowed_base_url(PRODUCTION_BASE_URL).is_ok());
        assert!(allowed_base_url("http://127.0.0.1:8080").is_ok());
        for bad in [
            "http://generativelanguage.googleapis.com",
            "https://evil.example.com",
            "https://generativelanguage.googleapis.com.evil.com",
            "http://127.0.0.1.evil.com:80",
            "http://localhost:8080",
            "http://127.0.0.1:",
        ] {
            assert!(allowed_base_url(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn gemini_3_generation_is_detected_from_the_model_id() {
        for yes in ["gemini-3.8-flash", "gemini-3-pro-preview", "gemini-30-x"] {
            assert!(is_gemini_3_or_later(yes), "{yes} should be 3.x or later");
        }
        for no in [
            "gemini-2.5-flash",
            "gemini-2.0-pro",
            "gemini-1.5-flash",
            "gpt-4",
            "",
            "gemini-",
            "gemini-flash",
        ] {
            assert!(
                !is_gemini_3_or_later(no),
                "{no} should not be treated as 3.x"
            );
        }
    }

    #[test]
    fn model_ids_cannot_inject_into_the_url() {
        assert_eq!(
            checked_model_id("models/gemini-flash").unwrap(),
            "gemini-flash"
        );
        for bad in ["../secret", "a/b", "m?key=x", "m#x", "", "-x", "m:generate"] {
            assert!(checked_model_id(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
