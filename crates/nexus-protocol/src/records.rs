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

use crate::ids::{ConversationId, RecordId, RegisterId};

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
