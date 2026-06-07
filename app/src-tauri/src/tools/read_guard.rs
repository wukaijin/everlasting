//! ReadGuard — tracks per-session file read fingerprints to prevent
//! the LLM from editing stale content.
//!
//! See `.trellis/tasks/06-07-06-07-extend-toolset/prd.md` §R5 for the
//! full contract. In short:
//! - `read_file` calls `record_read` after a successful read.
//! - `edit_file` calls `verify_read` (was the file read in this session?)
//!   and `verify_fresh` (did the file change on disk since the read?)
//!   before applying the edit.
//! - `edit_file` calls `invalidate` on the path after a successful write,
//!   forcing the LLM to re-read on the next edit.
//!
//! The guard is session-isolated: switching back to a previous session
//! restores the fingerprints recorded in that session, so the LLM does
//! not have to re-read files it already saw.
//!
//! Storage: in-process `Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>`.
//! The lifetime is the process; on restart the guard forgets everything,
//! which is safe (the first edit attempt will fail with "must read first",
//! and the LLM will re-read).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use xxhash_rust::xxh64::xxh64;

/// A session identifier — the same string used by the DB's
/// `sessions.id` column.
pub type SessionId = String;

/// One fingerprint of a file at the time it was last read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fingerprint {
    /// Last-modified time as reported by `std::fs::metadata` at read time.
    /// None when the OS/filesystem does not support mtime (e.g. some
    /// FAT filesystems); in that case freshness falls back to size + head
    /// hash alone.
    pub mtime: Option<SystemTime>,
    /// File size in bytes at read time. Cheap and works on all platforms.
    pub size: u64,
    /// xxh64 of the first 8 KiB of the file. Cheap fingerprint that
    /// detects content changes even when mtime is preserved (e.g.
    /// `touch -t`, `cp -p`).
    pub head_hash: u64,
}

impl Fingerprint {
    /// Capture a fingerprint of `path` by reading the first 8 KiB and
    /// running xxh64 over it.
    pub async fn capture(path: &Path) -> std::io::Result<Self> {
        let meta = tokio::fs::metadata(path).await?;
        let mtime = meta.modified().ok();
        let size = meta.len();
        let head_hash = compute_head_hash(path).await;
        Ok(Self {
            mtime,
            size,
            head_hash,
        })
    }
}

/// Read the first 8 KiB of `path` and compute xxh64. If the file is
/// shorter than 8 KiB, the hash is over whatever's there. If the read
/// fails (e.g. permission), returns 0; `verify_fresh` treats that as
/// "no head hash recorded" and falls back to mtime + size.
async fn compute_head_hash(path: &Path) -> u64 {
    const HEAD_CAP: usize = 8 * 1024;
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return 0;
    };
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; HEAD_CAP];
    let n = match f.read(&mut buf).await {
        Ok(n) => n,
        Err(_) => return 0,
    };
    xxh64(&buf[..n], 0)
}

/// In-process fingerprint store, keyed by session id then canonical path.
///
/// The wrapper is cheap to clone (`Arc` inside) and safe to share
/// across the agent loop's tool execution via `tauri::State`.
#[derive(Debug, Clone, Default)]
pub struct ReadGuard {
    /// Outer key: session id. Inner key: canonical absolute path.
    inner: Arc<Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>>,
}

impl ReadGuard {
    /// Create an empty guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful read of `path` in `session_id`. Replaces any
    /// prior fingerprint for the same (session_id, path) pair.
    ///
    /// The path is canonicalized for stable keying — the same physical
    /// file reached via different relative/absolute inputs (e.g. after
    /// a `cd` shell tool) is treated as the same entry.
    pub async fn record_read(&self, session_id: &str, path: &Path) {
        // Canonicalize for stable keying. If canonicalize fails (e.g.
        // file disappeared between the read and the record call), fall
        // back to the original absolute path — the next verify_fresh
        // will fail anyway and surface a clear "file changed on disk"
        // error to the LLM.
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let fp = match Fingerprint::capture(path).await {
            Ok(fp) => fp,
            Err(e) => {
                tracing::debug!(
                    path = %path.display(),
                    error = %e,
                    "ReadGuard::record_read: fingerprint capture failed; skipping"
                );
                return;
            }
        };
        let mut map = self.inner.lock().await;
        map.entry(session_id.to_string())
            .or_default()
            .insert(key, fp);
    }

    /// Check that the LLM read `path` in this session at some point.
    /// Returns `Ok(())` if the file was read; an error message otherwise.
    pub async fn verify_read(&self, session_id: &str, path: &Path) -> Result<(), String> {
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let map = self.inner.lock().await;
        let session_map = map.get(session_id);
        match session_map.and_then(|m| m.get(&key)) {
            Some(_) => Ok(()),
            None => Err(format!(
                "You must read_file '{}' first.",
                path.display()
            )),
        }
    }

    /// Check that the file on disk has not changed since the read was
    /// recorded. Re-stat the file and compare mtime / size / head hash
    /// against the stored fingerprint. Returns `Ok(())` if the file is
    /// unchanged; an error message otherwise.
    pub async fn verify_fresh(&self, session_id: &str, path: &Path) -> Result<(), String> {
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let stored = {
            let map = self.inner.lock().await;
            map.get(session_id)
                .and_then(|m| m.get(&key))
                .cloned()
        };
        let Some(stored) = stored else {
            return Err(format!(
                "You must read_file '{}' first.",
                path.display()
            ));
        };
        let current = match Fingerprint::capture(path).await {
            Ok(fp) => fp,
            Err(e) => {
                return Err(format!(
                    "Failed to stat '{}': {}",
                    path.display(),
                    e
                ));
            }
        };
        if current.size != stored.size {
            return Err(format!(
                "File '{}' has changed on disk since you last read it \
                 (size was {}, now {}). Re-read it first.",
                path.display(),
                stored.size,
                current.size
            ));
        }
        // mtime check: if both sides have mtime, prefer the strict
        // comparison. If only one side has it (rare; e.g. FAT
        // filesystem), fall through to head_hash.
        match (stored.mtime, current.mtime) {
            (Some(s), Some(c)) if s != c => {
                return Err(format!(
                    "File '{}' has changed on disk since you last read it. Re-read it first.",
                    path.display()
                ));
            }
            _ => {}
        }
        if current.head_hash != 0
            && stored.head_hash != 0
            && current.head_hash != stored.head_hash
        {
            return Err(format!(
                "File '{}' has changed on disk since you last read it \
                 (content differs). Re-read it first.",
                path.display()
            ));
        }
        Ok(())
    }

    /// Invalidate a path's fingerprint. Called by `edit_file` after a
    /// successful write, so the LLM is forced to re-read on the next
    /// edit attempt.
    pub async fn invalidate(&self, session_id: &str, path: &Path) {
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let mut map = self.inner.lock().await;
        if let Some(session_map) = map.get_mut(session_id) {
            session_map.remove(&key);
        }
    }

    /// Drop all fingerprints for a session. Called when the session
    /// is deleted via the Tauri command.
    pub async fn clear_session(&self, session_id: &str) {
        let mut map = self.inner.lock().await;
        map.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn record_then_verify_read_succeeds() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello").unwrap();

        let guard = ReadGuard::new();
        guard.record_read("s1", &p).await;
        guard.verify_read("s1", &p).await.expect("just recorded");
    }

    #[tokio::test]
    async fn verify_read_fails_when_not_read() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello").unwrap();

        let guard = ReadGuard::new();
        let err = guard.verify_read("s1", &p).await.unwrap_err();
        assert!(err.contains("read_file"));
        assert!(err.contains("first"));
    }

    #[tokio::test]
    async fn verify_fresh_passes_when_unchanged() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello").unwrap();

        let guard = ReadGuard::new();
        guard.record_read("s1", &p).await;
        guard.verify_fresh("s1", &p).await.expect("unchanged");
    }

    #[tokio::test]
    async fn verify_fresh_fails_after_external_modify() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello").unwrap();

        let guard = ReadGuard::new();
        guard.record_read("s1", &p).await;

        // Modify the file (longer content → size differs; mtime likely
        // advances too).
        std::fs::write(&p, "hello world").unwrap();
        // Give the FS a chance to bump mtime; on some filesystems the
        // resolution is 1s, so sleep a bit to be safe.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let err = guard.verify_fresh("s1", &p).await.unwrap_err();
        assert!(
            err.contains("changed on disk") || err.contains("size was"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn invalidate_drops_fingerprint() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello").unwrap();

        let guard = ReadGuard::new();
        guard.record_read("s1", &p).await;
        guard.invalidate("s1", &p).await;
        // After invalidate, verify_read fails.
        let err = guard.verify_read("s1", &p).await.unwrap_err();
        assert!(err.contains("read_file"));
    }

    #[tokio::test]
    async fn sessions_are_isolated() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello").unwrap();

        let guard = ReadGuard::new();
        guard.record_read("s1", &p).await;
        // s2 has not read this file.
        let err = guard.verify_read("s2", &p).await.unwrap_err();
        assert!(err.contains("read_file"));
        // s1 still can.
        guard.verify_read("s1", &p).await.expect("s1 still has it");
    }

    #[tokio::test]
    async fn clear_session_drops_all_fingerprints() {
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, "1").unwrap();
        std::fs::write(&p2, "2").unwrap();

        let guard = ReadGuard::new();
        guard.record_read("s1", &p1).await;
        guard.record_read("s1", &p2).await;
        guard.clear_session("s1").await;
        let err = guard.verify_read("s1", &p1).await.unwrap_err();
        assert!(err.contains("read_file"));
        let err = guard.verify_read("s1", &p2).await.unwrap_err();
        assert!(err.contains("read_file"));
    }

    #[tokio::test]
    async fn fingerprint_serde_roundtrip() {
        let fp = Fingerprint {
            mtime: Some(SystemTime::UNIX_EPOCH),
            size: 12345,
            head_hash: 0xDEAD_BEEF_CAFE_BABE,
        };
        let json = serde_json::to_string(&fp).unwrap();
        let fp2: Fingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(fp.size, fp2.size);
        assert_eq!(fp.head_hash, fp2.head_hash);
        assert_eq!(fp.mtime, fp2.mtime);
    }
}
