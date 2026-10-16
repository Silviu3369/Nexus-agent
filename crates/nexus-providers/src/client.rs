//! One client over every adapter, so the host never branches on the
//! provider: list the key's models, stream a reply.

use crate::anthropic::{self, AnthropicClient, Flavor};
use crate::gemini::{self, GeminiClient};
use crate::openai::{self, OpenAiClient};
use crate::{ApiKey, ModelInfo, ProviderError, ProviderEvent, ProviderKind, ProviderRequest};

pub enum Client {
    Gemini(GeminiClient),
    OpenAi(OpenAiClient),
    /// Anthropic itself, or DeepSeek through its Anthropic-compatible API.
    Anthropic(AnthropicClient),
}

/// The only origin each provider is reached at in a release build.
pub fn production_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Gemini => gemini::PRODUCTION_BASE_URL,
        ProviderKind::OpenAi => openai::PRODUCTION_BASE_URL,
        ProviderKind::Anthropic => anthropic::ANTHROPIC_BASE_URL,
        ProviderKind::DeepSeek => anthropic::DEEPSEEK_BASE_URL,
    }
}

impl Client {
    /// `base_url` is the provider's production origin, or a loopback test
    /// server; each adapter refuses anything else.
    pub fn new(kind: ProviderKind, key: ApiKey, base_url: &str) -> Result<Self, ProviderError> {
        Ok(match kind {
            ProviderKind::Gemini => Client::Gemini(GeminiClient::new(key, base_url)?),
            ProviderKind::OpenAi => Client::OpenAi(OpenAiClient::new(key, base_url)?),
            ProviderKind::Anthropic => {
                Client::Anthropic(AnthropicClient::new(key, base_url, Flavor::Anthropic)?)
            }
            ProviderKind::DeepSeek => {
                Client::Anthropic(AnthropicClient::new(key, base_url, Flavor::DeepSeek)?)
            }
        })
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        match self {
            Client::Gemini(c) => c.list_models().await,
            Client::OpenAi(c) => c.list_models().await,
            Client::Anthropic(c) => c.list_models().await,
        }
    }

    pub async fn stream(
        &self,
        request: &ProviderRequest,
        on_event: impl FnMut(ProviderEvent) + Send,
    ) -> Result<(), ProviderError> {
        match self {
            Client::Gemini(c) => c.stream(request, on_event).await,
            Client::OpenAi(c) => c.stream(request, on_event).await,
            Client::Anthropic(c) => c.stream(request, on_event).await,
        }
    }
}
