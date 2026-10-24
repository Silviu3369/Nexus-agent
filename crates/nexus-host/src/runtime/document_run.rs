//! Documents the agent proposes into a folder the user shared for writing
//! (`PLAN_NEXUS_RO.md` §3a, step 2): a cover letter, a summary, a table as
//! text. The model only proposes; the user approves the exact file — folder,
//! name and content (its SHA-256) — and the executor writes it once, the
//! intent recorded first, never replacing a file. The written file is the
//! user's: the run keeps its path, size and hash, not a copy.

use std::path::{Path, PathBuf};

use nexus_core::{AgentStepKind, AgentStepStatus, RunState};
use nexus_protocol::{FolderId, MessageId, NexusError, NotePreview, OperationId, RunId};
use nexus_storage::{
    EffectRequester, FolderRow, NewOperation, RunDetail, RunError, StorageError, SubmitOutcome,
    now_ms,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::named;
use super::{Runtime, to_nexus_error};
use crate::fault;
use crate::tools::document_file::{
    self, Destination, DocFormat, DocumentContent, Observed, TOOL_ID, TOOL_VERSION,
};
use crate::tools::markdown_artifact::sha256_hex;

/// `kind` of a stored document request.
pub(crate) const KIND: &str = "document";
pub(super) const DOC_TOOL: &str = "propose_document";

/// The file the user approves, as stored with its task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentRequest {
    pub kind: String,
    pub folder_id: String,
    pub folder_label: String,
    /// The folder's path when proposed; the write is refused if the shared
    /// folder points elsewhere by then.
    pub folder_path: String,
    pub file_name: String,
    pub format: String,
    pub title: String,
    pub body: String,
}

impl DocumentRequest {
    fn format(&self) -> Result<DocFormat, String> {
        DocFormat::parse(&self.format).ok_or_else(|| format!("unknown format {:?}", self.format))
    }

    pub fn content(&self) -> Result<DocumentContent, String> {
        document_file::build(self.format()?, &self.title, &self.body)
    }

    pub fn target(&self) -> PathBuf {
        Path::new(&self.folder_path).join(&self.file_name)
    }

    fn destination(&self, operation: OperationId) -> Destination {
        Destination::new(
            Path::new(&self.folder_path),
            &self.file_name,
            &operation.to_string(),
        )
    }
}

/// The run's document, if it is one.
pub(crate) fn request_of(detail: &RunDetail) -> Option<DocumentRequest> {
    let value: Value = serde_json::from_str(&detail.task.request_json).ok()?;
    if value.get("kind").and_then(Value::as_str) != Some(KIND) {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// What the user sees before deciding: shown only if it is exactly what the
/// approval's manifest covers.
pub(crate) fn preview(request: &DocumentRequest, manifest_json: &str) -> Option<NotePreview> {
    let content = request.content().ok()?;
    manifest_covers(manifest_json, request, &content).ok()?;
    Some(NotePreview {
        destination: request.target().display().to_string(),
        size_bytes: u32::try_from(content.size()).unwrap_or(u32::MAX),
        sha256: content.sha256_hex(),
        content: content.preview,
    })
}

fn manifest_covers(
    manifest_json: &str,
    request: &DocumentRequest,
    content: &DocumentContent,
) -> Result<(), String> {
    let manifest: Value =
        serde_json::from_str(manifest_json).map_err(|e| format!("manifest unreadable: {e}"))?;
    if manifest["destination"] != request.target().display().to_string().as_str() {
        return Err("the destination differs from the approved manifest".into());
    }
    if manifest["content_sha256"] != content.sha256_hex().as_str() {
        return Err("the content differs from the approved manifest".into());
    }
    Ok(())
}

/// How a proposed document ended, as the model hears it.
pub(crate) fn outcome(detail: &RunDetail, request: &DocumentRequest) -> Value {
    let file = format!("{}/{}", request.folder_label, request.file_name);
    match detail.run.state {
        RunState::Succeeded => json!({
            "status": "written",
            "file": file,
            "message": "The user approved the document; it was written and verified.",
        }),
        RunState::Denied => json!({
            "status": "rejected",
            "message": "The user rejected the document; nothing was written. Do not propose it again unless the user asks.",
        }),
        RunState::Cancelled => {
            json!({"status": "cancelled", "message": "The document was cancelled; nothing was written."})
        }
        RunState::NeedsReview => json!({
            "status": "uncertain",
            "message": "Whether the document was written could not be proven; the user has to check it. Do not say it was saved.",
        }),
        _ if detail.run.error_code.as_deref() == Some("ApprovalExpired") => {
            json!({"status": "expired", "message": "The user did not decide in time; nothing was written."})
        }
        _ => json!({
            "status": "failed",
            "message": format!("The document was not written: {}", detail.run.error_message.as_deref().unwrap_or("it failed")),
        }),
    }
}

fn text_arg<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The request a proposal makes, or why it cannot be made.
fn request_for(folders: &[FolderRow], arguments: &Value) -> Result<DocumentRequest, String> {
    let folder = named(folders, text_arg(arguments, "folder"))?;
    if !folder.can_write {
        return Err(format!(
            "the user shared \"{}\" for reading only; new files cannot be proposed there",
            folder.label
        ));
    }
    let format =
        DocFormat::parse(text_arg(arguments, "format")).ok_or("format must be docx, md or txt")?;
    let file_name = document_file::file_name(text_arg(arguments, "file_name"), format)?;
    let request = DocumentRequest {
        kind: KIND.to_string(),
        folder_id: folder.id.to_string(),
        folder_label: folder.label.clone(),
        folder_path: folder.path.clone(),
        file_name,
        format: format.extension().to_string(),
        title: text_arg(arguments, "title").to_string(),
        body: text_arg(arguments, "body").to_string(),
    };
    request.content()?;
    if std::fs::symlink_metadata(request.target()).is_ok() {
        return Err(format!(
            "\"{}\" already exists in \"{}\"; choose another file name (nothing is ever overwritten)",
            request.file_name, request.folder_label
        ));
    }
    Ok(request)
}

impl Runtime {
    /// The model's proposal: a run waiting for the user's approval.
    pub(super) async fn propose_document(
        &self,
        reply: MessageId,
        folders: &[FolderRow],
        arguments: &Value,
    ) -> Option<RunId> {
        let step = self
            .begin_step(
                reply,
                AgentStepKind::DocumentProposed,
                json!({"folder": text_arg(arguments, "folder"), "file_name": text_arg(arguments, "file_name")}),
            )
            .await;
        let submitted = match request_for(folders, arguments) {
            Err(error) => Err(error),
            Ok(request) => {
                let shown = json!({"folder": request.folder_label, "file_name": request.file_name});
                self.submit_document(format!("chat-document-{reply}"), request)
                    .await
                    .map(|run_id| (run_id, shown))
                    .map_err(|e| e.message)
            }
        };
        match submitted {
            Ok((run_id, mut shown)) => {
                shown["run_id"] = json!(run_id.to_string());
                self.finish_step(step, AgentStepStatus::Done, shown).await;
                Some(run_id)
            }
            Err(error) => {
                tracing::info!(error = %error, "a proposed document was refused");
                self.finish_step(step, AgentStepStatus::Failed, json!({ "error": error }))
                    .await;
                None
            }
        }
    }

    async fn submit_document(
        &self,
        idempotency_key: String,
        request: DocumentRequest,
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
                idempotency_key,
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

    /// The manifest the user approves: this folder, this file, these bytes.
    pub(super) async fn prepare_document(
        &self,
        detail: &RunDetail,
        request: &DocumentRequest,
    ) -> Result<(), StorageError> {
        let content = match request.content() {
            Ok(content) => content,
            Err(reason) => {
                self.store
                    .fail_run(
                        detail.run.id,
                        RunError::new("InvalidRequest", reason),
                        now_ms(),
                    )
                    .await?;
                return Ok(());
            }
        };
        let tool = crate::tools::registry::by_id(TOOL_ID).expect("registered");
        self.prepare_effect(
            detail,
            tool,
            OperationId::new(),
            super::broker::Target {
                effect: "create-new-file",
                destination: request.target().display().to_string(),
                content_sha256: content.sha256_hex(),
                content_size: content.size(),
            },
        )
        .await
    }

    /// The folder must still be shared, writable and the same place.
    pub(super) async fn folder_still_allows(
        &self,
        request: &DocumentRequest,
    ) -> Result<(), String> {
        let id = uuid::Uuid::parse_str(&request.folder_id)
            .map(FolderId)
            .map_err(|_| "the folder's id is unreadable".to_string())?;
        let folder = self
            .store
            .folder(id)
            .await
            .map_err(|_| "the folder is no longer shared".to_string())?;
        if !folder.can_write {
            return Err("writing to the folder is no longer allowed".into());
        }
        if !folder.path.eq_ignore_ascii_case(&request.folder_path) {
            return Err("the shared folder now points to another place".into());
        }
        if let Some(problem) = crate::documents::folder_problem(Path::new(&folder.path)) {
            return Err(problem);
        }
        Ok(())
    }

    /// Approved: re-check everything, consume the approval with the intent,
    /// then write once.
    pub(super) async fn execute_document(
        &self,
        detail: &RunDetail,
        request: &DocumentRequest,
    ) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        let action = detail
            .action
            .as_ref()
            .expect("execute_document is only called with an action");
        let invalidated = |reason: String| RunError::new("ApprovalInvalidated", reason);
        let content = match request.content() {
            Ok(content) => content,
            Err(reason) => {
                self.store
                    .fail_run(run_id, invalidated(reason), now_ms())
                    .await?;
                return Ok(());
            }
        };
        if let Err(reason) = manifest_covers(&action.manifest_json, request, &content) {
            self.store
                .fail_run(run_id, invalidated(reason), now_ms())
                .await?;
            return Ok(());
        }
        if let Err(reason) = self.folder_still_allows(request).await {
            self.store
                .fail_run(run_id, invalidated(reason), now_ms())
                .await?;
            return Ok(());
        }
        let dest = request.destination(action.operation_id);
        let observed = observe(&dest, &content).await;
        if observed != Observed::NoEffect {
            self.store
                .fail_run(
                    run_id,
                    RunError::new(
                        "ResourceChanged",
                        format!("the destination is no longer free: {observed:?}"),
                    ),
                    now_ms(),
                )
                .await?;
            return Ok(());
        }
        let intent = NewOperation {
            target_relpath: request.target().display().to_string(),
            staging_relpath: dest.root.join(&dest.staging_name).display().to_string(),
            expected_sha256: content.sha256_hex(),
            expected_size: content.size() as i64,
            algorithm_version: i64::from(TOOL_VERSION),
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
                    .fail_run(run_id, invalidated(reason), now_ms())
                    .await?;
                return Ok(());
            }
            Err(other) => return Err(other),
        }
        fault::hit("after_intent");
        self.write_document(run_id, &dest, &content, request).await
    }

    async fn write_document(
        &self,
        run_id: RunId,
        dest: &Destination,
        content: &DocumentContent,
        request: &DocumentRequest,
    ) -> Result<(), StorageError> {
        let (d, c) = (dest.clone(), content.clone());
        let written = tokio::task::spawn_blocking(move || document_file::create(&d, &c))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        match written {
            Ok(()) => self.complete(run_id, content, request).await,
            Err(reason) => match observe(dest, content).await {
                Observed::Complete => self.complete(run_id, content, request).await,
                Observed::NoEffect => {
                    self.store
                        .fail_run(run_id, RunError::new("EffectFailed", reason), now_ms())
                        .await?;
                    Ok(())
                }
                Observed::Uncertain(why) if !dest.target().exists() => {
                    document_file::discard_staging(dest);
                    self.store
                        .fail_run(
                            run_id,
                            RunError::new("EffectFailed", format!("{reason}; {why}")),
                            now_ms(),
                        )
                        .await?;
                    Ok(())
                }
                Observed::Uncertain(why) => {
                    self.store
                        .mark_needs_review(
                            run_id,
                            RunError::new("OutcomeUnknown", format!("{reason}; {why}")),
                            now_ms(),
                        )
                        .await?;
                    Ok(())
                }
            },
        }
    }

    async fn complete(
        &self,
        run_id: RunId,
        content: &DocumentContent,
        request: &DocumentRequest,
    ) -> Result<(), StorageError> {
        fault::hit("after_publish");
        let outcome_json = json!({
            "path": request.target().display().to_string(),
            "size_bytes": content.size(),
            "sha256": content.sha256_hex(),
        })
        .to_string();
        self.store
            .complete_document(run_id, outcome_json, now_ms())
            .await?;
        Ok(())
    }

    /// A run interrupted after its intent: decided from what the folder
    /// shows, never by writing blindly again.
    pub(super) async fn reconcile_document(
        &self,
        detail: &RunDetail,
        request: &DocumentRequest,
        operation_id: OperationId,
    ) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        let Ok(content) = request.content() else {
            self.store
                .mark_needs_review(
                    run_id,
                    RunError::new("OutcomeUnknown", "the stored document is unreadable"),
                    now_ms(),
                )
                .await?;
            return Ok(());
        };
        let dest = request.destination(operation_id);
        match observe(&dest, &content).await {
            Observed::Complete => {
                document_file::discard_staging(&dest);
                self.complete(run_id, &content, request).await
            }
            Observed::Uncertain(reason) if dest.target().exists() => {
                self.store
                    .mark_needs_review(run_id, RunError::new("OutcomeUnknown", reason), now_ms())
                    .await?;
                Ok(())
            }
            Observed::NoEffect | Observed::Uncertain(_) => {
                document_file::discard_staging(&dest);
                if detail.cancellation_requested {
                    self.store.finish_cancellation(run_id, now_ms()).await?;
                } else {
                    self.store.requeue_reconciled(run_id, now_ms()).await?;
                }
                Ok(())
            }
        }
    }

    /// A requeued run whose intent left no effect: write the same file.
    pub(super) async fn resume_document(
        &self,
        detail: &RunDetail,
        request: &DocumentRequest,
    ) -> Result<(), StorageError> {
        let action = detail
            .action
            .as_ref()
            .expect("resume_document is only called with an action");
        let Ok(content) = request.content() else {
            self.store
                .mark_needs_review(
                    detail.run.id,
                    RunError::new("OutcomeUnknown", "the stored document is unreadable"),
                    now_ms(),
                )
                .await?;
            return Ok(());
        };
        let dest = request.destination(action.operation_id);
        match observe(&dest, &content).await {
            Observed::NoEffect => {
                self.write_document(detail.run.id, &dest, &content, request)
                    .await
            }
            Observed::Complete => {
                document_file::discard_staging(&dest);
                self.complete(detail.run.id, &content, request).await
            }
            Observed::Uncertain(_) if !dest.target().exists() => {
                document_file::discard_staging(&dest);
                self.write_document(detail.run.id, &dest, &content, request)
                    .await
            }
            Observed::Uncertain(reason) => {
                self.store
                    .mark_needs_review(
                        detail.run.id,
                        RunError::new("OutcomeUnknown", reason),
                        now_ms(),
                    )
                    .await?;
                Ok(())
            }
        }
    }
}

async fn observe(dest: &Destination, content: &DocumentContent) -> Observed {
    let (d, c) = (dest.clone(), content.clone());
    tokio::task::spawn_blocking(move || document_file::observe(&d, &c))
        .await
        .unwrap_or_else(|e| Observed::Uncertain(e.to_string()))
}

/// The proposal tool, offered while a folder allows writing.
pub(super) fn document_tool(folders: &[FolderRow]) -> Option<nexus_providers::ToolDeclaration> {
    let writable: Vec<&str> = folders
        .iter()
        .filter(|f| f.can_write)
        .map(|f| f.label.as_str())
        .collect();
    if writable.is_empty() {
        return None;
    }
    Some(nexus_providers::ToolDeclaration {
        name: DOC_TOOL.to_string(),
        description: "Propose creating a new document in a folder the user shared for writing: a Word (.docx), \
            Markdown or text file. The user sees the exact file and must approve it before it is written; an \
            existing file is never replaced."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "folder": {"type": "string", "enum": writable, "description": "The shared folder to create it in"},
                "file_name": {"type": "string", "description": "The file's name, without folder (the extension is added)"},
                "format": {"type": "string", "enum": ["docx", "md", "txt"], "description": "docx for a letter or anything to send"},
                "title": {"type": "string", "description": "An optional heading on the first line"},
                "body": {"type": "string", "description": "The full text. A blank line starts a new paragraph."}
            },
            "required": ["folder", "file_name", "format", "body"]
        }),
    })
}
