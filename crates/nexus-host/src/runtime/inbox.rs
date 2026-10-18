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
    SendMessageRequest,
};
use nexus_storage::{FolderRow, InboxFileRow, now_ms};

use super::records::{JOB_APPLICATIONS, Register};
use super::{Runtime, to_nexus_error};
use crate::documents::{self, Format};
use crate::tools::markdown_artifact::sha256_hex;

/// How often an inbox folder is looked at.
pub const DEFAULT_INBOX_INTERVAL: Duration = Duration::from_secs(15);
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
/// file's own text is only read through `read_document`, as data).
fn task_text(register: &Register, folder: &FolderRow, file: &str) -> String {
    if register.row.key == JOB_APPLICATIONS {
        format!(
            "A new job ad was saved in the shared folder \"{label}\" as \"{file}\". Read it with read_document and record \
             it in job_applications: if an application with the same reference number, or the same company and role, \
             is already there, update it; otherwise add it with status \"To apply\". Fill every field the ad states: \
             company, role, reference (its reference or vacancy number, for example TopJobs's vacature number), source \
             (the platform or site it comes from, such as TopJobs, LinkedIn or Indeed), link (the ad's own web address, if it \
             shows one), location, work_mode, salary, contact, deadline (YYYY-MM-DD), requirements (the main ones, in \
             short). Set ad_file to \"{label}/{file}\". Do not apply, write or send anything else. Reply in one or two \
             lines with what you saved. If the file is not a job ad, save nothing and say so.",
            label = folder.label
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

    pub async fn save_inbox(
        &self,
        register: RegisterId,
        revision: i64,
        folder: Option<FolderId>,
        enabled: bool,
    ) -> Result<InboxView, NexusError> {
        self.register_by_id(register).await?;
        if let Some(id) = folder {
            self.store.folder(id).await.map_err(to_nexus_error)?;
        } else if enabled {
            return Err(invalid("choose the shared folder the inbox reads"));
        }
        self.store
            .save_inbox(register, revision, folder, enabled, now_ms())
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
            if let Err(err) = self.look_at(inbox.register_id, folder_id).await {
                tracing::warn!(error = %err.message, "inbox not read");
            }
        }
    }

    async fn look_at(
        self: &Arc<Self>,
        register_id: RegisterId,
        folder_id: FolderId,
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
                let skip = written
                    .contains(&sha)
                    .then(|| "a file Nexus wrote itself".to_string());
                self.store
                    .inbox_seen(register_id, name, sha, size as i64, skip, now_ms())
                    .await
                    .map_err(to_nexus_error)?;
            }
        }
        self.start_next(&register, &folder).await
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
            .next_inbox_file(register_id)
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
        let key = format!("inbox:{register_id}:{}", file.sha256);
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
        let sent = self
            .start_message(
                SendMessageRequest {
                    conversation_id: None,
                    project_id: None,
                    idempotency_key: key,
                    text: task_text(register, folder, &file.path),
                },
                Some(format!("{} · {}", inbox_title(register), file.path)),
            )
            .await;
        let (conversation, failure) = match sent {
            Ok(sent) => (Some(sent.conversation_id), None),
            Err(err) => (None, Some(err.message)),
        };
        self.store
            .inbox_outcome(file.id, conversation, failure, now_ms())
            .await
            .map_err(to_nexus_error)
    }
}

fn inbox_title(register: &Register) -> &'static str {
    if register.row.key == JOB_APPLICATIONS {
        "Job ad"
    } else {
        "New document"
    }
}
