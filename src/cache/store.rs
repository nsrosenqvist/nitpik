//! Filesystem-based cache store.
//!
//! Stores cached results as JSON files in `~/.config/nitpik/cache/`.
//! Each entry also writes a sidecar `.meta` file keyed by
//! `(file_path, agent_name, model)` so that prior findings can be
//! retrieved after a cache key changes (content invalidation).

use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::models::finding::Finding;

/// Filesystem-based cache store.
pub struct FileStore {
    cache_dir: Option<PathBuf>,
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStore {
    /// Create a new file store using the default cache directory.
    pub fn new() -> Self {
        let cache_dir =
            dirs::config_dir().map(|d| d.join(crate::constants::CONFIG_DIR).join("cache"));
        Self { cache_dir }
    }

    /// Create a file store with a specific cache directory (useful for testing).
    #[allow(dead_code)]
    pub fn new_with_dir(cache_dir: std::path::PathBuf) -> Self {
        Self {
            cache_dir: Some(cache_dir),
        }
    }

    /// Get cached findings by key.
    pub async fn get(&self, key: &str) -> Option<Vec<Finding>> {
        let path = self.key_path(key)?;

        let content = tokio::fs::read_to_string(&path).await.ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Store findings by key.
    pub async fn put(&self, key: &str, findings: &[Finding]) {
        let Some(path) = self.key_path(key) else {
            return;
        };

        // Ensure cache directory exists
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let content = match serde_json::to_string(findings) {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = tokio::fs::write(&path, content).await;
    }

    /// Remove all cached entries.
    pub async fn clear(&self) -> Result<CacheStats, std::io::Error> {
        let stats = self.stats().await;
        if let Some(ref dir) = self.cache_dir {
            if tokio::fs::try_exists(dir).await.unwrap_or(false) {
                tokio::fs::remove_dir_all(dir).await?;
            }
        }
        stats
    }

    /// Compute statistics about the cache.
    pub async fn stats(&self) -> Result<CacheStats, std::io::Error> {
        let Some(ref dir) = self.cache_dir else {
            return Ok(CacheStats {
                entries: 0,
                total_bytes: 0,
            });
        };

        if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
            return Ok(CacheStats {
                entries: 0,
                total_bytes: 0,
            });
        }

        let mut entries: usize = 0;
        let mut total_bytes: u64 = 0;

        let mut read_dir = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            match ext {
                Some("json") => {
                    entries += 1;
                    total_bytes += entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                }
                Some("meta") => {
                    // Sidecar files are not counted as cache entries
                    // but their size is included in the total.
                    total_bytes += entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                }
                _ => {}
            }
        }

        Ok(CacheStats {
            entries,
            total_bytes,
        })
    }

    /// Return the cache directory path.
    pub fn path(&self) -> Option<&PathBuf> {
        self.cache_dir.as_ref()
    }

    /// Retrieve the previous findings for a file×agent×model triple.
    ///
    /// Returns `Some(findings)` when the sidecar exists, references a
    /// *different* cache key than `current_cache_key`, and the old entry
    /// is still readable. Returns `None` on first run, cache hit
    /// (keys match), or any I/O / deserialisation error.
    pub async fn get_previous(
        &self,
        file_path: &str,
        agent_name: &str,
        model: &str,
        current_cache_key: &str,
        review_scope: &str,
    ) -> Option<Vec<Finding>> {
        let sidecar = self.sidecar_path(file_path, agent_name, model, review_scope)?;
        let previous_key = tokio::fs::read_to_string(&sidecar).await.ok()?;
        let previous_key = previous_key.trim();
        if previous_key == current_cache_key || previous_key.is_empty() {
            return None;
        }
        // Read the old cache entry
        self.get(previous_key).await
    }

    /// Write (or overwrite) the sidecar that maps a file×agent×model
    /// triple to the latest content-hash cache key.
    pub async fn put_sidecar(
        &self,
        file_path: &str,
        agent_name: &str,
        model: &str,
        cache_key: &str,
        review_scope: &str,
    ) {
        let Some(sidecar) = self.sidecar_path(file_path, agent_name, model, review_scope) else {
            return;
        };
        if let Some(parent) = sidecar.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&sidecar, cache_key).await;
    }

    /// Get the file path for a cache key.
    fn key_path(&self, key: &str) -> Option<PathBuf> {
        self.cache_dir
            .as_ref()
            .map(|dir| dir.join(format!("{key}.json")))
    }

    /// Compute the sidecar `.meta` path for a file×agent×model×scope tuple.
    pub(crate) fn sidecar_path(
        &self,
        file_path: &str,
        agent_name: &str,
        model: &str,
        review_scope: &str,
    ) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|dir| {
            let lookup_key = lookup_key(file_path, agent_name, model, review_scope);
            dir.join(format!("{lookup_key}.meta"))
        })
    }

    /// Remove `.meta` files older than the given duration.
    ///
    /// Returns the number of files removed.
    pub async fn cleanup_stale_sidecars(&self, max_age: std::time::Duration) -> usize {
        let Some(ref dir) = self.cache_dir else {
            return 0;
        };
        if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
            return 0;
        }

        let now = std::time::SystemTime::now();
        let mut removed = 0;

        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => return 0,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("meta") {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if let Ok(age) = now.duration_since(modified) {
                if age > max_age && tokio::fs::remove_file(&path).await.is_ok() {
                    removed += 1;
                }
            }
        }

        removed
    }
}

/// Compute a stable lookup key from a file×agent×model×scope tuple.
///
/// This is separate from the content-hash cache key — it identifies
/// *which* file×agent×model×scope combination the sidecar tracks,
/// regardless of the prompt content that went into the review.
/// The `review_scope` (typically branch name) isolates sidecars so that
/// parallel PRs/branches don't cross-contaminate prior findings.
pub fn lookup_key(file_path: &str, agent_name: &str, model: &str, review_scope: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_path.as_bytes());
    hasher.update(b"|");
    hasher.update(agent_name.as_bytes());
    hasher.update(b"|");
    hasher.update(model.as_bytes());
    hasher.update(b"|");
    hasher.update(review_scope.as_bytes());
    hex::encode(hasher.finalize())
}

/// Statistics about the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStats {
    /// Number of cached entries.
    pub entries: usize,
    /// Total size in bytes.
    pub total_bytes: u64,
}

impl CacheStats {
    /// Format total_bytes as a human-readable string.
    pub fn human_size(&self) -> String {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;

        if self.total_bytes >= MB {
            format!("{:.1} MiB", self.total_bytes as f64 / MB as f64)
        } else if self.total_bytes >= KB {
            format!("{:.1} KiB", self.total_bytes as f64 / KB as f64)
        } else {
            format!("{} B", self.total_bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    fn make_store(dir: &std::path::Path) -> FileStore {
        FileStore {
            cache_dir: Some(dir.to_path_buf()),
        }
    }

    fn sample_findings() -> Vec<Finding> {
        vec![Finding {
            file: "test.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: "Issue".into(),
            message: "Details".into(),
            suggestion: None,
            agent: "backend".into(),
        }]
    }

    #[tokio::test]
    async fn roundtrip_cache() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        let findings = sample_findings();

        store.put("test-key", &findings).await;
        let cached = store.get("test-key").await.unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].file, "test.rs");
    }

    #[tokio::test]
    async fn cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        assert!(store.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn stats_empty_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let store = FileStore {
            cache_dir: Some(cache_dir),
        };
        let stats = store.stats().await.unwrap();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.total_bytes, 0);
    }

    #[tokio::test]
    async fn stats_with_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        store.put("key1", &sample_findings()).await;
        store.put("key2", &sample_findings()).await;

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.entries, 2);
        assert!(stats.total_bytes > 0);
    }

    #[tokio::test]
    async fn clear_removes_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let store = FileStore {
            cache_dir: Some(cache_dir.clone()),
        };
        store.put("key1", &sample_findings()).await;
        assert!(store.get("key1").await.is_some());

        let stats = store.clear().await.unwrap();
        assert_eq!(stats.entries, 1);
        assert!(!cache_dir.exists());
    }

    #[tokio::test]
    async fn clear_empty_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("nonexistent_cache");
        let store = FileStore {
            cache_dir: Some(cache_dir),
        };
        let stats = store.clear().await.unwrap();
        assert_eq!(stats.entries, 0);
    }

    #[test]
    fn path_returns_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        assert_eq!(store.path(), Some(&dir.path().to_path_buf()));
    }

    #[test]
    fn path_none_when_no_dir() {
        let store = FileStore { cache_dir: None };
        assert!(store.path().is_none());
    }

    #[test]
    fn human_size_bytes() {
        let stats = CacheStats {
            entries: 1,
            total_bytes: 500,
        };
        assert_eq!(stats.human_size(), "500 B");
    }

    #[test]
    fn human_size_kib() {
        let stats = CacheStats {
            entries: 1,
            total_bytes: 2048,
        };
        assert_eq!(stats.human_size(), "2.0 KiB");
    }

    #[test]
    fn human_size_mib() {
        let stats = CacheStats {
            entries: 1,
            total_bytes: 2 * 1024 * 1024,
        };
        assert_eq!(stats.human_size(), "2.0 MiB");
    }

    // ── Sidecar / prior-findings tests ──────────────────────────────

    #[tokio::test]
    async fn get_previous_returns_none_on_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        assert!(
            store
                .get_previous("file.rs", "backend", "model-v1", "key-abc", "main")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn get_previous_returns_none_when_key_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        store.put("key-abc", &sample_findings()).await;
        store
            .put_sidecar("file.rs", "backend", "model-v1", "key-abc", "main")
            .await;

        // Same cache key → cache hit, no prior needed
        assert!(
            store
                .get_previous("file.rs", "backend", "model-v1", "key-abc", "main")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn get_previous_returns_old_findings_after_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        // First run: store findings + sidecar
        let old_findings = sample_findings();
        store.put("key-old", &old_findings).await;
        store
            .put_sidecar("file.rs", "backend", "model-v1", "key-old", "feature-a")
            .await;

        // Second run: file changed → new cache key
        let prior = store
            .get_previous("file.rs", "backend", "model-v1", "key-new", "feature-a")
            .await;
        assert!(prior.is_some());
        let prior = prior.unwrap();
        assert_eq!(prior.len(), 1);
        assert_eq!(prior[0].file, "test.rs");
    }

    #[tokio::test]
    async fn put_sidecar_overwrites_previous() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        store.put("key-v1", &sample_findings()).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key-v1", "main")
            .await;

        // Overwrite sidecar with new key
        store.put("key-v2", &[]).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key-v2", "main")
            .await;

        // Now prior should point to key-v2, not key-v1
        // Asking with key-v3 should return key-v2's findings (empty vec)
        let prior = store
            .get_previous("file.rs", "backend", "model", "key-v3", "main")
            .await;
        assert!(prior.is_some());
        assert!(prior.unwrap().is_empty());
    }

    #[test]
    fn lookup_key_is_deterministic() {
        let k1 = lookup_key("file.rs", "backend", "model", "main");
        let k2 = lookup_key("file.rs", "backend", "model", "main");
        assert_eq!(k1, k2);
    }

    #[test]
    fn lookup_key_varies_with_inputs() {
        let k1 = lookup_key("a.rs", "backend", "model", "main");
        let k2 = lookup_key("b.rs", "backend", "model", "main");
        let k3 = lookup_key("a.rs", "frontend", "model", "main");
        let k4 = lookup_key("a.rs", "backend", "other-model", "main");
        let k5 = lookup_key("a.rs", "backend", "model", "feature-b");
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k1, k4);
        assert_ne!(k1, k5);
    }

    #[tokio::test]
    async fn stats_excludes_meta_from_entry_count() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        store.put("key1", &sample_findings()).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key1", "main")
            .await;

        let stats = store.stats().await.unwrap();
        // Only 1 .json entry, .meta is not counted as an entry
        assert_eq!(stats.entries, 1);
        // But total bytes includes both files
        assert!(stats.total_bytes > 0);
    }

    #[tokio::test]
    async fn clear_removes_meta_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let store = FileStore {
            cache_dir: Some(cache_dir.clone()),
        };
        store.put("key1", &sample_findings()).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key1", "main")
            .await;

        store.clear().await.unwrap();
        assert!(!cache_dir.exists());
    }

    // ── Scope isolation tests ────────────────────────────────────────

    #[tokio::test]
    async fn sidecars_are_isolated_by_scope() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        // PR-1 on feature-a: store findings
        store.put("key-a1", &sample_findings()).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key-a1", "feature-a")
            .await;

        // PR-2 on feature-b: store different findings
        let findings_b = vec![Finding {
            file: "file.rs".into(),
            line: 10,
            end_line: None,
            severity: Severity::Error,
            title: "Branch B issue".into(),
            message: "Only on feature-b".into(),
            suggestion: None,
            agent: "backend".into(),
        }];
        store.put("key-b1", &findings_b).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key-b1", "feature-b")
            .await;

        // PR-1 push 2: should see feature-a findings, NOT feature-b
        let prior_a = store
            .get_previous("file.rs", "backend", "model", "key-a2", "feature-a")
            .await;
        assert!(prior_a.is_some());
        assert_eq!(prior_a.unwrap()[0].title, "Issue"); // from sample_findings

        // PR-2 push 2: should see feature-b findings, NOT feature-a
        let prior_b = store
            .get_previous("file.rs", "backend", "model", "key-b2", "feature-b")
            .await;
        assert!(prior_b.is_some());
        assert_eq!(prior_b.unwrap()[0].title, "Branch B issue");
    }

    #[tokio::test]
    async fn different_scope_has_no_prior_on_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        // Seed sidecar on main branch
        store.put("key-main", &sample_findings()).await;
        store
            .put_sidecar("file.rs", "backend", "model", "key-main", "main")
            .await;

        // New branch has no sidecar → no prior findings
        let prior = store
            .get_previous("file.rs", "backend", "model", "key-new", "feature-x")
            .await;
        assert!(prior.is_none());
    }

    // ── Stale sidecar cleanup tests ──────────────────────────────────

    #[tokio::test]
    async fn cleanup_removes_stale_meta_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        // Create a sidecar and backdate it
        store
            .put_sidecar("old.rs", "backend", "model", "key-old", "stale-branch")
            .await;
        let meta_path = store
            .sidecar_path("old.rs", "backend", "model", "stale-branch")
            .unwrap();
        let old_time =
            std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 24 * 60 * 60);
        filetime::set_file_mtime(&meta_path, filetime::FileTime::from_system_time(old_time))
            .unwrap();

        // Create a fresh sidecar
        store
            .put_sidecar("new.rs", "backend", "model", "key-new", "active-branch")
            .await;

        // Also create a .json cache entry (should not be removed)
        store.put("key-new", &sample_findings()).await;

        let removed = store
            .cleanup_stale_sidecars(std::time::Duration::from_secs(30 * 24 * 60 * 60))
            .await;
        assert_eq!(removed, 1);
        assert!(!meta_path.exists());

        // Fresh sidecar and .json still exist
        let fresh_meta = store
            .sidecar_path("new.rs", "backend", "model", "active-branch")
            .unwrap();
        assert!(fresh_meta.exists());
        assert!(store.get("key-new").await.is_some());
    }

    #[tokio::test]
    async fn cleanup_empty_cache_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("nonexistent");
        let store = FileStore {
            cache_dir: Some(cache_dir),
        };
        assert_eq!(
            store
                .cleanup_stale_sidecars(std::time::Duration::from_secs(1))
                .await,
            0
        );
    }
}
