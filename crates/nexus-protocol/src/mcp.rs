//! DTOs for remote MCP servers (`PLAN_NEXUS_RO.md`, step 3). Everything a
//! server says about its tools is shown to the user as the server's claim;
//! the user enables tools one by one, and only tools the server declares
//! read-only can be enabled until tools with effects get approvals.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::McpServerId;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpServerView {
    pub server_id: McpServerId,
    pub name: String,
    pub url: String,
    /// A key is held for it in the Windows vault (never sent to the UI).
    pub has_key: bool,
    pub checked_at_ms: Option<i64>,
    /// Why the last attempt to list its tools failed, if it did.
    pub last_error: Option<String>,
    pub tools: Vec<McpToolView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpToolView {
    pub name: String,
    pub title: Option<String>,
    /// As the server wrote it: data, not a promise.
    pub description: String,
    /// Parameter names, for the list.
    pub parameters: Vec<String>,
    /// The full parameter schema, pretty-printed, for inspection.
    pub schema_json: String,
    /// The server's own hints; untrusted.
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    /// The definition shown; enabling names it, so a definition that
    /// changed in the meantime is not enabled unseen.
    pub definition_sha256: String,
    pub enabled: bool,
    /// Enabled once, but the server has changed it since: off until enabled again.
    pub changed: bool,
    /// Listed by the server the last time it was asked.
    pub available: bool,
    /// Why it cannot be enabled, if it cannot.
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveMcpServerRequest {
    /// `None` adds a server.
    pub server_id: Option<McpServerId>,
    pub name: String,
    /// The server's MCP endpoint, `https://` only.
    pub url: String,
    /// A new key for its `Authorization` header; `None` keeps the stored one.
    pub key: Option<String>,
    /// Forget the stored key.
    pub clear_key: bool,
}
