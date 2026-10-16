//! Domain core: task, run, step, action, approval and policy state, kept
//! independent of Tauri, React, SQLite drivers and provider SDKs.

pub mod state;

pub use state::{
    ActionState, AgentStepKind, AgentStepStatus, IllegalTransition, MemoryScope, MemorySource,
    MemoryStatus, MessageRole, MessageStatus, OperationState, RunState, ThinkingLevel,
    UnknownState,
};
