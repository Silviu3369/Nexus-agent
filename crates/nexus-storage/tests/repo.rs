//! Repository transactions on a real SQLite file: idempotency, approval
//! binding, expiry, replay, cancellation and restart handling.

use std::path::PathBuf;

use nexus_core::{ActionState, OperationState, RunState};
use nexus_protocol::{OperationId, ProjectId, RunId};
use nexus_storage::{
    Decision, NewAction, NewOperation, RunDetail, StorageError, Store, SubmitOutcome,
};

const T0: i64 = 1_000_000;
const TTL: i64 = 60_000;

struct Env {
    store: Store,
    project: ProjectId,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-repo-test-{}", uuid::Uuid::new_v4()));
    let store = Store::open(&dir.join("nexus.db")).unwrap();
    let project = store.ensure_default_project(T0).await.unwrap();
    Env {
        store,
        project,
        dir,
    }
}

async fn submit(env: &Env, key: &str, hash: &str) -> Result<SubmitOutcome, StorageError> {
    env.store
        .submit_task(
            nexus_storage::EffectRequester::User,
            env.project,
            key.to_string(),
            r#"{"title":"t"}"#.to_string(),
            hash.to_string(),
            T0,
        )
        .await
}

fn action(manifest_sha256: &str) -> NewAction {
    NewAction {
        effect_class: nexus_core::EffectClass::InternalReversible,
        evidence: nexus_core::Evidence::Verified,
        tool_id: "CreateMarkdownArtifact".to_string(),
        tool_version: 1,
        operation_id: OperationId::new(),
        manifest_json: r#"{"k":"v"}"#.to_string(),
        manifest_sha256: manifest_sha256.to_string(),
        approval_expires_at_ms: T0 + TTL,
    }
}

fn operation(target: &str) -> NewOperation {
    NewOperation {
        target_relpath: target.to_string(),
        staging_relpath: format!("{target}.tmp"),
        expected_sha256: "abc".to_string(),
        expected_size: 3,
        algorithm_version: 1,
    }
}

/// Submit → claim → prepare: returns a run waiting for approval.
async fn awaiting_approval(env: &Env, key: &str) -> RunDetail {
    submit(env, key, "req").await.unwrap();
    let claimed = env.store.claim_next_queued_run(T0).await.unwrap().unwrap();
    assert_eq!(claimed.run.state, RunState::Running);
    env.store
        .prepare_action(claimed.run.id, action("m1"), T0)
        .await
        .unwrap()
}

fn approval_id(detail: &RunDetail) -> nexus_protocol::ApprovalId {
    detail.approval.as_ref().unwrap().id
}

async fn run_state(env: &Env, run: RunId) -> RunState {
    env.store.run_detail(run).await.unwrap().run.state
}

#[tokio::test]
async fn run03_same_key_same_request_returns_the_original_run() {
    let env = env().await;
    let first = submit(&env, "k", "req").await.unwrap();
    let again = submit(&env, "k", "req").await.unwrap();

    let SubmitOutcome::Created { task_id, run_id } = first else {
        panic!("first submit must create");
    };
    assert_eq!(again, SubmitOutcome::Existing { task_id, run_id });
    let (runs, _) = env.store.snapshot(10).await.unwrap();
    assert_eq!(
        runs.len(),
        1,
        "a repeated request must not create a second run"
    );
}

#[tokio::test]
async fn run03_same_key_different_request_is_a_conflict() {
    let env = env().await;
    submit(&env, "k", "req-a").await.unwrap();
    match submit(&env, "k", "req-b").await {
        Err(StorageError::IdempotencyConflict(key)) => assert_eq!(key, "k"),
        other => panic!("expected IdempotencyConflict, got {other:?}"),
    }
    let (runs, _) = env.store.snapshot(10).await.unwrap();
    assert_eq!(runs.len(), 1);
}

#[tokio::test]
async fn run01_full_lifecycle_records_every_step_with_events() {
    let env = env().await;
    let waiting = awaiting_approval(&env, "k").await;
    let run = waiting.run.id;
    assert_eq!(waiting.run.state, RunState::AwaitingApproval);
    assert_eq!(
        waiting.action.as_ref().unwrap().state,
        ActionState::AwaitingApproval
    );

    let decided = env
        .store
        .decide_approval(
            approval_id(&waiting),
            "m1".to_string(),
            Decision::Approved,
            T0 + 1,
        )
        .await
        .unwrap();
    assert_eq!(
        decided.run.state,
        RunState::Queued,
        "approval re-queues, it never resumes"
    );
    assert_eq!(
        decided.action.as_ref().unwrap().state,
        ActionState::Authorized
    );

    env.store
        .claim_next_queued_run(T0 + 2)
        .await
        .unwrap()
        .unwrap();
    let op = env
        .store
        .claim_effect(run, operation("p/a.md"), T0 + 3)
        .await
        .unwrap();
    assert_eq!(op.state, OperationState::Intended);

    let done = env.store.complete_operation(run, T0 + 4).await.unwrap();
    assert_eq!(done.run.state, RunState::Succeeded);
    assert_eq!(done.action.unwrap().state, ActionState::Verified);
    assert_eq!(done.operation.unwrap().state, OperationState::Verified);
    assert_eq!(done.approval.unwrap().consumed_at_ms, Some(T0 + 3));
    assert_eq!(done.artifact.unwrap().relpath, "p/a.md");

    let events = env.store.events_since(0, 1000).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    for expected in [
        "RunCreated",
        "ApprovalRequested",
        "ApprovalDecided",
        "OperationIntended",
        "ArtifactCreated",
    ] {
        assert!(
            kinds.contains(&expected),
            "missing event {expected}: {kinds:?}"
        );
    }
    assert!(
        events.windows(2).all(|w| w[0].seq < w[1].seq),
        "seq must be strictly increasing"
    );
}

#[tokio::test]
async fn sec01_decision_for_a_different_manifest_is_refused_without_effect() {
    let env = env().await;
    let waiting = awaiting_approval(&env, "k").await;

    let result = env
        .store
        .decide_approval(
            approval_id(&waiting),
            "tampered".to_string(),
            Decision::Approved,
            T0 + 1,
        )
        .await;
    assert!(
        matches!(result, Err(StorageError::ApprovalInvalidated(_))),
        "{result:?}"
    );

    let after = env.store.run_detail(waiting.run.id).await.unwrap();
    assert_eq!(after.run.state, RunState::AwaitingApproval);
    assert!(
        after.approval.unwrap().decision.is_none(),
        "nothing may be recorded"
    );
}

#[tokio::test]
async fn sec01_approval_replay_and_double_consumption_are_refused() {
    let env = env().await;
    let waiting = awaiting_approval(&env, "k").await;
    let run = waiting.run.id;
    let approval = approval_id(&waiting);

    env.store
        .decide_approval(approval, "m1".to_string(), Decision::Approved, T0 + 1)
        .await
        .unwrap();
    let replay = env
        .store
        .decide_approval(approval, "m1".to_string(), Decision::Approved, T0 + 2)
        .await;
    assert!(
        matches!(replay, Err(StorageError::ApprovalAlreadyDecided)),
        "{replay:?}"
    );

    env.store
        .claim_next_queued_run(T0 + 2)
        .await
        .unwrap()
        .unwrap();
    env.store
        .claim_effect(run, operation("p/a.md"), T0 + 3)
        .await
        .unwrap();
    let second = env
        .store
        .claim_effect(run, operation("p/b.md"), T0 + 4)
        .await;
    assert!(
        second.is_err(),
        "a consumed approval must not authorise a second effect"
    );

    let ops: i64 = env
        .store
        .call(|c| Ok(c.query_row("SELECT count(*) FROM operations", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(ops, 1);
}

#[tokio::test]
async fn run02_expired_approval_cannot_be_decided_and_fails_the_run() {
    let env = env().await;
    let waiting = awaiting_approval(&env, "k").await;
    let late = T0 + TTL;

    let result = env
        .store
        .decide_approval(
            approval_id(&waiting),
            "m1".to_string(),
            Decision::Approved,
            late,
        )
        .await;
    assert!(
        matches!(result, Err(StorageError::ApprovalExpired)),
        "{result:?}"
    );

    let expired = env.store.expire_approvals(late).await.unwrap();
    assert_eq!(expired, vec![waiting.run.id]);
    let after = env.store.run_detail(waiting.run.id).await.unwrap();
    assert_eq!(after.run.state, RunState::Failed);
    assert_eq!(after.run.error_code.as_deref(), Some("ApprovalExpired"));
    assert_eq!(after.action.unwrap().state, ActionState::Expired);
    assert!(
        after.operation.is_none(),
        "no intent, so no effect, may exist"
    );
}

#[tokio::test]
async fn run02_denied_action_ends_the_run_with_no_operation() {
    let env = env().await;
    let waiting = awaiting_approval(&env, "k").await;

    let denied = env
        .store
        .decide_approval(
            approval_id(&waiting),
            "m1".to_string(),
            Decision::Denied,
            T0 + 1,
        )
        .await
        .unwrap();
    assert_eq!(denied.run.state, RunState::Denied);
    assert_eq!(denied.action.unwrap().state, ActionState::Denied);
    assert!(
        env.store
            .claim_effect(waiting.run.id, operation("p/a.md"), T0 + 2)
            .await
            .is_err()
    );
    assert!(
        env.store
            .run_detail(waiting.run.id)
            .await
            .unwrap()
            .operation
            .is_none()
    );
}

#[tokio::test]
async fn run07_cancellation_in_each_state() {
    let env = env().await;

    // Queued: stops immediately.
    let SubmitOutcome::Created { run_id: queued, .. } = submit(&env, "q", "r").await.unwrap()
    else {
        panic!()
    };
    env.store.cancel_run(queued, T0).await.unwrap();
    assert_eq!(run_state(&env, queued).await, RunState::Cancelled);

    // Awaiting approval: stops and the approval can no longer be used.
    let waiting = awaiting_approval(&env, "w").await;
    env.store.cancel_run(waiting.run.id, T0 + 1).await.unwrap();
    assert_eq!(run_state(&env, waiting.run.id).await, RunState::Cancelled);
    let late = env
        .store
        .decide_approval(
            approval_id(&waiting),
            "m1".to_string(),
            Decision::Approved,
            T0 + 2,
        )
        .await;
    assert!(late.is_err(), "a cancelled run's approval must be unusable");

    // Running: only a request — the effect may already be in flight.
    submit(&env, "r", "r").await.unwrap();
    let running = env
        .store
        .claim_next_queued_run(T0 + 3)
        .await
        .unwrap()
        .unwrap();
    let requested = env.store.cancel_run(running.run.id, T0 + 4).await.unwrap();
    assert_eq!(requested.run.state, RunState::CancellationRequested);

    // Terminal: refused.
    assert!(env.store.cancel_run(queued, T0 + 5).await.is_err());
}

#[tokio::test]
async fn run04_interrupted_run_goes_to_reconciliation_then_retries_same_operation() {
    let env = env().await;
    let waiting = awaiting_approval(&env, "k").await;
    let run = waiting.run.id;
    env.store
        .decide_approval(
            approval_id(&waiting),
            "m1".to_string(),
            Decision::Approved,
            T0 + 1,
        )
        .await
        .unwrap();
    env.store
        .claim_next_queued_run(T0 + 2)
        .await
        .unwrap()
        .unwrap();
    let op = env
        .store
        .claim_effect(run, operation("p/a.md"), T0 + 3)
        .await
        .unwrap();

    // "Restart": the run was mid-flight.
    let interrupted = env.store.take_interrupted_runs(T0 + 10).await.unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].run.state, RunState::NeedsReconciliation);

    let requeued = env.store.requeue_reconciled(run, T0 + 11).await.unwrap();
    assert_eq!(requeued.run.state, RunState::Queued);
    let retried = requeued.operation.unwrap();
    assert_eq!(retried.id, op.id, "the same OperationId is reused");
    assert_eq!(retried.attempt, 2);
    assert_eq!(retried.state, OperationState::Intended);
}

#[tokio::test]
async fn run07_cancellation_requested_before_a_crash_is_honoured_after_restart() {
    let env = env().await;
    submit(&env, "k", "r").await.unwrap();
    let running = env.store.claim_next_queued_run(T0).await.unwrap().unwrap();
    env.store.cancel_run(running.run.id, T0 + 1).await.unwrap();

    // Crash, restart: the run is moved to reconciliation...
    let interrupted = env.store.take_interrupted_runs(T0 + 10).await.unwrap();
    assert_eq!(interrupted[0].run.state, RunState::NeedsReconciliation);
    // ...but the durable event log still says cancellation was requested,
    // and a second restart must not forget it either.
    assert!(interrupted[0].cancellation_requested);
    let again = env.store.take_interrupted_runs(T0 + 20).await.unwrap();
    assert!(again[0].cancellation_requested);

    let done = env
        .store
        .finish_cancellation(running.run.id, T0 + 21)
        .await
        .unwrap();
    assert_eq!(done.run.state, RunState::Cancelled);
}

#[tokio::test]
async fn evt01_cursor_returns_only_newer_events_and_snapshot_watermark_matches() {
    let env = env().await;
    awaiting_approval(&env, "a").await;
    let (_, watermark) = env.store.snapshot(10).await.unwrap();
    let all = env.store.events_since(0, 1000).await.unwrap();
    assert_eq!(all.last().unwrap().seq, watermark);

    submit(&env, "b", "req").await.unwrap();
    let newer = env.store.events_since(watermark, 1000).await.unwrap();
    assert!(!newer.is_empty());
    assert!(newer.iter().all(|e| e.seq > watermark));
    assert_eq!(newer[0].kind, "RunCreated");
}
