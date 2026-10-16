//! Provider layer (`ARHITECTURA_NEXUS_RO.md` §9): normalises requests and
//! events across AI providers while keeping their real capability
//! differences. It knows nothing about the UI, the database or Nexus policy;
//! the host decides what may be sent and what a proposal may lead to.

pub mod anthropic;
mod client;
pub mod gemini;
mod http;
mod kind;
pub mod openai;
mod sse;
pub mod web;

pub use client::{Client, production_base_url};
pub use kind::{KeyDetection, ProviderKind, detect_provider};

use std::fmt;
use std::time::Duration;

use zeroize::Zeroizing;

/// An API key. Never printed, never part of a URL, wiped from memory on drop.
pub struct ApiKey(Zeroizing<String>);

impl ApiKey {
    pub fn new(value: String) -> Result<Self, ProviderError> {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.len() > 512
            || trimmed.chars().any(|c| c.is_control() || c == ' ')
        {
            return Err(ProviderError::InvalidConfiguration(
                "the API key is empty or malformed".to_string(),
            ));
        }
        Ok(Self(Zeroizing::new(trimmed.to_string())))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Identifier used in requests, e.g. `gemini-…-flash`.
    pub id: String,
    pub display_name: String,
    pub input_token_limit: Option<u32>,
    pub output_token_limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Model,
}

/// One piece of a turn. A plain text exchange is a single [`Part::Text`]; a
/// tool round trip adds the model's own [`Part::FunctionCall`] (echoed back
/// unchanged, so the model keeps its own context) followed by a
/// [`Part::FunctionResponse`] turn carrying what the host ran. `id` is the
/// provider's call id (OpenAI, Anthropic and newer Gemini models pair the
/// result with the call through it); `None` where the provider gave none.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    FunctionCall {
        id: Option<String>,
        name: String,
        arguments: serde_json::Value,
    },
    FunctionResponse {
        id: Option<String>,
        name: String,
        response: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text(text.into())],
        }
    }
}

/// A tool the host offers the model. The model can only *propose* calling
/// it; the host validates and routes any proposal through approval.
#[derive(Debug, Clone)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    /// JSON Schema (OpenAPI subset) of the arguments.
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub model: String,
    pub system_instruction: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDeclaration>,
    pub max_output_tokens: u32,
    /// Whole call, from sending the request to the last event.
    pub deadline: Duration,
    /// The user's speed/quality choice. Only applied to models that
    /// support it (Gemini 3.x, checked by id — `gemini.rs`); harmless to
    /// set for any other model, since it is then simply not sent.
    pub thinking_level: Option<ThinkingLevel>,
}

/// How much a model reasons before answering. Lower is faster; higher can
/// do better on genuinely hard questions. The user chooses this per
/// connection (`ConnectionView::thinking_level`). Each adapter maps it to
/// its provider's own control, and leaves it out for a model that refuses
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Low => "LOW",
            ThinkingLevel::Medium => "MEDIUM",
            ThinkingLevel::High => "HIGH",
        }
    }

    pub fn as_lower(self) -> &'static str {
        match self {
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    MaxTokens,
    Safety,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    TextDelta(String),
    /// The model asks for a tool call. Nothing has been executed.
    ToolProposal {
        id: Option<String>,
        name: String,
        arguments: serde_json::Value,
    },
    Usage(Usage),
    Finished(FinishReason),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("the provider rejected the API key or permissions: {0}")]
    Unauthenticated(String),
    #[error("rate limited by the provider")]
    RateLimited { retry_after: Option<Duration> },
    #[error("the provider refused the request ({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("the provider is unavailable ({status}): {message}")]
    Unavailable { status: u16, message: String },
    #[error("the provider blocked the content: {0}")]
    Blocked(String),
    #[error("invalid response from the provider: {0}")]
    InvalidResponse(String),
    #[error("transport error: {0}")]
    Transport(String),
    /// The deadline passed. This does not prove the provider did not
    /// process (or bill) the request (`CONTRACTE_NEXUS_RO.md` §11).
    #[error("deadline exceeded")]
    Timeout,
}

impl ProviderError {
    /// Worth sending the same request again: the provider was down or
    /// overloaded (5xx, 529), or the connection broke. Never a refusal, a
    /// wrong key, a used-up quota or a deadline. The caller still decides,
    /// and only retries a call that produced nothing yet.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            ProviderError::Unavailable { .. } | ProviderError::Transport(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_never_shows_in_debug_output() {
        let key = ApiKey::new("SENTINEL-KEY-123".to_string()).unwrap();
        assert!(!format!("{key:?}").contains("SENTINEL"));
    }

    #[test]
    fn only_outages_and_broken_connections_are_transient() {
        assert!(
            ProviderError::Unavailable {
                status: 503,
                message: String::new()
            }
            .is_transient()
        );
        assert!(
            ProviderError::Unavailable {
                status: 529,
                message: String::new()
            }
            .is_transient()
        );
        assert!(ProviderError::Transport("reset".into()).is_transient());
        assert!(!ProviderError::RateLimited { retry_after: None }.is_transient());
        assert!(!ProviderError::Unauthenticated(String::new()).is_transient());
        assert!(
            !ProviderError::Rejected {
                status: 400,
                message: String::new()
            }
            .is_transient()
        );
        assert!(!ProviderError::Timeout.is_transient());
        assert!(!ProviderError::Blocked(String::new()).is_transient());
    }

    #[test]
    fn malformed_api_keys_are_refused() {
        assert!(ApiKey::new("   ".to_string()).is_err());
        assert!(ApiKey::new("has space".to_string()).is_err());
        assert!(ApiKey::new("line\nbreak".to_string()).is_err());
    }
}
