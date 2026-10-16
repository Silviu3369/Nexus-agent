//! Anthropic Messages API adapter (`POST /v1/messages`, streaming), checked
//! against platform.claude.com on 2026-10-16. It also serves DeepSeek, whose
//! Anthropic-compatible endpoint (`https://api.deepseek.com/anthropic`,
//! checked against api-docs.deepseek.com the same day) supports streaming,
//! `tool_use` and `tool_result`; DeepSeek's model list comes from its own
//! `/models`. The key travels only in a header; redirects are refused.

use std::collections::HashMap;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http::{
    self, LIST_BUDGET, LIST_TIMEOUT, MAX_BODY_BYTES, MAX_ERROR_BYTES, read_limited, transport,
};
use crate::{
    ApiKey, FinishReason, Message, ModelInfo, Part, ProviderError, ProviderEvent, ProviderKind,
    ProviderRequest, Role, Usage,
};

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const API_VERSION: &str = "2023-06-01";
const MAX_MODEL_PAGES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Anthropic,
    DeepSeek,
}

pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    key: ApiKey,
    flavor: Flavor,
}

impl AnthropicClient {
    /// `base_url` must be the flavour's production origin or a loopback
    /// test server.
    pub fn new(key: ApiKey, base_url: &str, flavor: Flavor) -> Result<Self, ProviderError> {
        let production = match flavor {
            Flavor::Anthropic => ANTHROPIC_BASE_URL,
            Flavor::DeepSeek => DEEPSEEK_BASE_URL,
        };
        Ok(Self {
            http: http::client()?,
            base_url: http::allowed_base_url(base_url, production)?,
            key,
            flavor,
        })
    }

    fn provider(&self) -> ProviderKind {
        match self.flavor {
            Flavor::Anthropic => ProviderKind::Anthropic,
            Flavor::DeepSeek => ProviderKind::DeepSeek,
        }
    }

    fn messages_url(&self) -> String {
        match self.flavor {
            Flavor::Anthropic => format!("{}/v1/messages", self.base_url),
            Flavor::DeepSeek => format!("{}/anthropic/v1/messages", self.base_url),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let call = async {
            match self.flavor {
                Flavor::Anthropic => self.list_anthropic().await,
                Flavor::DeepSeek => self.list_deepseek().await,
            }
        };
        tokio::time::timeout(LIST_BUDGET, call)
            .await
            .map_err(|_| ProviderError::Timeout)?
    }

    async fn list_anthropic(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_MODEL_PAGES {
            let mut url = reqwest::Url::parse(&format!("{}/v1/models", self.base_url))
                .map_err(|e| ProviderError::InvalidConfiguration(e.to_string()))?;
            url.query_pairs_mut().append_pair("limit", "100");
            if let Some(id) = &after {
                url.query_pairs_mut().append_pair("after_id", id);
            }
            let response = self
                .http
                .get(url)
                .header("x-api-key", self.key.expose())
                .header("anthropic-version", API_VERSION)
                .timeout(LIST_TIMEOUT)
                .send()
                .await
                .map_err(transport)?;
            if !response.status().is_success() {
                return Err(self.error_from(response).await);
            }
            let body = read_limited(response, MAX_BODY_BYTES).await?;
            let page: AnthropicModelPage = serde_json::from_slice(&body)
                .map_err(|e| ProviderError::InvalidResponse(format!("model list: {e}")))?;
            models.extend(page.data.into_iter().map(|m| ModelInfo {
                display_name: m.display_name.unwrap_or_else(|| m.id.clone()),
                id: m.id,
                input_token_limit: m.max_input_tokens,
                output_token_limit: m.max_tokens,
            }));
            match (page.has_more, page.last_id) {
                (true, Some(last)) => after = Some(last),
                _ => break,
            }
        }
        Ok(models)
    }

    async fn list_deepseek(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let response = self
            .http
            .get(format!("{}/models", self.base_url))
            .header(AUTHORIZATION, format!("Bearer {}", self.key.expose()))
            .timeout(LIST_TIMEOUT)
            .send()
            .await
            .map_err(transport)?;
        if !response.status().is_success() {
            return Err(self.error_from(response).await);
        }
        let body = read_limited(response, MAX_BODY_BYTES).await?;
        let list: DeepSeekModelList = serde_json::from_slice(&body)
            .map_err(|e| ProviderError::InvalidResponse(format!("model list: {e}")))?;
        Ok(list
            .data
            .into_iter()
            .map(|m| ModelInfo {
                display_name: m.id.clone(),
                id: m.id,
                input_token_limit: None,
                output_token_limit: None,
            })
            .collect())
    }

    /// Streams one message. A model that refuses the effort setting is
    /// asked once more without it, before anything was streamed.
    pub async fn stream(
        &self,
        request: &ProviderRequest,
        mut on_event: impl FnMut(ProviderEvent) + Send,
    ) -> Result<(), ProviderError> {
        let call = async {
            let with_effort = request.thinking_level.is_some();
            match self.stream_once(request, with_effort, &mut on_event).await {
                Err(ProviderError::Rejected {
                    status: 400,
                    message,
                }) if with_effort && {
                    let m = message.to_ascii_lowercase();
                    m.contains("effort") || m.contains("output_config")
                } =>
                {
                    tracing::info!(model = %request.model, "model refused the effort setting; asking without it");
                    self.stream_once(request, false, &mut on_event).await
                }
                other => other,
            }
        };
        tokio::time::timeout(request.deadline, call)
            .await
            .unwrap_or(Err(ProviderError::Timeout))
    }

    async fn stream_once(
        &self,
        request: &ProviderRequest,
        with_effort: bool,
        on_event: &mut (impl FnMut(ProviderEvent) + Send),
    ) -> Result<(), ProviderError> {
        let body = request_body(request, with_effort, self.provider())?;
        let response = self
            .http
            .post(self.messages_url())
            .header("x-api-key", self.key.expose())
            .header("anthropic-version", API_VERSION)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(transport)?;
        if !response.status().is_success() {
            return Err(self.error_from(response).await);
        }
        let key = self.key.expose().to_string();
        let mut state = StreamState::new(self.provider());
        http::read_sse(response, |data| state.handle(data, on_event, &key)).await
    }

    async fn error_from(&self, response: reqwest::Response) -> ProviderError {
        let status = response.status().as_u16();
        let retry_after = http::retry_after(&response);
        let body = read_limited(response, MAX_ERROR_BYTES)
            .await
            .unwrap_or_default();
        let message = serde_json::from_slice::<ErrorEnvelope>(&body)
            .map(|e| e.error.message)
            .unwrap_or_else(|_| String::from_utf8_lossy(&body[..body.len().min(300)]).into_owned());
        let message = message.replace(self.key.expose(), "<redacted>");
        http::status_error(status, message, retry_after)
    }
}

fn request_body(
    request: &ProviderRequest,
    with_effort: bool,
    provider: ProviderKind,
) -> Result<Value, ProviderError> {
    let model = http::checked_body_model_id(&request.model)?;
    let mut messages: Vec<Value> = Vec::new();
    let mut generated = 0usize;
    let mut pending: Vec<String> = Vec::new();
    for Message { role, parts } in &request.messages {
        // The conversation must open with the user; a reply left over at
        // the start of a trimmed history is dropped.
        if messages.is_empty() && *role == Role::Model {
            continue;
        }
        let mut content = Vec::new();
        for part in parts {
            match part {
                Part::Text(text) if !text.is_empty() => {
                    content.push(json!({ "type": "text", "text": text }))
                }
                Part::Text(_) => {}
                // Signed thinking goes back unchanged, before the tool use
                // it led to, and only to the provider that wrote it.
                Part::Replay {
                    provider: from,
                    block,
                } => {
                    if *from == provider {
                        content.push(block.clone());
                    }
                }
                Part::FunctionCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    let id = id.clone().unwrap_or_else(|| {
                        generated += 1;
                        format!("toolu_nexus_{generated}")
                    });
                    pending.push(id.clone());
                    content.push(
                        json!({ "type": "tool_use", "id": id, "name": name, "input": arguments }),
                    );
                }
                Part::FunctionResponse { id, response, .. } => {
                    let id = id
                        .clone()
                        .or_else(|| (!pending.is_empty()).then(|| pending.remove(0)))
                        .unwrap_or_else(|| "toolu_nexus_0".to_string());
                    content.push(json!({ "type": "tool_result", "tool_use_id": id, "content": response.to_string() }));
                }
            }
        }
        if content.is_empty() {
            continue;
        }
        messages.push(json!({
            "role": match role { Role::User => "user", Role::Model => "assistant" },
            "content": content,
        }));
    }
    let mut body = json!({
        "model": model,
        "max_tokens": request.max_output_tokens,
        "messages": messages,
        "stream": true,
    });
    if !request.system_instruction.is_empty() {
        body["system"] = json!(request.system_instruction);
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
                .collect(),
        );
    }
    if with_effort && let Some(level) = request.thinking_level {
        body["output_config"] = json!({ "effort": level.as_lower() });
    }
    Ok(body)
}

/// Tool-use blocks arrive as a start (id, name) followed by JSON fragments;
/// they are assembled per block index and proposed when the block stops.
/// Thinking blocks are assembled the same way (text and signature) and
/// handed back for replay, never shown: with thinking on, a tool call must
/// be continued with the unchanged signed thinking that preceded it.
struct StreamState {
    provider: ProviderKind,
    tools: HashMap<u64, (String, String, String)>,
    thinking: HashMap<u64, Value>,
    usage: Usage,
    stop_reason: Option<String>,
}

impl StreamState {
    fn new(provider: ProviderKind) -> Self {
        Self {
            provider,
            tools: HashMap::new(),
            thinking: HashMap::new(),
            usage: Usage::default(),
            stop_reason: None,
        }
    }

    fn handle(
        &mut self,
        data: &str,
        on_event: &mut impl FnMut(ProviderEvent),
        key: &str,
    ) -> Result<(), ProviderError> {
        let event: StreamEvent = serde_json::from_str(data)
            .map_err(|e| ProviderError::InvalidResponse(format!("stream event: {e}")))?;
        match event.kind.as_str() {
            "message_start" => {
                if let Some(u) = event.message.and_then(|m| m.usage) {
                    self.usage.prompt_tokens = u.input_tokens.unwrap_or(0);
                    self.usage.output_tokens = u.output_tokens.unwrap_or(0);
                }
            }
            "content_block_start" => {
                if let (Some(index), Some(block)) = (event.index, event.content_block) {
                    match block.kind.as_str() {
                        "tool_use" => {
                            self.tools.insert(
                                index,
                                (
                                    block.id.unwrap_or_default(),
                                    block.name.unwrap_or_default(),
                                    String::new(),
                                ),
                            );
                        }
                        "thinking" => {
                            self.thinking.insert(
                                index,
                                json!({
                                    "type": "thinking",
                                    "thinking": block.thinking.unwrap_or_default(),
                                    "signature": block.signature.unwrap_or_default(),
                                }),
                            );
                        }
                        "redacted_thinking" => {
                            self.thinking.insert(
                                index,
                                json!({ "type": "redacted_thinking", "data": block.data.unwrap_or_default() }),
                            );
                        }
                        _ => {}
                    }
                }
            }
            "content_block_delta" => {
                let delta = event.delta.unwrap_or_default();
                match delta.kind.as_deref() {
                    Some("text_delta") => {
                        if let Some(text) = delta.text.filter(|t| !t.is_empty()) {
                            on_event(ProviderEvent::TextDelta(text));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(entry) = event.index.and_then(|i| self.tools.get_mut(&i)) {
                            entry.2.push_str(&delta.partial_json.unwrap_or_default());
                        }
                    }
                    // Thinking is not part of the answer; it is kept only
                    // to be sent back with a tool call.
                    Some("thinking_delta") => {
                        if let Some(block) = event.index.and_then(|i| self.thinking.get_mut(&i)) {
                            append(block, "thinking", &delta.thinking.unwrap_or_default());
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(block) = event.index.and_then(|i| self.thinking.get_mut(&i)) {
                            append(block, "signature", &delta.signature.unwrap_or_default());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = event.index.and_then(|i| self.thinking.remove(&i)) {
                    on_event(ProviderEvent::Replay {
                        provider: self.provider,
                        block,
                    });
                }
                if let Some((id, name, raw)) = event.index.and_then(|i| self.tools.remove(&i)) {
                    let arguments = if raw.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&raw).map_err(|e| {
                            ProviderError::InvalidResponse(format!("tool input is not JSON: {e}"))
                        })?
                    };
                    on_event(ProviderEvent::ToolProposal {
                        id: (!id.is_empty()).then_some(id),
                        name,
                        arguments,
                        signature: None,
                    });
                }
            }
            "message_delta" => {
                if let Some(reason) = event.delta.and_then(|d| d.stop_reason) {
                    self.stop_reason = Some(reason);
                }
                if let Some(output) = event.usage.and_then(|u| u.output_tokens) {
                    self.usage.output_tokens = output;
                }
            }
            "message_stop" => {
                self.usage.total_tokens = self.usage.prompt_tokens + self.usage.output_tokens;
                on_event(ProviderEvent::Usage(self.usage));
                on_event(ProviderEvent::Finished(match self.stop_reason.as_deref() {
                    None | Some("end_turn" | "tool_use" | "stop_sequence" | "pause_turn") => {
                        FinishReason::Stop
                    }
                    Some("max_tokens") => FinishReason::MaxTokens,
                    Some("refusal") => FinishReason::Safety,
                    Some(other) => FinishReason::Other(other.to_string()),
                }));
            }
            "error" => {
                let error = event.error.unwrap_or_default();
                let message = error.message.unwrap_or_default().replace(key, "<redacted>");
                return Err(match error.kind.as_deref() {
                    Some("overloaded_error") => ProviderError::Unavailable {
                        status: 529,
                        message,
                    },
                    Some("rate_limit_error") => ProviderError::RateLimited { retry_after: None },
                    Some("authentication_error" | "permission_error") => {
                        ProviderError::Unauthenticated(message)
                    }
                    Some("invalid_request_error") => ProviderError::Rejected {
                        status: 400,
                        message,
                    },
                    _ => ProviderError::Unavailable {
                        status: 500,
                        message,
                    },
                });
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct AnthropicModelPage {
    #[serde(default)]
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
    max_input_tokens: Option<u32>,
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct DeepSeekModelList {
    #[serde(default)]
    data: Vec<DeepSeekModel>,
}

#[derive(Deserialize)]
struct DeepSeekModel {
    id: String,
}

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    index: Option<u64>,
    message: Option<StartMessage>,
    content_block: Option<ContentBlock>,
    delta: Option<Delta>,
    usage: Option<ApiUsage>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct StartMessage {
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    name: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    data: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    partial_json: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    stop_reason: Option<String>,
}

/// Appends a streamed fragment to a string field of a thinking block.
fn append(block: &mut Value, field: &str, fragment: &str) {
    let current = block[field].as_str().unwrap_or_default().to_string();
    block[field] = json!(current + fragment);
}

#[derive(Deserialize)]
struct ApiUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

#[derive(Deserialize, Default)]
struct ApiError {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolDeclaration;
    use std::time::Duration;

    fn request(messages: Vec<Message>) -> ProviderRequest {
        ProviderRequest {
            model: "claude-test".into(),
            system_instruction: "be brief".into(),
            messages,
            tools: vec![ToolDeclaration {
                name: "fetch_url".into(),
                description: "read".into(),
                parameters: json!({"type": "object"}),
            }],
            max_output_tokens: 2048,
            deadline: Duration::from_secs(5),
            thinking_level: Some(crate::ThinkingLevel::Medium),
        }
    }

    #[test]
    fn a_tool_round_trip_uses_tool_use_and_tool_result_blocks() {
        let body = request_body(
            &request(vec![
                Message::text(Role::Model, "a reply left over at the start"),
                Message::text(Role::User, "hi"),
                Message {
                    role: Role::Model,
                    parts: vec![Part::FunctionCall {
                        signature: None,
                        id: Some("toolu_1".into()),
                        name: "fetch_url".into(),
                        arguments: json!({"url": "https://x"}),
                    }],
                },
                Message {
                    role: Role::User,
                    parts: vec![Part::FunctionResponse {
                        id: Some("toolu_1".into()),
                        name: "fetch_url".into(),
                        response: json!({"ok": true}),
                    }],
                },
            ]),
            true,
            ProviderKind::Anthropic,
        )
        .unwrap();
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["output_config"], json!({"effort": "medium"}));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3, "the leading reply is dropped");
        assert_eq!(
            messages[0],
            json!({"role": "user", "content": [{"type": "text", "text": "hi"}]})
        );
        assert_eq!(
            messages[1]["content"][0],
            json!({"type": "tool_use", "id": "toolu_1", "name": "fetch_url", "input": {"url": "https://x"}})
        );
        assert_eq!(
            messages[2]["content"][0],
            json!({"type": "tool_result", "tool_use_id": "toolu_1", "content": "{\"ok\":true}"})
        );
        assert_eq!(body["tools"][0]["input_schema"], json!({"type": "object"}));
        assert!(
            request_body(&request(vec![]), false, ProviderKind::Anthropic)
                .unwrap()
                .get("output_config")
                .is_none()
        );
    }

    #[test]
    fn streamed_text_and_tool_input_are_assembled() {
        let mut state = StreamState::new(ProviderKind::Anthropic);
        let mut events = Vec::new();
        for data in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_9","name":"fetch_url","input":{}}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"url\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"https://a\"}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":15}}"#,
            r#"{"type":"message_stop"}"#,
        ] {
            state.handle(data, &mut |e| events.push(e), "KEY").unwrap();
        }
        assert_eq!(events[0], ProviderEvent::TextDelta("Hel".into()));
        assert_eq!(events[1], ProviderEvent::TextDelta("lo".into()));
        assert_eq!(
            events[2],
            ProviderEvent::ToolProposal {
                id: Some("toolu_9".into()),
                name: "fetch_url".into(),
                arguments: json!({"url": "https://a"}),
                signature: None,
            }
        );
        assert_eq!(
            events[3],
            ProviderEvent::Usage(Usage {
                prompt_tokens: 25,
                output_tokens: 15,
                total_tokens: 40
            })
        );
        assert_eq!(events[4], ProviderEvent::Finished(FinishReason::Stop));
    }

    #[test]
    fn signed_thinking_before_a_tool_call_is_kept_and_sent_back_unchanged() {
        // With thinking on, a tool call must be continued with the signed
        // thinking that preceded it, unchanged (Anthropic and DeepSeek).
        let mut state = StreamState::new(ProviderKind::DeepSeek);
        let mut events = Vec::new();
        for data in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Need the "}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"page."}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG-1"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"OPAQUE"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_5","name":"fetch_url","input":{}}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"url\":\"https://a\"}"}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
        ] {
            state.handle(data, &mut |e| events.push(e), "KEY").unwrap();
        }
        let thinking =
            json!({"type": "thinking", "thinking": "Need the page.", "signature": "SIG-1"});
        let redacted = json!({"type": "redacted_thinking", "data": "OPAQUE"});
        assert_eq!(
            events,
            vec![
                ProviderEvent::Replay {
                    provider: ProviderKind::DeepSeek,
                    block: thinking.clone()
                },
                ProviderEvent::Replay {
                    provider: ProviderKind::DeepSeek,
                    block: redacted.clone()
                },
                ProviderEvent::ToolProposal {
                    id: Some("toolu_5".into()),
                    name: "fetch_url".into(),
                    arguments: json!({"url": "https://a"}),
                    signature: None,
                },
            ],
            "no thinking text reaches the answer"
        );

        let turn = vec![
            Message::text(Role::User, "read it"),
            Message {
                role: Role::Model,
                parts: vec![
                    Part::Replay {
                        provider: ProviderKind::DeepSeek,
                        block: thinking.clone(),
                    },
                    Part::Replay {
                        provider: ProviderKind::DeepSeek,
                        block: redacted.clone(),
                    },
                    Part::FunctionCall {
                        id: Some("toolu_5".into()),
                        name: "fetch_url".into(),
                        arguments: json!({"url": "https://a"}),
                        signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                parts: vec![Part::FunctionResponse {
                    id: Some("toolu_5".into()),
                    name: "fetch_url".into(),
                    response: json!({"ok": true}),
                }],
            },
        ];
        let same = request_body(&request(turn.clone()), false, ProviderKind::DeepSeek).unwrap();
        assert_eq!(
            same["messages"][1]["content"][0], thinking,
            "first, unchanged"
        );
        assert_eq!(same["messages"][1]["content"][1], redacted);
        assert_eq!(same["messages"][1]["content"][2]["type"], "tool_use");
        let other = request_body(&request(turn), false, ProviderKind::Anthropic).unwrap();
        assert_eq!(
            other["messages"][1]["content"].as_array().unwrap().len(),
            1,
            "another provider's thinking is never sent"
        );
    }

    #[test]
    fn an_overloaded_event_is_transient_and_redacted() {
        let mut state = StreamState::new(ProviderKind::Anthropic);
        let err = state
            .handle(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded KEY"}}"#, &mut |_| {}, "KEY")
            .unwrap_err();
        assert!(err.is_transient());
        assert!(!err.to_string().contains("KEY"));
    }
}
