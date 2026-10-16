//! OpenAI adapter on the Responses API (`POST /v1/responses`), checked
//! against developers.openai.com on 2026-10-16: the newest models (GPT-6
//! Astra, GPT-6.1 Sol) call tools only through this API, not Chat
//! Completions. The key travels only in the `Authorization` header,
//! redirects are refused, and `store: false` keeps OpenAI from retaining
//! the conversation for later retrieval.

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http::{
    self, LIST_BUDGET, LIST_TIMEOUT, MAX_BODY_BYTES, MAX_ERROR_BYTES, read_limited, transport,
};
use crate::{
    ApiKey, FinishReason, Message, ModelInfo, Part, ProviderError, ProviderEvent, ProviderRequest,
    Role, Usage,
};

pub const PRODUCTION_BASE_URL: &str = "https://api.openai.com";

/// Families in OpenAI's model list that cannot answer a text request
/// (embeddings, speech, transcription, images, moderation). The list itself
/// says nothing about capabilities, so these are left out by name; anything
/// else is offered and the user chooses.
const NOT_FOR_TEXT: [&str; 9] = [
    "embedding",
    "tts",
    "whisper",
    "transcribe",
    "dall-e",
    "image",
    "moderation",
    "audio",
    "realtime",
];

pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    key: ApiKey,
}

impl OpenAiClient {
    /// `base_url` must be [`PRODUCTION_BASE_URL`] or a loopback test server.
    pub fn new(key: ApiKey, base_url: &str) -> Result<Self, ProviderError> {
        Ok(Self {
            http: http::client()?,
            base_url: http::allowed_base_url(base_url, PRODUCTION_BASE_URL)?,
            key,
        })
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.key.expose())
    }

    /// Models this key can use, newest first, without the families that
    /// cannot answer text.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let call = async {
            let response = self
                .http
                .get(format!("{}/v1/models", self.base_url))
                .header(AUTHORIZATION, self.bearer())
                .timeout(LIST_TIMEOUT)
                .send()
                .await
                .map_err(transport)?;
            if !response.status().is_success() {
                return Err(self.error_from(response).await);
            }
            let body = read_limited(response, MAX_BODY_BYTES).await?;
            let list: ModelList = serde_json::from_slice(&body)
                .map_err(|e| ProviderError::InvalidResponse(format!("model list: {e}")))?;
            let mut models: Vec<ApiModel> = list
                .data
                .into_iter()
                .filter(|m| !NOT_FOR_TEXT.iter().any(|f| m.id.contains(f)))
                .collect();
            models.sort_by(|a, b| b.created.cmp(&a.created).then(a.id.cmp(&b.id)));
            Ok(models
                .into_iter()
                .map(|m| ModelInfo {
                    display_name: m.id.clone(),
                    id: m.id,
                    input_token_limit: None,
                    output_token_limit: None,
                })
                .collect())
        };
        tokio::time::timeout(LIST_BUDGET, call)
            .await
            .map_err(|_| ProviderError::Timeout)?
    }

    /// Streams one response. A model that refuses the reasoning setting
    /// (non-reasoning models answer 400 for `reasoning`) is asked once more
    /// without it, before anything was streamed.
    pub async fn stream(
        &self,
        request: &ProviderRequest,
        mut on_event: impl FnMut(ProviderEvent) + Send,
    ) -> Result<(), ProviderError> {
        let call = async {
            let with_reasoning = request.thinking_level.is_some();
            match self
                .stream_once(request, with_reasoning, &mut on_event)
                .await
            {
                Err(ProviderError::Rejected {
                    status: 400,
                    message,
                }) if with_reasoning && message.to_ascii_lowercase().contains("reasoning") => {
                    tracing::info!(model = %request.model, "model refused the reasoning setting; asking without it");
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
        with_reasoning: bool,
        on_event: &mut (impl FnMut(ProviderEvent) + Send),
    ) -> Result<(), ProviderError> {
        let body = request_body(request, with_reasoning)?;
        let response = self
            .http
            .post(format!("{}/v1/responses", self.base_url))
            .header(AUTHORIZATION, self.bearer())
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(transport)?;
        if !response.status().is_success() {
            return Err(self.error_from(response).await);
        }
        let key = self.key.expose().to_string();
        http::read_sse(response, |data| handle_event(data, on_event, &key)).await
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
        // Defence in depth: a provider message must never carry our key out.
        let message = message.replace(self.key.expose(), "<redacted>");
        http::status_error(status, message, retry_after)
    }
}

fn request_body(request: &ProviderRequest, with_reasoning: bool) -> Result<Value, ProviderError> {
    let model = http::checked_body_model_id(&request.model)?;
    let mut input = Vec::new();
    let mut generated = 0usize;
    // A call and its result must carry the same id; when the provider gave
    // none (it always does here), pair them by order.
    let mut pending: Vec<String> = Vec::new();
    for Message { role, parts } in &request.messages {
        for part in parts {
            match part {
                Part::Text(text) => input.push(json!({
                    "role": match role { Role::User => "user", Role::Model => "assistant" },
                    "content": text,
                })),
                Part::FunctionCall {
                    id,
                    name,
                    arguments,
                } => {
                    let call_id = id.clone().unwrap_or_else(|| {
                        generated += 1;
                        format!("call_nexus_{generated}")
                    });
                    pending.push(call_id.clone());
                    // No `id`: an item id would require the reasoning item
                    // that produced it, which `store: false` does not keep.
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments.to_string(),
                    }));
                }
                Part::FunctionResponse { id, response, .. } => {
                    let call_id = id
                        .clone()
                        .or_else(|| (!pending.is_empty()).then(|| pending.remove(0)));
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id.unwrap_or_else(|| "call_nexus_0".to_string()),
                        "output": response.to_string(),
                    }));
                }
            }
        }
    }
    let mut body = json!({
        "model": model,
        "input": input,
        "stream": true,
        "store": false,
        "max_output_tokens": request.max_output_tokens,
    });
    if !request.system_instruction.is_empty() {
        body["instructions"] = json!(request.system_instruction);
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect(),
        );
    }
    if with_reasoning && let Some(level) = request.thinking_level {
        body["reasoning"] = json!({ "effort": level.as_lower() });
    }
    Ok(body)
}

fn handle_event(
    data: &str,
    on_event: &mut impl FnMut(ProviderEvent),
    key: &str,
) -> Result<(), ProviderError> {
    let event: StreamEvent = serde_json::from_str(data)
        .map_err(|e| ProviderError::InvalidResponse(format!("stream event: {e}")))?;
    match event.kind.as_str() {
        "response.output_text.delta" | "response.refusal.delta" => {
            if let Some(delta) = event.delta.filter(|d| !d.is_empty()) {
                on_event(ProviderEvent::TextDelta(delta));
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.item
                && item.kind == "function_call"
            {
                let raw = item.arguments.unwrap_or_default();
                let arguments = if raw.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&raw).map_err(|e| {
                        ProviderError::InvalidResponse(format!("tool arguments are not JSON: {e}"))
                    })?
                };
                on_event(ProviderEvent::ToolProposal {
                    id: item.call_id,
                    name: item.name.unwrap_or_default(),
                    arguments,
                });
            }
        }
        "response.completed" | "response.incomplete" => {
            let response = event.response.unwrap_or_default();
            if let Some(u) = response.usage {
                on_event(ProviderEvent::Usage(Usage {
                    prompt_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    total_tokens: u.total_tokens,
                }));
            }
            let reason = response.incomplete_details.and_then(|d| d.reason);
            on_event(ProviderEvent::Finished(match reason.as_deref() {
                None => FinishReason::Stop,
                Some("max_output_tokens") => FinishReason::MaxTokens,
                Some("content_filter") => FinishReason::Safety,
                Some(other) => FinishReason::Other(other.to_string()),
            }));
        }
        "response.failed" => {
            let error = event.response.and_then(|r| r.error).unwrap_or_default();
            return Err(stream_error(error.code, error.message, key));
        }
        "error" => return Err(stream_error(event.code, event.message, key)),
        _ => {}
    }
    Ok(())
}

/// An error reported inside the stream, after the HTTP status was 200.
fn stream_error(code: Option<String>, message: Option<String>, key: &str) -> ProviderError {
    let code = code.unwrap_or_default();
    let message = message
        .unwrap_or_else(|| code.clone())
        .replace(key, "<redacted>");
    if code.contains("rate_limit") || code.contains("quota") {
        ProviderError::RateLimited { retry_after: None }
    } else if code.contains("invalid") || code.contains("content") {
        ProviderError::Rejected {
            status: 400,
            message,
        }
    } else {
        ProviderError::Unavailable {
            status: 500,
            message,
        }
    }
}

#[derive(Deserialize)]
struct ModelList {
    #[serde(default)]
    data: Vec<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
    #[serde(default)]
    created: i64,
}

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
    item: Option<OutputItem>,
    response: Option<ResponseBody>,
    code: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    kind: String,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize, Default)]
struct ResponseBody {
    usage: Option<ApiUsage>,
    incomplete_details: Option<IncompleteDetails>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

#[derive(Deserialize)]
struct IncompleteDetails {
    reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ApiError {
    code: Option<String>,
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
            model: "gpt-test".into(),
            system_instruction: "be brief".into(),
            messages,
            tools: vec![ToolDeclaration {
                name: "fetch_url".into(),
                description: "read".into(),
                parameters: json!({"type": "object"}),
            }],
            max_output_tokens: 100,
            deadline: Duration::from_secs(5),
            thinking_level: Some(crate::ThinkingLevel::Low),
        }
    }

    #[test]
    fn a_tool_round_trip_pairs_call_and_output_by_call_id() {
        let body = request_body(
            &request(vec![
                Message::text(Role::User, "hi"),
                Message {
                    role: Role::Model,
                    parts: vec![
                        Part::Text("looking".into()),
                        Part::FunctionCall {
                            id: Some("call_1".into()),
                            name: "fetch_url".into(),
                            arguments: json!({"url": "https://x"}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    parts: vec![Part::FunctionResponse {
                        id: Some("call_1".into()),
                        name: "fetch_url".into(),
                        response: json!({"ok": true}),
                    }],
                },
            ]),
            true,
        )
        .unwrap();
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["instructions"], json!("be brief"));
        assert_eq!(body["reasoning"], json!({"effort": "low"}));
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0], json!({"role": "user", "content": "hi"}));
        assert_eq!(input[1], json!({"role": "assistant", "content": "looking"}));
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["arguments"], json!("{\"url\":\"https://x\"}"));
        assert!(input[2].get("id").is_none());
        assert_eq!(
            input[3],
            json!({"type": "function_call_output", "call_id": "call_1", "output": "{\"ok\":true}"})
        );
        assert_eq!(body["tools"][0]["type"], "function");
        assert!(
            request_body(&request(vec![]), false)
                .unwrap()
                .get("reasoning")
                .is_none()
        );
    }

    #[test]
    fn stream_events_become_provider_events() {
        let mut events = Vec::new();
        let mut push = |e| events.push(e);
        for data in [
            r#"{"type":"response.created","response":{}}"#,
            r#"{"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":"Hel"}"#,
            r#"{"type":"response.output_text.delta","delta":"lo"}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_9","name":"fetch_url","arguments":"{\"url\":\"https://a\"}","status":"completed"}}"#,
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":7,"output_tokens":3,"total_tokens":10}}}"#,
        ] {
            handle_event(data, &mut push, "KEY").unwrap();
        }
        assert_eq!(events[0], ProviderEvent::TextDelta("Hel".into()));
        assert_eq!(events[1], ProviderEvent::TextDelta("lo".into()));
        assert_eq!(
            events[2],
            ProviderEvent::ToolProposal {
                id: Some("call_9".into()),
                name: "fetch_url".into(),
                arguments: json!({"url": "https://a"})
            }
        );
        assert_eq!(
            events[3],
            ProviderEvent::Usage(Usage {
                prompt_tokens: 7,
                output_tokens: 3,
                total_tokens: 10
            })
        );
        assert_eq!(events[4], ProviderEvent::Finished(FinishReason::Stop));
    }

    #[test]
    fn stream_failures_are_classified_and_redacted() {
        let mut sink = |_| {};
        let failed = handle_event(
            r#"{"type":"response.failed","response":{"error":{"code":"server_error","message":"boom KEY"}}}"#,
            &mut sink,
            "KEY",
        )
        .unwrap_err();
        assert!(failed.is_transient());
        assert!(!failed.to_string().contains("KEY"));
        let limited = handle_event(
            r#"{"type":"error","code":"rate_limit_exceeded","message":"slow"}"#,
            &mut sink,
            "KEY",
        )
        .unwrap_err();
        assert!(matches!(limited, ProviderError::RateLimited { .. }));
        let mut ends = Vec::new();
        handle_event(
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
            &mut |e| ends.push(e),
            "KEY",
        )
        .unwrap();
        assert_eq!(ends, vec![ProviderEvent::Finished(FinishReason::MaxTokens)]);
    }
}
