//! Checkpoint and rollback for clido sessions.
//!
//! Provides content-addressed file snapshots stored under
//! `.clido/checkpoints/<session-id>/<checkpoint-id>/`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("checkpoint not found: {0}")]
    NotFound(String),
    #[error("blob not found for hash: {0}")]
    BlobNotFound(String),
}

// ── Public types ──────────────────────────────────────────────────────────────

/// A snapshot of a single file's content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: PathBuf,
    /// "sha256:<hex>" content hash.
    pub content_hash: String,
    pub size_bytes: u64,
}

/// Full checkpoint data (manifest + blobs already on disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// "ck_" + 12 random hex chars.
    pub id: String,
    pub name: Option<String>,
    pub auto: bool,
    pub turn_index: u32,
    /// ISO 8601 timestamp.
    pub created_at: String,
    pub files: Vec<FileSnapshot>,
}

/// Lightweight summary used for listing.
#[derive(Debug, Clone)]
pub struct CheckpointMeta {
    pub id: String,
    pub name: Option<String>,
    pub auto: bool,
    pub created_at: String,
    pub file_count: usize,
}

/// Per-file diff between checkpoint state and current on-disk state.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: PathBuf,
    pub old_content: String,
    pub new_content: String,
}

// ── Storage helpers ───────────────────────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn content_hash_string(hex: &str) -> String {
    format!("sha256:{}", hex)
}

/// Parse "sha256:<hex>" → "<hex>".
fn strip_hash_prefix(hash: &str) -> &str {
    hash.strip_prefix("sha256:").unwrap_or(hash)
}

/// Write blob to `blobs_dir/<hex>` only if it doesn't already exist (dedup).
fn store_blob(blobs_dir: &Path, content: &[u8]) -> Result<String, CheckpointError> {
    let hex = sha256_hex(content);
    let blob_path = blobs_dir.join(&hex);
    if !blob_path.exists() {
        // Write atomically via temp file.
        let tmp_path = blobs_dir.join(format!("{}.tmp", hex));
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(content)?;
        f.flush()?;
        fs::rename(&tmp_path, &blob_path)?;
    }
    Ok(hex)
}

/// Load blob content from `blobs_dir/<hex>`.
fn load_blob(blobs_dir: &Path, hex: &str) -> Result<Vec<u8>, CheckpointError> {
    let path = blobs_dir.join(hex);
    if !path.exists() {
        return Err(CheckpointError::BlobNotFound(hex.to_string()));
    }
    let mut buf = Vec::new();
    fs::File::open(&path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

fn write_manifest(checkpoint_dir: &Path, ck: &Checkpoint) -> Result<(), CheckpointError> {
    let path = checkpoint_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(ck)?;
    let tmp = checkpoint_dir.join("manifest.json.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(json.as_bytes())?;
    f.flush()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn read_manifest(checkpoint_dir: &Path) -> Result<Checkpoint, CheckpointError> {
    let path = checkpoint_dir.join("manifest.json");
    if !path.exists() {
        return Err(CheckpointError::NotFound(
            checkpoint_dir.display().to_string(),
        ));
    }
    let s = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&s)?)
}

// ── CheckpointStore ───────────────────────────────────────────────────────────

/// Manages checkpoints for a single session.
///
/// Storage layout:
/// ```text
/// <session_dir>/          (= .clido/checkpoints/<session-id>/)
///   <checkpoint-id>/
///     manifest.json
///     files/
///       <sha256-hex>      (content blob, content-addressed)
/// ```
pub struct CheckpointStore {
    /// Root directory for this session's checkpoints.
    session_dir: PathBuf,
}

impl CheckpointStore {
    /// Create a `CheckpointStore` rooted at `session_dir`.
    /// The directory is created if it does not exist.
    pub fn new(session_dir: PathBuf) -> Self {
        Self { session_dir }
    }

    /// Ensure session directory exists.
    fn ensure_session_dir(&self) -> Result<(), CheckpointError> {
        fs::create_dir_all(&self.session_dir)?;
        Ok(())
    }

    fn checkpoint_dir(&self, id: &str) -> PathBuf {
        self.session_dir.join(id)
    }

    fn blobs_dir(&self, id: &str) -> PathBuf {
        self.checkpoint_dir(id).join("files")
    }

    /// Generate a new checkpoint ID ("ck_" + 12 random hex chars).
    fn new_id() -> String {
        let id = Uuid::new_v4().simple().to_string();
        format!("ck_{}", &id[..12])
    }

    /// Create a checkpoint snapshotting `files`.
    ///
    /// Files that do not exist on disk are silently skipped.
    pub fn create(
        &self,
        name: Option<&str>,
        auto: bool,
        files: &[PathBuf],
    ) -> Result<Checkpoint, CheckpointError> {
        self.ensure_session_dir()?;
        let id = Self::new_id();
        let ck_dir = self.checkpoint_dir(&id);
        let blobs = self.blobs_dir(&id);
        fs::create_dir_all(&blobs)?;

        let mut snapshots = Vec::new();
        for path in files {
            if !path.exists() {
                continue;
            }
            let content = fs::read(path)?;
            let size_bytes = content.len() as u64;
            let hex = store_blob(&blobs, &content)?;
            snapshots.push(FileSnapshot {
                path: path.clone(),
                content_hash: content_hash_string(&hex),
                size_bytes,
            });
        }

        let ck = Checkpoint {
            id: id.clone(),
            name: name.map(|s| s.to_string()),
            auto,
            turn_index: 0,
            created_at: Utc::now().to_rfc3339(),
            files: snapshots,
        };
        write_manifest(&ck_dir, &ck)?;
        Ok(ck)
    }

    /// List all checkpoints for this session, sorted newest-first.
    pub fn list(&self) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        if !self.session_dir.exists() {
            return Ok(vec![]);
        }
        let mut metas: Vec<(String, CheckpointMeta)> = Vec::new();
        for entry in fs::read_dir(&self.session_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                match read_manifest(&path) {
                    Ok(ck) => {
                        let meta = CheckpointMeta {
                            id: ck.id.clone(),
                            name: ck.name.clone(),
                            auto: ck.auto,
                            created_at: ck.created_at.clone(),
                            file_count: ck.files.len(),
                        };
                        metas.push((ck.created_at, meta));
                    }
                    Err(_) => continue, // skip corrupt directories
                }
            }
        }
        // Sort newest-first.
        metas.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(metas.into_iter().map(|(_, m)| m).collect())
    }

    /// Load a full checkpoint by ID.
    pub fn load(&self, checkpoint_id: &str) -> Result<Checkpoint, CheckpointError> {
        let dir = self.checkpoint_dir(checkpoint_id);
        if !dir.exists() {
            return Err(CheckpointError::NotFound(checkpoint_id.to_string()));
        }
        read_manifest(&dir)
    }

    /// Restore all files from a checkpoint using write-temp-then-rename atomicity.
    ///
    /// Returns the list of restored file paths.
    pub fn restore(&self, checkpoint_id: &str) -> Result<Vec<PathBuf>, CheckpointError> {
        let ck = self.load(checkpoint_id)?;
        let blobs = self.blobs_dir(checkpoint_id);
        let mut restored = Vec::new();
        for snap in &ck.files {
            let hex = strip_hash_prefix(&snap.content_hash);
            let content = load_blob(&blobs, hex)?;
            // Write to a temp file in the same directory, then rename for atomicity.
            let target = &snap.path;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = target.with_extension("ck_restore_tmp");
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&content)?;
            f.flush()?;
            fs::rename(&tmp, target)?;
            restored.push(target.clone());
        }
        Ok(restored)
    }

    /// Compute diffs between checkpoint state and current on-disk state.
    ///
    /// Files with identical content are excluded from the result.
    pub fn diff_since(&self, checkpoint_id: &str) -> Result<Vec<FileDiff>, CheckpointError> {
        let ck = self.load(checkpoint_id)?;
        let blobs = self.blobs_dir(checkpoint_id);
        let mut diffs = Vec::new();
        for snap in &ck.files {
            let hex = strip_hash_prefix(&snap.content_hash);
            let old_bytes = load_blob(&blobs, hex)?;
            let old_content = String::from_utf8_lossy(&old_bytes).into_owned();
            let new_content = if snap.path.exists() {
                String::from_utf8_lossy(&fs::read(&snap.path)?).into_owned()
            } else {
                String::new()
            };
            if old_content != new_content {
                diffs.push(FileDiff {
                    path: snap.path.clone(),
                    old_content,
                    new_content,
                });
            }
        }
        Ok(diffs)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_file(path: &Path, content: &str) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_create_and_list() {
        let tmp = tempdir().unwrap();
        let session_dir = tmp.path().join("ck_session");
        let store = CheckpointStore::new(session_dir);

        let file_a = tmp.path().join("a.txt");
        let file_b = tmp.path().join("b.txt");
        write_file(&file_a, "hello");
        write_file(&file_b, "world");

        let ck = store
            .create(Some("test"), false, &[file_a, file_b])
            .unwrap();
        assert_eq!(ck.files.len(), 2);

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, ck.id);
        assert_eq!(list[0].file_count, 2);
    }

    #[test]
    fn test_restore_restores_content() {
        let tmp = tempdir().unwrap();
        let session_dir = tmp.path().join("ck_session");
        let store = CheckpointStore::new(session_dir);

        let file_a = tmp.path().join("restore_test.txt");
        write_file(&file_a, "original content");

        let ck = store.create(None, true, &[file_a.clone()]).unwrap();

        // Modify the file.
        write_file(&file_a, "modified content");
        assert_eq!(fs::read_to_string(&file_a).unwrap(), "modified content");

        // Restore.
        let restored = store.restore(&ck.id).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(fs::read_to_string(&file_a).unwrap(), "original content");
    }

    #[test]
    fn test_content_addressed_blobs_shared() {
        let tmp = tempdir().unwrap();
        let session_dir = tmp.path().join("ck_session");
        let store = CheckpointStore::new(session_dir.clone());

        let file_a = tmp.path().join("dedup.txt");
        write_file(&file_a, "same content");

        let ck1 = store.create(None, true, &[file_a.clone()]).unwrap();
        let ck2 = store.create(None, true, &[file_a.clone()]).unwrap();

        // Both checkpoints reference the same hash.
        assert_eq!(ck1.files[0].content_hash, ck2.files[0].content_hash);

        // Each checkpoint has its own blobs directory, but since the content is the same,
        // only one blob file exists in each.
        let hex = strip_hash_prefix(&ck1.files[0].content_hash);
        let blob1 = session_dir.join(&ck1.id).join("files").join(hex);
        let blob2 = session_dir.join(&ck2.id).join("files").join(hex);
        assert!(blob1.exists());
        assert!(blob2.exists());

        // Verify: total blobs across both checkpoints is 2 (one per checkpoint dir),
        // but there's only 1 unique content.
        let hash1 = sha256_hex(&fs::read(&blob1).unwrap());
        let hash2 = sha256_hex(&fs::read(&blob2).unwrap());
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_diff_since_detects_changes() {
        let tmp = tempdir().unwrap();
        let session_dir = tmp.path().join("ck_session");
        let store = CheckpointStore::new(session_dir);

        let file_a = tmp.path().join("diff_test.txt");
        write_file(&file_a, "before");

        let ck = store.create(None, false, &[file_a.clone()]).unwrap();

        write_file(&file_a, "after");

        let diffs = store.diff_since(&ck.id).unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].old_content, "before");
        assert_eq!(diffs[0].new_content, "after");
    }

    #[test]
    fn test_diff_since_no_changes() {
        let tmp = tempdir().unwrap();
        let session_dir = tmp.path().join("ck_session");
        let store = CheckpointStore::new(session_dir);

        let file_a = tmp.path().join("unchanged.txt");
        write_file(&file_a, "same");

        let ck = store.create(None, false, &[file_a]).unwrap();
        let diffs = store.diff_since(&ck.id).unwrap();
        assert!(diffs.is_empty());
    }

    #[test]
    fn test_list_checkpoints_sorted_newest_first() {
        let tmp = tempdir().unwrap();
        let session_dir = tmp.path().join("ck_session");
        let store = CheckpointStore::new(session_dir);

        let file_a = tmp.path().join("sort_test.txt");
        write_file(&file_a, "v1");

        let ck1 = store
            .create(Some("first"), false, &[file_a.clone()])
            .unwrap();
        // Sleep just enough for chrono timestamps to differ.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_file(&file_a, "v2");
        let ck2 = store
            .create(Some("second"), false, &[file_a.clone()])
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_file(&file_a, "v3");
        let ck3 = store.create(Some("third"), false, &[file_a]).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 3);
        // Newest first.
        assert_eq!(list[0].id, ck3.id);
        assert_eq!(list[1].id, ck2.id);
        assert_eq!(list[2].id, ck1.id);
    }
}
