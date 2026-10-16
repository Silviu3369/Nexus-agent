//! Bridge commands for remote MCP servers. A key typed by the user goes
//! straight to the host, which keeps it in the Windows vault; it never comes
//! back to the webview. Every check (address, read-only, definition) is made
//! in the host.

use nexus_protocol::{McpServerId, McpServerView, SaveMcpServerRequest};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_mcp_servers(connection: State<'_>) -> Result<Vec<McpServerView>, BridgeError> {
    connection.call(async |c| c.mcp_servers().await).await
}

/// `server_id: None` adds a server; `key: None` keeps the stored key.
#[tauri::command]
pub async fn save_mcp_server(
    connection: State<'_>,
    server_id: Option<McpServerId>,
    name: String,
    url: String,
    key: Option<String>,
    clear_key: bool,
) -> Result<McpServerView, BridgeError> {
    connection
        .call(async |c| {
            c.save_mcp_server(SaveMcpServerRequest {
                server_id,
                name,
                url,
                key,
                clear_key,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn refresh_mcp_tools(
    connection: State<'_>,
    server_id: McpServerId,
) -> Result<McpServerView, BridgeError> {
    connection
        .call(async |c| c.refresh_mcp_tools(server_id).await)
        .await
}

#[tauri::command]
pub async fn remove_mcp_server(
    connection: State<'_>,
    server_id: McpServerId,
) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.remove_mcp_server(server_id).await)
        .await
}

/// `definition_sha256` names the definition the user looked at; `None`
/// turns the tool off.
#[tauri::command]
pub async fn set_mcp_tool_enabled(
    connection: State<'_>,
    server_id: McpServerId,
    tool: String,
    definition_sha256: Option<String>,
) -> Result<McpServerView, BridgeError> {
    connection
        .call(async |c| {
            c.set_mcp_tool_enabled(server_id, tool, definition_sha256)
                .await
        })
        .await
}
