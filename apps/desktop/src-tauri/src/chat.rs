//! Bridge commands for M2 chat. The API key passes through once, on its way
//! to the host's vault; nothing here stores, logs or returns it.

use nexus_protocol::{
    AgentStepView, ConnectionId, ConnectionView, ConversationId, ConversationSummary,
    ConversationView, MessageId, ModelView, PersonaView, SaveGeminiConnectionRequest,
    SavePersonaRequest, SendMessageRequest, SendMessageResponse, ThinkingLevel,
};

use crate::host::{BridgeError, HostConnection};

const DASHBOARD_CONVERSATIONS: u32 = 50;
/// Enough for the activity log; the host caps it at 200.
const DASHBOARD_ACTIVITY: u32 = 100;

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_connections(connection: State<'_>) -> Result<Vec<ConnectionView>, BridgeError> {
    connection.call(async |c| c.list_connections().await).await
}

/// `api_key: None` keeps the key already stored for `connection_id`.
#[tauri::command]
pub async fn save_gemini_connection(
    connection: State<'_>,
    connection_id: Option<ConnectionId>,
    display_name: String,
    free_tier: bool,
    api_key: Option<String>,
) -> Result<ConnectionView, BridgeError> {
    connection
        .call(async |c| {
            c.save_gemini_connection(SaveGeminiConnectionRequest {
                connection_id,
                display_name,
                free_tier,
                api_key,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn remove_connection(
    connection: State<'_>,
    connection_id: ConnectionId,
) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.remove_connection(connection_id).await)
        .await
}

#[tauri::command]
pub async fn list_models(
    connection: State<'_>,
    connection_id: ConnectionId,
) -> Result<Vec<ModelView>, BridgeError> {
    connection
        .call(async |c| c.list_models(connection_id).await)
        .await
}

#[tauri::command]
pub async fn select_model(
    connection: State<'_>,
    connection_id: ConnectionId,
    model: String,
) -> Result<ConnectionView, BridgeError> {
    connection
        .call(async |c| c.select_model(connection_id, model).await)
        .await
}

#[tauri::command]
pub async fn set_thinking_level(
    connection: State<'_>,
    connection_id: ConnectionId,
    level: ThinkingLevel,
) -> Result<ConnectionView, BridgeError> {
    connection
        .call(async |c| c.set_thinking_level(connection_id, level).await)
        .await
}

#[tauri::command]
pub async fn get_persona(connection: State<'_>) -> Result<PersonaView, BridgeError> {
    connection.call(async |c| c.persona().await).await
}

#[tauri::command]
pub async fn save_persona(
    connection: State<'_>,
    name: String,
    tone: String,
    style: String,
) -> Result<PersonaView, BridgeError> {
    connection
        .call(async |c| {
            c.save_persona(SavePersonaRequest { name, tone, style })
                .await
        })
        .await
}

#[tauri::command]
pub async fn list_conversations(
    connection: State<'_>,
) -> Result<Vec<ConversationSummary>, BridgeError> {
    connection
        .call(async |c| c.conversations(DASHBOARD_CONVERSATIONS).await)
        .await
}

#[tauri::command]
pub async fn get_conversation(
    connection: State<'_>,
    conversation_id: ConversationId,
) -> Result<ConversationView, BridgeError> {
    connection
        .call(async |c| c.conversation(conversation_id).await)
        .await
}

#[tauri::command]
pub async fn send_message(
    connection: State<'_>,
    conversation_id: Option<ConversationId>,
    idempotency_key: String,
    text: String,
) -> Result<SendMessageResponse, BridgeError> {
    connection
        .call(async |c| {
            c.send_message(SendMessageRequest {
                conversation_id,
                idempotency_key,
                text,
            })
            .await
        })
        .await
}

/// The agent's most recent steps, for the dashboard's live feed.
#[tauri::command]
pub async fn get_activity(connection: State<'_>) -> Result<Vec<AgentStepView>, BridgeError> {
    connection
        .call(async |c| c.activity(DASHBOARD_ACTIVITY).await)
        .await
}

#[tauri::command]
pub async fn cancel_reply(connection: State<'_>, message_id: MessageId) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.cancel_reply(message_id).await)
        .await
}
