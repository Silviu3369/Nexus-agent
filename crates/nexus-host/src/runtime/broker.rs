//! The execution broker (ADR-ACTION-001): the one path every effect of the
//! agent takes. A model's call is routed by the tool registry: a read runs
//! at once; a register write runs at once as an action of its turn; a fact
//! waits for the user in Memory; anything with an external effect (and a
//! note) becomes an effect run that waits for the user's approval of its
//! exact manifest. Every effect run goes through the same steps here —
//! manifest, approval, intent, execution, verification, recovery — with the
//! tool's own implementation for what is specific to it. An effect that can
//! be undone (a document Nexus created) is undone by a new operation whose
//! intent is written first and which is decided from the resource after a
//! crash, never repeated blindly.

use std::path::PathBuf;

use nexus_core::{ActionState, PolicyDecision};
use nexus_protocol::{ErrorCode, NexusError, OperationId, RunId, RunStatus, RunView};
use nexus_storage::{CompensationRow, NewAction, RunDetail, RunError, StorageError, now_ms};
use serde_json::{Value, json};

use super::document_run::{self, DocumentRequest};
use super::remote_call::{self, McpCallRequest};
use super::{MANIFEST_VERSION, Manifest, Runtime, note_content, to_nexus_error};
use crate::fault;
use crate::tools::markdown_artifact::{NoteContent, sha256_hex};
use crate::tools::registry::{self, Execution, ToolDescriptor};

/// What an effect run does, read from its stored request.
pub(super) enum Effect {
    Note(NoteContent),
    Document(DocumentRequest),
    Call(McpCallRequest),
}

impl Effect {
    pub(super) fn of(detail: &RunDetail) -> Result<Effect, String> {
        if let Some(document) = document_run::request_of(detail) {
            return Ok(Effect::Document(document));
        }
        if let Some(call) = remote_call::request_of(detail) {
            return Ok(Effect::Call(call));
        }
        note_content(detail).map(Effect::Note)
    }

    fn kind(&self) -> &'static str {
        match self {
            Effect::Note(_) => "run",
            Effect::Document(_) => "document",
            Effect::Call(_) => "call",
        }
    }
}

/// What an effect's manifest names besides the fields every manifest has.
pub(super) struct Target {
    pub effect: &'static str,
    pub destination: String,
    pub content_sha256: String,
    pub content_size: u64,
}

/// The registered tool a model's call names, if Nexus offers it now: an MCP
/// tool by the server's offer (an effect if it is not declared read-only),
/// a new document only while a folder is shared for writing.
pub(super) fn route(
    name: &str,
    mcp_offer: &[super::mcp::OfferedTool],
    writable_folder: bool,
) -> Option<&'static ToolDescriptor> {
    if let Some(offered) = mcp_offer.iter().find(|o| o.function == name) {
        return registry::by_id(if offered.needs_approval {
            registry::MCP_CALL_ID
        } else {
            registry::MCP_READ_ID
        });
    }
    registry::by_model_name(name)
        .filter(|t| t.id != crate::tools::document_file::TOOL_ID || writable_folder)
}

/// Whether a call ends the step and waits for the user's approval.
pub(super) fn waits_for_approval(tool: Option<&'static ToolDescriptor>) -> bool {
    tool.is_some_and(|t| {
        t.execution == Execution::EffectRun && registry::decide(t) == PolicyDecision::Ask
    })
}

fn file_sha256(path: &std::path::Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

impl Runtime {
    /// The exact manifest of an effect, its action (with the tool's class
    /// and evidence) and the approval it waits for. Nothing is done yet.
    pub(super) async fn prepare_effect(
        &self,
        detail: &RunDetail,
        tool: &'static ToolDescriptor,
        operation_id: OperationId,
        target: Target,
    ) -> Result<(), StorageError> {
        let now = now_ms();
        let expires_at_ms = now.saturating_add(self.approval_ttl_ms);
        let manifest = Manifest {
            manifest_version: MANIFEST_VERSION,
            actor: "local-user",
            channel: "dashboard",
            project_id: detail.task.project_id.to_string(),
            operation_id: operation_id.to_string(),
            tool_id: tool.id,
            tool_version: tool.version,
            effect: target.effect,
            destination: &target.destination,
            content_sha256: target.content_sha256,
            content_size: target.content_size,
            policy_revision: nexus_core::policy::BUILT_IN_REVISION,
            single_use: true,
            expires_at_ms,
        };
        let manifest_json =
            serde_json::to_string(&manifest).map_err(|e| StorageError::Corrupt(e.to_string()))?;
        let manifest_sha256 = sha256_hex(manifest_json.as_bytes());
        self.store
            .prepare_action(
                detail.run.id,
                NewAction {
                    tool_id: tool.id.to_string(),
                    tool_version: i64::from(tool.version),
                    effect_class: tool.effect,
                    evidence: tool.evidence,
                    operation_id,
                    manifest_json,
                    manifest_sha256,
                    approval_expires_at_ms: expires_at_ms,
                    approval_grant_ms: self.approval_grant_ms,
                },
                now,
            )
            .await?;
        Ok(())
    }

    /// One queued effect run: its manifest is prepared, or the approved
    /// effect is carried out, or an effect left without proof of its end is
    /// resumed from what the resource shows.
    pub(super) async fn run_effect(&self, detail: &RunDetail) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        let effect = match Effect::of(detail) {
            Ok(effect) => effect,
            Err(reason) => {
                self.store
                    .fail_run(run_id, RunError::new("InvalidRequest", reason), now_ms())
                    .await?;
                return Ok(());
            }
        };
        match (detail.action.as_ref().map(|a| a.state), &effect) {
            (None, Effect::Note(content)) => self.prepare(detail, content).await,
            (None, Effect::Document(document)) => self.prepare_document(detail, document).await,
            (None, Effect::Call(call)) => self.prepare_call(detail, call).await,
            (Some(ActionState::Authorized), Effect::Note(content)) => {
                self.execute(detail, content).await
            }
            (Some(ActionState::Authorized), Effect::Document(document)) => {
                self.execute_document(detail, document).await
            }
            (Some(ActionState::Authorized), Effect::Call(call)) => {
                self.execute_call(detail, call).await
            }
            (Some(ActionState::Executing), Effect::Note(content)) => {
                self.resume(detail, content).await
            }
            (Some(ActionState::Executing), Effect::Document(document)) => {
                self.resume_document(detail, document).await
            }
            (Some(other), effect) => {
                self.store
                    .mark_needs_review(
                        run_id,
                        RunError::new(
                            "OutcomeUnknown",
                            format!("queued {} has action in state {other}", effect.kind()),
                        ),
                        now_ms(),
                    )
                    .await?;
                Ok(())
            }
        }
    }

    /// Startup: an effect run interrupted after its intent is decided from
    /// what the resource shows, never by doing it again blindly.
    pub(super) async fn reconcile_effect(
        &self,
        detail: &RunDetail,
        operation_id: OperationId,
    ) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        match Effect::of(detail) {
            Ok(Effect::Document(document)) => {
                tracing::info!(run = %run_id, "reconciling an interrupted document");
                self.reconcile_document(detail, &document, operation_id)
                    .await
            }
            Ok(Effect::Call(_)) => {
                // A call cannot be observed afterwards: it may have reached
                // the server, so it is left for the user, never repeated.
                tracing::info!(run = %run_id, "reconciling an interrupted MCP call");
                self.reconcile_call(run_id).await
            }
            Ok(Effect::Note(content)) => self.reconcile_note(detail, &content, operation_id).await,
            Err(reason) => {
                self.store
                    .mark_needs_review(run_id, RunError::new("OutcomeUnknown", reason), now_ms())
                    .await?;
                Ok(())
            }
        }
    }

    /// The user undoes an effect that can be undone: a document Nexus
    /// created is deleted, only if it is unchanged since it was written and
    /// its folder is still shared for writing. The intent is recorded first.
    pub async fn undo_effect(&self, run_id: RunId) -> Result<RunView, NexusError> {
        let invalid = |m: &str| NexusError::new(ErrorCode::InvalidRequest, m.to_string());
        let detail = self
            .store
            .run_detail(run_id)
            .await
            .map_err(to_nexus_error)?;
        let Ok(Effect::Document(document)) = Effect::of(&detail) else {
            return Err(invalid("only a document Nexus created can be undone"));
        };
        if super::status(detail.run.state) != RunStatus::Succeeded {
            return Err(invalid("only a document that was written can be undone"));
        }
        let outcome: Value = detail
            .run
            .outcome_json
            .as_deref()
            .and_then(|o| serde_json::from_str(o).ok())
            .ok_or_else(|| invalid("the written document is not recorded"))?;
        if outcome.get("undone_at_ms").is_some() {
            return Err(invalid("it was already undone"));
        }
        let path = PathBuf::from(outcome["path"].as_str().unwrap_or_default());
        let sha256 = outcome["sha256"].as_str().unwrap_or_default().to_string();
        if path != document.target() {
            return Err(invalid("the recorded file is not the one approved"));
        }
        self.folder_still_allows(&document)
            .await
            .map_err(|reason| invalid(&format!("{reason}; nothing was deleted")))?;
        let current = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || file_sha256(&path))
                .await
                .ok()
                .flatten()
        };
        match current {
            None => {
                return Err(invalid(
                    "the file is no longer there; there is nothing to undo",
                ));
            }
            Some(current) if current != sha256 => {
                return Err(invalid(
                    "the file changed since Nexus wrote it; nothing was deleted",
                ));
            }
            Some(_) => {}
        }
        let tool = registry::by_id(registry::DELETE_DOCUMENT_ID).expect("registered");
        let action = self
            .store
            .begin_compensation(
                run_id,
                tool.id.to_string(),
                i64::from(tool.version),
                tool.effect,
                json!({"path": path.display().to_string(), "sha256": sha256}),
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        fault::hit("undo_after_intent");
        let removed = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || std::fs::remove_file(&path).map(|_| !path.exists()))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
        };
        let (done, reason) = match removed {
            Ok(true) => (true, None),
            Ok(false) => (false, Some("the file is still there".to_string())),
            Err(reason) => (!path.exists(), Some(reason)),
        };
        self.store
            .finish_compensation(action, done, reason.clone(), now_ms())
            .await
            .map_err(to_nexus_error)?;
        if !done {
            return Err(NexusError::new(
                ErrorCode::InternalFailure,
                format!(
                    "the document could not be deleted: {}",
                    reason.unwrap_or_default()
                ),
            ));
        }
        self.wake.notify_one();
        let detail = self
            .store
            .run_detail(run_id)
            .await
            .map_err(to_nexus_error)?;
        Ok(self.view(&detail, false))
    }

    /// Startup: an undo whose intent was written but whose end was not.
    /// The file gone means it was done; the same file still there means it
    /// was not (the user can undo again); anything else is not touched.
    pub(super) async fn reconcile_compensations(&self) -> Result<(), StorageError> {
        for open in self.store.open_compensations().await? {
            let (done, reason) = compensation_outcome(&open);
            tracing::info!(action = %open.action_id, done, "reconciling an interrupted undo");
            self.store
                .finish_compensation(open.action_id, done, reason, now_ms())
                .await?;
        }
        Ok(())
    }
}

/// How an interrupted undo ended, from what the file shows.
fn compensation_outcome(open: &CompensationRow) -> (bool, Option<String>) {
    let manifest: Value = serde_json::from_str(&open.manifest_json).unwrap_or_default();
    let path = PathBuf::from(manifest["path"].as_str().unwrap_or_default());
    if !path.exists() {
        return (true, None);
    }
    match file_sha256(&path) {
        Some(sha) if Some(sha.as_str()) == manifest["sha256"].as_str() => (
            false,
            Some("Nexus stopped before deleting the file; nothing was deleted".into()),
        ),
        _ => (
            false,
            Some("the file there is not the one Nexus wrote; it was not touched".into()),
        ),
    }
}
