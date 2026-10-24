//! Actions outside the approval queue and compensations (`schema::V17`,
//! ADR-ACTION-001) on a real SQLite file: the agent's register writes and
//! proposed facts are actions of their turn, with their effect class and
//! evidence, written in the same transaction as the effect (the user's own
//! writes are not the agent's actions); an undo is an action naming the one
//! it reverses; a compensation's intent is written first, runs once, and
//! does not hide the run's own action; and the migration classes the
//! actions already recorded.

mod common;

use std::path::PathBuf;

use nexus_core::{
    ActionState, EffectClass, Evidence, MemoryScope, MemorySource, MessageStatus, RecordActor,
};
use nexus_protocol::{ConversationId, OperationId, RunId};
use nexus_storage::{
    ActionRow, AgentOrigin, Decision, Exchange, NewAction, NewExchange, NewMemory, NewOperation,
    SUPPORTED_VERSION, Settlement, Store, SubmitOutcome, Values,
};
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
    let dir = std::env::temp_dir().join(format!("nexus-actions-test-{}", uuid::Uuid::new_v4()));
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
                requester: nexus_storage::Requester::User,
                pack: None,
            },
            2,
        )
        .await
        .unwrap()
}

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// (tool, state, class, evidence, is a compensation) of each action.
type Summary = (
    String,
    ActionState,
    Option<EffectClass>,
    Option<Evidence>,
    bool,
);

fn summary(actions: &[ActionRow]) -> Vec<Summary> {
    actions
        .iter()
        .map(|a| {
            (
                a.tool_id.clone(),
                a.state,
                a.effect_class,
                a.evidence,
                a.compensates.is_some(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_agents_writes_and_proposals_are_actions_of_its_turn() {
    let env = env();
    let s = &env.store;
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
    let ex = exchange(s, None, "a").await;
    let turn = ex.model_message.run_id.unwrap();
    let origin = AgentOrigin {
        conversation_id: ex.conversation_id,
        message_id: ex.model_message.id,
        read_hosts: Vec::new(),
    };
    let added = s
        .add_record(
            register,
            values(&[("company", "Acme"), ("role", "IT")]),
            RecordActor::Agent,
            Some(origin.clone()),
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
            Some(origin.clone()),
            11,
        )
        .await
        .unwrap()
        .unwrap();
    // The user's own record is not the agent's action.
    s.add_record(
        register,
        values(&[("company", "Mine"), ("role", "Ops")]),
        RecordActor::User,
        None,
        12,
    )
    .await
    .unwrap();
    s.undo_record_change(changed.change_id, 13).await.unwrap();
    let fact = s
        .add_memory(
            NewMemory {
                scope: MemoryScope::Personal,
                project_id: None,
                content: "Lives in Springfield".into(),
                source: MemorySource::Agent,
                conversation_id: Some(ex.conversation_id),
                message_id: Some(ex.model_message.id),
                influences: Vec::new(),
            },
            14,
        )
        .await
        .unwrap();
    let other = s
        .add_memory(
            NewMemory {
                scope: MemoryScope::Personal,
                project_id: None,
                content: "Has a car".into(),
                source: MemorySource::Agent,
                conversation_id: Some(ex.conversation_id),
                message_id: Some(ex.model_message.id),
                influences: Vec::new(),
            },
            15,
        )
        .await
        .unwrap();
    let actions = s.turn_actions(turn).await.unwrap();
    let verified = Some(Evidence::Verified);
    let internal = Some(EffectClass::InternalReversible);
    assert_eq!(
        summary(&actions),
        [
            (
                "RecordWrite".into(),
                ActionState::Verified,
                internal,
                verified,
                false
            ),
            (
                "RecordWrite".into(),
                ActionState::Verified,
                internal,
                verified,
                false
            ),
            (
                "UndoRecordChange".into(),
                ActionState::Verified,
                internal,
                verified,
                true
            ),
            (
                "MemoryFact".into(),
                ActionState::AwaitingApproval,
                internal,
                verified,
                false
            ),
            (
                "MemoryFact".into(),
                ActionState::AwaitingApproval,
                internal,
                verified,
                false
            ),
        ]
    );
    assert_eq!(
        actions[2].compensates,
        Some(actions[1].id),
        "the undo names the change it reverses"
    );
    let manifest: serde_json::Value = serde_json::from_str(&actions[1].manifest_json).unwrap();
    assert_eq!(manifest["fields"], json!(["status"]));
    assert_eq!(actions[1].manifest_sha256.len(), 64);

    s.decide_memory(fact.id, fact.revision, true, 16)
        .await
        .unwrap();
    s.decide_memory(other.id, other.revision, false, 17)
        .await
        .unwrap();
    let states: Vec<ActionState> = s.turn_actions(turn).await.unwrap()[3..]
        .iter()
        .map(|a| a.state)
        .collect();
    assert_eq!(states, [ActionState::Verified, ActionState::Denied]);
}

/// A note proposed by the reply `ex`, approved and carried out (its intent
/// and its end recorded), as the executor does.
async fn done_effect(s: &Store, ex: &Exchange, key: &str) -> RunId {
    let project = s.ensure_default_project(0).await.unwrap();
    let run = match s
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
    s.settle_message(
        ex.model_message.id,
        MessageStatus::Complete,
        Settlement {
            proposed_run_id: Some(run),
            ..Settlement::default()
        },
        4,
    )
    .await
    .unwrap();
    while s.claim_next_queued_run(5).await.unwrap().unwrap().run.id != run {}
    s.prepare_action(
        run,
        NewAction {
            tool_id: "CreateDocument".into(),
            tool_version: 1,
            effect_class: EffectClass::ExternalReversible,
            evidence: Evidence::Verified,
            operation_id: OperationId::new(),
            manifest_json: "{}".into(),
            manifest_sha256: format!("sha-{key}"),
            approval_expires_at_ms: i64::MAX,
        },
        6,
    )
    .await
    .unwrap();
    let approval = s.run_detail(run).await.unwrap().approval.unwrap();
    s.decide_approval(approval.id, approval.manifest_sha256, Decision::Approved, 7)
        .await
        .unwrap();
    while s.claim_next_queued_run(8).await.unwrap().unwrap().run.id != run {}
    s.claim_effect(
        run,
        NewOperation {
            target_relpath: format!("C:/Letters/{key}.docx"),
            staging_relpath: format!("C:/Letters/~{key}.tmp"),
            expected_sha256: "ab".repeat(32),
            expected_size: 10,
            algorithm_version: 1,
        },
        9,
    )
    .await
    .unwrap();
    s.complete_document(
        run,
        json!({"path": format!("C:/Letters/{key}.docx"), "sha256": "ab".repeat(32)}).to_string(),
        10,
    )
    .await
    .unwrap();
    run
}

#[tokio::test]
async fn an_undo_records_its_intent_first_runs_once_and_keeps_the_runs_own_action() {
    let env = env();
    let s = &env.store;
    let ex = exchange(s, None, "u").await;
    let run = done_effect(s, &ex, "letter").await;
    let manifest = json!({"path": "C:/Letters/letter.docx", "sha256": "ab".repeat(32)});
    let undo = s
        .begin_compensation(
            run,
            "DeleteCreatedDocument".into(),
            1,
            EffectClass::ExternalIrreversible,
            manifest.clone(),
            20,
        )
        .await
        .unwrap();
    assert!(
        s.begin_compensation(
            run,
            "DeleteCreatedDocument".into(),
            1,
            EffectClass::ExternalIrreversible,
            manifest.clone(),
            21
        )
        .await
        .is_err(),
        "one undo at a time"
    );
    assert_eq!(
        s.open_compensations().await.unwrap().len(),
        1,
        "the intent is written before the effect"
    );
    let detail = s.run_detail(run).await.unwrap();
    assert_eq!(
        detail.action.unwrap().tool_id,
        "CreateDocument",
        "the run keeps its own action"
    );

    s.finish_compensation(undo, true, None, 22).await.unwrap();
    assert!(s.open_compensations().await.unwrap().is_empty());
    let detail = s.run_detail(run).await.unwrap();
    let outcome: serde_json::Value =
        serde_json::from_str(detail.run.outcome_json.as_deref().unwrap()).unwrap();
    assert_eq!(outcome["undone_at_ms"], 22);
    assert_eq!(
        s.compensation_of(run).await.unwrap().unwrap().state,
        ActionState::Verified
    );
    assert!(
        s.begin_compensation(
            run,
            "DeleteCreatedDocument".into(),
            1,
            EffectClass::ExternalIrreversible,
            manifest,
            23
        )
        .await
        .is_err(),
        "an undone effect is not undone again"
    );
    let undo_recorded: i64 = s
        .call(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM interventions WHERE kind = 'Undo'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        undo_recorded, 1,
        "the undo is the user's intervention on the turn"
    );

    // An undo that failed leaves the effect as it was; it can be tried again.
    let other = exchange(s, Some(ex.conversation_id), "v").await;
    let second = done_effect(s, &other, "letter2").await;
    let failed = s
        .begin_compensation(
            second,
            "DeleteCreatedDocument".into(),
            1,
            EffectClass::ExternalIrreversible,
            json!({}),
            30,
        )
        .await
        .unwrap();
    s.finish_compensation(failed, false, Some("locked".into()), 31)
        .await
        .unwrap();
    assert_eq!(
        s.compensation_of(second).await.unwrap().unwrap().state,
        ActionState::Failed
    );
    assert!(
        s.begin_compensation(
            second,
            "DeleteCreatedDocument".into(),
            1,
            EffectClass::ExternalIrreversible,
            json!({}),
            32
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn migrating_to_v17_classes_the_actions_already_recorded() {
    let env = env();
    let path = env.dir.join("nexus.db");
    let s = &env.store;
    let ex = exchange(s, None, "m").await;
    let run = done_effect(s, &ex, "old").await;
    s.call(|c| {
        common::undo_v18(c)?;
        c.execute_batch(
            "DROP INDEX actions_by_run; DROP INDEX actions_by_compensated;
             ALTER TABLE actions DROP COLUMN compensates_action_id;
             ALTER TABLE actions DROP COLUMN effect_class;
             ALTER TABLE actions DROP COLUMN evidence;
             PRAGMA user_version = 16;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let action = s.run_detail(run).await.unwrap().action.unwrap();
    assert_eq!(
        (action.effect_class, action.evidence),
        (
            Some(EffectClass::ExternalReversible),
            Some(Evidence::Verified)
        )
    );
}
