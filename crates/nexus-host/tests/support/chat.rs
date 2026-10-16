//! Chat helpers shared by the M2 test binaries.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_protocol::{
    ChatMessageView, ChatStatus, ConversationId, MessageId, ProviderChoice, SaveConnectionRequest,
    SendMessageRequest, SendMessageResponse,
};

pub fn gemini(key: &str) -> SaveConnectionRequest {
    SaveConnectionRequest {
        connection_id: None,
        display_name: Some("Google AI Studio".to_string()),
        // Test keys do not follow Google's format, so the provider is named.
        provider: Some(ProviderChoice::Gemini),
        free_tier: true,
        api_key: Some(key.to_string()),
    }
}

pub async fn send(
    client: &NexusClient,
    conversation: Option<ConversationId>,
    key: &str,
    text: &str,
) -> SendMessageResponse {
    client
        .send_message(SendMessageRequest {
            conversation_id: conversation,
            project_id: None,
            idempotency_key: key.to_string(),
            text: text.to_string(),
        })
        .await
        .unwrap()
}

pub async fn message(
    client: &NexusClient,
    conversation: ConversationId,
    id: MessageId,
) -> ChatMessageView {
    client
        .conversation(conversation)
        .await
        .unwrap()
        .messages
        .into_iter()
        .find(|m| m.message_id == id)
        .expect("the message belongs to this conversation")
}

/// Waits until the reply leaves Streaming.
pub async fn settled(client: &NexusClient, sent: &SendMessageResponse) -> ChatMessageView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let reply = message(client, sent.conversation_id, sent.reply_message_id).await;
        if reply.status != ChatStatus::Streaming {
            return reply;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the reply never settled"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(files_under(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}
