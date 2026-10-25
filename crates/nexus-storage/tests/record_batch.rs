//! A batch of the agent's register changes the user approved
//! (ADR-POLICY-001) on a real SQLite file: written in one transaction with
//! the end of its run, each change in the record's history and an action of
//! the turn the policy asked for; a record that changed since the user saw
//! it refuses the whole batch, and nothing is written.

use std::path::PathBuf;

use nexus_core::{MessageStatus, PolicyDecision, RecordActor, RunState};
use nexus_protocol::{OperationId, RegisterId, RunId};
use nexus_storage::{
    AgentOrigin, BatchWrite, Decision, EffectRequester, Exchange, NewAction, NewExchange,
    NewOperation, Pause, Requester, Settlement, StorageError, Store, SubmitOutcome, Values,
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
    let dir = std::env::temp_dir().join(format!("nexus-batch-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

async fn exchange(store: &Store, key: &str) -> Exchange {
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    let connection = match store.connections().await.unwrap().first() {
        Some(c) => c.id,
        None => {
            store
                .save_connection(None, "Gemini", "g".into(), false, "gemini-b".into(), 1)
                .await
                .unwrap()
                .id
        }
    };
    store
        .append_exchange(
            NewExchange {
                conversation_id: None,
                project_id: None,
                idempotency_key: key.into(),
                text: format!("request {key}"),
                connection_id: connection,
                model: "gemini-test".into(),
                persona_version: 1,
                title: None,
                requester: Requester::User,
                pack: None,
            },
            2,
        )
        .await
        .unwrap()
}

async fn register(store: &Store) -> RegisterId {
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

/// The batch the reply `ex` stopped on, approved and claimed, as the
/// executor does before writing.
async fn claimed_batch(s: &Store, ex: &Exchange, key: &str) -> RunId {
    let project = s.ensure_default_project(0).await.unwrap();
    let run = match s
        .submit_task(
            EffectRequester::Agent,
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
    while s.claim_next_queued_run(5).await.unwrap().unwrap().run.id != run {}
    s.prepare_action(
        run,
        NewAction {
            tool_id: "RecordWriteBatch".into(),
            tool_version: 1,
            effect_class: nexus_core::EffectClass::InternalReversible,
            evidence: nexus_core::Evidence::Verified,
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
    let approval = s.run_detail(run).await.unwrap().approval.unwrap();
    s.decide_approval(
        approval.id,
        approval.manifest_sha256,
        Decision::Approved,
        nexus_core::DecisionPath::Window,
        7,
    )
    .await
    .unwrap();
    while s.claim_next_queued_run(8).await.unwrap().unwrap().run.id != run {}
    s.claim_effect(
        run,
        NewOperation {
            target_relpath: format!("records:{run}"),
            staging_relpath: format!("records:{run}:staging"),
            expected_sha256: "abc".into(),
            expected_size: 3,
            algorithm_version: 1,
        },
        9,
    )
    .await
    .unwrap();
    run
}

fn origin(ex: &Exchange) -> AgentOrigin {
    AgentOrigin {
        conversation_id: ex.conversation_id,
        message_id: ex.model_message.id,
        read_hosts: Vec::new(),
        policy: PolicyDecision::Ask,
    }
}

#[tokio::test]
async fn an_approved_batch_is_written_at_once_with_the_end_of_its_run() {
    let env = env();
    let s = &env.store;
    let reg = register(s).await;
    let mine = s
        .add_record(
            reg,
            values(&[("company", "Beta"), ("role", "Ops")]),
            RecordActor::User,
            None,
            1,
        )
        .await
        .unwrap();
    let ex = exchange(s, "batch").await;
    let run = claimed_batch(s, &ex, "batch").await;

    let ids = s
        .apply_record_batch(
            run,
            vec![
                BatchWrite {
                    register_id: reg,
                    record_id: None,
                    expected_revision: None,
                    values: values(&[("company", "Acme"), ("role", "IT")]),
                },
                BatchWrite {
                    register_id: reg,
                    record_id: Some(mine.record.id),
                    expected_revision: Some(1),
                    values: values(&[("company", "Beta"), ("role", "Ops"), ("status", "Applied")]),
                },
            ],
            origin(&ex),
            10,
        )
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[1], mine.record.id);
    let added = s.record(ids[0]).await.unwrap();
    assert_eq!(added.created_by, RecordActor::Agent);
    let changed = s.record(mine.record.id).await.unwrap();
    assert_eq!(changed.values["status"], "Applied");
    assert_eq!(changed.revision, 2);

    let detail = s.run_detail(run).await.unwrap();
    assert_eq!(detail.run.state, RunState::Succeeded);
    let outcome: serde_json::Value =
        serde_json::from_str(detail.run.outcome_json.as_deref().unwrap()).unwrap();
    assert_eq!(outcome["records"].as_array().unwrap().len(), 2);

    // Each change is an action of the turn the policy asked for, and can
    // be undone on its own.
    let turn = ex.model_message.run_id.unwrap();
    let writes: Vec<_> = s
        .turn_actions(turn)
        .await
        .unwrap()
        .into_iter()
        .filter(|a| a.tool_id == "RecordWrite")
        .map(|a| a.policy_decision)
        .collect();
    assert_eq!(
        writes,
        [Some(PolicyDecision::Ask), Some(PolicyDecision::Ask)]
    );
    let history = s.record_history(mine.record.id).await.unwrap();
    s.undo_record_change(history[0].id, 11).await.unwrap();
    assert_eq!(
        s.record(mine.record.id).await.unwrap().values.get("status"),
        None
    );
}

#[tokio::test]
async fn a_record_changed_since_the_user_saw_it_refuses_the_whole_batch() {
    let env = env();
    let s = &env.store;
    let reg = register(s).await;
    let mine = s
        .add_record(
            reg,
            values(&[("company", "Beta"), ("role", "Ops")]),
            RecordActor::User,
            None,
            1,
        )
        .await
        .unwrap();
    let ex = exchange(s, "stale").await;
    let run = claimed_batch(s, &ex, "stale").await;
    // The user edits the record after approving the batch.
    s.update_record(
        mine.record.id,
        Some(1),
        values(&[("company", "Beta"), ("role", "Ops"), ("notes", "called")]),
        RecordActor::User,
        None,
        9,
    )
    .await
    .unwrap();

    let refused = s
        .apply_record_batch(
            run,
            vec![
                BatchWrite {
                    register_id: reg,
                    record_id: None,
                    expected_revision: None,
                    values: values(&[("company", "Acme"), ("role", "IT")]),
                },
                BatchWrite {
                    register_id: reg,
                    record_id: Some(mine.record.id),
                    expected_revision: Some(1),
                    values: values(&[("company", "Beta"), ("role", "Ops"), ("status", "Applied")]),
                },
            ],
            origin(&ex),
            10,
        )
        .await;
    assert!(
        matches!(refused, Err(StorageError::RevisionConflict(_))),
        "{refused:?}"
    );
    let (all, _) = s
        .records(reg, nexus_storage::RecordFilter::default())
        .await
        .unwrap();
    assert_eq!(
        all.len(),
        1,
        "the new record of the batch was not written either"
    );
    assert_eq!(
        all[0].values.get("notes").map(String::as_str),
        Some("called")
    );
    assert_ne!(
        s.run_detail(run).await.unwrap().run.state,
        RunState::Succeeded
    );
}
