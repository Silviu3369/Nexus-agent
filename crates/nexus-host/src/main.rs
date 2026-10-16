use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Rotate once past this size so the log stays bounded (≤ 2× this on disk).
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("restore") {
        return restore(&args[1..]);
    }

    let profile = nexus_protocol::pairing::profile_dir();
    init_logging(&profile)?;

    let pairing_token = nexus_protocol::pairing::load_or_create_token_in(&profile)?;
    let name = nexus_protocol::pairing::pipe_name_for(&profile);

    // The single-instance lock comes first: a second host must never open
    // the database or reconcile runs owned by the first.
    let listener = nexus_host::bind_first_instance(&name).inspect_err(|err| {
        // Logged before returning: a detached host has no console for stderr.
        tracing::error!(pid = std::process::id(), error = %err, "nexus-host not started");
    })?;

    let runtime = nexus_host::Runtime::open_with(&profile, runtime_config())
        .await
        .inspect_err(|err| tracing::error!(error = %err, "could not open the profile database"))?;
    let executor = tokio::spawn(runtime.clone().run_executor());
    tracing::info!(pipe = %name, pid = std::process::id(), "nexus-host listening");

    nexus_host::serve_forever(listener, name, pairing_token, runtime).await?;
    // Let the step in progress finish and record its outcome before exiting.
    executor.await?;
    tracing::info!(pid = std::process::id(), "nexus-host stopped");
    Ok(())
}

/// Release builds always talk to each provider's own endpoint. Debug builds
/// may point a provider at a local test server (its adapter still refuses
/// anything but loopback).
fn runtime_config() -> nexus_host::RuntimeConfig {
    #[allow(unused_mut)]
    let mut config = nexus_host::RuntimeConfig::default();
    #[cfg(debug_assertions)]
    {
        let set = |name: &str, field: &mut String| {
            if let Ok(url) = std::env::var(name) {
                *field = url;
            }
        };
        set("NEXUS_GEMINI_BASE_URL", &mut config.gemini_base_url);
        set("NEXUS_OPENAI_BASE_URL", &mut config.openai_base_url);
        set("NEXUS_ANTHROPIC_BASE_URL", &mut config.anthropic_base_url);
        set("NEXUS_DEEPSEEK_BASE_URL", &mut config.deepseek_base_url);
        config.allow_loopback_mcp =
            std::env::var("NEXUS_ALLOW_LOOPBACK_MCP").is_ok_and(|v| v == "1");
    }
    config
}

/// `nexus-host restore <backup-dir> <new-profile-dir>`: offline, never touches
/// a running host or an existing profile.
fn restore(args: &[String]) -> anyhow::Result<()> {
    let [backup_dir, new_profile] = args else {
        anyhow::bail!("usage: nexus-host restore <backup-dir> <new-profile-dir>");
    };
    let report = nexus_host::backup::restore(Path::new(backup_dir), Path::new(new_profile))?;
    println!(
        "restored {} note(s) into {}; execution stays paused until {} is removed from that profile",
        report.manifest.artifacts.len(),
        report.profile.display(),
        nexus_host::backup::RESTORED_MARKER
    );
    Ok(())
}

/// When the dashboard starts the host detached there is no console, so the
/// log also goes to the profile directory.
fn init_logging(profile: &Path) -> anyhow::Result<()> {
    let log_dir = profile.join("logs");
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
