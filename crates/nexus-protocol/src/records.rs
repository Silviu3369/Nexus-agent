//! DTOs for registers (`PLAN_NEXUS_RO.md` §3a): tables Nexus keeps for the
//! user — job applications first — with defined fields, a status, a
//! follow-up date and the history of every change. The agent reads and
//! writes them directly; the user sees each change and can undo it.
//! Values travel as text: dates as `YYYY-MM-DD` (the user's local day),
//! choices as one of the field's choices. The Host checks every value
//! against the register's fields; an empty value clears the field.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{ConversationId, FolderId, RecordId, RegisterId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum FieldKind {
    /// One line.
    Text,
    /// Several lines.
    LongText,
    /// An http(s) address.
    Url,
    /// `YYYY-MM-DD`.
    Date,
    /// One of `choices`.
    Choice,
    /// A whole number, within `min`..=`max` when given.
    Number,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FieldDef {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
    pub choices: Vec<String>,
    pub max_chars: u32,
    /// What belongs in it, for the user and the model.
    pub help: String,
    /// Bounds of a number field.
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RegisterView {
    pub register_id: RegisterId,
    /// Stable name used by the agent's tools, e.g. `job_applications`.
    pub key: String,
    pub name: String,
    /// One record, e.g. "application".
    pub item_name: String,
    pub fields: Vec<FieldDef>,
    /// The choice field that is the record's status.
    pub status_field: String,
    /// Statuses after which a record needs no follow-up.
    pub closed_statuses: Vec<String>,
    /// The date field that says when to come back to a record.
    pub follow_up_field: Option<String>,
    /// Fields that name a record, e.g. company and role.
    pub title_fields: Vec<String>,
    pub record_count: u32,
    /// Open records whose follow-up date is today or earlier.
    pub follow_ups_due: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum RecordActor {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum RecordChangeKind {
    Created,
    Updated,
    Undo,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RecordView {
    pub record_id: RecordId,
    pub register_id: RegisterId,
    pub values: BTreeMap<String, String>,
    pub revision: i64,
    pub created_by: RecordActor,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Its follow-up date is today or earlier and it is still open.
    pub follow_up_due: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct FieldChange {
    pub key: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RecordChangeView {
    pub change_id: i64,
    pub kind: RecordChangeKind,
    pub actor: RecordActor,
    pub at_ms: i64,
    pub fields: Vec<FieldChange>,
    /// The task in which the agent made it.
    pub conversation_id: Option<ConversationId>,
    /// The agent of that task (`None`: Nexus, or the user's own change).
    pub agent: Option<String>,
    /// Web hosts the agent had read in that reply before making it.
    pub read_hosts: Vec<String>,
    pub undone: bool,
    /// It is the latest change and can be reversed.
    pub can_undo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RecordDetailView {
    pub record: RecordView,
    /// Newest first.
    pub history: Vec<RecordChangeView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RecordQuery {
    pub register_id: RegisterId,
    /// Any of these statuses; empty: all.
    pub statuses: Vec<String>,
    /// Text found in any field.
    pub text: Option<String>,
    /// Only open records due for follow-up.
    pub follow_up_due: bool,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveRecordRequest {
    pub register_id: RegisterId,
    /// `None` adds one.
    pub record_id: Option<RecordId>,
    /// The revision being edited (ignored when adding).
    pub revision: i64,
    /// Every field the user sees; an empty value clears it.
    pub values: BTreeMap<String, String>,
}

/// A register written to an Excel file in a shared folder.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ExportView {
    pub path: String,
    pub rows: u32,
}

/// What an Excel or CSV file would import, before anything is added.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportPreviewView {
    pub file: String,
    pub sheet: String,
    /// The file's header row.
    pub columns: Vec<String>,
    /// The field each column goes to (`None`: not imported), as guessed.
    pub mapping: Vec<Option<String>>,
    /// Data rows (after the header).
    pub rows: u32,
    /// The first rows, as in the file.
    pub sample: Vec<Vec<String>>,
    /// The file has more rows than one import reads.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportRequest {
    pub register_id: RegisterId,
    pub folder_id: FolderId,
    /// The file, relative to the shared folder.
    pub path: String,
    /// The field each column goes to, as the user confirmed it.
    pub mapping: Vec<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportSkipView {
    /// The row's number in the file (the header is row 1).
    pub row: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportResultView {
    pub added: u32,
    /// Rows whose value could not be used as is (kept in the notes).
    pub adjusted: u32,
    pub skipped: Vec<ImportSkipView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum InboxFileStatus {
    /// Seen; it is read and added once it stops changing and its turn comes.
    Waiting,
    /// Sent to the agent as a task, which is still answering.
    Started,
    /// Its task finished (what it did is in that task).
    Done,
    /// The task could not be started (no model, budget used up, …).
    Failed,
    /// Not a job ad to read (a file Nexus wrote itself, or unreadable).
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct InboxFileView {
    pub file_id: i64,
    /// Relative to the inbox folder.
    pub path: String,
    pub status: InboxFileStatus,
    pub reason: Option<String>,
    pub conversation_id: Option<ConversationId>,
    pub seen_at_ms: i64,
    pub started_at_ms: Option<i64>,
}

/// A register's inbox: a shared folder whose new documents the agent reads
/// and records on its own, one task per file.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct InboxView {
    pub register_id: RegisterId,
    pub folder_id: Option<FolderId>,
    pub folder_label: Option<String>,
    pub enabled: bool,
    pub revision: i64,
    /// The user's CV each new ad is compared with, if chosen.
    pub cv_folder_id: Option<FolderId>,
    pub cv_folder_label: Option<String>,
    pub cv_path: Option<String>,
    /// Files waiting for their turn.
    pub waiting: u32,
    /// Newest first.
    pub files: Vec<InboxFileView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveInboxRequest {
    pub register_id: RegisterId,
    /// The revision being changed (0 for an inbox never set).
    pub revision: i64,
    /// The shared folder it reads (`None`: no inbox).
    pub folder_id: Option<FolderId>,
    pub enabled: bool,
    /// The user's CV each new ad is compared with (`None`: no comparison).
    pub cv_folder_id: Option<FolderId>,
    pub cv_path: Option<String>,
}

/// Applications sent in one week (Monday first, local time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WeekCountView {
    /// The week's Monday, `YYYY-MM-DD`.
    pub week_start: String,
    pub sent: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ScoreBand {
    /// A match score of 70 or more.
    High,
    /// Below 70.
    Low,
    /// No match score.
    Unscored,
}

/// Sent applications in one group, and how many of them reached an
/// interview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct GroupCountView {
    pub sent: u32,
    pub interviews: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ScoreBandView {
    pub band: ScoreBand,
    pub counts: GroupCountView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SourceCountView {
    /// The source as written (platform, site); "Not set" when empty and
    /// "Other" for the rest after the largest ones.
    pub source: String,
    pub counts: GroupCountView,
}

/// What the job applications led to (`PLAN_NEXUS_RO.md` §3a), counted from
/// the applications and their history; nothing is estimated. "Sent" is an
/// application that was applied for (Applied, Interviewing, Offer,
/// Rejected, or withdrawn after applying); a stage counts once it was
/// reached, even if the status moved on, unless the user undid it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct JobResultsView {
    pub sent: u32,
    pub to_apply: u32,
    /// Sent applications that got an answer: an interview, an offer or a
    /// rejection.
    pub answered: u32,
    pub interviews: u32,
    pub offers: u32,
    /// Sent and still waiting for an answer; of those, sent more than 14
    /// days ago.
    pub waiting: u32,
    pub waiting_long: u32,
    /// Days from applying to the first answer (median), when known.
    pub median_days_to_answer: Option<u32>,
    /// The last 8 weeks, oldest first; the last one is this week.
    pub weeks: Vec<WeekCountView>,
    /// Sent applications whose date is unknown (not in `weeks`).
    pub undated: u32,
    pub by_score: Vec<ScoreBandView>,
    /// Most sent first.
    pub by_source: Vec<SourceCountView>,
    /// Applications Nexus added, job ads it read from the inbox, and its
    /// changes to applications, of which the user undid how many (an
    /// undone addition removes the application and is not counted).
    pub added_by_nexus: u32,
    pub ads_read: u32,
    pub nexus_changes: u32,
    pub undone_changes: u32,
    /// Only the newest applications were counted (a very large register).
    pub partial: bool,
    /// How the agent handled the job ads the inbox gave it.
    pub ads: AdsHandlingView,
}

/// A failure cause (ADR-TASK-001's closed list, or `NotStarted` when the
/// task could not even start) and how many ads it stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CauseCountView {
    pub cause: String,
    pub count: u32,
}

/// How the agent handled the job ads from the inbox (ADR-TASK-001),
/// counted from each ad's task and its runs; the cost is an estimate from
/// the prices the user entered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AdsHandlingView {
    /// Ads taken in (skipped files not counted).
    pub seen: u32,
    /// Ads whose task ended well; of those, the ones that left an
    /// application the user kept, and the ones where it recorded nothing.
    pub finished: u32,
    pub recorded: u32,
    pub nothing_recorded: u32,
    pub failed: u32,
    /// Waiting their turn or a retry, or being worked on.
    pub waiting: u32,
    /// Finished ads whose turn the user rejected, undid, corrected or
    /// stopped within its 7-day review window; accepted once the window
    /// passed with none of that; still in review before it ends.
    pub corrected: u32,
    pub accepted: u32,
    pub in_review: u32,
    pub failure_causes: Vec<CauseCountView>,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    /// For the finished ads, in US dollars; `None` when a model they used
    /// has no price entered.
    pub estimated_cost_usd: Option<f64>,
    /// Median time from the ad's task starting to its last reply.
    pub median_seconds: Option<u32>,
}
