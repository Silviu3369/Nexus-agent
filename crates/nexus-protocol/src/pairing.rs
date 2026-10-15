//! Local pairing-secret file shared by host and client. This is deliberately
//! not a new cryptographic protocol (`ARHITECTURA_NEXUS_RO.md` §6 forbids
//! inventing one) — it is a random token in a per-profile file, readable
//! only by the owning Windows account via the directory's inherited ACL.
//! It proves "this caller can read my profile", nothing stronger; it is one
//! input to IPC-02, not the whole authentication story.

use std::fs;
use std::io;
use std::path::PathBuf;

use uuid::Uuid;

/// Per-profile data directory (`%LOCALAPPDATA%\Nexus`), never the checkout or
/// install directory (`ARHITECTURA_NEXUS_RO.md` §7).
pub fn profile_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Nexus")
}

pub fn token_file_path() -> PathBuf {
    profile_dir().join("host.pairing-token")
}

/// Named pipe path, scoped per Windows account so two users on the same
/// machine never collide (`ARHITECTURA_NEXUS_RO.md` §4: per-profile, not
/// merely per-machine). Host and client must compute this identically.
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
    format!(r"\\.\pipe\nexus-host-{user}")
}

/// Host-side only: create the token on first run, or load the existing one.
pub fn load_or_create_token() -> io::Result<String> {
    fs::create_dir_all(profile_dir())?;
    let path = token_file_path();
    match fs::read_to_string(&path) {
        Ok(existing) if !existing.trim().is_empty() => Ok(existing.trim().to_string()),
        _ => {
            let token = Uuid::new_v4().to_string();
            fs::write(&path, &token)?;
            Ok(token)
        }
    }
}

/// Client-side: read the token the host already created. Fails if the host
/// has never started in this profile — there is nothing to present yet.
pub fn read_token() -> io::Result<String> {
    fs::read_to_string(token_file_path()).map(|s| s.trim().to_string())
}
