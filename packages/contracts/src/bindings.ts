// Generated from Rust by `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`. Do not edit.

export type RequestId = string

export type OperationId = string

/**
 * Stable error codes from `CONTRACTE_NEXUS_RO.md` §9. The code is what
 * callers branch on; `message` is user-facing text, never a stack trace.
 */
export type ErrorCode = "InvalidRequest" | "UnsupportedVersion" | "Unauthenticated" | "Forbidden" | "ApprovalRequired" | "ApprovalExpired" | "ApprovalInvalidated" | "RevisionConflict" | "ResourceChanged" | "NotFound" | "Busy" | "QuotaExceeded" | "DeadlineExceeded" | "ProviderUnavailable" | "Cancelled" | "OutcomeUnknown" | "InternalFailure"

/**
 * Wire-level error envelope. `retryable` is a declared characteristic, not
 * an instruction to retry blindly — callers still consult operation state,
 * tool semantics and budget before retrying (`CONTRACTE_NEXUS_RO.md` §9).
 */
export type NexusError = { code: ErrorCode; message: string; request_id: RequestId | null; operation_id: OperationId | null; retryable: boolean }

/**
 * What the dashboard may show about the host. Every variant comes from a
 * real connection attempt; there is no optimistic or cached "connected".
 */
export type HostStatus = { state: "Connected"; host_identity: string; protocol_version: number; uptime_seconds: number } | { state: "NotRunning" } | { state: "Unreachable"; reason: string } | { state: "Rejected"; error: NexusError } | { state: "Untrusted"; reason: string }

