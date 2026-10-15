use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Rotate once past this size so the log stays bounded (≤ 2× this on disk).
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging()?;

    let pairing_token = nexus_protocol::pairing::load_or_create_token()?;
    let name = nexus_protocol::pairing::pipe_name();

    // Logged before returning: a detached host has no console for stderr.
    let listener = nexus_host::bind_first_instance(&name).inspect_err(|err| {
        tracing::error!(pid = std::process::id(), error = %err, "nexus-host not started");
    })?;
    tracing::info!(pipe = %name, pid = std::process::id(), "nexus-host listening");

    nexus_host::serve_forever(listener, name, pairing_token).await
}

/// When the dashboard starts the host detached there is no console, so the
/// log also goes to the profile directory.
fn init_logging() -> anyhow::Result<()> {
    let log_dir = nexus_protocol::pairing::profile_dir().join("logs");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("nexus-host.log");
    rotate_if_large(&log_path)?;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::io::stdout.and(Mutex::new(file)))
        .try_init()
        .ok();
    Ok(())
}

fn rotate_if_large(log_path: &Path) -> std::io::Result<()> {
    match fs::metadata(log_path) {
        Ok(meta) if meta.len() > MAX_LOG_BYTES => {
            fs::rename(log_path, log_path.with_extension("log.old"))
        }
        _ => Ok(()),
    }
}
