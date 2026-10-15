//! Shared test harness: a real host (pipe + runtime + SQLite) in an isolated
//! temporary profile, never the user's real `%LOCALAPPDATA%\Nexus`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nexus_host::Runtime;

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

    pub async fn shutdown(self) {
        self.runtime.request_stop();
        let _ = self.serve.await;
        let _ = self.executor.await;
    }
}
