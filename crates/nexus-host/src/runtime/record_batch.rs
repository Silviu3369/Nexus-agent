//! The agent's register changes beyond what one reply makes without asking
//! (ADR-POLICY-001): gathered into one batch the user decides together, each
//! change shown exactly, before → after. The batch is an effect run of its
//! turn, and its manifest covers every change. Once approved, every change
//! is written in one transaction with the end of the run, or none is: a
//! record that changed since the user saw it refuses the whole batch. The
//! turn then goes on with each change's outcome.

use nexus_core::RunState;
use nexus_protocol::{
    ConversationId, MessageId, NexusError, OperationId, RecordChangePreview, RecordId, RegisterId,
    RunId,
};
use nexus_storage::{
    AgentOrigin, BatchWrite, EffectRequester, NewOperation, RunDetail, RunError, StorageError,
    SubmitOutcome, Values, now_ms,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::records::diff;
use super::{Runtime, to_nexus_error};
use crate::fault;
use crate::tools::markdown_artifact::sha256_hex;
use crate::tools::registry::{self, RECORD_BATCH_ID};

/// `kind` of a stored batch.
pub(crate) const KIND: &str = "record-batch";

/// One change, as the user approves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BatchItem {
    pub register_id: String,
    pub register_key: String,
    pub register_name: String,
    /// `None`: a new record.
    pub record_id: Option<String>,
    /// The revision the user sees (for a change to a record).
    pub expected_revision: Option<i64>,
    pub title: String,
    pub before: Option<Values>,
    pub after: Values,
}

/// The batch, as stored with its task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BatchRequest {
    pub kind: String,
    /// The reply that asked: the changes are the agent's, in its turn.
    pub conversation_id: String,
    pub message_id: String,
    pub read_hosts: Vec<String>,
    pub items: Vec<BatchItem>,
}

impl BatchRequest {
    pub fn new(
        conversation: ConversationId,
        reply: MessageId,
        read_hosts: &[String],
        items: Vec<BatchItem>,
    ) -> Self {
        Self {
            kind: KIND.to_string(),
            conversation_id: conversation.to_string(),
            message_id: reply.to_string(),
            read_hosts: read_hosts.to_vec(),
            items,
        }
    }

    /// What the manifest covers: every change, in order.
    fn content(&self) -> String {
        serde_json::to_string(&self.items).expect("plain values serialise")
    }

    /// "3 changes in Job applications".
    pub fn title(&self) -> String {
        let mut registers: Vec<&str> = self
            .items
            .iter()
            .map(|i| i.register_name.as_str())
            .collect();
        registers.dedup();
        let count = self.items.len();
        let changes = if count == 1 { "change" } else { "changes" };
        format!("{count} {changes} in {}", registers.join(", "))
    }

    fn origin(&self) -> Result<AgentOrigin, String> {
        let id = |s: &str| uuid::Uuid::parse_str(s).map_err(|_| "an id is unreadable".to_string());
        Ok(AgentOrigin {
            conversation_id: ConversationId(id(&self.conversation_id)?),
            message_id: MessageId(id(&self.message_id)?),
            read_hosts: self.read_hosts.clone(),
            policy: nexus_core::PolicyDecision::Ask,
        })
    }

    fn writes(&self) -> Result<Vec<BatchWrite>, String> {
        let id = |s: &str| uuid::Uuid::parse_str(s).map_err(|_| "an id is unreadable".to_string());
        self.items
            .iter()
            .map(|i| {
                Ok(BatchWrite {
                    register_id: RegisterId(id(&i.register_id)?),
                    record_id: i.record_id.as_deref().map(id).transpose()?.map(RecordId),
                    expected_revision: i.expected_revision,
                    values: i.after.clone(),
                })
            })
            .collect()
    }
}

/// The run's batch, if it is one.
pub(crate) fn request_of(detail: &RunDetail) -> Option<BatchRequest> {
    let value: Value = serde_json::from_str(&detail.task.request_json).ok()?;
    if value.get("kind").and_then(Value::as_str) != Some(KIND) {
        return None;
    }
    serde_json::from_value(value).ok()
}

fn manifest_covers(manifest_json: &str, request: &BatchRequest) -> Result<(), String> {
    let manifest: Value =
        serde_json::from_str(manifest_json).map_err(|e| format!("manifest unreadable: {e}"))?;
    if manifest["content_sha256"] != sha256_hex(request.content().as_bytes()).as_str() {
        return Err("the changes differ from the approved manifest".into());
    }
    Ok(())
}

/// What the user sees before deciding: shown only if it is exactly what the
/// approval's manifest covers.
pub(crate) fn preview(
    request: &BatchRequest,
    manifest_json: &str,
) -> Option<Vec<RecordChangePreview>> {
    manifest_covers(manifest_json, request).ok()?;
    Some(
        request
            .items
            .iter()
            .map(|i| RecordChangePreview {
                register_key: i.register_key.clone(),
                register_name: i.register_name.clone(),
                record_id: i
                    .record_id
                    .as_deref()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .map(RecordId),
                title: i.title.clone(),
                fields: diff(i.before.as_ref(), &i.after),
            })
            .collect(),
    )
}

/// How each change of the batch ended, as the model hears it, in order.
pub(crate) fn outcomes(detail: &RunDetail, request: &BatchRequest) -> Vec<Value> {
    let count = request.items.len();
    let written: Vec<String> = detail
        .run
        .outcome_json
        .as_deref()
        .and_then(|o| serde_json::from_str::<Value>(o).ok())
        .and_then(|o| {
            o["records"].as_array().map(|ids| {
                ids.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default();
    request
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| match detail.run.state {
            RunState::Succeeded => json!({
                "saved": true,
                "record_id": written.get(i),
                "values": item.after,
                "note": if count == 1 {
                    "The user approved this change; it is saved and can be undone.".to_string()
                } else {
                    format!("The user approved this change with the other {} of its batch; it is saved and can be undone.", count - 1)
                },
            }),
            RunState::Denied => json!({
                "status": "rejected",
                "message": "The user rejected this batch of changes; nothing was saved. Do not make these changes again unless the user asks.",
            }),
            RunState::Cancelled => json!({"status": "cancelled", "message": "The batch was cancelled; nothing was saved."}),
            _ if detail.run.error_code.as_deref() == Some("ApprovalExpired") => {
                json!({"status": "expired", "message": "The user did not decide in time; nothing was saved."})
            }
            _ => json!({
                "status": "failed",
                "message": format!(
                    "Nothing of the batch was saved: {}",
                    detail.run.error_message.as_deref().unwrap_or("it failed")
                ),
            }),
        })
        .collect()
}

impl Runtime {
    /// The changes a reply makes beyond its threshold: one run waiting for
    /// the user's approval. `number` tells the batches of one reply apart.
    pub(super) async fn propose_record_batch(
        &self,
        reply: MessageId,
        number: u32,
        request: BatchRequest,
    ) -> Result<RunId, NexusError> {
        let request_json = serde_json::to_string(&request).map_err(|e| {
            NexusError::new(nexus_protocol::ErrorCode::InvalidRequest, e.to_string())
        })?;
        let request_sha256 = sha256_hex(request_json.as_bytes());
        let project = self
            .store
            .ensure_default_project(now_ms())
            .await
            .map_err(to_nexus_error)?;
        let outcome = self
            .store
            .submit_task(
                EffectRequester::Agent,
                project,
                format!("chat-records-{reply}-{number}"),
                request_json,
                request_sha256,
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        self.wake.notify_one();
        Ok(match outcome {
            SubmitOutcome::Created { run_id, .. } | SubmitOutcome::Existing { run_id, .. } => {
                run_id
            }
        })
    }

    /// The manifest the user approves: these changes, exactly.
    pub(super) async fn prepare_record_batch(
        &self,
        detail: &RunDetail,
        request: &BatchRequest,
    ) -> Result<(), StorageError> {
        let tool = registry::by_id(RECORD_BATCH_ID).expect("registered");
        let content = request.content();
        self.prepare_effect(
            detail,
            tool,
            OperationId::new(),
            super::broker::Target {
                effect: "write-records",
                destination: request.title(),
                content_sha256: sha256_hex(content.as_bytes()),
                content_size: content.len() as u64,
            },
        )
        .await
    }

    /// Approved: the intent is recorded with the approval's use, then every
    /// change is written with the end of the run.
    pub(super) async fn execute_record_batch(
        &self,
        detail: &RunDetail,
        request: &BatchRequest,
    ) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        let action = detail
            .action
            .as_ref()
            .expect("execute_record_batch is only called with an action");
        if let Err(reason) = manifest_covers(&action.manifest_json, request) {
            self.store
                .fail_run(
                    run_id,
                    RunError::new("ApprovalInvalidated", reason),
                    now_ms(),
                )
                .await?;
            return Ok(());
        }
        let content = request.content();
        let intent = NewOperation {
            target_relpath: format!("records:{run_id}"),
            staging_relpath: format!("records:{run_id}:intent"),
            expected_sha256: sha256_hex(content.as_bytes()),
            expected_size: content.len() as i64,
            algorithm_version: 1,
        };
        match self.store.claim_effect(run_id, intent, now_ms()).await {
            Ok(_) => {}
            Err(StorageError::CancellationRequested) => {
                self.store.finish_cancellation(run_id, now_ms()).await?;
                return Ok(());
            }
            Err(StorageError::ApprovalExpired) => {
                self.store
                    .fail_run(
                        run_id,
                        RunError::new("ApprovalExpired", "approval expired before execution"),
                        now_ms(),
                    )
                    .await?;
                return Ok(());
            }
            Err(StorageError::ApprovalInvalidated(reason)) => {
                self.store
                    .fail_run(
                        run_id,
                        RunError::new("ApprovalInvalidated", reason),
                        now_ms(),
                    )
                    .await?;
                return Ok(());
            }
            Err(other) => return Err(other),
        }
        fault::hit("after_intent");
        self.write_record_batch(run_id, request).await
    }

    /// The batch writes with the end of its run, in one transaction: an
    /// interrupted one wrote nothing, so it is written now (a record that
    /// changed since refuses it, as it would have before).
    pub(super) async fn write_record_batch(
        &self,
        run_id: RunId,
        request: &BatchRequest,
    ) -> Result<(), StorageError> {
        let refused = |reason: String| RunError::new("ResourceChanged", reason);
        let (writes, origin) = match (request.writes(), request.origin()) {
            (Ok(writes), Ok(origin)) => (writes, origin),
            (Err(reason), _) | (_, Err(reason)) => {
                self.store
                    .fail_run(run_id, RunError::new("InvalidRequest", reason), now_ms())
                    .await?;
                return Ok(());
            }
        };
        match self
            .store
            .apply_record_batch(run_id, writes, origin, now_ms())
            .await
        {
            Ok(_) => Ok(()),
            Err(StorageError::RevisionConflict(_) | StorageError::NotFound(_)) => {
                self.store
                    .fail_run(
                        run_id,
                        refused(
                            "a record changed or was deleted after the batch was shown; nothing was written"
                                .into(),
                        ),
                        now_ms(),
                    )
                    .await?;
                Ok(())
            }
            Err(other) => Err(other),
        }
    }
}
