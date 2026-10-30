//! A register's inbox (`PLAN_NEXUS_RO.md` §3a): the user picks a shared
//! folder; every new document put there (PDF, Word, text) is read by the
//! agent and recorded on its own — for job applications: a new row with the
//! ad's company, role, reference number and the rest. One task per file, one
//! at a time, exactly as if the user had sent it (the active model, the
//! daily budget, every change visible and undoable). A file is taken once it
//! stops changing (it may still be copying), and once per content: renamed
//! or copied again it is not read twice. Files Nexus wrote itself (an
//! approved letter) are skipped. Nothing in the folder is moved or changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nexus_protocol::{
    ErrorCode, FolderId, InboxFileStatus, InboxFileView, InboxView, NexusError, RegisterId,
    SaveInboxRequest, SendMessageRequest,
};
use nexus_storage::{FolderRow, InboxFileRow, InboxSettle, Requester, now_ms};

use super::packs;
use super::records::Register;
use super::{Runtime, to_nexus_error};
use crate::documents::{self, Format};
use crate::tools::markdown_artifact::sha256_hex;

/// How often an inbox folder is looked at.
pub const DEFAULT_INBOX_INTERVAL: Duration = Duration::from_secs(15);
/// The pause before a task stopped by a temporary provider error is tried
/// again; it grows with each try.
pub const DEFAULT_INBOX_RETRY: Duration = Duration::from_secs(120);
/// Tries per file before it waits for the user's Retry.
const MAX_ATTEMPTS: i64 = 3;
/// Reply errors that pass on their own (`ErrorCode` names as stored).
const TEMPORARY: [&str; 4] = [
    "QuotaExceeded",
    "ProviderUnavailable",
    "DeadlineExceeded",
    "Busy",
];
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const FILES_SHOWN: u32 = 50;

/// What the last look saw, to tell a file still being copied.
#[derive(Default)]
pub(super) struct InboxScan {
    last_ms: i64,
    /// Path → (size, modified) at the last look.
    seen: HashMap<PathBuf, (u64, i64)>,
}

pub(super) type InboxState = Mutex<InboxScan>;

fn invalid(message: impl Into<String>) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message)
}

fn file_view(row: &InboxFileRow) -> InboxFileView {
    InboxFileView {
        file_id: row.id,
        path: row.path.clone(),
        status: match row.status.as_str() {
            "Started" => InboxFileStatus::Started,
            "Done" => InboxFileStatus::Done,
            "Failed" => InboxFileStatus::Failed,
            "Skipped" => InboxFileStatus::Skipped,
            _ => InboxFileStatus::Waiting,
        },
        reason: row.reason.clone(),
        conversation_id: row.conversation_id,
        seen_at_ms: row.seen_at_ms,
        started_at_ms: row.started_at_ms,
    }
}

/// A document an inbox reads: PDF, Word or text, at the top of the folder,
/// not hidden, not a temporary file, not a link.
fn candidates(root: &Path) -> Vec<(String, PathBuf, u64, i64)> {
    use std::os::windows::fs::MetadataExt;
    let Ok(read) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('~') || name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.file_attributes() & HIDDEN_OR_SYSTEM != 0
            || meta.len() == 0
            || meta.len() > documents::MAX_FILE_BYTES
        {
            continue;
        }
        if !matches!(
            Format::of(&path),
            Some(Format::Pdf | Format::Word | Format::Text)
        ) {
            continue;
        }
        if path.extension().is_some_and(|e| {
            ["csv", "tsv", "json", "log"].contains(&e.to_ascii_lowercase().to_str().unwrap_or(""))
        }) {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push((name, path, meta.len(), modified));
    }
    out.sort();
    out
}

fn file_sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() as u64 <= documents::MAX_FILE_BYTES).then(|| sha256_hex(&bytes))
}

/// The message the agent receives for one file, written by Nexus (the
/// file's own text is only read through `read_document`, as data): where
/// the file and the CV are, then the instructions of the skill the pack's
/// intake names (ADR-PACK-001), given at once rather than loaded later.
fn task_text(
    register: &Register,
    folder: &FolderRow,
    file: &str,
    cv: Option<(&str, &str)>,
) -> String {
    let skill =
        packs::intake_of(&register.row.key).and_then(|(pack, intake)| pack.skill(&intake.skill));
    if let Some(skill) = skill {
        let cv = match cv {
            Some((cv_folder, cv_path)) => {
                format!(" The user's CV is in the folder \"{cv_folder}\", path \"{cv_path}\".")
            }
            None => " No CV is set to compare it with.".to_string(),
        };
        format!(
            "A new document was saved in the shared folder \"{label}\" as \"{file}\"; its ad_file is \
             \"{label}/{file}\".{cv} Follow the instructions of the skill {name}:\n\n{instructions}",
            label = folder.label,
            name = skill.name,
            instructions = skill.instructions
        )
    } else {
        format!(
            "A new document was saved in the shared folder \"{label}\" as \"{file}\". Read it with read_document and, if it \
             belongs in {key} ({name}), record it there (update a record that is already there rather than adding it \
             twice). Do not write or send anything else. Reply in one or two lines with what you saved.",
            label = folder.label,
            key = register.row.key,
            name = register.row.name
        )
    }
}

impl Runtime {
    async fn inbox_view(&self, register: RegisterId) -> Result<InboxView, NexusError> {
        let row = self.store.inbox(register).await.map_err(to_nexus_error)?;
        let folder_label = match row.folder_id {
            Some(id) => self.store.folder(id).await.ok().map(|f| f.label),
            None => None,
        };
        let cv_folder_label = match row.cv_folder_id {
            Some(id) => self.store.folder(id).await.ok().map(|f| f.label),
            None => None,
        };
        let files = self
            .store
            .inbox_files(register, FILES_SHOWN)
            .await
            .map_err(to_nexus_error)?;
        Ok(InboxView {
            register_id: register,
            folder_id: row.folder_id,
            folder_label,
            enabled: row.enabled && row.folder_id.is_some(),
            revision: row.revision,
            cv_folder_id: row.cv_folder_id.filter(|_| cv_folder_label.is_some()),
            cv_folder_label,
            cv_path: row.cv_path.filter(|_| row.cv_folder_id.is_some()),
            waiting: self
                .store
                .inbox_waiting(register)
                .await
                .map_err(to_nexus_error)?,
            files: files.iter().map(file_view).collect(),
        })
    }

    pub async fn get_inbox(&self, register: RegisterId) -> Result<InboxView, NexusError> {
        self.register_by_id(register).await?;
        self.inbox_view(register).await
    }

    pub async fn save_inbox(&self, request: SaveInboxRequest) -> Result<InboxView, NexusError> {
        let SaveInboxRequest {
            register_id: register,
            revision,
            folder_id: folder,
            enabled,
            cv_folder_id: cv_folder,
            cv_path,
        } = request;
        self.register_by_id(register).await?;
        if let Some(id) = folder {
            self.store.folder(id).await.map_err(to_nexus_error)?;
        } else if enabled {
            return Err(invalid("choose the shared folder the inbox reads"));
        }
        let cv_path = match (
            cv_folder,
            cv_path
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty()),
        ) {
            (None, _) => None,
            (Some(_), None) => return Err(invalid("name the CV file in that folder")),
            (Some(id), Some(path)) => {
                let cv_root =
                    PathBuf::from(self.store.folder(id).await.map_err(to_nexus_error)?.path);
                let checked = path.clone();
                let resolved =
                    tokio::task::spawn_blocking(move || documents::resolve_in(&cv_root, &checked))
                        .await
                        .map_err(|e| NexusError::new(ErrorCode::InternalFailure, e.to_string()))?
                        .map_err(invalid)?;
                if !matches!(
                    Format::of(&resolved),
                    Some(Format::Pdf | Format::Word | Format::Text)
                ) {
                    return Err(invalid("the CV must be a PDF, Word (.docx) or text file"));
                }
                Some(path.replace('\\', "/"))
            }
        };
        self.store
            .save_inbox(
                register,
                revision,
                folder,
                enabled,
                cv_folder,
                cv_path,
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        // Look at it on the next tick rather than in a whole interval.
        self.inbox_state.lock().unwrap().last_ms = 0;
        self.inbox_view(register).await
    }

    pub async fn retry_inbox_file(
        &self,
        register: RegisterId,
        file_id: i64,
    ) -> Result<InboxView, NexusError> {
        self.store
            .retry_inbox_file(register, file_id, now_ms())
            .await
            .map_err(to_nexus_error)?;
        self.inbox_view(register).await
    }

    /// Called by the executor every tick; looks at each inbox once per
    /// interval and starts at most one waiting file per inbox.
    pub(super) async fn run_inboxes(self: &Arc<Self>) {
        let now = now_ms();
        {
            let mut state = self.inbox_state.lock().unwrap();
            let interval = i64::try_from(self.inbox_interval.as_millis()).unwrap_or(i64::MAX);
            if now - state.last_ms < interval {
                return;
            }
            state.last_ms = now;
        }
        let inboxes = match self.store.active_inboxes().await {
            Ok(inboxes) => inboxes,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the inboxes");
                return;
            }
        };
        for inbox in inboxes {
            let Some(folder_id) = inbox.folder_id else {
                continue;
            };
            // A CV kept in the inbox folder itself is not an ad.
            let cv = inbox
                .cv_path
                .filter(|_| inbox.cv_folder_id == Some(folder_id));
            if let Err(err) = self.look_at(inbox.register_id, folder_id, cv).await {
                tracing::warn!(error = %err.message, "inbox not read");
            }
        }
    }

    async fn look_at(
        self: &Arc<Self>,
        register_id: RegisterId,
        folder_id: FolderId,
        cv: Option<String>,
    ) -> Result<(), NexusError> {
        let folder = self.store.folder(folder_id).await.map_err(to_nexus_error)?;
        let root = PathBuf::from(&folder.path);
        if documents::folder_problem(&root).is_some() {
            return Ok(());
        }
        let register = self.register_by_id(register_id).await?;
        let found = {
            let root = root.clone();
            tokio::task::spawn_blocking(move || candidates(&root))
                .await
                .unwrap_or_default()
        };
        // Only a file that looked the same at the last look is ready.
        let ready: Vec<(String, PathBuf, u64)> = {
            let mut state = self.inbox_state.lock().unwrap();
            let mut ready = Vec::new();
            let mut now_seen = HashMap::new();
            for (name, path, size, modified) in found {
                if state.seen.get(&path) == Some(&(size, modified)) {
                    ready.push((name, path.clone(), size));
                }
                now_seen.insert(path, (size, modified));
            }
            state.seen.retain(|p, _| !p.starts_with(&root));
            state.seen.extend(now_seen);
            ready
        };
        if !ready.is_empty() {
            let known: HashSet<String> = self
                .store
                .inbox_known(register_id)
                .await
                .map_err(to_nexus_error)?
                .into_iter()
                .collect();
            let written: HashSet<String> = self
                .store
                .written_document_hashes()
                .await
                .map_err(to_nexus_error)?
                .into_iter()
                .collect();
            for (name, path, size) in ready {
                let Some(sha) = tokio::task::spawn_blocking(move || file_sha256(&path))
                    .await
                    .ok()
                    .flatten()
                else {
                    continue;
                };
                if known.contains(&sha) {
                    continue;
                }
                let skip = if cv
                    .as_deref()
                    .is_some_and(|cv| cv.to_lowercase() == name.to_lowercase())
                {
                    Some("your CV, used for the match score".to_string())
                } else {
                    written
                        .contains(&sha)
                        .then(|| "a file Nexus wrote itself".to_string())
                };
                self.store
                    .inbox_seen(register_id, name, sha, size as i64, skip, now_ms())
                    .await
                    .map_err(to_nexus_error)?;
            }
        }
        self.settle_started(register_id).await?;
        self.start_next(&register, &folder).await
    }

    /// Follows each started file's task to its end: done, tried again later
    /// after a temporary provider error (or tomorrow once the daily budget
    /// is used up), or failed for the user to retry.
    async fn settle_started(&self, register_id: RegisterId) -> Result<(), NexusError> {
        let now = now_ms();
        for file in self
            .store
            .inbox_started(register_id)
            .await
            .map_err(to_nexus_error)?
        {
            let settle = match file.conversation_id {
                // The user deleted the task: nothing more to follow.
                None => InboxSettle::Done,
                Some(conversation) => {
                    match self
                        .store
                        .last_reply(conversation)
                        .await
                        .map_err(to_nexus_error)?
                    {
                        Some((status, _, _)) if status == "Streaming" => continue,
                        None => InboxSettle::Done,
                        Some((status, _, _)) if status == "Complete" => InboxSettle::Done,
                        Some((status, _, _)) if status == "Cancelled" => {
                            InboxSettle::Failed("stopped by you".to_string())
                        }
                        Some((_, code, message)) => {
                            let message = message.unwrap_or_else(|| "the reply failed".to_string());
                            self.later(code.as_deref(), &message, file.attempts, now)
                                .unwrap_or(InboxSettle::Failed(message))
                        }
                    }
                }
            };
            self.store
                .settle_inbox_file(file.id, settle, now)
                .await
                .map_err(to_nexus_error)?;
        }
        Ok(())
    }

    /// When a file whose task could not run is tried again on its own, if
    /// at all.
    fn later(
        &self,
        code: Option<&str>,
        message: &str,
        attempts: i64,
        now: i64,
    ) -> Option<InboxSettle> {
        match code {
            Some("BudgetExceeded") => Some(InboxSettle::RetryAt {
                at: super::usage::local_day_start_ms(now) + DAY_MS + 60_000,
                reason: "the daily token budget is used up; tried again tomorrow".to_string(),
            }),
            Some(code) if TEMPORARY.contains(&code) && attempts < MAX_ATTEMPTS => {
                let pause = i64::try_from(self.inbox_retry.as_millis())
                    .unwrap_or(i64::MAX)
                    .saturating_mul(attempts.max(1));
                let at = now.saturating_add(pause);
                let clock = crate::schedule::local_label(at, None);
                Some(InboxSettle::RetryAt {
                    at,
                    reason: format!(
                        "tried again at {}: {message}",
                        clock.get(11..).unwrap_or(&clock)
                    ),
                })
            }
            _ => None,
        }
    }

    /// The oldest waiting file, unless the previous one's reply is still
    /// being written (one at a time).
    async fn start_next(
        self: &Arc<Self>,
        register: &Register,
        folder: &FolderRow,
    ) -> Result<(), NexusError> {
        let register_id = register.row.id;
        let Some(file) = self
            .store
            .next_inbox_file(register_id, now_ms())
            .await
            .map_err(to_nexus_error)?
        else {
            return Ok(());
        };
        let recent = self
            .store
            .inbox_files(register_id, 20)
            .await
            .map_err(to_nexus_error)?;
        if let Some(previous) = recent
            .iter()
            .find(|f| f.status == "Started")
            .and_then(|f| f.conversation_id)
            && self
                .store
                .conversation_answering(previous)
                .await
                .map_err(to_nexus_error)?
        {
            return Ok(());
        }
        // A new key for each try: a retry is a new task, never a replay.
        let key = match file.attempts {
            0 => format!("inbox:{register_id}:{}", file.sha256),
            n => format!("inbox:{register_id}:{}:{n}", file.sha256),
        };
        // Sent just before a stop: record the task it started, never send again.
        if let Some(conversation) = self
            .store
            .conversation_with_key(key.clone())
            .await
            .map_err(to_nexus_error)?
        {
            return self
                .store
                .inbox_outcome(file.id, Some(conversation), None, now_ms())
                .await
                .map_err(to_nexus_error);
        }
        let inbox = self
            .store
            .inbox(register_id)
            .await
            .map_err(to_nexus_error)?;
        let cv = match (inbox.cv_folder_id, inbox.cv_path) {
            (Some(id), Some(path)) => self.store.folder(id).await.ok().map(|f| (f.label, path)),
            _ => None,
        };
        let sent = self
            .start_message(
                SendMessageRequest {
                    conversation_id: None,
                    agent_id: None,
                    project_id: None,
                    idempotency_key: key,
                    text: task_text(
                        register,
                        folder,
                        &file.path,
                        cv.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
                    ),
                },
                Some(format!("{} · {}", inbox_title(register), file.path)),
                Requester::Inbox,
                // The pack whose intake rules the task runs under, if any.
                Some(match packs::intake_of(&register.row.key) {
                    Some((pack, _)) => pack.id.clone(),
                    None => register.row.key.clone(),
                }),
            )
            .await;
        let (conversation, failure) = match sent {
            Ok(sent) => (Some(sent.conversation_id), None),
            Err(err) => {
                // The daily budget passes with the day: wait for it.
                let code = format!("{:?}", err.code);
                if let Some(settle) =
                    self.later(Some(code.as_str()), &err.message, MAX_ATTEMPTS, now_ms())
                {
                    return self
                        .store
                        .settle_inbox_file(file.id, settle, now_ms())
                        .await
                        .map_err(to_nexus_error);
                }
                (None, Some(err.message))
            }
        };
        self.store
            .inbox_outcome(file.id, conversation, failure, now_ms())
            .await
            .map_err(to_nexus_error)
    }
}

fn inbox_title(register: &Register) -> &'static str {
    packs::intake_of(&register.row.key).map_or("New document", |(_, intake)| intake.title.as_str())
}
