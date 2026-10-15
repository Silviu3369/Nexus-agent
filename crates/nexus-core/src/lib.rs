//! Domain core: task, run, step, action, approval and policy state, kept
//! independent of Tauri, React, SQLite drivers and provider SDKs.

pub mod state;

pub use state::{
    ActionState, IllegalTransition, MessageRole, MessageStatus, OperationState, RunState,
    UnknownState,
};
