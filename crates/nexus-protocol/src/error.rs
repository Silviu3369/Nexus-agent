use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::ids::{OperationId, RequestId};

/// Stable error codes from `CONTRACTE_NEXUS_RO.md` §9. The code is what
/// callers branch on; `message` is user-facing text, never a stack trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Error)]
pub enum ErrorCode {
    #[error("invalid request")]
    InvalidRequest,
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("forbidden")]
    Forbidden,
    #[error("approval required")]
    ApprovalRequired,
    #[error("approval expired")]
    ApprovalExpired,
    #[error("approval invalidated")]
    ApprovalInvalidated,
    #[error("revision conflict")]
    RevisionConflict,
    #[error("resource changed")]
    ResourceChanged,
    #[error("not found")]
    NotFound,
    #[error("busy")]
    Busy,
    #[error("quota exceeded")]
    QuotaExceeded,
    /// The user's own daily token budget is used up (not a provider limit).
    #[error("budget exceeded")]
    BudgetExceeded,
    #[error("deadline exceeded")]
    DeadlineExceeded,
    #[error("provider unavailable")]
    ProviderUnavailable,
    #[error("cancelled")]
    Cancelled,
    #[error("outcome unknown")]
    OutcomeUnknown,
    #[error("internal failure")]
    InternalFailure,
}

/// Wire-level error envelope. `retryable` is a declared characteristic, not
/// an instruction to retry blindly — callers still consult operation state,
/// tool semantics and budget before retrying (`CONTRACTE_NEXUS_RO.md` §9).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Error)]
#[error("{code}: {message}")]
pub struct NexusError {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: Option<RequestId>,
    pub operation_id: Option<OperationId>,
    pub retryable: bool,
}

impl NexusError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            request_id: None,
            operation_id: None,
            retryable: false,
        }
    }
}
