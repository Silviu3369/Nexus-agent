//! MCP servers and tools on a real SQLite file (`schema::V7`): the
//! migration keeps the agent's steps, a tool is enabled only in the form
//! the user saw, a changed or vanished tool is off, and a removed server
//! takes its tools with it. Events never carry what a server wrote.

use std::path::PathBuf;

use nexus_core::{AgentStepKind, AgentStepStatus};
use nexus_protocol::McpServerId;
use nexus_storage::{NewExchange, NewMcpTool, SUPPORTED_VERSION, StorageError, Store};
use serde_json::json;

struct Env {
    store: Store,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-mcp-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

const SECRET_DESCRIPTION: &str = "Search the docs. SENTINEL-description-91aa";

fn tool(name: &str, description: &str, sha: char) -> NewMcpTool {
    NewMcpTool {
        name: name.to_string(),
        title: None,
        description: description.to_string(),
        input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        annotations: json!({"readOnlyHint": true}),
        definition_sha256: sha.to_string().repeat(64),
    }
}

async fn server(env: &Env, url: &str) -> McpServerId {
    env.store
        .create_mcp_server("Docs".into(), url.into(), Some("mcp-entry".into()), 1)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn migrating_to_v7_keeps_the_steps_and_accepts_mcp_calls() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v7-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let message = {
        let store = Store::open(&path).unwrap();
        store.persona(0).await.unwrap();
        store.ensure_default_project(0).await.unwrap();
        let connection = store
            .save_connection(
                None,
                "Gemini",
                "g".into(),
                false,
                "gemini-v7-test".into(),
                1,
            )
            .await
            .unwrap()
            .id;
        let sent = store
            .append_exchange(
                NewExchange {
                    conversation_id: None,
                    project_id: None,
                    idempotency_key: "k-v7".into(),
                    text: "Salut".into(),
                    connection_id: connection,
                    model: "gemini-test".into(),
                    persona_version: 1,
                },
                2,
            )
            .await
            .unwrap();
        let step = store
            .begin_step(
                sent.model_message.id,
                AgentStepKind::ModelCall,
                json!({"model": "m"}),
                3,
            )
            .await
            .unwrap();
        store
            .finish_step(step.id, AgentStepStatus::Done, json!({"duration_ms": 5}), 4)
            .await
            .unwrap();
        // Back to version 6, as the previous host left it.
        store
            .call(|c| {
                c.execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     DROP TABLE paused_turns; ALTER TABLE runs DROP COLUMN outcome_json;
                     DROP TABLE mcp_tools; DROP TABLE mcp_servers;
                     CREATE TABLE agent_steps_v6 (
                        id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages (id),
                        seq INTEGER NOT NULL CHECK (seq >= 1),
                        kind TEXT NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed')),
                        status TEXT NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
                        detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
                        started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER,
                        CHECK ((status = 'Running') = (finished_at_ms IS NULL)),
                        UNIQUE (message_id, seq)) STRICT;
                     INSERT INTO agent_steps_v6 SELECT * FROM agent_steps;
                     DROP TABLE agent_steps;
                     ALTER TABLE agent_steps_v6 RENAME TO agent_steps;
                     CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
                     CREATE INDEX agent_steps_by_status ON agent_steps (status);
                     PRAGMA foreign_keys = ON;
                     PRAGMA user_version = 6;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        (sent.conversation_id, sent.model_message.id)
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let steps = store.steps_for_conversation(message.0).await.unwrap();
    assert_eq!(steps.len(), 1, "the steps table was rebuilt with its rows");
    assert_eq!(steps[0].detail, json!({"model": "m", "duration_ms": 5}));
    store
        .begin_step(
            message.1,
            AgentStepKind::McpCall,
            json!({"server": "Docs", "tool": "search"}),
            5,
        )
        .await
        .expect("the new step kind is accepted");
    assert!(store.mcp_servers().await.unwrap().is_empty());
    let enforced: i64 = store
        .call(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(enforced, 1);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_tool_is_enabled_only_in_the_form_the_user_saw() {
    let env = env();
    let id = server(&env, "https://docs.example/mcp").await;
    let listed = env
        .store
        .record_mcp_listing(
            id,
            vec![
                tool("search", SECRET_DESCRIPTION, 'a'),
                tool("fetch", "Fetch", 'b'),
            ],
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["search", "fetch"],
        "kept in the server's order"
    );
    assert!(listed.iter().all(|t| !t.enabled() && t.available));
    assert!(env.store.enabled_mcp_tools().await.unwrap().is_empty());

    // A definition other than the current one is refused.
    let wrong = env
        .store
        .set_mcp_tool_enabled(id, "search".into(), Some("c".repeat(64)), 3)
        .await
        .unwrap_err();
    assert!(matches!(wrong, StorageError::RevisionConflict(_)));
    let on = env
        .store
        .set_mcp_tool_enabled(id, "search".into(), Some("a".repeat(64)), 3)
        .await
        .unwrap();
    assert!(on.enabled());
    let offered = env.store.enabled_mcp_tools().await.unwrap();
    assert_eq!(offered.len(), 1);
    assert_eq!(
        (offered[0].0.id, offered[0].1.name.as_str()),
        (id, "search")
    );

    // The server changes `search` and stops listing `fetch`.
    let relisted = env
        .store
        .record_mcp_listing(id, vec![tool("search", "Search, and more", 'd')], 4)
        .await
        .unwrap();
    let search = relisted.iter().find(|t| t.name == "search").unwrap();
    assert!(search.changed() && !search.enabled());
    let fetch = relisted.iter().find(|t| t.name == "fetch").unwrap();
    assert!(!fetch.available, "kept, but not available");
    assert!(env.store.enabled_mcp_tools().await.unwrap().is_empty());
    assert!(matches!(
        env.store
            .set_mcp_tool_enabled(id, "fetch".into(), Some("b".repeat(64)), 5)
            .await
            .unwrap_err(),
        StorageError::RevisionConflict(_)
    ));

    // A failure keeps the tools as they were and records why, bounded.
    env.store
        .record_mcp_failure(id, "x".repeat(2000), 6)
        .await
        .unwrap();
    let row = env.store.mcp_server(id).await.unwrap();
    assert_eq!(row.last_error.unwrap().chars().count(), 500);
    assert_eq!(env.store.mcp_tools(id).await.unwrap().len(), 2);

    let events = env.store.events_since(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "McpToolEnabled"));
    assert!(events.iter().any(|e| e.kind == "McpToolsListed"));
    assert!(
        events
            .iter()
            .all(|e| !e.payload_json.contains("SENTINEL-description")),
        "events never carry what a server wrote"
    );
}

#[tokio::test]
async fn a_new_address_drops_the_tools_and_removal_takes_everything() {
    let env = env();
    let id = server(&env, "https://docs.example/mcp").await;
    env.store
        .record_mcp_listing(id, vec![tool("search", "Search", 'a')], 2)
        .await
        .unwrap();
    env.store
        .set_mcp_tool_enabled(id, "search".into(), Some("a".repeat(64)), 3)
        .await
        .unwrap();

    // Same address twice is refused.
    assert!(
        env.store
            .create_mcp_server("Again".into(), "https://docs.example/mcp".into(), None, 4)
            .await
            .is_err()
    );

    // Renaming keeps the tools; another address is another server.
    env.store
        .update_mcp_server(
            id,
            "Docs 2".into(),
            "https://docs.example/mcp".into(),
            None,
            5,
        )
        .await
        .unwrap();
    assert_eq!(env.store.enabled_mcp_tools().await.unwrap().len(), 1);
    let moved = env
        .store
        .update_mcp_server(
            id,
            "Docs 2".into(),
            "https://other.example/mcp".into(),
            None,
            6,
        )
        .await
        .unwrap();
    assert_eq!(moved.revision, 3);
    assert!(moved.checked_at_ms.is_none());
    assert!(env.store.mcp_tools(id).await.unwrap().is_empty());

    env.store
        .record_mcp_listing(id, vec![tool("search", "Search", 'a')], 7)
        .await
        .unwrap();
    let id2 = server(&env, "https://third.example/mcp").await;
    assert_eq!(
        env.store
            .delete_mcp_server(id2, 8)
            .await
            .unwrap()
            .as_deref(),
        Some("mcp-entry"),
        "the vault entry to delete is returned"
    );
    env.store.delete_mcp_server(id, 9).await.unwrap();
    assert!(env.store.mcp_servers().await.unwrap().is_empty());
    let left: i64 = env
        .store
        .call(|c| Ok(c.query_row("SELECT count(*) FROM mcp_tools", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(left, 0, "tools go with their server");
    assert!(matches!(
        env.store.mcp_server(id).await.unwrap_err(),
        StorageError::NotFound(_)
    ));
}
