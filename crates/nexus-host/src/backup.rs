//! BACKUP-01 (`ARHITECTURA_NEXUS_RO.md` §7): a consistent snapshot of the
//! database plus the artifact files it references, with a manifest of
//! hashes; and restore into a *new* profile that starts with execution
//! paused, so nothing is resumed automatically.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nexus_storage::{StorageError, Store, now_ms};
use serde::{Deserialize, Serialize};

use crate::tools::markdown_artifact::sha256_hex;

pub const MANIFEST_FILE: &str = "manifest.json";
pub const DATABASE_FILE: &str = "nexus.db";
/// Present in a restored profile: the host opens it with the executor and
/// reconciliation paused until a person reviews it and removes the marker.
pub const RESTORED_MARKER: &str = "restored.json";
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("verification failed: {0}")]
    Verification(String),
    #[error("{0} is not an empty directory; restore only creates new profiles")]
    TargetNotEmpty(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupArtifact {
    pub relpath: String,
    pub size_bytes: u64,
    /// Hash recorded when Nexus created the note.
    pub recorded_sha256: String,
    /// Hash of the file actually copied; differs if it was edited outside Nexus.
    pub copied_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format_version: u32,
    pub created_at_ms: i64,
    pub schema_version: i64,
    pub database_sha256: String,
    pub artifacts: Vec<BackupArtifact>,
}

impl BackupManifest {
    pub fn unmatched_artifacts(&self) -> usize {
        self.artifacts
            .iter()
            .filter(|a| a.recorded_sha256 != a.copied_sha256)
            .count()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RestoredMarker {
    restored_from: String,
    restored_at_ms: i64,
    executor: String,
}

/// Creates `<backups_dir>/<timestamp>/` holding the database copy, the
/// artifact files and the manifest. Writes are held off for the duration.
pub async fn create(
    store: &Store,
    artifact_root: &Path,
    backups_dir: &Path,
) -> Result<(PathBuf, BackupManifest), BackupError> {
    let created_at_ms = now_ms();
    let dir = backups_dir.join(format!(
        "{created_at_ms}-{}",
        &uuid::Uuid::new_v4().to_string()[..8]
    ));
    fs::create_dir_all(&dir)?;

    let (source_root, target_root) = (artifact_root.to_path_buf(), dir.join("artifacts"));
    let artifacts = store
        .backup_consistent(dir.join(DATABASE_FILE), move |rows| {
            rows.iter()
                .map(|row| copy_artifact(&source_root, &target_root, &row.relpath, &row.sha256))
                .collect::<Result<Vec<_>, _>>()
                .map_err(StorageError::Io)
        })
        .await?;

    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        created_at_ms,
        schema_version: store.schema_version().await?,
        database_sha256: file_sha256(&dir.join(DATABASE_FILE))?,
        artifacts,
    };
    write_json(&dir.join(MANIFEST_FILE), &manifest)?;
    Ok((dir, manifest))
}

fn copy_artifact(
    source_root: &Path,
    target_root: &Path,
    relpath: &str,
    recorded_sha256: &str,
) -> io::Result<BackupArtifact> {
    let target = target_root.join(relpath);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let size_bytes = fs::copy(source_root.join(relpath), &target)?;
    Ok(BackupArtifact {
        relpath: relpath.to_string(),
        size_bytes,
        recorded_sha256: recorded_sha256.to_string(),
        copied_sha256: file_sha256(&target)?,
    })
}

#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub profile: PathBuf,
    pub manifest: BackupManifest,
}

/// Restores a backup into `new_profile`, which must not exist or be empty.
/// Every file is checked against the manifest before and after copying; the
/// result is marked restored so the host keeps execution paused.
pub fn restore(backup_dir: &Path, new_profile: &Path) -> Result<RestoreReport, BackupError> {
    if new_profile.exists() && fs::read_dir(new_profile)?.next().is_some() {
        return Err(BackupError::TargetNotEmpty(new_profile.to_path_buf()));
    }
    let manifest: BackupManifest =
        serde_json::from_slice(&fs::read(backup_dir.join(MANIFEST_FILE))?)
            .map_err(|e| BackupError::Manifest(e.to_string()))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(BackupError::Manifest(format!(
            "backup format {} is not supported by this build",
            manifest.format_version
        )));
    }
    expect_hash(&backup_dir.join(DATABASE_FILE), &manifest.database_sha256)?;
    for artifact in &manifest.artifacts {
        expect_hash(
            &backup_dir.join("artifacts").join(&artifact.relpath),
            &artifact.copied_sha256,
        )?;
    }

    fs::create_dir_all(new_profile)?;
    fs::copy(
        backup_dir.join(DATABASE_FILE),
        new_profile.join(DATABASE_FILE),
    )?;
    expect_hash(&new_profile.join(DATABASE_FILE), &manifest.database_sha256)?;
    for artifact in &manifest.artifacts {
        let target = new_profile.join("artifacts").join(&artifact.relpath);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(
            backup_dir.join("artifacts").join(&artifact.relpath),
            &target,
        )?;
        expect_hash(&target, &artifact.copied_sha256)?;
    }
    write_json(
        &new_profile.join(RESTORED_MARKER),
        &RestoredMarker {
            restored_from: backup_dir.display().to_string(),
            restored_at_ms: now_ms(),
            executor: "paused".to_string(),
        },
    )?;
    Ok(RestoreReport {
        profile: new_profile.to_path_buf(),
        manifest,
    })
}

pub fn is_restored_profile(profile: &Path) -> bool {
    profile.join(RESTORED_MARKER).exists()
}

fn expect_hash(path: &Path, expected: &str) -> Result<(), BackupError> {
    let actual = file_sha256(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(BackupError::Verification(format!(
            "{} has SHA-256 {actual}, manifest says {expected}",
            path.display()
        )))
    }
}

fn file_sha256(path: &Path) -> io::Result<String> {
    Ok(sha256_hex(&fs::read(path)?))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    io::Write::write_all(&mut file, &text)?;
    file.sync_all()
}
