//! What data may leave for which AI provider (ADR-PROVIDER-002,
//! `schema::V22`): each source's class (a shared folder, a register, an MCP
//! server: the user's own data unless the user marks it as someone else's),
//! what the user says about each connection (a signed data processing
//! addendum, consent to the provider using their own data) and where they
//! use Nexus from. The Host decides from these; this only keeps them.

use std::collections::HashMap;

use nexus_core::DataClass;
use nexus_protocol::{ConnectionId, FolderId, McpServerId, RegisterId};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{StorageError, Store};

/// The setting that says the user uses Nexus from the EEA, Switzerland or
/// the UK (where Google applies its paid data terms to the free quota).
const IN_EEA: &str = "data.in_eea";

/// A source of data the agent may read and send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Folder(FolderId),
    Register(RegisterId),
    McpServer(McpServerId),
}

impl Source {
    fn table(self) -> (&'static str, String) {
        match self {
            Source::Folder(id) => ("folders", id.to_string()),
            Source::Register(id) => ("registers", id.to_string()),
            Source::McpServer(id) => ("mcp_servers", id.to_string()),
        }
    }
}

/// What the user says about a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConnectionData {
    pub dpa_signed: bool,
    pub allows_training: bool,
}

/// Every source's class, and what the user says about connections.
#[derive(Debug, Clone, Default)]
pub struct DataSettings {
    pub classes: HashMap<Source, DataClass>,
    pub connections: HashMap<ConnectionId, ConnectionData>,
    pub in_eea: bool,
}

impl DataSettings {
    /// A source's class; one not known is the user's own (the default).
    pub fn class_of(&self, source: Source) -> DataClass {
        self.classes
            .get(&source)
            .copied()
            .unwrap_or(DataClass::Personal)
    }

    pub fn connection(&self, id: ConnectionId) -> ConnectionData {
        self.connections.get(&id).copied().unwrap_or_default()
    }
}

impl Store {
    pub async fn data_settings(&self) -> Result<DataSettings, StorageError> {
        self.call(|conn| {
            let mut settings = DataSettings::default();
            for (table, make) in [
                (
                    "folders",
                    (|u| Source::Folder(FolderId(u))) as fn(uuid::Uuid) -> Source,
                ),
                ("registers", |u| Source::Register(RegisterId(u))),
                ("mcp_servers", |u| Source::McpServer(McpServerId(u))),
            ] {
                let mut stmt = conn.prepare(&format!("SELECT id, data_class FROM {table}"))?;
                let rows: Vec<(String, String)> = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?;
                for (id, class) in rows {
                    settings
                        .classes
                        .insert(make(parse_uuid(&id)?), parse_state(&class)?);
                }
            }
            let mut stmt =
                conn.prepare("SELECT id, dpa_signed, allows_training FROM provider_connections")?;
            let rows: Vec<(String, bool, bool)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?;
            for (id, dpa_signed, allows_training) in rows {
                settings.connections.insert(
                    ConnectionId(parse_uuid(&id)?),
                    ConnectionData {
                        dpa_signed,
                        allows_training,
                    },
                );
            }
            settings.in_eea = conn
                .query_row(
                    "SELECT json_extract(value_json, '$') FROM settings WHERE key = ?1",
                    [IN_EEA],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                == Some(1);
            Ok(settings)
        })
        .await
    }

    /// Marks a source as the user's own data or someone else's.
    pub async fn set_source_class(
        &self,
        source: Source,
        class: DataClass,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let (table, id) = source.table();
            let changed = tx.execute(
                &format!("UPDATE {table} SET data_class = ?1 WHERE id = ?2"),
                params![class.as_str(), id],
            )?;
            if changed == 0 {
                return Err(StorageError::NotFound("source"));
            }
            insert_event(
                &tx,
                "DataClassChanged",
                &id,
                0,
                json!({"table": table, "class": class.as_str()}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn set_connection_data(
        &self,
        id: ConnectionId,
        data: ConnectionData,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE provider_connections SET dpa_signed = ?1, allows_training = ?2 WHERE id = ?3",
                params![data.dpa_signed, data.allows_training, id.to_string()],
            )?;
            if changed == 0 {
                return Err(StorageError::NotFound("connection"));
            }
            insert_event(
                &tx,
                "ConnectionDataChanged",
                &id.to_string(),
                0,
                json!({"dpa_signed": data.dpa_signed, "allows_training": data.allows_training}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn set_in_eea(&self, in_eea: bool, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
                params![IN_EEA, if in_eea { "1" } else { "0" }],
            )?;
            insert_event(
                &tx,
                "LocationChanged",
                IN_EEA,
                0,
                json!({"in_eea": in_eea}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
