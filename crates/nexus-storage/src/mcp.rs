//! Remote MCP servers and their tools (`schema::V7`). A tool is offered to
//! the model only while the user has enabled exactly the definition the
//! server last listed: if the server changes a tool's description, schema
//! or hints, it is off until the user looks at it again. Keys live in the
//! Windows vault; a row only names its vault entry. Events never carry
//! descriptions, schemas or arguments.

use nexus_protocol::McpServerId;
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};

use crate::repo::{insert_event, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRow {
    pub id: McpServerId,
    pub name: String,
    pub url: String,
    /// The vault entry holding its key; `None` when it needs none.
    pub secret_name: Option<String>,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub checked_at_ms: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpToolRow {
    pub server_id: McpServerId,
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    pub definition_sha256: String,
    /// The definition the user enabled, if any.
    pub enabled_sha256: Option<String>,
    /// Listed by the server the last time it was asked.
    pub available: bool,
    pub position: i64,
}

impl McpToolRow {
    /// Offered to the model: listed now, and enabled in exactly this form.
    pub fn enabled(&self) -> bool {
        self.available && self.enabled_sha256.as_deref() == Some(self.definition_sha256.as_str())
    }

    /// Enabled once, but the server has changed it since.
    pub fn changed(&self) -> bool {
        self.enabled_sha256
            .as_deref()
            .is_some_and(|e| e != self.definition_sha256)
    }
}

#[derive(Debug, Clone)]
pub struct NewMcpTool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    pub definition_sha256: String,
}

const SERVER_COLUMNS: &str =
    "id, name, url, secret_name, revision, created_at_ms, updated_at_ms, checked_at_ms, last_error";
const TOOL_COLUMNS: &str =
    "server_id, name, title, description, input_schema_json, annotations_json,
    definition_sha256, enabled_sha256, available, position";

impl Store {
    pub async fn mcp_servers(&self) -> Result<Vec<McpServerRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let ids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT id FROM mcp_servers ORDER BY created_at_ms, id")?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            ids.iter()
                .map(|id| load_server(&tx, McpServerId(parse_uuid(id)?)))
                .collect()
        })
        .await
    }

    pub async fn mcp_server(&self, id: McpServerId) -> Result<McpServerRow, StorageError> {
        self.call(move |conn| load_server(&conn.transaction()?, id))
            .await
    }

    pub async fn create_mcp_server(
        &self,
        name: String,
        url: String,
        secret_name: Option<String>,
        now: i64,
    ) -> Result<McpServerRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let id = McpServerId::new();
            tx.execute(
                "INSERT INTO mcp_servers (id, name, url, secret_name, revision, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                params![id.to_string(), name, url, secret_name, now],
            )?;
            insert_event(&tx, "McpServerAdded", &id.to_string(), 1, json!({}), now)?;
            let row = load_server(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Renames or repoints a server. A new address is another server: every
    /// tool of the old one is turned off.
    pub async fn update_mcp_server(
        &self,
        id: McpServerId,
        name: String,
        url: String,
        secret_name: Option<String>,
        now: i64,
    ) -> Result<McpServerRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_server(&tx, id)?;
            if current.url != url {
                tx.execute("DELETE FROM mcp_tools WHERE server_id = ?1", [id.to_string()])?;
                tx.execute(
                    "UPDATE mcp_servers SET checked_at_ms = NULL, last_error = NULL WHERE id = ?1",
                    [id.to_string()],
                )?;
            }
            tx.execute(
                "UPDATE mcp_servers SET name = ?1, url = ?2, secret_name = ?3, revision = revision + 1,
                 updated_at_ms = ?4 WHERE id = ?5",
                params![name, url, secret_name, now, id.to_string()],
            )?;
            insert_event(&tx, "McpServerUpdated", &id.to_string(), current.revision + 1, json!({}), now)?;
            let row = load_server(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Removes a server with its tools; returns its vault entry to delete.
    pub async fn delete_mcp_server(
        &self,
        id: McpServerId,
        now: i64,
    ) -> Result<Option<String>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_server(&tx, id)?;
            tx.execute("DELETE FROM mcp_servers WHERE id = ?1", [id.to_string()])?;
            insert_event(
                &tx,
                "McpServerRemoved",
                &id.to_string(),
                current.revision + 1,
                json!({}),
                now,
            )?;
            tx.commit()?;
            Ok(current.secret_name)
        })
        .await
    }

    /// Records what the server listed: new tools arrive off, a changed
    /// definition is no longer enabled, tools it no longer lists are kept
    /// but unavailable.
    pub async fn record_mcp_listing(
        &self,
        id: McpServerId,
        tools: Vec<NewMcpTool>,
        now: i64,
    ) -> Result<Vec<McpToolRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_server(&tx, id)?;
            tx.execute("UPDATE mcp_tools SET available = 0 WHERE server_id = ?1", [id.to_string()])?;
            let mut changed = 0usize;
            for (position, tool) in tools.iter().enumerate() {
                let previous: Option<String> = tx
                    .query_row(
                        "SELECT definition_sha256 FROM mcp_tools WHERE server_id = ?1 AND name = ?2",
                        params![id.to_string(), tool.name],
                        |r| r.get(0),
                    )
                    .optional()?;
                if previous.as_deref().is_some_and(|p| p != tool.definition_sha256) {
                    changed += 1;
                }
                tx.execute(
                    "INSERT INTO mcp_tools (server_id, name, title, description, input_schema_json, annotations_json,
                       definition_sha256, enabled_sha256, available, position, first_seen_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8, ?9, ?9)
                     ON CONFLICT (server_id, name) DO UPDATE SET title = excluded.title,
                       description = excluded.description, input_schema_json = excluded.input_schema_json,
                       annotations_json = excluded.annotations_json, definition_sha256 = excluded.definition_sha256,
                       available = 1, position = excluded.position, updated_at_ms = excluded.updated_at_ms",
                    params![
                        id.to_string(),
                        tool.name,
                        tool.title,
                        tool.description,
                        tool.input_schema.to_string(),
                        tool.annotations.to_string(),
                        tool.definition_sha256,
                        position as i64,
                        now
                    ],
                )?;
            }
            tx.execute(
                "UPDATE mcp_servers SET checked_at_ms = ?1, last_error = NULL WHERE id = ?2",
                params![now, id.to_string()],
            )?;
            insert_event(&tx, "McpToolsListed", &id.to_string(), 0, json!({"tools": tools.len(), "changed": changed}), now)?;
            let rows = load_tools(&tx, id)?;
            tx.commit()?;
            Ok(rows)
        })
        .await
    }

    /// The server could not be asked; its tools stay as last recorded.
    pub async fn record_mcp_failure(
        &self,
        id: McpServerId,
        error: String,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_server(&tx, id)?;
            let error: String = error.chars().take(500).collect();
            tx.execute(
                "UPDATE mcp_servers SET checked_at_ms = ?1, last_error = ?2 WHERE id = ?3",
                params![now, error, id.to_string()],
            )?;
            insert_event(
                &tx,
                "McpServerCheckFailed",
                &id.to_string(),
                0,
                json!({}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn mcp_tools(&self, id: McpServerId) -> Result<Vec<McpToolRow>, StorageError> {
        self.call(move |conn| load_tools(&conn.transaction()?, id))
            .await
    }

    /// Every tool the model may be offered now, with its server.
    pub async fn enabled_mcp_tools(&self) -> Result<Vec<(McpServerRow, McpToolRow)>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let ids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT id FROM mcp_servers ORDER BY created_at_ms, id")?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            let mut out = Vec::new();
            for id in ids {
                let id = McpServerId(parse_uuid(&id)?);
                let server = load_server(&tx, id)?;
                for tool in load_tools(&tx, id)?.into_iter().filter(McpToolRow::enabled) {
                    out.push((server.clone(), tool));
                }
            }
            Ok(out)
        })
        .await
    }

    /// Enables exactly the definition the user saw (`Some(sha)`), or turns
    /// the tool off (`None`). A definition that changed meanwhile is
    /// refused rather than enabled unseen.
    pub async fn set_mcp_tool_enabled(
        &self,
        id: McpServerId,
        name: String,
        definition_sha256: Option<String>,
        now: i64,
    ) -> Result<McpToolRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_tools(&tx, id)?
                .into_iter()
                .find(|t| t.name == name)
                .ok_or(StorageError::NotFound("MCP tool"))?;
            if let Some(sha) = &definition_sha256
                && (sha != &current.definition_sha256 || !current.available)
            {
                return Err(StorageError::RevisionConflict("MCP tool definition"));
            }
            tx.execute(
                "UPDATE mcp_tools SET enabled_sha256 = ?1, updated_at_ms = ?2 WHERE server_id = ?3 AND name = ?4",
                params![definition_sha256, now, id.to_string(), name],
            )?;
            let kind = if definition_sha256.is_some() { "McpToolEnabled" } else { "McpToolDisabled" };
            insert_event(&tx, kind, &id.to_string(), 0, json!({ "tool": name }), now)?;
            let row = load_tools(&tx, id)?
                .into_iter()
                .find(|t| t.name == name)
                .ok_or(StorageError::NotFound("MCP tool"))?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }
}

type RawServer = (
    String,
    String,
    String,
    Option<String>,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<String>,
);

fn load_server(tx: &Transaction, id: McpServerId) -> Result<McpServerRow, StorageError> {
    let raw: Option<RawServer> = tx
        .query_row(
            &format!("SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE id = ?1"),
            [id.to_string()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .optional()?;
    let (
        raw_id,
        name,
        url,
        secret_name,
        revision,
        created_at_ms,
        updated_at_ms,
        checked_at_ms,
        last_error,
    ) = raw.ok_or(StorageError::NotFound("MCP server"))?;
    Ok(McpServerRow {
        id: McpServerId(parse_uuid(&raw_id)?),
        name,
        url,
        secret_name,
        revision,
        created_at_ms,
        updated_at_ms,
        checked_at_ms,
        last_error,
    })
}

type RawTool = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    i64,
);

fn load_tools(tx: &Transaction, id: McpServerId) -> Result<Vec<McpToolRow>, StorageError> {
    let mut stmt = tx.prepare(&format!(
        "SELECT {TOOL_COLUMNS} FROM mcp_tools WHERE server_id = ?1 ORDER BY available DESC, position, name"
    ))?;
    let raws: Vec<RawTool> = stmt
        .query_map([id.to_string()], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    raws.into_iter()
        .map(
            |(
                server,
                name,
                title,
                description,
                schema,
                annotations,
                definition,
                enabled,
                available,
                position,
            )| {
                Ok(McpToolRow {
                    server_id: McpServerId(parse_uuid(&server)?),
                    name,
                    title,
                    description,
                    input_schema: serde_json::from_str(&schema)
                        .map_err(|e| StorageError::Corrupt(e.to_string()))?,
                    annotations: serde_json::from_str(&annotations)
                        .map_err(|e| StorageError::Corrupt(e.to_string()))?,
                    definition_sha256: definition,
                    enabled_sha256: enabled,
                    available: available == 1,
                    position,
                })
            },
        )
        .collect()
}
