//! Local pairing-secret file shared by host and client. This is deliberately
//! not a new cryptographic protocol (`ARHITECTURA_NEXUS_RO.md` §6 forbids
//! inventing one) — it is a random token in a per-profile file, readable
//! only by the owning Windows account via the directory's inherited ACL.
//! It proves "this caller can read my profile", nothing stronger; it is one
//! input to IPC-02, not the whole authentication story.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// Overrides the profile directory: isolated test profiles and restoring a
/// backup into a separate profile (`PLAN_NEXUS_RO.md` M1 §9).
pub const PROFILE_DIR_ENV: &str = "NEXUS_PROFILE_DIR";

/// Per-profile data directory (`%LOCALAPPDATA%\Nexus` by default), never the
/// checkout or install directory (`ARHITECTURA_NEXUS_RO.md` §7).
pub fn profile_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(PROFILE_DIR_ENV) {
        return PathBuf::from(dir);
    }
    default_profile_dir()
}

fn default_profile_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Nexus")
}

pub fn token_file_path() -> PathBuf {
    token_file_path_in(&profile_dir())
}

pub fn token_file_path_in(profile: &Path) -> PathBuf {
    profile.join("host.pairing-token")
}

/// Named pipe path for the active profile. Host and client must compute
/// this identically.
pub fn pipe_name() -> String {
    pipe_name_for(&profile_dir())
}

/// Scoped per Windows account so two users never collide, and per profile
/// so a test or restored profile never reaches the user's real host
/// (`ARHITECTURA_NEXUS_RO.md` §4: per-profile, not merely per-machine).
pub fn pipe_name_for(profile: &Path) -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
    if profile == default_profile_dir() {
        format!(r"\\.\pipe\nexus-host-{user}")
    } else {
        format!(r"\\.\pipe\nexus-host-{user}-{:016x}", fnv1a(profile))
    }
}

/// Stable, dependency-free name suffix. Not a security property: the pipe is
/// protected by the SID check and the token, not by an unguessable name.
fn fnv1a(path: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Host-side only: create the token on first run, or load the existing one.
pub fn load_or_create_token() -> io::Result<String> {
    load_or_create_token_in(&profile_dir())
}

pub fn load_or_create_token_in(profile: &Path) -> io::Result<String> {
    fs::create_dir_all(profile)?;
    let path = token_file_path_in(profile);
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
    read_token_in(&profile_dir())
}

pub fn read_token_in(profile: &Path) -> io::Result<String> {
    fs::read_to_string(token_file_path_in(profile)).map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternate_profiles_get_their_own_pipe() {
        let default = pipe_name_for(&default_profile_dir());
        let test_a = pipe_name_for(Path::new(r"C:\tmp\profile-a"));
        let test_b = pipe_name_for(Path::new(r"C:\tmp\profile-b"));
        assert_ne!(default, test_a);
        assert_ne!(test_a, test_b);
        assert_eq!(
            test_a,
            pipe_name_for(Path::new(r"C:\TMP\Profile-A")),
            "case-insensitive like NTFS"
        );
    }
}
