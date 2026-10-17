//! Isolation backend for work Nexus does not trust (`ARHITECTURA_NEXUS_RO.md`
//! §10, ADR-ISO-001). A process runs in a strict AppContainer (less
//! privileged, LPAC) inside a Job Object: it can read and write only the
//! paths it is granted, reaches the network only if allowed (never this
//! machine's own services), cannot open the user's other processes, pipes
//! or secrets, and the whole process tree is held to memory, process,
//! CPU-time, wall-time and output limits.
//!
//! A process, a working directory or a Job Object alone is not a sandbox;
//! the AppContainer token is the boundary and the probes in
//! `tests/isolation.rs` are the evidence that it holds.

#[cfg(windows)]
pub mod container;

#[cfg(windows)]
pub use container::{Access, ContainerError, ContainerSpec, Grant, Limits, Outcome, run};
