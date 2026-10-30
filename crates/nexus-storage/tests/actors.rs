//! Who does what (`schema::V18`, ADR-IDENTITY-001) on a real SQLite file:
//! the actors the Host names exist, an automation keeps its own actor
//! (renamed with it, disabled when it is deleted); every task names who
//! asked for it, every action who did it, every decision who made it; and
//! the migration names them for what was recorded before it.

mod common;

use std::path::PathBuf;

use nexus_core::{ActorKind, MemoryScope, MemorySource, MessageStatus, RecordActor};
use nexus_protocol::{ConversationId, OperationId, RunId, Schedule};
use nexus_storage::{
    ActorRow, AgentOrigin, AutomationRunRow, Decision, EffectRequester, Exchange, NewAction,
    NewAutomation, NewExchange, NewMemory, Pause, Requester, SUPPORTED_VERSION, Settlement, Store,
    SubmitOutcome, Values,
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
    let dir = std::env::temp_dir().join(format!("nexus-actors-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

fn actor(kind: ActorKind, name: &str) -> Option<ActorRow> {
    Some(ActorRow {
        kind,
        name: name.into(),
    })
}

async fn exchange(
    store: &Store,
    conversation: Option<ConversationId>,
    key: &str,
    requester: Requester,
) -> Exchange {
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    let connection = match store.connections().await.unwrap().first() {
        Some(c) => c.id,
        None => {
            store
                .save_connection(None, "Gemini", "g".into(), false, "gemini-a".into(), 1)
                .await
                .unwrap()
                .id
        }
    };
    store
        .append_exchange(
            NewExchange {
                conversation_id: conversation,
                project_id: None,
                idempotency_key: key.into(),
                text: format!("request {key}"),
                connection_id: connection,
                model: "gemini-test".into(),
                persona_version: 1,
                title: None,
                requester,
                pack: None,
                agent: None,
            },
            2,
        )
        .await
        .unwrap()
}

fn morning() -> NewAutomation {
    NewAutomation {
        name: "Morning".into(),
        prompt: "summarise".into(),
        project_id: None,
        schedule: Schedule::Weekly {
            days: vec![0, 1, 2, 3, 4, 5, 6],
            hour: 9,
            minute: 0,
        },
        enabled: true,
        catch_up: false,
        next_run_at_ms: Some(1_000),
    }
}

/// A task the automation `automation` started, as its scheduler does.
async fn started_by(
    store: &Store,
    automation: nexus_protocol::AutomationId,
    key: &str,
) -> Exchange {
    let ex = exchange(store, None, key, Requester::Automation(automation)).await;
    store
        .record_automation_runs(
            automation,
            None,
            vec![AutomationRunRow {
                automation_id: automation,
                scheduled_for_ms: 1_000,
                at_ms: 1_001,
                status: nexus_protocol::AutomationRunStatus::Started,
                reason: None,
                conversation_id: Some(ex.conversation_id),
                catch_up: false,
                manual: true,
            }],
            None,
            3,
        )
        .await
        .unwrap();
    ex
}

/// An effect run asked for by `requester`, waiting for approval; when the
/// agent asked, it is the proposal of the reply `ex`.
async fn effect(
    store: &Store,
    requester: EffectRequester,
    ex: Option<&Exchange>,
    key: &str,
) -> RunId {
    let project = store.ensure_default_project(0).await.unwrap();
    let run = match store
        .submit_task(requester, project, key.into(), "{}".into(), key.into(), 3)
        .await
        .unwrap()
    {
        SubmitOutcome::Created { run_id, .. } | SubmitOutcome::Existing { run_id, .. } => run_id,
    };
    if let Some(ex) = ex {
        store
            .settle_message(
                ex.model_message.id,
                MessageStatus::Complete,
                Settlement {
                    proposed_run_id: Some(run),
                    pause: Some(Pause {
                        run_id: run,
                        state_json: r#"{"version":1}"#.into(),
                    }),
                    ..Settlement::default()
                },
                4,
            )
            .await
            .unwrap();
    }
    // Claim this run (an approved one may be queued too, waiting to run).
    while store
        .claim_next_queued_run(5)
        .await
        .unwrap()
        .unwrap()
        .run
        .id
        != run
    {}
    store
        .prepare_action(
            run,
            NewAction {
                effect_class: nexus_core::EffectClass::InternalReversible,
                evidence: nexus_core::Evidence::Verified,
                tool_id: "CreateMarkdownArtifact".into(),
                tool_version: 1,
                operation_id: OperationId::new(),
                manifest_json: "{}".into(),
                manifest_sha256: format!("sha-{key}"),
                approval_expires_at_ms: i64::MAX,
                approval_grant_ms: 600_000,
            },
            6,
        )
        .await
        .unwrap();
    run
}

async fn decide(store: &Store, run: RunId, decision: Decision) {
    let approval = store.run_detail(run).await.unwrap().approval.unwrap();
    store
        .decide_approval(
            approval.id,
            approval.manifest_sha256,
            decision,
            nexus_core::DecisionPath::Window,
            7,
        )
        .await
        .unwrap();
}

/// (kind, name) of the actor `sql` names, by its id.
async fn named(store: &Store, sql: &'static str) -> Vec<(String, String)> {
    store
        .call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT a.kind, a.name FROM ({sql}) x JOIN actors a ON a.id = x.actor ORDER BY 1, 2"
            ))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

async fn count(store: &Store, sql: &'static str) -> i64 {
    store
        .call(move |c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .await
        .unwrap()
}

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn the_host_finds_every_actor_and_an_automation_keeps_its_own() {
    let env = env();
    let s = &env.store;
    let all = || async {
        s.call(|c| {
            let mut stmt = c.prepare(
                "SELECT kind, ref, name, disabled_at_ms IS NOT NULL FROM actors ORDER BY ref",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<(String, String, String, bool)>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
    };
    let row = |kind: &str, reference: &str, name: &str, disabled: bool| {
        (
            kind.to_string(),
            reference.to_string(),
            name.to_string(),
            disabled,
        )
    };
    assert_eq!(
        all().await,
        vec![
            row("Agent", "agent", "Nexus", false),
            row("System", "host", "Nexus Host", false),
            row("Intake", "intake:inbox", "Inbox", false),
            row("User", "user:local", "Local user", false),
        ]
    );

    let automation = s.create_automation(morning(), 10).await.unwrap();
    let reference = format!("automation:{}", automation.id);
    let id_of = |reference: String| async move {
        s.call(move |c| {
            Ok(
                c.query_row("SELECT id FROM actors WHERE ref = ?1", [reference], |r| {
                    r.get::<_, String>(0)
                })?,
            )
        })
        .await
        .unwrap()
    };
    let first = id_of(reference.clone()).await;
    let mut renamed = morning();
    renamed.name = "Evening".into();
    s.update_automation(automation.id, automation.revision, renamed, 11)
        .await
        .unwrap();
    assert_eq!(id_of(reference.clone()).await, first, "renamed, same actor");
    assert!(
        all()
            .await
            .contains(&row("Automation", &reference, "Evening", false))
    );

    s.delete_automation(automation.id, 12).await.unwrap();
    assert!(
        all()
            .await
            .contains(&row("Automation", &reference, "Evening", true)),
        "a deleted automation's actor stays, disabled"
    );

    // The kinds are exactly the domain's.
    let rejected = s
        .call(|c| {
            for (i, kind) in ActorKind::ALL.iter().enumerate() {
                c.execute(
                    "INSERT INTO actors (id, kind, ref, name, created_at_ms) VALUES (?1, ?2, ?1, 'x', 0)",
                    (format!("k{i}"), kind.as_str()),
                )?;
            }
            Ok(c
                .execute(
                    "INSERT INTO actors (id, kind, ref, name, created_at_ms) VALUES ('kx', 'Admin', 'kx', 'x', 0)",
                    [],
                )
                .is_err())
        })
        .await
        .unwrap();
    assert!(rejected, "an unknown kind is refused");
}

#[tokio::test]
async fn every_task_names_who_asked_and_every_act_who_did_it() {
    let env = env();
    let s = &env.store;

    let chat = exchange(s, None, "chat", Requester::User).await;
    assert_eq!(
        s.task_requester(chat.conversation_id).await.unwrap(),
        actor(ActorKind::User, "Local user")
    );
    let inbox = exchange(s, None, "inbox", Requester::Inbox).await;
    assert_eq!(
        s.task_requester(inbox.conversation_id).await.unwrap(),
        actor(ActorKind::Intake, "Inbox")
    );
    let automation = s.create_automation(morning(), 1).await.unwrap();
    let auto = started_by(s, automation.id, "auto").await;
    assert_eq!(
        s.task_requester(auto.conversation_id).await.unwrap(),
        actor(ActorKind::Automation, "Morning")
    );
    // A follow-up in the automation's task is the user's turn, in the same task.
    s.settle_message(
        auto.model_message.id,
        MessageStatus::Complete,
        Settlement::default(),
        3,
    )
    .await
    .unwrap();
    let follow = exchange(s, Some(auto.conversation_id), "follow", Requester::User).await;
    assert_eq!(follow.conversation_id, auto.conversation_id);
    assert_eq!(
        s.task_requester(auto.conversation_id).await.unwrap(),
        actor(ActorKind::Automation, "Morning")
    );

    // A proposal is asked for and done by the agent, decided by the user.
    let proposal = effect(s, EffectRequester::Agent, Some(&chat), "proposal").await;
    let detail = s.run_detail(proposal).await.unwrap();
    assert_eq!(
        (detail.requested_by, detail.decided_by),
        (actor(ActorKind::Agent, "Nexus"), None)
    );
    decide(s, proposal, Decision::Approved).await;
    let detail = s.run_detail(proposal).await.unwrap();
    assert_eq!(
        (detail.requested_by, detail.decided_by),
        (
            actor(ActorKind::Agent, "Nexus"),
            actor(ActorKind::User, "Local user")
        )
    );

    // A note the user sends directly is theirs, and so is its decision.
    let note = effect(s, EffectRequester::User, None, "note").await;
    decide(s, note, Decision::Denied).await;
    let detail = s.run_detail(note).await.unwrap();
    assert_eq!(
        (detail.requested_by, detail.decided_by),
        (
            actor(ActorKind::User, "Local user"),
            actor(ActorKind::User, "Local user")
        )
    );

    // The agent writes a record and proposes a fact; the user undoes the write.
    let register = s
        .ensure_register(
            "job_applications".into(),
            "Jobs".into(),
            "{}".into(),
            "status".into(),
            None,
            1,
        )
        .await
        .unwrap()
        .id;
    let follow_origin = AgentOrigin {
        conversation_id: follow.conversation_id,
        message_id: follow.model_message.id,
        read_hosts: Vec::new(),
        policy: nexus_core::PolicyDecision::Auto,
    };
    let added = s
        .add_record(
            register,
            values(&[("company", "Acme"), ("role", "IT"), ("status", "To apply")]),
            RecordActor::Agent,
            Some(follow_origin),
            8,
        )
        .await
        .unwrap();
    s.add_memory(
        NewMemory {
            scope: MemoryScope::Personal,
            project_id: None,
            content: "Prefers remote roles".into(),
            source: MemorySource::Agent,
            conversation_id: Some(follow.conversation_id),
            message_id: Some(follow.model_message.id),
            influences: Vec::new(),
        },
        9,
    )
    .await
    .unwrap();
    s.undo_record_change(added.change_id, 10).await.unwrap();

    assert_eq!(
        named(
            s,
            "SELECT tool_id || '' AS t, actor_id AS actor FROM actions"
        )
        .await,
        vec![
            ("Agent".to_string(), "Nexus".to_string()),
            ("Agent".to_string(), "Nexus".to_string()),
            ("Agent".to_string(), "Nexus".to_string()),
            ("User".to_string(), "Local user".to_string()),
            ("User".to_string(), "Local user".to_string()),
        ],
        "the proposal, the record write and the fact are the agent's; the note and the undo the user's"
    );
    let user_acts = count(
        s,
        "SELECT count(*) FROM actions a JOIN actors x ON x.id = a.actor_id
         WHERE x.kind = 'User' AND (a.tool_id = 'UndoRecordChange' OR a.manifest_sha256 = 'sha-note')",
    )
    .await;
    assert_eq!(user_acts, 2);
    // Approved, Rejected and Undo: every decision was the user's.
    assert_eq!(
        named(s, "SELECT actor_id AS actor FROM interventions").await,
        vec![("User".to_string(), "Local user".to_string()); 3]
    );
    for sql in [
        "SELECT count(*) FROM tasks WHERE requested_by IS NULL",
        "SELECT count(*) FROM actions WHERE actor_id IS NULL",
        "SELECT count(*) FROM approvals WHERE decision IS NOT NULL AND decided_by IS NULL",
        "SELECT count(*) FROM interventions WHERE actor_id IS NULL",
    ] {
        assert_eq!(count(s, sql).await, 0, "{sql}");
    }
}

#[tokio::test]
async fn migrating_to_v18_names_the_actors_of_what_was_recorded() {
    let env = env();
    let path = env.dir.join("nexus.db");
    let s = &env.store;

    let chat = exchange(s, None, "chat", Requester::User).await;
    let inbox = exchange(s, None, "inbox", Requester::Inbox).await;
    let kept = s.create_automation(morning(), 1).await.unwrap();
    let auto = started_by(s, kept.id, "auto").await;
    let mut other = morning();
    other.name = "Gone".into();
    let gone = s.create_automation(other, 1).await.unwrap();
    let orphan = started_by(s, gone.id, "orphan").await;
    s.delete_automation(gone.id, 2).await.unwrap();
    let proposal = effect(s, EffectRequester::Agent, Some(&chat), "proposal").await;
    decide(s, proposal, Decision::Approved).await;
    let note = effect(s, EffectRequester::User, None, "note").await;

    s.call(|c| {
        common::undo_v18(c)?;
        Ok(())
    })
    .await
    .unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().await.unwrap(), SUPPORTED_VERSION);

    assert_eq!(
        s.task_requester(chat.conversation_id).await.unwrap(),
        actor(ActorKind::User, "Local user")
    );
    assert_eq!(
        s.task_requester(inbox.conversation_id).await.unwrap(),
        actor(ActorKind::Intake, "Inbox")
    );
    assert_eq!(
        s.task_requester(auto.conversation_id).await.unwrap(),
        actor(ActorKind::Automation, "Morning")
    );
    assert_eq!(
        s.task_requester(orphan.conversation_id).await.unwrap(),
        None,
        "an automation deleted before version 18 left nothing to name"
    );
    let detail = s.run_detail(proposal).await.unwrap();
    assert_eq!(
        (detail.requested_by, detail.decided_by),
        (
            actor(ActorKind::Agent, "Nexus"),
            actor(ActorKind::User, "Local user")
        )
    );
    assert_eq!(
        s.run_detail(note).await.unwrap().requested_by,
        actor(ActorKind::User, "Local user")
    );
    assert_eq!(
        named(&s, "SELECT actor_id AS actor FROM actions").await,
        vec![
            ("Agent".to_string(), "Nexus".to_string()),
            ("User".to_string(), "Local user".to_string()),
        ]
    );
    assert_eq!(
        named(&s, "SELECT actor_id AS actor FROM interventions").await,
        vec![("User".to_string(), "Local user".to_string())]
    );
    // Only the automation deleted before had no actor to carry over.
    assert_eq!(
        count(&s, "SELECT count(*) FROM actors WHERE kind = 'Automation'").await,
        1
    );
}
