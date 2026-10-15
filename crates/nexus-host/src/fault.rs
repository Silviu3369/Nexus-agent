//! Fault injection for real-process recovery tests (`SECURITATE_SI_VALIDARE_RO.md`
//! §5). Compiled only into debug builds: release binaries contain no hook.

pub const FAULT_POINT_ENV: &str = "NEXUS_FAULT_POINT";

/// Aborts the process — no destructors, no flushing beyond what is already
/// durable — when `NEXUS_FAULT_POINT` names this point.
#[cfg(debug_assertions)]
pub fn hit(point: &str) {
    if std::env::var(FAULT_POINT_ENV).as_deref() == Ok(point) {
        tracing::error!(point, "fault injection: aborting the host");
        std::process::abort();
    }
}

#[cfg(not(debug_assertions))]
pub fn hit(_point: &str) {}
