//! Domain operations. Each public method is exactly one SQLite transaction
//! that changes state *and* records its events, so the two can never diverge
//! (`ARHITECTURA_NEXUS_RO.md` §7). Every state change goes through the
//! `nexus-core` transition tables. Time is passed in so tests control expiry.

use std::path::PathBuf;

use nexus_core::{ActionState, OperationState, RunState};
use nexus_protocol::{
    ActionId, ApprovalId, ArtifactId, ConversationId, EventId, MessageId, OperationId, ProjectId,
    RunId, TaskId,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{StorageError, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Denied,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Decision::Approved => "Approved",
            Decision::Denied => "Denied",
        }
    }

    fn parse(s: &str) -> Result<Self, StorageError> {
        match s {
            "Approved" => Ok(Decision::Approved),
            "Denied" => Ok(Decision::Denied),
            other => Err(StorageError::Corrupt(format!("unknown decision {other:?}"))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskRow {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub idempotency_key: String,
    pub request_json: String,
    pub request_sha256: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: RunId,
    pub task_id: TaskId,
    pub state: RunState,
    pub revision: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// How an effect that leaves no file ended (sizes, duration; never
    /// content).
    pub outcome_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActionRow {
    pub id: ActionId,
    pub run_id: RunId,
    pub operation_id: OperationId,
    pub tool_id: String,
    pub tool_version: i64,
    pub state: ActionState,
    pub manifest_json: String,
    pub manifest_sha256: String,
    pub revision: i64,
}

#[derive(Debug, Clone)]
pub struct ApprovalRow {
    pub id: ApprovalId,
    pub action_id: ActionId,
    pub manifest_sha256: String,
    pub expires_at_ms: i64,
    pub decision: Option<Decision>,
    pub decided_at_ms: Option<i64>,
    pub consumed_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct OperationRow {
    pub id: OperationId,
    pub action_id: ActionId,
    pub state: OperationState,
    pub attempt: i64,
    pub target_relpath: String,
    pub staging_relpath: String,
    pub expected_sha256: String,
    pub expected_size: i64,
    pub algorithm_version: i64,
}

#[derive(Debug, Clone)]
pub struct ArtifactRow {
    pub id: ArtifactId,
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub relpath: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub seq: i64,
    pub id: EventId,
    pub kind: String,
    pub entity_id: String,
    pub entity_revision: i64,
    pub payload_json: String,
    pub created_at_ms: i64,
}

/// Everything the host needs to act on or show one run.
#[derive(Debug, Clone)]
pub struct RunDetail {
    pub run: RunRow,
    pub task: TaskRow,
    pub action: Option<ActionRow>,
    pub approval: Option<ApprovalRow>,
    pub operation: Option<OperationRow>,
    pub artifact: Option<ArtifactRow>,
    /// Read from the durable event log, so it survives repeated restarts
    /// even after the run itself has moved on to NeedsReconciliation.
    pub cancellation_requested: bool,
    /// The reply that proposed this run; `None` for a note the user
    /// submitted directly.
    pub origin: Option<RunOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOrigin {
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub conversation_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    Created {
        task_id: TaskId,
        run_id: RunId,
    },
    /// Same idempotency key with the same request: the original task, no new run.
    Existing {
        task_id: TaskId,
        run_id: RunId,
    },
}

/// Fields of an action the host has prepared for approval.
#[derive(Debug, Clone)]
pub struct NewAction {
    pub tool_id: String,
    pub tool_version: i64,
    pub operation_id: OperationId,
    pub manifest_json: String,
    pub manifest_sha256: String,
    pub approval_expires_at_ms: i64,
}

/// Fields of the intent written before the side effect.
#[derive(Debug, Clone)]
pub struct NewOperation {
    pub target_relpath: String,
    pub staging_relpath: String,
    pub expected_sha256: String,
    pub expected_size: i64,
    pub algorithm_version: i64,
}

/// Why a run stopped, recorded on the run itself.
#[derive(Debug, Clone)]
pub struct RunError {
    pub code: String,
    pub message: String,
}

impl RunError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl Store {
    /// M1 has one default project; returns it, creating it on first use.
    pub async fn ensure_default_project(&self, now: i64) -> Result<ProjectId, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let existing: Option<String> = tx
                .query_row("SELECT id FROM projects ORDER BY created_at_ms LIMIT 1", [], |r| {
                    r.get(0)
                })
                .optional()?;
            let id = match existing {
                Some(id) => ProjectId(parse_uuid(&id)?),
                None => {
                    let id = ProjectId::new();
                    tx.execute(
                        "INSERT INTO projects (id, name, revision, created_at_ms) VALUES (?1, 'Nexus', 1, ?2)",
                        params![id.to_string(), now],
                    )?;
                    insert_event(&tx, "ProjectCreated", &id.to_string(), 1, json!({"name": "Nexus"}), now)?;
                    id
                }
            };
            tx.commit()?;
            Ok(id)
        })
        .await
    }

    /// RUN-03: the same key with the same request returns the original task;
    /// the same key with a different request is a conflict, never a new run.
    pub async fn submit_task(
        &self,
        project_id: ProjectId,
        idempotency_key: String,
        request_json: String,
        request_sha256: String,
        now: i64,
    ) -> Result<SubmitOutcome, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let existing = tx
                .query_row(
                    "SELECT t.id, t.request_sha256, r.id FROM tasks t JOIN runs r ON r.task_id = t.id
                     WHERE t.idempotency_key = ?1",
                    [&idempotency_key],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
                )
                .optional()?;
            if let Some((task_id, hash, run_id)) = existing {
                if hash != request_sha256 {
                    return Err(StorageError::IdempotencyConflict(idempotency_key));
                }
                return Ok(SubmitOutcome::Existing {
                    task_id: TaskId(parse_uuid(&task_id)?),
                    run_id: RunId(parse_uuid(&run_id)?),
                });
            }

            let task_id = TaskId::new();
            let run_id = RunId::new();
            tx.execute(
                "INSERT INTO tasks (id, project_id, idempotency_key, request_json, request_sha256, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![task_id.to_string(), project_id.to_string(), idempotency_key, request_json, request_sha256, now],
            )?;
            tx.execute(
                "INSERT INTO runs (id, task_id, state, revision, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 'Queued', 1, ?3, ?3)",
                params![run_id.to_string(), task_id.to_string(), now],
            )?;
            insert_event(
                &tx,
                "RunCreated",
                &run_id.to_string(),
                1,
                json!({"task_id": task_id.to_string(), "state": "Queued"}),
                now,
            )?;
            tx.commit()?;
            Ok(SubmitOutcome::Created { task_id, run_id })
        })
        .await
    }

    /// Moves the oldest queued run to Running and returns it with its context.
    pub async fn claim_next_queued_run(&self, now: i64) -> Result<Option<RunDetail>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let next: Option<String> = tx
                .query_row(
                    "SELECT id FROM runs WHERE state = 'Queued' ORDER BY created_at_ms, id LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(run_id) = next else {
                return Ok(None);
            };
            let run_id = RunId(parse_uuid(&run_id)?);
            transition_run(&tx, run_id, RunState::Running, None, now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(Some(detail))
        })
        .await
    }

    /// Records the exact action awaiting approval and its approval request.
    pub async fn prepare_action(
        &self,
        run_id: RunId,
        action: NewAction,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let action_id = ActionId::new();
            let approval_id = ApprovalId::new();
            tx.execute(
                "INSERT INTO actions (id, run_id, operation_id, tool_id, tool_version, state, manifest_json,
                                      manifest_sha256, revision, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'AwaitingApproval', ?6, ?7, 1, ?8, ?8)",
                params![
                    action_id.to_string(),
                    run_id.to_string(),
                    action.operation_id.to_string(),
                    action.tool_id,
                    action.tool_version,
                    action.manifest_json,
                    action.manifest_sha256,
                    now
                ],
            )?;
            tx.execute(
                "INSERT INTO approvals (id, action_id, manifest_sha256, expires_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![approval_id.to_string(), action_id.to_string(), action.manifest_sha256, action.approval_expires_at_ms],
            )?;
            insert_event(
                &tx,
                "ApprovalRequested",
                &approval_id.to_string(),
                1,
                json!({"run_id": run_id.to_string(), "action_id": action_id.to_string(),
                       "manifest_sha256": action.manifest_sha256, "expires_at_ms": action.approval_expires_at_ms}),
                now,
            )?;
            transition_run(&tx, run_id, RunState::AwaitingApproval, None, now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// Records the user's decision. The caller must present the hash of the
    /// manifest it previewed; any mismatch, expiry, revocation or earlier
    /// decision refuses without changing anything (SEC-01, RUN-02).
    pub async fn decide_approval(
        &self,
        approval_id: ApprovalId,
        presented_manifest_sha256: String,
        decision: Decision,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let approval = load_approval(&tx, approval_id)?;
            if approval.revoked_at_ms.is_some() {
                return Err(StorageError::ApprovalInvalidated(
                    "approval was revoked".to_string(),
                ));
            }
            if approval.decision.is_some() {
                return Err(StorageError::ApprovalAlreadyDecided);
            }
            if now >= approval.expires_at_ms {
                return Err(StorageError::ApprovalExpired);
            }
            let action = load_action(&tx, approval.action_id)?;
            if presented_manifest_sha256 != approval.manifest_sha256
                || presented_manifest_sha256 != action.manifest_sha256
            {
                return Err(StorageError::ApprovalInvalidated(
                    "the presented manifest is not the one awaiting approval".to_string(),
                ));
            }

            tx.execute(
                "UPDATE approvals SET decision = ?1, decided_at_ms = ?2 WHERE id = ?3",
                params![decision.as_str(), now, approval_id.to_string()],
            )?;
            insert_event(
                &tx,
                "ApprovalDecided",
                &approval_id.to_string(),
                2,
                json!({"decision": decision.as_str(), "action_id": action.id.to_string()}),
                now,
            )?;
            match decision {
                Decision::Approved => {
                    transition_action(&tx, action.id, ActionState::Authorized, now)?;
                    // Re-queued, not resumed: execution re-checks everything.
                    transition_run(&tx, action.run_id, RunState::Queued, None, now)?;
                }
                Decision::Denied => {
                    transition_action(&tx, action.id, ActionState::Denied, now)?;
                    transition_run(
                        &tx,
                        action.run_id,
                        RunState::Denied,
                        Some(RunError::new("Denied", "the user denied the action")),
                        now,
                    )?;
                }
            }
            let detail = load_detail(&tx, action.run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// Contract §5: in one transaction, re-check and consume the approval for
    /// this OperationId and persist the intent — before any side effect.
    pub async fn claim_effect(
        &self,
        run_id: RunId,
        operation: NewOperation,
        now: i64,
    ) -> Result<OperationRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            // Checked in the same transaction as the consumption, so a
            // cancellation that lands meanwhile can never be overtaken.
            match detail.run.state {
                RunState::Running => {}
                RunState::CancellationRequested => return Err(StorageError::CancellationRequested),
                other => {
                    return Err(StorageError::IllegalTransition(nexus_core::IllegalTransition {
                        from: other.as_str(),
                        to: "Executing",
                    }));
                }
            }
            let action = detail.action.ok_or(StorageError::NotFound("action"))?;
            let approval = detail.approval.ok_or(StorageError::NotFound("approval"))?;

            let refusal = if approval.decision != Some(Decision::Approved) {
                Some(StorageError::ApprovalInvalidated("no approved decision".to_string()))
            } else if approval.consumed_at_ms.is_some() {
                Some(StorageError::ApprovalInvalidated("approval already consumed".to_string()))
            } else if approval.revoked_at_ms.is_some() {
                Some(StorageError::ApprovalInvalidated("approval was revoked".to_string()))
            } else if approval.manifest_sha256 != action.manifest_sha256 {
                Some(StorageError::ApprovalInvalidated("manifest changed after approval".to_string()))
            } else if now >= approval.expires_at_ms {
                Some(StorageError::ApprovalExpired)
            } else {
                None
            };
            if let Some(err) = refusal {
                return Err(err);
            }

            tx.execute(
                "UPDATE approvals SET consumed_at_ms = ?1 WHERE id = ?2 AND consumed_at_ms IS NULL",
                params![now, approval.id.to_string()],
            )?;
            transition_action(&tx, action.id, ActionState::Executing, now)?;
            tx.execute(
                "INSERT INTO operations (id, action_id, state, attempt, target_relpath, staging_relpath,
                                         expected_sha256, expected_size, algorithm_version, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 'Intended', 1, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    action.operation_id.to_string(),
                    action.id.to_string(),
                    operation.target_relpath,
                    operation.staging_relpath,
                    operation.expected_sha256,
                    operation.expected_size,
                    operation.algorithm_version,
                    now
                ],
            )?;
            insert_event(
                &tx,
                "OperationIntended",
                &action.operation_id.to_string(),
                1,
                json!({"run_id": run_id.to_string(), "target_relpath": operation.target_relpath,
                       "expected_sha256": operation.expected_sha256}),
                now,
            )?;
            let op = load_operation(&tx, action.operation_id)?.ok_or(StorageError::NotFound("operation"))?;
            tx.commit()?;
            Ok(op)
        })
        .await
    }

    /// The effect is proven: record the artifact and finish the run.
    pub async fn complete_operation(
        &self,
        run_id: RunId,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            let action = detail.action.ok_or(StorageError::NotFound("action"))?;
            let op = detail.operation.ok_or(StorageError::NotFound("operation"))?;
            transition_operation(&tx, op.id, op.state, OperationState::Verified, now)?;
            let artifact_id = ArtifactId::new();
            tx.execute(
                "INSERT INTO artifacts (id, operation_id, project_id, relpath, sha256, size_bytes, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    artifact_id.to_string(),
                    op.id.to_string(),
                    detail.task.project_id.to_string(),
                    op.target_relpath,
                    op.expected_sha256,
                    op.expected_size,
                    now
                ],
            )?;
            insert_event(
                &tx,
                "ArtifactCreated",
                &artifact_id.to_string(),
                1,
                json!({"run_id": run_id.to_string(), "relpath": op.target_relpath, "sha256": op.expected_sha256}),
                now,
            )?;
            transition_action(&tx, action.id, ActionState::Verified, now)?;
            // A reconciled run already proved the effect; a live one passes
            // through Verifying as the contract's happy path requires.
            match detail.run.state {
                RunState::Verifying | RunState::NeedsReconciliation => {}
                _ => transition_run(&tx, run_id, RunState::Verifying, None, now)?,
            }
            transition_run(&tx, run_id, RunState::Succeeded, None, now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// A call to a remote tool returned: the effect happened as far as the
    /// server says, and leaves no file to verify. Only its size and
    /// duration are recorded (`outcome_json`), never what it answered.
    pub async fn complete_remote_call(
        &self,
        run_id: RunId,
        outcome_json: String,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            let action = detail.action.ok_or(StorageError::NotFound("action"))?;
            let op = detail
                .operation
                .ok_or(StorageError::NotFound("operation"))?;
            transition_operation(&tx, op.id, op.state, OperationState::Verified, now)?;
            transition_action(&tx, action.id, ActionState::Verified, now)?;
            tx.execute(
                "UPDATE runs SET outcome_json = ?1 WHERE id = ?2",
                params![outcome_json, run_id.to_string()],
            )?;
            insert_event(
                &tx,
                "RemoteCallCompleted",
                &run_id.to_string(),
                1,
                json!({"run_id": run_id.to_string()}),
                now,
            )?;
            transition_run(&tx, run_id, RunState::Verifying, None, now)?;
            transition_run(&tx, run_id, RunState::Succeeded, None, now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// The effect definitely did not happen (or the request was invalid).
    pub async fn fail_run(
        &self,
        run_id: RunId,
        error: RunError,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            if let Some(op) = &detail.operation
                && op.state == OperationState::Intended
            {
                transition_operation(&tx, op.id, op.state, OperationState::Failed, now)?;
            }
            if let Some(action) = &detail.action
                && !action.state.is_terminal()
            {
                let next = match action.state {
                    ActionState::Prepared
                    | ActionState::AwaitingApproval
                    | ActionState::Authorized => ActionState::Invalidated,
                    _ => ActionState::Failed,
                };
                transition_action(&tx, action.id, next, now)?;
            }
            transition_run(&tx, run_id, RunState::Failed, Some(error), now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// Something exists but cannot be proven to be our complete effect
    /// (RUN-06): stop and ask a human. Nothing is overwritten or retried.
    pub async fn mark_needs_review(
        &self,
        run_id: RunId,
        error: RunError,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            if let Some(op) = &detail.operation
                && op.state == OperationState::Intended
            {
                transition_operation(&tx, op.id, op.state, OperationState::NeedsReview, now)?;
            }
            if let Some(action) = &detail.action
                && action.state == ActionState::Executing
            {
                transition_action(&tx, action.id, ActionState::NeedsReconciliation, now)?;
            }
            if detail.run.state != RunState::NeedsReconciliation {
                transition_run(&tx, run_id, RunState::NeedsReconciliation, None, now)?;
            }
            transition_run(&tx, run_id, RunState::NeedsReview, Some(error), now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// Startup: runs that were mid-flight when the host stopped. They are
    /// moved to NeedsReconciliation, never straight back to Queued.
    pub async fn take_interrupted_runs(&self, now: i64) -> Result<Vec<RunDetail>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let ids = run_ids_in_states(
                &tx,
                &[
                    RunState::Running,
                    RunState::Verifying,
                    RunState::CancellationRequested,
                    RunState::NeedsReconciliation,
                ],
            )?;
            let mut details = Vec::with_capacity(ids.len());
            for run_id in ids {
                let run = load_run(&tx, run_id)?;
                if run.state != RunState::NeedsReconciliation {
                    transition_run(&tx, run_id, RunState::NeedsReconciliation, None, now)?;
                }
                details.push(load_detail(&tx, run_id)?);
            }
            tx.commit()?;
            Ok(details)
        })
        .await
    }

    /// Reconciliation proved no effect happened: run it again. With an
    /// existing intent the same OperationId is reused (attempt + 1), so the
    /// consumed approval still covers exactly this operation.
    pub async fn requeue_reconciled(
        &self,
        run_id: RunId,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            if let Some(op) = &detail.operation {
                tx.execute(
                    "UPDATE operations SET attempt = attempt + 1, updated_at_ms = ?1 WHERE id = ?2 AND state = 'Intended'",
                    params![now, op.id.to_string()],
                )?;
                insert_event(&tx, "OperationRetried", &op.id.to_string(), op.attempt + 1, json!({"attempt": op.attempt + 1}), now)?;
            }
            transition_run(&tx, run_id, RunState::Queued, None, now)?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// RUN-02: pending approvals past their expiry end their run.
    pub async fn expire_approvals(&self, now: i64) -> Result<Vec<RunId>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let expired: Vec<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT a.id, ac.run_id FROM approvals a JOIN actions ac ON ac.id = a.action_id
                     WHERE a.decision IS NULL AND a.revoked_at_ms IS NULL AND a.expires_at_ms <= ?1
                       AND ac.state = 'AwaitingApproval'",
                )?;
                stmt.query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?
            };
            let mut runs = Vec::new();
            for (approval_id, run_id) in expired {
                let run_id = RunId(parse_uuid(&run_id)?);
                let action =
                    load_action_for_run(&tx, run_id)?.ok_or(StorageError::NotFound("action"))?;
                transition_action(&tx, action.id, ActionState::Expired, now)?;
                insert_event(
                    &tx,
                    "ApprovalExpired",
                    &approval_id,
                    2,
                    json!({"run_id": run_id.to_string()}),
                    now,
                )?;
                transition_run(
                    &tx,
                    run_id,
                    RunState::Failed,
                    Some(RunError::new(
                        "ApprovalExpired",
                        "the approval expired before a decision",
                    )),
                    now,
                )?;
                runs.push(run_id);
            }
            tx.commit()?;
            Ok(runs)
        })
        .await
    }

    /// RUN-07. Queued or waiting runs stop at once and their approval is
    /// revoked; a run already executing is only *asked* to stop, because its
    /// effect may still land and must then be reported truthfully.
    pub async fn cancel_run(&self, run_id: RunId, now: i64) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            match detail.run.state {
                RunState::Queued | RunState::AwaitingApproval => {
                    if let Some(approval) = &detail.approval
                        && approval.consumed_at_ms.is_none()
                        && approval.revoked_at_ms.is_none()
                    {
                        tx.execute(
                            "UPDATE approvals SET revoked_at_ms = ?1 WHERE id = ?2",
                            params![now, approval.id.to_string()],
                        )?;
                        insert_event(
                            &tx,
                            "ApprovalRevoked",
                            &approval.id.to_string(),
                            3,
                            json!({"reason": "run cancelled"}),
                            now,
                        )?;
                    }
                    if let Some(action) = &detail.action
                        && !action.state.is_terminal()
                    {
                        transition_action(&tx, action.id, ActionState::Invalidated, now)?;
                    }
                    transition_run(
                        &tx,
                        run_id,
                        RunState::Cancelled,
                        Some(RunError::new("Cancelled", "cancelled by the user")),
                        now,
                    )?;
                }
                RunState::Running => {
                    transition_run(&tx, run_id, RunState::CancellationRequested, None, now)?;
                }
                state => {
                    return Err(StorageError::IllegalTransition(
                        nexus_core::IllegalTransition {
                            from: state.as_str(),
                            to: RunState::Cancelled.as_str(),
                        },
                    ));
                }
            }
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    /// Cancellation was requested and no effect happened — either the
    /// executor saw the request before acting, or reconciliation proved the
    /// recorded intent left nothing behind.
    pub async fn finish_cancellation(
        &self,
        run_id: RunId,
        now: i64,
    ) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let detail = load_detail(&tx, run_id)?;
            if let Some(op) = &detail.operation
                && op.state == OperationState::Intended
            {
                transition_operation(&tx, op.id, op.state, OperationState::Failed, now)?;
            }
            if let Some(action) = &detail.action
                && !action.state.is_terminal()
            {
                let next = match action.state {
                    ActionState::Executing | ActionState::NeedsReconciliation => {
                        ActionState::Failed
                    }
                    _ => ActionState::Invalidated,
                };
                transition_action(&tx, action.id, next, now)?;
            }
            transition_run(
                &tx,
                run_id,
                RunState::Cancelled,
                Some(RunError::new("Cancelled", "cancelled before any effect")),
                now,
            )?;
            let detail = load_detail(&tx, run_id)?;
            tx.commit()?;
            Ok(detail)
        })
        .await
    }

    pub async fn run_detail(&self, run_id: RunId) -> Result<RunDetail, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_detail(&tx, run_id)
        })
        .await
    }

    /// A consistent view: the runs and the event sequence they reflect, read
    /// in one transaction so a client can continue from `watermark`.
    pub async fn snapshot(&self, limit: u32) -> Result<(Vec<RunDetail>, i64), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let watermark: i64 =
                tx.query_row("SELECT coalesce(max(seq), 0) FROM events", [], |r| r.get(0))?;
            let ids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT id FROM runs ORDER BY created_at_ms DESC, id LIMIT ?1")?;
                stmt.query_map([limit], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            let mut runs = Vec::with_capacity(ids.len());
            for id in ids {
                runs.push(load_detail(&tx, RunId(parse_uuid(&id)?))?);
            }
            Ok((runs, watermark))
        })
        .await
    }

    /// EVT-01: events after `after_seq`, oldest first. Seq numbers are
    /// monotonic and unique, so a client dedupes simply by remembering the
    /// last one it applied.
    pub async fn events_since(
        &self,
        after_seq: i64,
        limit: u32,
    ) -> Result<Vec<EventRow>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, id, kind, entity_id, entity_revision, payload_json, created_at_ms
                 FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![after_seq, limit], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(
                    |(seq, id, kind, entity_id, entity_revision, payload_json, created_at_ms)| {
                        Ok(EventRow {
                            seq,
                            id: EventId(parse_uuid(&id)?),
                            kind,
                            entity_id,
                            entity_revision,
                            payload_json,
                            created_at_ms,
                        })
                    },
                )
                .collect()
        })
        .await
    }

    pub async fn artifact(&self, artifact_id: ArtifactId) -> Result<ArtifactRow, StorageError> {
        self.call(move |conn| {
            conn.query_row(
                "SELECT id, operation_id, project_id, relpath, sha256, size_bytes, created_at_ms FROM artifacts WHERE id = ?1",
                [artifact_id.to_string()],
                artifact_from_row,
            )
            .optional()?
            .ok_or(StorageError::NotFound("artifact"))?
        })
        .await
    }

    /// BACKUP-01: copies the database with SQLite's online backup API while
    /// this thread holds the only connection, then hands `during` the
    /// artifact rows *before* releasing it — no write can land between the
    /// database copy and whatever `during` copies alongside it.
    pub async fn backup_consistent<T, F>(
        &self,
        dest_db: PathBuf,
        during: F,
    ) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&[ArtifactRow]) -> Result<T, StorageError> + Send + 'static,
    {
        self.call(move |conn| {
            if dest_db.exists() {
                return Err(StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{} already exists", dest_db.display()),
                )));
            }
            {
                let mut dst = rusqlite::Connection::open(&dest_db)?;
                let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
                backup.run_to_completion(256, std::time::Duration::ZERO, None)?;
            }
            let artifacts: Vec<ArtifactRow> = {
                let mut stmt = conn.prepare(
                    "SELECT id, operation_id, project_id, relpath, sha256, size_bytes, created_at_ms
                     FROM artifacts ORDER BY created_at_ms, id",
                )?;
                stmt.query_map([], artifact_from_row)?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<Result<_, _>>()?
            };
            during(&artifacts)
        })
        .await
    }

    /// `PRAGMA integrity_check`: "ok", or SQLite's description of the damage.
    pub async fn integrity_check(&self) -> Result<String, StorageError> {
        self.call(|conn| Ok(conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?))
            .await
    }
}

pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(s).map_err(|e| StorageError::Corrupt(format!("bad id {s:?}: {e}")))
}

pub(crate) fn parse_state<T: std::str::FromStr<Err = nexus_core::UnknownState>>(
    s: &str,
) -> Result<T, StorageError> {
    s.parse::<T>()
        .map_err(|e| StorageError::Corrupt(e.to_string()))
}

pub(crate) fn insert_event(
    tx: &Transaction,
    kind: &str,
    entity_id: &str,
    entity_revision: i64,
    payload: Value,
    now: i64,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO events (id, kind, entity_id, entity_revision, payload_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            EventId::new().to_string(),
            kind,
            entity_id,
            entity_revision,
            payload.to_string(),
            now
        ],
    )?;
    Ok(())
}

fn transition_run(
    tx: &Transaction,
    run_id: RunId,
    to: RunState,
    error: Option<RunError>,
    now: i64,
) -> Result<(), StorageError> {
    let run = load_run(tx, run_id)?;
    run.state.transition(to)?;
    let revision = run.revision + 1;
    let (code, message) = match &error {
        Some(e) => (Some(e.code.as_str()), Some(e.message.as_str())),
        None => (None, None),
    };
    let changed = tx.execute(
        "UPDATE runs SET state = ?1, revision = ?2, error_code = ?3, error_message = ?4, updated_at_ms = ?5
         WHERE id = ?6 AND revision = ?7",
        params![to.as_str(), revision, code, message, now, run_id.to_string(), run.revision],
    )?;
    if changed != 1 {
        return Err(StorageError::RevisionConflict("run"));
    }
    insert_event(
        tx,
        "RunStateChanged",
        &run_id.to_string(),
        revision,
        json!({"from": run.state.as_str(), "to": to.as_str(), "error_code": code}),
        now,
    )
}

fn transition_action(
    tx: &Transaction,
    action_id: ActionId,
    to: ActionState,
    now: i64,
) -> Result<(), StorageError> {
    let action = load_action(tx, action_id)?;
    action.state.transition(to)?;
    let revision = action.revision + 1;
    let changed = tx.execute(
        "UPDATE actions SET state = ?1, revision = ?2, updated_at_ms = ?3 WHERE id = ?4 AND revision = ?5",
        params![to.as_str(), revision, now, action_id.to_string(), action.revision],
    )?;
    if changed != 1 {
        return Err(StorageError::RevisionConflict("action"));
    }
    insert_event(
        tx,
        "ActionStateChanged",
        &action_id.to_string(),
        revision,
        json!({"from": action.state.as_str(), "to": to.as_str()}),
        now,
    )
}

fn transition_operation(
    tx: &Transaction,
    op_id: OperationId,
    from: OperationState,
    to: OperationState,
    now: i64,
) -> Result<(), StorageError> {
    if !from.can_transition_to(to) {
        return Err(StorageError::IllegalTransition(
            nexus_core::IllegalTransition {
                from: from.as_str(),
                to: to.as_str(),
            },
        ));
    }
    let changed = tx.execute(
        "UPDATE operations SET state = ?1, updated_at_ms = ?2 WHERE id = ?3 AND state = ?4",
        params![to.as_str(), now, op_id.to_string(), from.as_str()],
    )?;
    if changed != 1 {
        return Err(StorageError::RevisionConflict("operation"));
    }
    insert_event(
        tx,
        "OperationStateChanged",
        &op_id.to_string(),
        0,
        json!({"from": from.as_str(), "to": to.as_str()}),
        now,
    )
}

fn run_ids_in_states(tx: &Transaction, states: &[RunState]) -> Result<Vec<RunId>, StorageError> {
    let mut ids = Vec::new();
    let mut stmt = tx.prepare("SELECT id FROM runs WHERE state = ?1 ORDER BY created_at_ms, id")?;
    for state in states {
        for id in stmt.query_map([state.as_str()], |r| r.get::<_, String>(0))? {
            ids.push(RunId(parse_uuid(&id?)?));
        }
    }
    Ok(ids)
}

fn load_run(tx: &Transaction, run_id: RunId) -> Result<RunRow, StorageError> {
    let row = tx
        .query_row(
            "SELECT id, task_id, state, revision, error_code, error_message, created_at_ms, updated_at_ms,
                    outcome_json
             FROM runs WHERE id = ?1",
            [run_id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or(StorageError::NotFound("run"))?;
    Ok(RunRow {
        id: RunId(parse_uuid(&row.0)?),
        task_id: TaskId(parse_uuid(&row.1)?),
        state: parse_state(&row.2)?,
        revision: row.3,
        error_code: row.4,
        error_message: row.5,
        created_at_ms: row.6,
        updated_at_ms: row.7,
        outcome_json: row.8,
    })
}

fn load_task(tx: &Transaction, task_id: TaskId) -> Result<TaskRow, StorageError> {
    let row = tx
        .query_row(
            "SELECT id, project_id, idempotency_key, request_json, request_sha256, created_at_ms FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?
        .ok_or(StorageError::NotFound("task"))?;
    Ok(TaskRow {
        id: TaskId(parse_uuid(&row.0)?),
        project_id: ProjectId(parse_uuid(&row.1)?),
        idempotency_key: row.2,
        request_json: row.3,
        request_sha256: row.4,
        created_at_ms: row.5,
    })
}

const ACTION_COLUMNS: &str = "id, run_id, operation_id, tool_id, tool_version, state, manifest_json, manifest_sha256, revision";

fn action_from_row(r: &rusqlite::Row) -> rusqlite::Result<Result<ActionRow, StorageError>> {
    let id: String = r.get(0)?;
    let run_id: String = r.get(1)?;
    let op: String = r.get(2)?;
    let state: String = r.get(5)?;
    let (tool_id, tool_version, manifest_json, manifest_sha256, revision) =
        (r.get(3)?, r.get(4)?, r.get(6)?, r.get(7)?, r.get(8)?);
    Ok((|| {
        Ok(ActionRow {
            id: ActionId(parse_uuid(&id)?),
            run_id: RunId(parse_uuid(&run_id)?),
            operation_id: OperationId(parse_uuid(&op)?),
            tool_id,
            tool_version,
            state: parse_state(&state)?,
            manifest_json,
            manifest_sha256,
            revision,
        })
    })())
}

fn load_action(tx: &Transaction, action_id: ActionId) -> Result<ActionRow, StorageError> {
    tx.query_row(
        &format!("SELECT {ACTION_COLUMNS} FROM actions WHERE id = ?1"),
        [action_id.to_string()],
        action_from_row,
    )
    .optional()?
    .ok_or(StorageError::NotFound("action"))?
}

fn load_action_for_run(tx: &Transaction, run_id: RunId) -> Result<Option<ActionRow>, StorageError> {
    tx.query_row(
        &format!("SELECT {ACTION_COLUMNS} FROM actions WHERE run_id = ?1"),
        [run_id.to_string()],
        action_from_row,
    )
    .optional()?
    .transpose()
}

fn approval_from_row(r: &rusqlite::Row) -> rusqlite::Result<Result<ApprovalRow, StorageError>> {
    let id: String = r.get(0)?;
    let action_id: String = r.get(1)?;
    let decision: Option<String> = r.get(4)?;
    let (manifest_sha256, expires_at_ms, decided_at_ms, consumed_at_ms, revoked_at_ms) =
        (r.get(2)?, r.get(3)?, r.get(5)?, r.get(6)?, r.get(7)?);
    Ok((|| {
        Ok(ApprovalRow {
            id: ApprovalId(parse_uuid(&id)?),
            action_id: ActionId(parse_uuid(&action_id)?),
            manifest_sha256,
            expires_at_ms,
            decision: decision.as_deref().map(Decision::parse).transpose()?,
            decided_at_ms,
            consumed_at_ms,
            revoked_at_ms,
        })
    })())
}

const APPROVAL_COLUMNS: &str = "id, action_id, manifest_sha256, expires_at_ms, decision, decided_at_ms, consumed_at_ms, revoked_at_ms";

fn load_approval(tx: &Transaction, approval_id: ApprovalId) -> Result<ApprovalRow, StorageError> {
    tx.query_row(
        &format!("SELECT {APPROVAL_COLUMNS} FROM approvals WHERE id = ?1"),
        [approval_id.to_string()],
        approval_from_row,
    )
    .optional()?
    .ok_or(StorageError::NotFound("approval"))?
}

fn load_approval_for_action(
    tx: &Transaction,
    action_id: ActionId,
) -> Result<Option<ApprovalRow>, StorageError> {
    tx.query_row(
        &format!("SELECT {APPROVAL_COLUMNS} FROM approvals WHERE action_id = ?1"),
        [action_id.to_string()],
        approval_from_row,
    )
    .optional()?
    .transpose()
}

fn load_operation(
    tx: &Transaction,
    op_id: OperationId,
) -> Result<Option<OperationRow>, StorageError> {
    tx.query_row(
        "SELECT id, action_id, state, attempt, target_relpath, staging_relpath, expected_sha256,
                expected_size, algorithm_version FROM operations WHERE id = ?1",
        [op_id.to_string()],
        |r| {
            let id: String = r.get(0)?;
            let action_id: String = r.get(1)?;
            let state: String = r.get(2)?;
            let rest = (
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            );
            Ok((|| {
                Ok(OperationRow {
                    id: OperationId(parse_uuid(&id)?),
                    action_id: ActionId(parse_uuid(&action_id)?),
                    state: parse_state(&state)?,
                    attempt: rest.0,
                    target_relpath: rest.1,
                    staging_relpath: rest.2,
                    expected_sha256: rest.3,
                    expected_size: rest.4,
                    algorithm_version: rest.5,
                })
            })())
        },
    )
    .optional()?
    .transpose()
}

fn artifact_from_row(r: &rusqlite::Row) -> rusqlite::Result<Result<ArtifactRow, StorageError>> {
    let id: String = r.get(0)?;
    let op: String = r.get(1)?;
    let project: String = r.get(2)?;
    let (relpath, sha256, size_bytes, created_at_ms) = (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?);
    Ok((|| {
        Ok(ArtifactRow {
            id: ArtifactId(parse_uuid(&id)?),
            operation_id: OperationId(parse_uuid(&op)?),
            project_id: ProjectId(parse_uuid(&project)?),
            relpath,
            sha256,
            size_bytes,
            created_at_ms,
        })
    })())
}

fn load_artifact_for_operation(
    tx: &Transaction,
    op_id: OperationId,
) -> Result<Option<ArtifactRow>, StorageError> {
    tx.query_row(
        "SELECT id, operation_id, project_id, relpath, sha256, size_bytes, created_at_ms FROM artifacts WHERE operation_id = ?1",
        [op_id.to_string()],
        artifact_from_row,
    )
    .optional()?
    .transpose()
}

fn load_detail(tx: &Transaction, run_id: RunId) -> Result<RunDetail, StorageError> {
    let run = load_run(tx, run_id)?;
    let task = load_task(tx, run.task_id)?;
    let action = load_action_for_run(tx, run_id)?;
    let (approval, operation) = match &action {
        Some(a) => (
            load_approval_for_action(tx, a.id)?,
            load_operation(tx, a.operation_id)?,
        ),
        None => (None, None),
    };
    let artifact = match &operation {
        Some(op) => load_artifact_for_operation(tx, op.id)?,
        None => None,
    };
    let cancellation_requested = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM events WHERE kind = 'RunStateChanged' AND entity_id = ?1
                        AND json_extract(payload_json, '$.to') = 'CancellationRequested')",
        [run_id.to_string()],
        |r| r.get(0),
    )?;
    let origin = load_origin(tx, run_id)?;
    Ok(RunDetail {
        run,
        task,
        action,
        approval,
        operation,
        artifact,
        cancellation_requested,
        origin,
    })
}

/// A settled reply records the run it proposed; while it is still being
/// settled, its NoteProposed step already names the run.
fn load_origin(tx: &Transaction, run_id: RunId) -> Result<Option<RunOrigin>, StorageError> {
    let raw: Option<(String, String, String)> = tx
        .query_row(
            "SELECT m.id, m.conversation_id, c.title FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.proposed_run_id = ?1
                OR m.id IN (SELECT message_id FROM agent_steps WHERE kind = 'NoteProposed'
                            AND json_extract(detail_json, '$.run_id') = ?1)
             ORDER BY m.seq LIMIT 1",
            [run_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    raw.map(|(message, conversation, title)| {
        Ok(RunOrigin {
            message_id: MessageId(parse_uuid(&message)?),
            conversation_id: ConversationId(parse_uuid(&conversation)?),
            conversation_title: title,
        })
    })
    .transpose()
}
