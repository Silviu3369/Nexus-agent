//! M1 runtime: the serial executor, startup reconciliation and the
//! operations behind the RPC surface. Every decision re-reads durable state;
//! the client only ever proposes (`ARHITECTURA_NEXUS_RO.md` §8).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nexus_core::{ActionState, DecisionPath, OperationState, RunState};
use nexus_protocol::{
    ActorView, ApprovalDecision, ApprovalPath, ArtifactId, ArtifactSummary, BackupSummary,
    DecideApprovalRequest, ErrorCode, EventView, EventsPage, NexusError, NotePreview, OperationId,
    PendingApproval, RunFailure, RunId, RunKind, RunOrigin, RunStatus, RunView, Snapshot,
    SubmitNoteRequest, SubmitNoteResponse, SystemStatusView,
};
use nexus_storage::{
    ActorRow, Decision, EffectRequester, NewOperation, RunDetail, RunError, StorageError, Store,
    SubmitOutcome, now_ms,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, watch};

use crate::backup;
use crate::fault;
use crate::system::SystemMonitor;
use crate::tools::markdown_artifact::{self as note_tool, ArtifactPaths, NoteContent, Observed};
use crate::tools::registry;
use crate::vault::SecretVault;

mod acceptance;
mod automation;
mod broker;
mod chat;
mod data_policy;
mod document_run;
mod files;
mod inbox;
mod job_results;
mod mcp;
mod memory;
mod packs;
mod permissions;
mod record_batch;
mod records;
mod records_io;
mod remote_call;
mod skills;
mod usage;

/// How long a request for approval waits for the user's decision
/// (ADR-POLICY-001: a proposal made while the user is away waits for them).
pub const DEFAULT_APPROVAL_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an approval given may wait to be used.
pub const DEFAULT_APPROVAL_GRANT: Duration = Duration::from_secs(10 * 60);
const IDLE_TICK: Duration = Duration::from_secs(1);
const MAX_SNAPSHOT_RUNS: u32 = 200;
const MAX_EVENTS_PAGE: u32 = 500;
const MAX_IDEMPOTENCY_KEY: usize = 200;
const MANIFEST_VERSION: u32 = 1;

/// The persisted task request; normalised before hashing so equal notes
/// produce equal idempotency hashes.
#[derive(Debug, Serialize, Deserialize)]
struct NoteRequest {
    title: String,
    body: String,
}

/// Everything an approval is bound to (`CONTRACTE_NEXUS_RO.md` §5). Field
/// order is the canonical serialisation, so an action always hashes the same.
#[derive(Serialize)]
struct Manifest<'a> {
    manifest_version: u32,
    actor: &'a str,
    channel: &'a str,
    project_id: String,
    operation_id: String,
    tool_id: &'a str,
    tool_version: u32,
    effect: &'a str,
    destination: &'a str,
    content_sha256: String,
    content_size: u64,
    policy_revision: i64,
    single_use: bool,
    expires_at_ms: i64,
}

#[derive(Deserialize)]
struct ManifestCheck {
    destination: String,
    content_sha256: String,
}

pub struct Runtime {
    store: Store,
    profile: PathBuf,
    artifact_root: PathBuf,
    approval_ttl_ms: i64,
    approval_grant_ms: i64,
    /// A restored profile: no reconciliation and no execution until a
    /// person removes the restore marker (BACKUP-01, "nicio execuție reluată").
    paused: bool,
    wake: Notify,
    stop: watch::Sender<bool>,
    vault: SecretVault,
    gemini_base_url: String,
    openai_base_url: String,
    anthropic_base_url: String,
    deepseek_base_url: String,
    reply_deadline: Duration,
    step_deadline: Duration,
    retry_waits: Vec<Duration>,
    replies: chat::Replies,
    system: SystemMonitor,
    /// Tests only: lets a loopback MCP server stand in for a remote one.
    allow_loopback_mcp: bool,
    mcp_clients: mcp::McpClients,
    /// Answers of approved MCP calls, until their task continues.
    answers: remote_call::Answers,
    /// The document reader program (`None`: this executable).
    extractor_exe: Option<PathBuf>,
    inbox_interval: Duration,
    inbox_retry: Duration,
    inbox_state: inbox::InboxState,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// How long a request for approval waits for the decision.
    pub approval_ttl: Duration,
    /// How long an approval given may wait to be used.
    pub approval_grant: Duration,
    /// Each provider's own endpoint in production; tests point them at
    /// local servers (each adapter refuses anything but its origin or
    /// loopback).
    pub gemini_base_url: String,
    pub openai_base_url: String,
    pub anthropic_base_url: String,
    pub deepseek_base_url: String,
    /// Whole budget of one model reply, across its steps.
    pub reply_deadline: Duration,
    /// Budget of one model call within it: a model that thinks long on one
    /// step no longer eats the time of the next ones, and one that hangs
    /// is still cut.
    pub step_deadline: Duration,
    /// Pauses before trying a model call again after a temporary outage;
    /// one entry per retry (so at most `len + 1` tries).
    pub retry_waits: Vec<Duration>,
    /// Tests and debug builds only: accept a loopback `http://` MCP server.
    pub allow_loopback_mcp: bool,
    /// The program that reads documents (`nexus-host extract …`); `None`
    /// is this executable. Tests running the Host in-process name the built
    /// `nexus-host.exe`.
    pub extractor_exe: Option<PathBuf>,
    /// How often an inbox folder is looked at.
    pub inbox_interval: Duration,
    /// The pause before an inbox task stopped by a temporary provider error
    /// is tried again (it grows with each try).
    pub inbox_retry: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        use nexus_providers::{ProviderKind, production_base_url};
        Self {
            approval_ttl: DEFAULT_APPROVAL_TTL,
            approval_grant: DEFAULT_APPROVAL_GRANT,
            gemini_base_url: production_base_url(ProviderKind::Gemini).to_string(),
            openai_base_url: production_base_url(ProviderKind::OpenAi).to_string(),
            anthropic_base_url: production_base_url(ProviderKind::Anthropic).to_string(),
            deepseek_base_url: production_base_url(ProviderKind::DeepSeek).to_string(),
            // Measured on the user's real Gemini calls (2026-10-25): a few
            // took 30–108 s before their first byte, and replies of two or
            // three steps failed under a 120 s limit for the whole reply.
            reply_deadline: Duration::from_secs(300),
            step_deadline: Duration::from_secs(120),
            retry_waits: vec![Duration::from_secs(2), Duration::from_secs(5)],
            allow_loopback_mcp: false,
            extractor_exe: None,
            inbox_interval: inbox::DEFAULT_INBOX_INTERVAL,
            inbox_retry: inbox::DEFAULT_INBOX_RETRY,
        }
    }
}

impl Runtime {
    pub async fn open(profile: &Path, approval_ttl: Duration) -> anyhow::Result<Arc<Self>> {
        Self::open_with(
            profile,
            RuntimeConfig {
                approval_ttl,
                ..RuntimeConfig::default()
            },
        )
        .await
    }

    /// Opens the profile's database and reconciles interrupted runs *before*
    /// the executor or any client can act on them.
    pub async fn open_with(profile: &Path, config: RuntimeConfig) -> anyhow::Result<Arc<Self>> {
        let store = Store::open(&profile.join(backup::DATABASE_FILE))?;
        store.ensure_default_project(now_ms()).await?;
        let paused = backup::is_restored_profile(profile);
        let (stop, _) = watch::channel(false);
        let runtime = Arc::new(Self {
            store,
            profile: profile.to_path_buf(),
            artifact_root: profile.join("artifacts"),
            approval_ttl_ms: i64::try_from(config.approval_ttl.as_millis()).unwrap_or(i64::MAX),
            approval_grant_ms: i64::try_from(config.approval_grant.as_millis())
                .unwrap_or(i64::MAX)
                .max(1),
            paused,
            wake: Notify::new(),
            stop,
            vault: SecretVault::for_profile(profile),
            gemini_base_url: config.gemini_base_url,
            openai_base_url: config.openai_base_url,
            anthropic_base_url: config.anthropic_base_url,
            deepseek_base_url: config.deepseek_base_url,
            reply_deadline: config.reply_deadline,
            step_deadline: config.step_deadline,
            retry_waits: config.retry_waits,
            replies: chat::Replies::default(),
            system: SystemMonitor::default(),
            allow_loopback_mcp: config.allow_loopback_mcp,
            mcp_clients: mcp::McpClients::default(),
            answers: remote_call::Answers::default(),
            extractor_exe: config.extractor_exe,
            inbox_interval: config.inbox_interval,
            inbox_retry: config.inbox_retry,
            inbox_state: inbox::InboxState::default(),
        });
        runtime.ensure_registers().await?;
        if paused {
            tracing::warn!(
                profile = %profile.display(),
                "restored profile: reconciliation and execution are paused until {} is removed",
                backup::RESTORED_MARKER
            );
        } else {
            runtime.reconcile_interrupted().await?;
            runtime.reconcile_compensations().await?;
            runtime.store.fail_interrupted_messages(now_ms()).await?;
            runtime
                .store
                .fail_running_steps(None, "the host stopped before this step finished", now_ms())
                .await?;
            runtime.sweep_unreferenced_secrets().await?;
        }
        Ok(runtime)
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub fn request_stop(&self) {
        self.stop.send_replace(true);
        self.wake.notify_one();
    }

    /// One run at a time, in order (`PLAN_NEXUS_RO.md` M1 §3).
    pub async fn run_executor(self: Arc<Self>) {
        let mut stop = self.stop.subscribe();
        if self.paused {
            // Nothing runs, not even approval expiry: a restored profile is
            // inspected as it was, not advanced.
            while !*stop.borrow() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
            return;
        }
        loop {
            if *stop.borrow() {
                break;
            }
            if let Err(err) = self.store.expire_approvals(now_ms()).await {
                tracing::warn!(error = %err, "could not expire approvals");
            }
            // Tasks whose proposal the user has decided go on from there.
            self.continue_waiting_replies().await;
            self.run_due_automations().await;
            self.run_inboxes().await;
            match self.step().await {
                Ok(true) => continue,
                Ok(false) => {}
                // The run stays where it is; startup reconciliation will
                // decide it from durable state rather than guess now.
                Err(err) => tracing::error!(error = %err, "executor step failed"),
            }
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(IDLE_TICK) => {}
                _ = stop.changed() => {}
            }
        }
        tracing::info!("executor stopped");
    }

    async fn step(&self) -> Result<bool, StorageError> {
        let Some(detail) = self.store.claim_next_queued_run(now_ms()).await? else {
            return Ok(false);
        };
        self.run_effect(&detail).await?;
        Ok(true)
    }

    /// The note's manifest: its place in Nexus's store and its bytes.
    async fn prepare(&self, detail: &RunDetail, content: &NoteContent) -> Result<(), StorageError> {
        let operation_id = OperationId::new();
        let paths =
            ArtifactPaths::for_operation(&self.artifact_root, detail.task.project_id, operation_id);
        let tool = registry::by_id(note_tool::TOOL_ID).expect("registered");
        self.prepare_effect(
            detail,
            tool,
            operation_id,
            broker::Target {
                effect: "create-new-file",
                destination: paths.target_relpath.clone(),
                content_sha256: content.sha256_hex(),
                content_size: content.size(),
            },
        )
        .await
    }

    /// Approved: re-check preconditions, consume the approval together with
    /// the intent, then perform the effect.
    async fn execute(&self, detail: &RunDetail, content: &NoteContent) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        let action = detail
            .action
            .as_ref()
            .expect("execute is only called with an action");
        let paths = self.paths(detail, action.operation_id);

        if let Err(reason) = manifest_matches(&action.manifest_json, &paths, content) {
            self.store
                .fail_run(
                    run_id,
                    RunError::new("ApprovalInvalidated", reason),
                    now_ms(),
                )
                .await?;
            return Ok(());
        }
        // SEC-01 "resource changed": the destination must still be untouched.
        let observed = observe(&paths, content).await;
        if observed != Observed::NoEffect {
            self.store
                .fail_run(
                    run_id,
                    RunError::new(
                        "ResourceChanged",
                        format!("destination is no longer untouched: {observed:?}"),
                    ),
                    now_ms(),
                )
                .await?;
            return Ok(());
        }

        let intent = NewOperation {
            target_relpath: paths.target_relpath.clone(),
            staging_relpath: paths.staging_relpath.clone(),
            expected_sha256: content.sha256_hex(),
            expected_size: content.size() as i64,
            algorithm_version: i64::from(note_tool::ALGORITHM_VERSION),
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
        self.perform(run_id, &paths, content).await
    }

    /// A reconciled run whose intent left no effect: try the same operation.
    async fn resume(&self, detail: &RunDetail, content: &NoteContent) -> Result<(), StorageError> {
        let action = detail
            .action
            .as_ref()
            .expect("resume is only called with an action");
        let paths = self.paths(detail, action.operation_id);
        match observe(&paths, content).await {
            Observed::NoEffect => self.perform(detail.run.id, &paths, content).await,
            Observed::Complete => {
                discard_staging(&paths).await;
                self.store
                    .complete_operation(detail.run.id, now_ms())
                    .await?;
                Ok(())
            }
            Observed::Uncertain(_) if !paths.target().exists() => {
                discard_staging(&paths).await;
                self.perform(detail.run.id, &paths, content).await
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

    async fn perform(
        &self,
        run_id: RunId,
        paths: &ArtifactPaths,
        content: &NoteContent,
    ) -> Result<(), StorageError> {
        let (p, c) = (paths.clone(), content.clone());
        let outcome = tokio::task::spawn_blocking(move || note_tool::create(&p, &c)).await;
        match outcome {
            Ok(Ok(())) => {
                fault::hit("after_publish");
                self.store.complete_operation(run_id, now_ms()).await?;
                Ok(())
            }
            Ok(Err(err)) => self.settle(run_id, paths, content, err.to_string()).await,
            Err(join) => self.settle(run_id, paths, content, join.to_string()).await,
        }
    }

    /// The tool reported a problem: decide only from what the filesystem
    /// actually shows, never by retrying blindly.
    async fn settle(
        &self,
        run_id: RunId,
        paths: &ArtifactPaths,
        content: &NoteContent,
        reason: String,
    ) -> Result<(), StorageError> {
        match observe(paths, content).await {
            Observed::Complete => {
                discard_staging(paths).await;
                self.store.complete_operation(run_id, now_ms()).await?;
            }
            Observed::NoEffect => {
                self.store
                    .fail_run(run_id, RunError::new("EffectFailed", reason), now_ms())
                    .await?;
            }
            Observed::Uncertain(_) if !paths.target().exists() => {
                discard_staging(paths).await;
                self.store
                    .fail_run(run_id, RunError::new("EffectFailed", reason), now_ms())
                    .await?;
            }
            Observed::Uncertain(why) => {
                self.store
                    .mark_needs_review(
                        run_id,
                        RunError::new("OutcomeUnknown", format!("{reason}; {why}")),
                        now_ms(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Startup (RUN-04/05/06): decide every interrupted run from the
    /// filesystem and the durable log — never by replaying an effect blindly.
    async fn reconcile_interrupted(&self) -> Result<(), StorageError> {
        for detail in self.store.take_interrupted_runs(now_ms()).await? {
            let run_id = detail.run.id;
            let intent = match (&detail.action, &detail.operation) {
                (Some(action), Some(op)) if op.state == OperationState::Intended => {
                    Some(action.operation_id)
                }
                _ => None,
            };
            let Some(operation_id) = intent else {
                // No intent was recorded, so no effect can have happened.
                if detail.cancellation_requested {
                    self.store.finish_cancellation(run_id, now_ms()).await?;
                } else {
                    self.store.requeue_reconciled(run_id, now_ms()).await?;
                }
                continue;
            };

            self.reconcile_effect(&detail, operation_id).await?;
        }
        self.wake.notify_one();
        Ok(())
    }

    /// Startup: a note interrupted after its intent, decided from Nexus's
    /// store (RUN-04…06).
    async fn reconcile_note(
        &self,
        detail: &RunDetail,
        content: &NoteContent,
        operation_id: OperationId,
    ) -> Result<(), StorageError> {
        let run_id = detail.run.id;
        let paths = self.paths(detail, operation_id);
        let observed = observe(&paths, content).await;
        tracing::info!(run = %run_id, ?observed, "reconciling interrupted run");
        match observed {
            Observed::Complete => {
                // RUN-05: the file was published before the crash; record
                // it instead of creating a second one.
                discard_staging(&paths).await;
                self.store.complete_operation(run_id, now_ms()).await?;
            }
            Observed::Uncertain(reason) if paths.target().exists() => {
                // RUN-06: something is there but it is not provably ours.
                self.store
                    .mark_needs_review(run_id, RunError::new("OutcomeUnknown", reason), now_ms())
                    .await?;
            }
            Observed::NoEffect | Observed::Uncertain(_) => {
                // Nothing published (at most an unpublished staging file).
                discard_staging(&paths).await;
                if detail.cancellation_requested {
                    self.store.finish_cancellation(run_id, now_ms()).await?;
                } else {
                    self.store.requeue_reconciled(run_id, now_ms()).await?;
                }
            }
        }
        Ok(())
    }

    fn paths(&self, detail: &RunDetail, operation_id: OperationId) -> ArtifactPaths {
        ArtifactPaths::for_operation(&self.artifact_root, detail.task.project_id, operation_id)
    }

    // ---- RPC surface ----

    /// A note to create, asked for by the user (sent directly) or by the
    /// agent (a proposal in a turn).
    pub async fn submit_note(
        &self,
        request: SubmitNoteRequest,
        requester: EffectRequester,
    ) -> Result<SubmitNoteResponse, NexusError> {
        let key = request.idempotency_key.trim();
        if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY {
            return Err(NexusError::new(
                ErrorCode::InvalidRequest,
                "idempotency key must be 1..=200 bytes",
            ));
        }
        // `agent:` names the task of a conversation (ADR-TASK-001).
        if key.starts_with("agent:") {
            return Err(NexusError::new(
                ErrorCode::InvalidRequest,
                "idempotency keys starting with \"agent:\" are reserved",
            ));
        }
        // Validate before persisting: an invalid note never becomes a task.
        NoteContent::new(&request.title, &request.body)
            .map_err(|e| NexusError::new(ErrorCode::InvalidRequest, e.to_string()))?;
        let normalised = NoteRequest {
            title: request.title,
            body: request.body,
        };
        let request_json = serde_json::to_string(&normalised).expect("plain strings serialise");
        let request_sha256 = note_tool::sha256_hex(request_json.as_bytes());
        let project = self
            .store
            .ensure_default_project(now_ms())
            .await
            .map_err(to_nexus_error)?;
        let outcome = self
            .store
            .submit_task(
                requester,
                project,
                key.to_string(),
                request_json,
                request_sha256,
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        self.wake.notify_one();
        Ok(match outcome {
            SubmitOutcome::Created { task_id, run_id } => SubmitNoteResponse {
                task_id,
                run_id,
                created: true,
            },
            SubmitOutcome::Existing { task_id, run_id } => SubmitNoteResponse {
                task_id,
                run_id,
                created: false,
            },
        })
    }

    pub async fn snapshot(&self, limit: u32) -> Result<Snapshot, NexusError> {
        let (runs, watermark) = self
            .store
            .snapshot(limit.clamp(1, MAX_SNAPSHOT_RUNS))
            .await
            .map_err(to_nexus_error)?;
        Ok(Snapshot {
            runs: runs.iter().map(|d| self.view(d, false)).collect(),
            watermark,
        })
    }

    pub async fn run(&self, run_id: RunId) -> Result<RunView, NexusError> {
        let detail = self
            .store
            .run_detail(run_id)
            .await
            .map_err(to_nexus_error)?;
        Ok(self.view(&detail, true))
    }

    pub async fn decide(&self, request: DecideApprovalRequest) -> Result<RunView, NexusError> {
        let decision = match request.decision {
            ApprovalDecision::Approve => Decision::Approved,
            ApprovalDecision::Deny => Decision::Denied,
        };
        // Said by the app's own code; the page cannot reach the Host.
        let via = match request.via {
            ApprovalPath::Window => DecisionPath::Window,
            ApprovalPath::TrustedDialog => DecisionPath::TrustedDialog,
        };
        let detail = self
            .store
            .decide_approval(
                request.approval_id,
                request.manifest_sha256,
                decision,
                via,
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        self.wake.notify_one();
        Ok(self.view(&detail, false))
    }

    pub async fn cancel(&self, run_id: RunId) -> Result<RunView, NexusError> {
        let detail = self
            .store
            .cancel_run(run_id, now_ms())
            .await
            .map_err(to_nexus_error)?;
        Ok(self.view(&detail, false))
    }

    pub async fn events(&self, after: i64, limit: u32) -> Result<EventsPage, NexusError> {
        let rows = self
            .store
            .events_since(after, limit.clamp(1, MAX_EVENTS_PAGE))
            .await
            .map_err(to_nexus_error)?;
        let next_after = rows.last().map_or(after, |e| e.seq);
        Ok(EventsPage {
            events: rows
                .into_iter()
                .map(|e| EventView {
                    seq: e.seq,
                    kind: e.kind,
                    entity_id: e.entity_id,
                    created_at_ms: e.created_at_ms,
                })
                .collect(),
            next_after,
        })
    }

    pub async fn system_status(&self) -> SystemStatusView {
        self.system.status().await
    }

    pub async fn create_backup(&self) -> Result<BackupSummary, NexusError> {
        let (dir, manifest) = backup::create(
            &self.store,
            &self.artifact_root,
            &self.profile.join("backups"),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "backup failed");
            NexusError::new(ErrorCode::InternalFailure, format!("backup failed: {e}"))
        })?;
        tracing::info!(path = %dir.display(), artifacts = manifest.artifacts.len(), "backup created");
        Ok(BackupSummary {
            path: dir.display().to_string(),
            created_at_ms: manifest.created_at_ms,
            artifacts: manifest.artifacts.len() as u32,
            unmatched_artifacts: manifest.unmatched_artifacts() as u32,
            database_sha256: manifest.database_sha256,
        })
    }

    /// Reads a verified artifact and re-checks its hash; a file changed
    /// outside Nexus is reported, not served as if it were the original.
    pub async fn artifact_content(&self, artifact_id: ArtifactId) -> Result<String, NexusError> {
        let row = self
            .store
            .artifact(artifact_id)
            .await
            .map_err(to_nexus_error)?;
        let path = self.artifact_root.join(&row.relpath);
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(|e| NexusError::new(ErrorCode::InternalFailure, e.to_string()))?
            .map_err(|e| {
                NexusError::new(
                    ErrorCode::NotFound,
                    format!("artifact file unreadable: {e}"),
                )
            })?;
        if note_tool::sha256_hex(&bytes) != row.sha256 {
            return Err(NexusError::new(
                ErrorCode::ResourceChanged,
                "the artifact file was changed outside Nexus",
            ));
        }
        String::from_utf8(bytes)
            .map_err(|_| NexusError::new(ErrorCode::ResourceChanged, "artifact is not UTF-8"))
    }

    fn view(&self, d: &RunDetail, with_preview: bool) -> RunView {
        let call = remote_call::request_of(d);
        let document = document_run::request_of(d);
        let batch = record_batch::request_of(d);
        let request: Option<NoteRequest> = match (&call, &document, &batch) {
            (None, None, None) => serde_json::from_str(&d.task.request_json).ok(),
            _ => None,
        };
        let title = match (&call, &document, &request) {
            (Some(c), _, _) => format!("{} on {}", c.tool, c.server_name),
            (_, Some(doc), _) => format!("{} in {}", doc.file_name, doc.folder_label),
            (_, _, Some(r)) => r.title.trim().to_string(),
            _ => batch.as_ref().map(|b| b.title()).unwrap_or_default(),
        };
        let awaiting = match (&d.action, &d.approval) {
            (Some(action), Some(approval))
                if action.state == ActionState::AwaitingApproval
                    && approval.decision.is_none()
                    && approval.revoked_at_ms.is_none() =>
            {
                Some(approval)
            }
            _ => None,
        };

        let pending_approval = match (&d.action, &d.approval, &request) {
            (Some(action), Some(approval), Some(req))
                if action.state == ActionState::AwaitingApproval
                    && approval.decision.is_none()
                    && approval.revoked_at_ms.is_none() =>
            {
                let preview = if with_preview {
                    // Rebuilt from the stored request and shown only if it is
                    // exactly what the approval's manifest covers.
                    NoteContent::new(&req.title, &req.body)
                        .ok()
                        .filter(|c| {
                            let paths = self.paths(d, action.operation_id);
                            manifest_matches(&action.manifest_json, &paths, c).is_ok()
                        })
                        .map(|content| NotePreview {
                            destination: self
                                .paths(d, action.operation_id)
                                .target()
                                .display()
                                .to_string(),
                            size_bytes: content.size() as u32,
                            sha256: content.sha256_hex(),
                            content: content.text,
                        })
                } else {
                    None
                };
                Some(PendingApproval {
                    approval_id: approval.id,
                    manifest_sha256: approval.manifest_sha256.clone(),
                    expires_at_ms: approval.expires_at_ms,
                    preview,
                    call: None,
                    records: None,
                })
            }
            _ => None,
        };
        // A document shows the exact file, as its manifest covers it.
        let pending_approval = match (&document, &d.action, awaiting) {
            (Some(doc), Some(action), Some(approval)) => Some(PendingApproval {
                approval_id: approval.id,
                manifest_sha256: approval.manifest_sha256.clone(),
                expires_at_ms: approval.expires_at_ms,
                preview: if with_preview {
                    document_run::preview(doc, &action.manifest_json)
                } else {
                    None
                },
                call: None,
                records: None,
            }),
            _ => pending_approval,
        };
        // A batch shows each change exactly, as its manifest covers them.
        let pending_approval = match (&batch, &d.action, awaiting) {
            (Some(b), Some(action), Some(approval)) => Some(PendingApproval {
                approval_id: approval.id,
                manifest_sha256: approval.manifest_sha256.clone(),
                expires_at_ms: approval.expires_at_ms,
                preview: None,
                call: None,
                records: if with_preview {
                    record_batch::preview(b, &action.manifest_json)
                } else {
                    None
                },
            }),
            _ => pending_approval,
        };
        // A call shows what the server says about the tool, as approved.
        let pending_approval = match (&call, awaiting) {
            (Some(c), Some(approval)) => Some(PendingApproval {
                approval_id: approval.id,
                manifest_sha256: approval.manifest_sha256.clone(),
                expires_at_ms: approval.expires_at_ms,
                preview: None,
                call: with_preview.then(|| remote_call::call_preview(c)),
                records: None,
            }),
            _ => pending_approval,
        };

        // ADR-ACTION-001: what kind of effect it is, how its success is
        // proven, and whether it was (or can be) undone.
        let undone_at_ms = d
            .run
            .outcome_json
            .as_deref()
            .and_then(|o| serde_json::from_str::<serde_json::Value>(o).ok())
            .and_then(|o| o.get("undone_at_ms").and_then(serde_json::Value::as_i64));
        let can_undo = document.is_some()
            && d.run.state == RunState::Succeeded
            && undone_at_ms.is_none()
            && d.action.as_ref().is_some_and(|a| {
                registry::by_id(&a.tool_id)
                    .is_some_and(|t| t.compensation == registry::Compensation::DeleteIfUnchanged)
            });
        RunView {
            run_id: d.run.id,
            title,
            status: status(d.run.state),
            effect_class: d
                .action
                .as_ref()
                .and_then(|a| a.effect_class)
                .map(|c| c.as_str().to_string()),
            evidence: d
                .action
                .as_ref()
                .and_then(|a| a.evidence)
                .map(|e| e.as_str().to_string()),
            undone_at_ms,
            can_undo,
            revision: d.run.revision,
            failure: d.run.error_code.as_ref().map(|code| RunFailure {
                code: code.clone(),
                message: d.run.error_message.clone().unwrap_or_default(),
            }),
            pending_approval,
            artifact: d.artifact.as_ref().map(|a| ArtifactSummary {
                artifact_id: a.id,
                path: self.artifact_root.join(&a.relpath).display().to_string(),
                sha256: a.sha256.clone(),
                size_bytes: u32::try_from(a.size_bytes).unwrap_or(u32::MAX),
                created_at_ms: a.created_at_ms,
            }),
            origin: d.origin.as_ref().map(|o| RunOrigin {
                message_id: o.message_id,
                conversation_id: o.conversation_id,
                conversation_title: o.conversation_title.clone(),
            }),
            kind: if call.is_some() {
                RunKind::McpCall
            } else if batch.is_some() {
                RunKind::RecordBatch
            } else if document.is_some() {
                RunKind::Document
            } else {
                RunKind::Note
            },
            call: call
                .as_ref()
                .map(|c| remote_call::call_view(c, d.run.outcome_json.as_deref())),
            requested_by: d.requested_by.as_ref().map(actor_view),
            decided_by: d.decided_by.as_ref().map(actor_view),
            created_at_ms: d.run.created_at_ms,
            updated_at_ms: d.run.updated_at_ms,
        }
    }
}

/// An actor as the dashboard shows it (ADR-IDENTITY-001).
pub(crate) fn actor_view(actor: &ActorRow) -> ActorView {
    ActorView {
        kind: actor.kind.as_str().to_string(),
        name: actor.name.clone(),
    }
}

fn note_content(detail: &RunDetail) -> Result<NoteContent, String> {
    let request: NoteRequest = serde_json::from_str(&detail.task.request_json)
        .map_err(|e| format!("stored request unreadable: {e}"))?;
    NoteContent::new(&request.title, &request.body).map_err(|e| e.to_string())
}

/// The bytes about to be written must be the ones the approval covers.
fn manifest_matches(
    manifest_json: &str,
    paths: &ArtifactPaths,
    content: &NoteContent,
) -> Result<(), String> {
    let manifest: ManifestCheck =
        serde_json::from_str(manifest_json).map_err(|e| format!("manifest unreadable: {e}"))?;
    if manifest.destination != paths.target_relpath {
        return Err("destination differs from the approved manifest".to_string());
    }
    if manifest.content_sha256 != content.sha256_hex() {
        return Err("content differs from the approved manifest".to_string());
    }
    Ok(())
}

async fn observe(paths: &ArtifactPaths, content: &NoteContent) -> Observed {
    let (p, c) = (paths.clone(), content.clone());
    tokio::task::spawn_blocking(move || note_tool::observe(&p, &c))
        .await
        .unwrap_or_else(|e| Observed::Uncertain(format!("observation failed: {e}")))
}

async fn discard_staging(paths: &ArtifactPaths) {
    let p = paths.clone();
    if let Ok(Err(err)) = tokio::task::spawn_blocking(move || note_tool::discard_staging(&p)).await
    {
        tracing::warn!(error = %err, "could not remove a leftover staging file");
    }
}

fn status(state: RunState) -> RunStatus {
    match state {
        RunState::Queued => RunStatus::Queued,
        RunState::Running => RunStatus::Running,
        RunState::AwaitingApproval => RunStatus::AwaitingApproval,
        RunState::Verifying => RunStatus::Verifying,
        RunState::CancellationRequested => RunStatus::CancellationRequested,
        RunState::NeedsReconciliation => RunStatus::NeedsReconciliation,
        RunState::NeedsReview => RunStatus::NeedsReview,
        RunState::Succeeded => RunStatus::Succeeded,
        RunState::Failed => RunStatus::Failed,
        RunState::Cancelled => RunStatus::Cancelled,
        RunState::Denied => RunStatus::Denied,
    }
}

/// User-facing errors keep a stable code; internal detail stays in the log.
fn to_nexus_error(err: StorageError) -> NexusError {
    let (code, message) = match &err {
        StorageError::NotFound(what) => (ErrorCode::NotFound, format!("{what} not found")),
        StorageError::IdempotencyConflict(_) => (ErrorCode::RevisionConflict, err.to_string()),
        StorageError::ApprovalExpired => (ErrorCode::ApprovalExpired, err.to_string()),
        StorageError::ConfirmationRequired => (ErrorCode::Forbidden, err.to_string()),
        StorageError::ApprovalInvalidated(_) | StorageError::ApprovalAlreadyDecided => {
            (ErrorCode::ApprovalInvalidated, err.to_string())
        }
        StorageError::IllegalTransition(_) => (ErrorCode::InvalidRequest, err.to_string()),
        StorageError::RevisionConflict(_) => (ErrorCode::RevisionConflict, err.to_string()),
        StorageError::ReplyInProgress => (ErrorCode::InvalidRequest, err.to_string()),
        StorageError::NotUndoable(_) => (ErrorCode::InvalidRequest, err.to_string()),
        StorageError::AlreadyShared => (ErrorCode::InvalidRequest, err.to_string()),
        _ => {
            tracing::error!(error = %err, "storage failure");
            (
                ErrorCode::InternalFailure,
                "internal storage failure".to_string(),
            )
        }
    };
    NexusError::new(code, message)
}
