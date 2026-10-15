//! `CreateMarkdownArtifact` (`CONTRACTE_NEXUS_RO.md` §7): create one new note
//! inside the Nexus artifact store. The host derives every path from the
//! OperationId, so nothing the user typed can become a path (PATH-01).
//!
//! Write algorithm v1: stage under `.staging`, flush to disk, publish with a
//! hard link (fails if the target exists, so it never replaces), remove the
//! staging name, then re-read the target and check size and SHA-256.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nexus_protocol::{OperationId, ProjectId};
use sha2::{Digest, Sha256};

pub const TOOL_ID: &str = "CreateMarkdownArtifact";
pub const TOOL_VERSION: u32 = 1;
pub const ALGORITHM_VERSION: u32 = 1;
pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;
pub const MAX_TITLE_CHARS: usize = 200;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const STAGING_DIR: &str = ".staging";

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("title must be 1..={MAX_TITLE_CHARS} characters without control characters")]
    InvalidTitle,
    #[error("note content is {0} bytes; the limit is {MAX_CONTENT_BYTES}")]
    TooLarge(usize),
    #[error("{0} is a reparse point (junction or symlink); refusing to write through it")]
    ReparsePoint(PathBuf),
    #[error("{0} already exists; the tool never overwrites")]
    AlreadyExists(PathBuf),
    #[error("verification failed for {path}: {reason}")]
    VerificationFailed { path: PathBuf, reason: String },
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

/// The exact bytes the user approves. Built once by the host; the same value
/// feeds the preview, the manifest hash and the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteContent {
    pub title: String,
    pub text: String,
}

impl NoteContent {
    pub fn new(title: &str, body: &str) -> Result<Self, ArtifactError> {
        let title = title.trim();
        let chars = title.chars().count();
        if chars == 0 || chars > MAX_TITLE_CHARS || title.chars().any(char::is_control) {
            return Err(ArtifactError::InvalidTitle);
        }
        let text = format!("# {title}\n\n{body}\n");
        if text.len() > MAX_CONTENT_BYTES {
            return Err(ArtifactError::TooLarge(text.len()));
        }
        Ok(Self {
            title: title.to_string(),
            text,
        })
    }

    pub fn sha256_hex(&self) -> String {
        sha256_hex(self.text.as_bytes())
    }

    pub fn size(&self) -> u64 {
        self.text.len() as u64
    }
}

/// Where an operation's files live. Relative paths are what the database
/// stores; they are always `<project>/<operation>.md` style, never user text.
#[derive(Debug, Clone)]
pub struct ArtifactPaths {
    pub store_root: PathBuf,
    pub target_relpath: String,
    pub staging_relpath: String,
}

impl ArtifactPaths {
    pub fn for_operation(store_root: &Path, project: ProjectId, operation: OperationId) -> Self {
        Self {
            store_root: store_root.to_path_buf(),
            target_relpath: format!("{project}/{operation}.md"),
            staging_relpath: format!("{project}/{STAGING_DIR}/{operation}.tmp"),
        }
    }

    pub fn target(&self) -> PathBuf {
        self.store_root.join(&self.target_relpath)
    }

    pub fn staging(&self) -> PathBuf {
        self.store_root.join(&self.staging_relpath)
    }
}

/// What the filesystem shows for an operation, used on retry/restart to
/// decide without replaying the effect (`CONTRACTE_NEXUS_RO.md` §7 "La reluare").
#[derive(Debug, PartialEq, Eq)]
pub enum Observed {
    /// Neither the target nor a staging file exists.
    NoEffect,
    /// The target exists with exactly the expected size and hash.
    Complete,
    /// Something exists but it is partial, different or unreadable.
    Uncertain(String),
}

pub fn create(paths: &ArtifactPaths, content: &NoteContent) -> Result<(), ArtifactError> {
    let target = paths.target();
    let staging = paths.staging();
    let project_dir = target.parent().expect("target has a project directory");
    let staging_dir = staging.parent().expect("staging has a directory");

    ensure_real_dirs(&paths.store_root, staging_dir)?;
    if target.exists() {
        return Err(ArtifactError::AlreadyExists(target));
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    file.write_all(content.text.as_bytes())?;
    file.sync_all()?;
    drop(file);

    refuse_reparse_points(&paths.store_root, project_dir)?;
    // A hard link fails if `target` exists, so publishing never replaces.
    match fs::hard_link(&staging, &target) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            return Err(ArtifactError::AlreadyExists(target));
        }
        Err(err) => return Err(err.into()),
    }
    fs::remove_file(&staging)?;

    match observe(paths, content) {
        Observed::Complete => Ok(()),
        Observed::NoEffect => Err(ArtifactError::VerificationFailed {
            path: target,
            reason: "target missing right after publishing".to_string(),
        }),
        Observed::Uncertain(reason) => Err(ArtifactError::VerificationFailed {
            path: target,
            reason,
        }),
    }
}

pub fn observe(paths: &ArtifactPaths, expected: &NoteContent) -> Observed {
    let target = paths.target();
    let staging = paths.staging();
    match fs::symlink_metadata(&target) {
        Ok(meta) if is_reparse_point(&meta) => {
            Observed::Uncertain("target is a reparse point".to_string())
        }
        Ok(meta) if meta.len() != expected.size() => Observed::Uncertain(format!(
            "target is {} bytes, expected {}",
            meta.len(),
            expected.size()
        )),
        Ok(_) => match file_sha256_hex(&target) {
            Ok(hash) if hash == expected.sha256_hex() => Observed::Complete,
            Ok(_) => {
                Observed::Uncertain("target content differs from the approved note".to_string())
            }
            Err(err) => Observed::Uncertain(format!("target unreadable: {err}")),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if staging.exists() {
                Observed::Uncertain("staging file exists but was never published".to_string())
            } else {
                Observed::NoEffect
            }
        }
        Err(err) => Observed::Uncertain(format!("target state unknown: {err}")),
    }
}

/// Removes a leftover staging name once the target is proven complete.
pub fn discard_staging(paths: &ArtifactPaths) -> io::Result<()> {
    match fs::remove_file(paths.staging()) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Creates `dir` one level at a time, checking each level *before* descending
/// into it, so a planted junction is refused before anything is written
/// through it (unlike `create_dir_all`, which would follow it first).
fn ensure_real_dirs(store_root: &Path, dir: &Path) -> Result<(), ArtifactError> {
    fs::create_dir_all(store_root)?;
    check_not_reparse(store_root)?;
    let relative = dir
        .strip_prefix(store_root)
        .map_err(|_| ArtifactError::Io(io::Error::other("artifact path escaped the store root")))?;
    let mut current = store_root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::create_dir(&current) {
            Err(err) if err.kind() != io::ErrorKind::AlreadyExists => return Err(err.into()),
            _ => {}
        }
        check_not_reparse(&current)?;
    }
    Ok(())
}

/// Every directory from the store root down to `dir` must be a real
/// directory, so a planted junction cannot redirect the write elsewhere.
fn refuse_reparse_points(store_root: &Path, dir: &Path) -> Result<(), ArtifactError> {
    let mut current = store_root.to_path_buf();
    check_not_reparse(&current)?;
    let relative = dir
        .strip_prefix(store_root)
        .map_err(|_| ArtifactError::Io(io::Error::other("artifact path escaped the store root")))?;
    for component in relative.components() {
        current.push(component);
        check_not_reparse(&current)?;
    }
    Ok(())
}

fn check_not_reparse(path: &Path) -> Result<(), ArtifactError> {
    let meta = fs::symlink_metadata(path)?;
    if is_reparse_point(&meta) {
        return Err(ArtifactError::ReparsePoint(path.to_path_buf()));
    }
    Ok(())
}

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn file_sha256_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
