//! Bridge commands for usage: today's tokens per model, the daily budget
//! and the prices the user enters. Every check is made in the host.

use nexus_protocol::{ModelPrice, UsageView};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn get_usage(connection: State<'_>) -> Result<UsageView, BridgeError> {
    connection.call(async |c| c.usage().await).await
}

/// `tokens: None` removes the limit.
#[tauri::command]
pub async fn set_daily_token_budget(
    connection: State<'_>,
    tokens: Option<u64>,
) -> Result<UsageView, BridgeError> {
    connection
        .call(async |c| c.set_daily_token_budget(tokens).await)
        .await
}

/// `price: None` forgets the model's price.
#[tauri::command]
pub async fn set_model_price(
    connection: State<'_>,
    provider: String,
    model: String,
    price: Option<ModelPrice>,
) -> Result<UsageView, BridgeError> {
    connection
        .call(async |c| c.set_model_price(provider, model, price).await)
        .await
}
