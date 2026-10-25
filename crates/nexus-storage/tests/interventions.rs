//! What the user did about the agent's work (`schema::V16`, ADR-TASK-001)
//! on a real SQLite file: each decision, undo, correction, retry and stop
//! is recorded once, in the same transaction as the act, on the turn it
//! concerns; the user's own edits to their own values are not; and the
//! migration carries over the decisions already recorded before it.

mod common;

use std::path::PathBuf;

use nexus_core::{InterventionKind, MemoryScope, MemorySource, MessageStatus, RecordActor};
use nexus_protocol::{ConversationId, OperationId, RunId};
use nexus_storage::{
    AgentOrigin, Decision, Exchange, InboxSettle, NewAction, NewExchange, NewMemory, Pause,
    SUPPORTED_VERSION, Settlement, Store, SubmitOutcome, Values,
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
    let dir =
        std::env::temp_dir().join(format!("nexus-interventions-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

async fn exchange(store: &Store, conversation: Option<ConversationId>, key: &str) -> Exchange {
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    let connection = match store.connections().await.unwrap().first() {
        Some(c) => c.id,
        None => {
            store
                .save_connection(None, "Gemini", "g".into(), false, "gemini-i".into(), 1)
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
                requester: nexus_storage::Requester::User,
                pack: None,
            },
            2,
        )
        .await
        .unwrap()
}

/// (kind, turn, effect run) of every intervention, oldest first.
async fn interventions(store: &Store) -> Vec<(String, Option<String>, Option<String>)> {
    store
        .call(|c| {
            let mut stmt =
                c.prepare("SELECT kind, run_id, effect_run_id FROM interventions ORDER BY id")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

fn turn(ex: &Exchange) -> String {
    ex.model_message.run_id.unwrap().to_string()
}

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

async fn register(store: &Store) -> nexus_protocol::RegisterId {
    store
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
        .id
}

/// An effect run the reply `ex` proposed, waiting for approval.
async fn proposed_effect(store: &Store, ex: &Exchange, key: &str) -> RunId {
    let project = store.ensure_default_project(0).await.unwrap();
    let run = match store
        .submit_task(
            nexus_storage::EffectRequester::Agent,
            project,
            key.into(),
            "{}".into(),
            key.into(),
            3,
        )
        .await
        .unwrap()
    {
        SubmitOutcome::Created { run_id, .. } | SubmitOutcome::Existing { run_id, .. } => run_id,
    };
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

#[tokio::test]
async fn each_decision_about_the_agents_work_is_recorded_once_on_its_turn() {
    let env = env();
    let s = &env.store;
    let reg = register(s).await;
    let ex = exchange(s, None, "a").await;
    let a = turn(&ex);
    let origin = AgentOrigin {
        conversation_id: ex.conversation_id,
        message_id: ex.model_message.id,
        read_hosts: Vec::new(),
    };

    // The agent adds an application; the user corrects one of its fields.
    let added = s
        .add_record(
            reg,
            values(&[("company", "Acme"), ("role", "IT"), ("status", "To apply")]),
            RecordActor::Agent,
            Some(origin.clone()),
            10,
        )
        .await
        .unwrap();
    let mut edited = added.record.values.clone();
    edited.insert("role".into(), "IT Support".into());
    let after_edit = s
        .update_record(
            added.record.id,
            Some(1),
            edited.clone(),
            RecordActor::User,
            None,
            11,
        )
        .await
        .unwrap()
        .unwrap();
    // A second edit of the user's own value is not a correction.
    edited.insert("role".into(), "IT Support Analyst".into());
    s.update_record(
        after_edit.record.id,
        Some(2),
        edited,
        RecordActor::User,
        None,
        12,
    )
    .await
    .unwrap();
    // The user's own application, edited by the user: nothing to record.
    let mine = s
        .add_record(
            reg,
            values(&[("company", "Beta"), ("role", "Ops")]),
            RecordActor::User,
            None,
            13,
        )
        .await
        .unwrap();
    s.update_record(
        mine.record.id,
        Some(1),
        values(&[("company", "Beta"), ("role", "DevOps")]),
        RecordActor::User,
        None,
        14,
    )
    .await
    .unwrap();
    assert_eq!(
        interventions(s).await,
        [("Correction".into(), Some(a.clone()), None)]
    );

    // The agent changes a record; the user undoes it.
    let changed = s
        .update_record(
            mine.record.id,
            None,
            values(&[
                ("company", "Beta"),
                ("role", "DevOps"),
                ("status", "Applied"),
            ]),
            RecordActor::Agent,
            Some(origin.clone()),
            15,
        )
        .await
        .unwrap()
        .unwrap();
    s.undo_record_change(changed.change_id, 16).await.unwrap();
    // Deleting what the agent added corrects it too.
    s.delete_record(added.record.id, 17).await.unwrap();
    let kinds: Vec<String> = interventions(s).await.into_iter().map(|i| i.0).collect();
    assert_eq!(kinds, ["Correction", "Undo", "Correction"]);

    // A proposal approved, another cancelled; a reply stopped.
    let second = exchange(s, Some(ex.conversation_id), "b").await;
    let effect = proposed_effect(s, &second, "b-note").await;
    let detail = s.run_detail(effect).await.unwrap();
    let approval = detail.approval.unwrap();
    s.decide_approval(
        approval.id,
        approval.manifest_sha256,
        Decision::Approved,
        nexus_core::DecisionPath::Window,
        20,
    )
    .await
    .unwrap();
    let third = exchange(s, Some(ex.conversation_id), "c").await;
    let cancelled = proposed_effect(s, &third, "c-note").await;
    s.cancel_run(cancelled, 21).await.unwrap();
    let fourth = exchange(s, Some(ex.conversation_id), "d").await;
    s.settle_message(
        fourth.model_message.id,
        MessageStatus::Cancelled,
        Settlement::default(),
        22,
    )
    .await
    .unwrap();
    let all = interventions(s).await;
    assert_eq!(
        all[3],
        (
            "Approved".into(),
            Some(turn(&second)),
            Some(effect.to_string())
        )
    );
    assert_eq!(
        all[4],
        (
            "Stopped".into(),
            Some(turn(&third)),
            Some(cancelled.to_string())
        )
    );
    assert_eq!(all[5], ("Stopped".into(), Some(turn(&fourth)), None));

    // A fact the agent proposed, rejected.
    let fact = s
        .add_memory(
            NewMemory {
                scope: MemoryScope::Personal,
                project_id: None,
                content: "Prefers Springfield".into(),
                source: MemorySource::Agent,
                conversation_id: Some(ex.conversation_id),
                message_id: Some(ex.model_message.id),
                influences: Vec::new(),
            },
            23,
        )
        .await
        .unwrap();
    s.decide_memory(fact.id, fact.revision, false, 24)
        .await
        .unwrap();
    assert_eq!(
        interventions(s).await[6],
        ("Rejected".into(), Some(a.clone()), None)
    );

    // The turns of the task, each with what the user did about it.
    let reviews = s.turn_reviews(ex.conversation_id).await.unwrap();
    assert_eq!(reviews.len(), 4);
    let first: Vec<InterventionKind> = reviews[0].interventions.iter().map(|i| i.0).collect();
    assert_eq!(
        first,
        [
            InterventionKind::Correction,
            InterventionKind::Undo,
            InterventionKind::Correction,
            InterventionKind::Rejected
        ]
    );
    assert_eq!(reviews[3].interventions[0].0, InterventionKind::Stopped);

    // Deleting the task removes what was recorded about it.
    s.settle_message(
        ex.model_message.id,
        MessageStatus::Complete,
        Settlement::default(),
        25,
    )
    .await
    .unwrap();
    s.delete_conversation(ex.conversation_id, 26).await.unwrap();
    assert!(
        interventions(s)
            .await
            .iter()
            .all(|i| i.1.is_none() || i.2.is_some()),
        "only the effect runs' interventions can outlive the turns, unlinked"
    );
}

#[tokio::test]
async fn a_retry_of_a_failed_inbox_item_is_recorded_on_its_last_turn() {
    let env = env();
    let s = &env.store;
    let reg = register(s).await;
    let folder = s
        .add_folder("Ads".into(), r"C:\Ads".into(), false, 1)
        .await
        .unwrap();
    s.save_inbox(reg, 0, Some(folder.id), true, None, None, 2)
        .await
        .unwrap();
    s.inbox_seen(reg, "ad.txt".into(), "cd".repeat(32), 1, None, 3)
        .await
        .unwrap();
    let file = s.next_inbox_file(reg, 3).await.unwrap().unwrap();
    let ex = exchange(s, None, "ad").await;
    s.settle_message(
        ex.model_message.id,
        MessageStatus::Failed,
        Settlement {
            error_code: Some("QuotaExceeded".into()),
            ..Settlement::default()
        },
        4,
    )
    .await
    .unwrap();
    s.inbox_outcome(file.id, Some(ex.conversation_id), None, 5)
        .await
        .unwrap();
    s.settle_inbox_file(file.id, InboxSettle::Failed("quota".into()), 5)
        .await
        .unwrap();
    s.retry_inbox_file(reg, file.id, 6).await.unwrap();
    assert_eq!(
        interventions(s).await,
        [("Retry".into(), Some(turn(&ex)), None)]
    );
}

#[tokio::test]
async fn migrating_to_v16_carries_over_the_decisions_already_recorded() {
    let env = env();
    let path = env.dir.join("nexus.db");
    let s = &env.store;
    let reg = register(s).await;
    let ex = exchange(s, None, "m").await;
    let added = s
        .add_record(
            reg,
            values(&[("company", "Acme"), ("role", "IT")]),
            RecordActor::Agent,
            Some(AgentOrigin {
                conversation_id: ex.conversation_id,
                message_id: ex.model_message.id,
                read_hosts: Vec::new(),
            }),
            10,
        )
        .await
        .unwrap();
    let changed = s
        .update_record(
            added.record.id,
            None,
            values(&[("company", "Acme"), ("role", "IT"), ("status", "Applied")]),
            RecordActor::Agent,
            Some(AgentOrigin {
                conversation_id: ex.conversation_id,
                message_id: ex.model_message.id,
                read_hosts: Vec::new(),
            }),
            11,
        )
        .await
        .unwrap()
        .unwrap();
    s.undo_record_change(changed.change_id, 12).await.unwrap();
    let second = exchange(s, Some(ex.conversation_id), "m2").await;
    let effect = proposed_effect(s, &second, "m2-note").await;
    let approval = s.run_detail(effect).await.unwrap().approval.unwrap();
    s.decide_approval(
        approval.id,
        approval.manifest_sha256,
        Decision::Denied,
        nexus_core::DecisionPath::Window,
        13,
    )
    .await
    .unwrap();
    assert_eq!(interventions(s).await.len(), 2);

    s.call(|c| {
        common::undo_v18(c)?;
        c.execute_batch(
            "DROP TABLE interventions;
             DELETE FROM actions WHERE compensates_action_id IS NOT NULL
                 OR run_id IN (SELECT id FROM runs WHERE kind = 'Agent');
             DROP INDEX actions_by_run; DROP INDEX actions_by_compensated;
             ALTER TABLE actions DROP COLUMN compensates_action_id;
             ALTER TABLE actions DROP COLUMN effect_class;
             ALTER TABLE actions DROP COLUMN evidence;
             PRAGMA user_version = 15;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let mut carried = interventions(&s).await;
    carried.sort();
    assert_eq!(
        carried,
        [
            (
                "Rejected".into(),
                Some(turn(&second)),
                Some(effect.to_string())
            ),
            ("Undo".into(), Some(turn(&ex)), None),
        ]
    );
}
