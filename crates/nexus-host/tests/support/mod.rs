//! Shared test harness: a real host (pipe + runtime + SQLite) in an isolated
//! temporary profile, never the user's real `%LOCALAPPDATA%\Nexus`.

#![allow(dead_code)]

pub mod chat;
pub mod fake_gemini;
pub mod fake_mcp;
pub mod fake_provider;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::vault::SecretVault;
use nexus_host::{Runtime, RuntimeConfig};
use nexus_protocol::pairing::{self, PROFILE_DIR_ENV};

/// Debug-only override read by `nexus-host` (see `main.rs`).
pub const GEMINI_BASE_URL_ENV: &str = "NEXUS_GEMINI_BASE_URL";

/// Starts the real `nexus-host` binary on `profile` (its own pipe), optionally
/// set to abort at a fault point (debug builds only).
/// A real host process started by a test, stopped when it goes out of
/// scope: a test that fails half-way does not leave it running (holding
/// the test runner's output open and the built executable locked).
pub struct HostProcess(Child);

impl std::ops::Deref for HostProcess {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for HostProcess {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn spawn_host(profile: &Path, fault: Option<&str>) -> HostProcess {
    spawn_host_env(profile, fault, &[])
}

pub fn spawn_host_env(profile: &Path, fault: Option<&str>, envs: &[(&str, &str)]) -> HostProcess {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nexus-host"));
    cmd.env(PROFILE_DIR_ENV, profile)
        .env_remove(nexus_host::fault::FAULT_POINT_ENV)
        .env_remove(GEMINI_BASE_URL_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(point) = fault {
        cmd.env(nexus_host::fault::FAULT_POINT_ENV, point);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    HostProcess(cmd.spawn().expect("nexus-host binary must start"))
}

/// Connects to the real host on `profile` once it is up.
pub async fn connect(profile: &Path) -> NexusClient {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(token) = pairing::read_token_in(profile)
            && let Ok((client, _)) =
                NexusClient::connect_with(&pairing::pipe_name_for(profile), token, "real-host-test")
                    .await
        {
            return client;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "host did not come up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(std::time::Instant::now() < deadline, "host did not exit");
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub struct TempProfile(PathBuf);

impl TempProfile {
    pub fn new(kind: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nexus-{kind}-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempProfile {
    fn drop(&mut self) {
        // Test secrets live in Credential Manager under this profile's
        // prefix; remove them even when the test failed half-way.
        let _ = SecretVault::for_profile(&self.0).purge();
        // Best effort: on Windows a file still open by a lingering task
        // cannot be removed; that only leaves a stray temp directory.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn unique_pipe_name() -> String {
    format!(r"\\.\pipe\nexus-test-{}", uuid::Uuid::new_v4())
}

pub struct TestHost {
    pub pipe_name: String,
    pub token: String,
    pub runtime: Arc<Runtime>,
    pub profile: TempProfile,
    serve: tokio::task::JoinHandle<anyhow::Result<()>>,
    executor: tokio::task::JoinHandle<()>,
}

impl TestHost {
    pub async fn start(token: &str) -> Self {
        Self::start_with_ttl(token, nexus_host::DEFAULT_APPROVAL_TTL).await
    }

    pub async fn start_with_ttl(token: &str, approval_ttl: Duration) -> Self {
        Self::start_with(
            token,
            RuntimeConfig {
                approval_ttl,
                ..RuntimeConfig::default()
            },
        )
        .await
    }

    pub async fn start_with(token: &str, config: RuntimeConfig) -> Self {
        Self::start_in(TempProfile::new("host"), token, config).await
    }

    /// Starts on an existing profile (e.g. one prepared by the test).
    pub async fn start_in(profile: TempProfile, token: &str, config: RuntimeConfig) -> Self {
        let pipe_name = unique_pipe_name();
        let listener = nexus_host::bind_first_instance(&pipe_name)
            .expect("binding a fresh, uniquely-named pipe must succeed");
        let runtime = Runtime::open_with(profile.path(), config)
            .await
            .expect("runtime must open the profile");
        let executor = tokio::spawn(runtime.clone().run_executor());
        let serve = tokio::spawn(nexus_host::serve_forever(
            listener,
            pipe_name.clone(),
            token.to_string(),
            runtime.clone(),
        ));
        // Give the accept loop a tick to start listening before clients connect.
        tokio::task::yield_now().await;
        Self {
            pipe_name,
            token: token.to_string(),
            runtime,
            profile,
            serve,
            executor,
        }
    }

    pub async fn client(&self) -> nexus_client::NexusClient {
        nexus_client::NexusClient::connect_with(&self.pipe_name, self.token.clone(), "test-client")
            .await
            .expect("test client must connect")
            .0
    }

    /// Stops the host; the profile is returned so a test can keep inspecting
    /// it (e.g. a backup inside it) — it is deleted when dropped.
    pub async fn shutdown(self) -> TempProfile {
        self.runtime.request_stop();
        let _ = self.serve.await;
        let _ = self.executor.await;
        self.profile
    }
}
