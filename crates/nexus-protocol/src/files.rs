//! DTOs for the folders the user shares with Nexus (`PLAN_NEXUS_RO.md` §3a,
//! step 2): Nexus reads documents only inside them, and writes into one only
//! when the user allowed it there and approved the exact file.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::FolderId;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct FolderView {
    pub folder_id: FolderId,
    /// The name the agent and the user call it by, e.g. "Jobs".
    pub label: String,
    pub path: String,
    /// The agent may propose new files here (each one waits for approval).
    pub can_write: bool,
    /// It exists and is a folder Nexus can read now.
    pub available: bool,
    /// Why not, when it is not available.
    pub problem: Option<String>,
    pub revision: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveFolderRequest {
    /// `None` adds one.
    pub folder_id: Option<FolderId>,
    pub revision: i64,
    pub path: String,
    pub label: String,
    pub can_write: bool,
}

/// One file or folder inside a shared folder.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct FolderEntryView {
    /// Relative to the shared folder, with `/` between parts.
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
    pub modified_ms: i64,
    /// A format Nexus can read (text, Word, PDF, Excel, CSV).
    pub readable: bool,
}
