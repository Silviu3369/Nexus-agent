//! Shared test harness: a real host (pipe + runtime + SQLite) in an isolated
//! temporary profile, never the user's real `%LOCALAPPDATA%\Nexus`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::Runtime;
use nexus_protocol::pairing::{self, PROFILE_DIR_ENV};

/// Starts the real `nexus-host` binary on `profile` (its own pipe), optionally
/// set to abort at a fault point (debug builds only).
pub fn spawn_host(profile: &Path, fault: Option<&str>) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nexus-host"));
    cmd.env(PROFILE_DIR_ENV, profile)
        .env_remove(nexus_host::fault::FAULT_POINT_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(point) = fault {
        cmd.env(nexus_host::fault::FAULT_POINT_ENV, point);
    }
    cmd.spawn().expect("nexus-host binary must start")
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
        let profile = TempProfile::new("host");
        let pipe_name = unique_pipe_name();
        let listener = nexus_host::bind_first_instance(&pipe_name)
            .expect("binding a fresh, uniquely-named pipe must succeed");
        let runtime = Runtime::open(profile.path(), approval_ttl)
            .await
            .expect("runtime must open a fresh profile");
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
