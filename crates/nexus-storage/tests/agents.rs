//! The user's agents (`schema::V24`, ADR-AGENT-001) on a real SQLite file:
//! an agent is saved with its own actor, a stale edit is refused, an
//! archived agent keeps its name for what it did, a task keeps the agent it
//! was started with, a removed connection takes the agent back to the
//! active model, and the migration keeps every task.

mod common;

use std::path::PathBuf;

use nexus_core::RecordActor;
use nexus_storage::{
    AgentOrigin, NewAgent, NewExchange, Requester, SUPPORTED_VERSION, StorageError, Store,
};

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
    let dir = std::env::temp_dir().join(format!("nexus-agents-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

fn helpdesk(connection: Option<nexus_protocol::ConnectionId>) -> NewAgent {
    NewAgent {
        name: "Support desk".into(),
        description: "Tickets and Microsoft 365 questions at work.".into(),
        instructions: "SENTINEL-agent-instructions: answer in Dutch.".into(),
        tools_off: vec!["WebRead".into()],
        skills: vec!["ticket-replies".into()],
        mcp_servers: Vec::new(),
        connection_id: connection,
        model: connection.map(|_| "gemini-helpdesk".into()),
        project_id: None,
    }
}

async fn exchange(
    s: &Store,
    connection: nexus_protocol::ConnectionId,
    agent: Option<nexus_protocol::AgentId>,
    key: &str,
) -> nexus_storage::Exchange {
    s.append_exchange(
        NewExchange {
            requester: Requester::User,
            pack: None,
            agent,
            conversation_id: None,
            project_id: None,
            idempotency_key: key.into(),
            text: "hi".into(),
            connection_id: connection,
            model: "m".into(),
            persona_version: 1,
            title: None,
        },
        2,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn an_agent_is_kept_with_its_actor_and_a_task_keeps_its_agent() {
    let env = env();
    let s = &env.store;
    s.persona(0).await.unwrap();
    s.ensure_default_project(0).await.unwrap();
    let connection = s
        .save_connection(None, "Gemini", "g".into(), false, "gemini-a".into(), 1)
        .await
        .unwrap()
        .id;

    let saved = s
        .save_agent(None, 0, helpdesk(Some(connection)), 10)
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.tools_off, ["WebRead"]);
    assert_eq!(saved.model.as_deref(), Some("gemini-helpdesk"));
    assert_eq!(s.agents().await.unwrap(), vec![saved.clone()]);

    // A stale edit is refused; the current one is kept, and the actor
    // takes the new name.
    let mut renamed = helpdesk(Some(connection));
    renamed.name = "Helpdesk".into();
    let edited = s
        .save_agent(Some(saved.id), 1, renamed.clone(), 11)
        .await
        .unwrap();
    assert_eq!(edited.revision, 2);
    assert!(matches!(
        s.save_agent(Some(saved.id), 1, renamed, 12).await,
        Err(StorageError::RevisionConflict("agent"))
    ));

    // A task started with it keeps it; the default agent has none.
    let with = exchange(s, connection, Some(saved.id), "with").await;
    let without = exchange(s, connection, None, "without").await;
    assert_eq!(
        s.conversation_agent(with.conversation_id)
            .await
            .unwrap()
            .map(|a| a.name),
        Some("Helpdesk".to_string())
    );
    assert_eq!(
        s.conversation_agent(without.conversation_id).await.unwrap(),
        None
    );
    assert_eq!(s.agent_task_counts().await.unwrap(), vec![(saved.id, 1)]);
    let actor: (String, String, Option<i64>) = s
        .call(move |c| {
            Ok(c.query_row(
                "SELECT ac.kind, ac.name, ac.disabled_at_ms FROM agents a JOIN actors ac ON ac.id = a.actor_id
                 WHERE a.id = ?1",
                [saved.id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(actor, ("Agent".to_string(), "Helpdesk".to_string(), None));

    // What the agent writes in its task is an action of its own actor; in
    // a task of the default agent, of Nexus.
    let register = s
        .ensure_register(
            "job_applications".into(),
            "Jobs".into(),
            "{}".into(),
            "status".into(),
            None,
            13,
        )
        .await
        .unwrap()
        .id;
    for (ex, company) in [(&with, "Acme"), (&without, "Beta")] {
        s.add_record(
            register,
            [
                ("company".to_string(), company.to_string()),
                ("status".to_string(), "To apply".to_string()),
            ]
            .into(),
            RecordActor::Agent,
            Some(AgentOrigin {
                conversation_id: ex.conversation_id,
                message_id: ex.model_message.id,
                read_hosts: Vec::new(),
                policy: nexus_core::PolicyDecision::Auto,
            }),
            14,
        )
        .await
        .unwrap();
    }
    let writers = s
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT x.name FROM actions a JOIN actors x ON x.id = a.actor_id
                 WHERE a.tool_id = 'RecordWrite' ORDER BY a.created_at_ms, x.name",
            )?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    assert_eq!(writers, ["Helpdesk", "Nexus"]);

    // Archived: gone from the list, its actor disabled but still named;
    // its task keeps it.
    s.archive_agent(saved.id, 20).await.unwrap();
    assert!(s.agents().await.unwrap().is_empty());
    assert!(s.agent(saved.id).await.unwrap().archived_at_ms.is_some());
    assert!(
        s.conversation_agent(with.conversation_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        s.save_agent(Some(saved.id), 3, helpdesk(None), 21).await,
        Err(StorageError::RevisionConflict("agent"))
    ));

    // The events name it, never its instructions.
    let events = s
        .call(|c| {
            let mut stmt = c.prepare("SELECT payload_json FROM events WHERE kind LIKE 'Agent%'")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|e| !e.contains("SENTINEL")));
}

#[tokio::test]
async fn a_removed_connection_takes_its_agents_back_to_the_active_model() {
    let env = env();
    let s = &env.store;
    s.persona(0).await.unwrap();
    s.ensure_default_project(0).await.unwrap();
    let connection = s
        .save_connection(None, "Gemini", "g".into(), false, "gemini-b".into(), 1)
        .await
        .unwrap()
        .id;
    let agent = s
        .save_agent(None, 0, helpdesk(Some(connection)), 2)
        .await
        .unwrap();
    s.delete_connection(connection, 3).await.unwrap();
    let after = s.agent(agent.id).await.unwrap();
    assert_eq!((after.connection_id, after.model), (None, None));
}

#[tokio::test]
async fn migrating_to_v24_keeps_every_task_with_the_default_agent() {
    let env = env();
    let s = &env.store;
    s.persona(0).await.unwrap();
    s.ensure_default_project(0).await.unwrap();
    let connection = s
        .save_connection(None, "Gemini", "g".into(), false, "gemini-v24".into(), 1)
        .await
        .unwrap()
        .id;
    let before = exchange(s, connection, None, "old").await;
    s.call(|c| {
        common::undo_v24(c)?;
        Ok(())
    })
    .await
    .unwrap();
    let path = env.dir.join("nexus.db");
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().await.unwrap(), SUPPORTED_VERSION);
    assert_eq!(
        reopened
            .conversation_agent(before.conversation_id)
            .await
            .unwrap(),
        None
    );
    assert!(reopened.agents().await.unwrap().is_empty());
}
