//! Filesystem-based cache store.
//!
//! Stores cached results as JSON files in `~/.config/nitpik/cache/`.

use std::path::PathBuf;

use crate::models::finding::Finding;

/// Filesystem-based cache store.
pub struct FileStore {
    cache_dir: Option<PathBuf>,
}

impl FileStore {
    /// Create a new file store using the default cache directory.
    pub fn new() -> Self {
        let cache_dir = dirs::config_dir().map(|d| d.join(crate::constants::CONFIG_DIR).join("cache"));
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
    pub fn get(&self, key: &str) -> Option<Vec<Finding>> {
        let path = self.key_path(key)?;
        if !path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Store findings by key.
    pub fn put(&self, key: &str, findings: &[Finding]) {
        let Some(path) = self.key_path(key) else {
            return;
        };

        // Ensure cache directory exists
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let content = match serde_json::to_string(findings) {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = std::fs::write(&path, content);
    }

    /// Remove all cached entries.
    pub fn clear(&self) -> Result<CacheStats, std::io::Error> {
        let stats = self.stats();
        if let Some(ref dir) = self.cache_dir {
            if dir.exists() {
                std::fs::remove_dir_all(dir)?;
            }
        }
        stats
    }

    /// Compute statistics about the cache.
    pub fn stats(&self) -> Result<CacheStats, std::io::Error> {
        let Some(ref dir) = self.cache_dir else {
            return Ok(CacheStats {
                entries: 0,
                total_bytes: 0,
            });
        };

        if !dir.exists() {
            return Ok(CacheStats {
                entries: 0,
                total_bytes: 0,
            });
        }

        let mut entries: usize = 0;
        let mut total_bytes: u64 = 0;

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                entries += 1;
                total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
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

    /// Get the file path for a cache key.
    fn key_path(&self, key: &str) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|dir| dir.join(format!("{key}.json")))
    }
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

    #[test]
    fn roundtrip_cache() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        let findings = sample_findings();

        store.put("test-key", &findings);
        let cached = store.get("test-key").unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].file, "test.rs");
    }

    #[test]
    fn cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn stats_empty_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let store = FileStore {
            cache_dir: Some(cache_dir),
        };
        let stats = store.stats().unwrap();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.total_bytes, 0);
    }

    #[test]
    fn stats_with_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        store.put("key1", &sample_findings());
        store.put("key2", &sample_findings());

        let stats = store.stats().unwrap();
        assert_eq!(stats.entries, 2);
        assert!(stats.total_bytes > 0);
    }

    #[test]
    fn clear_removes_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let store = FileStore {
            cache_dir: Some(cache_dir.clone()),
        };
        store.put("key1", &sample_findings());
        assert!(store.get("key1").is_some());

        let stats = store.clear().unwrap();
        assert_eq!(stats.entries, 1);
        assert!(!cache_dir.exists());
    }

    #[test]
    fn clear_empty_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("nonexistent_cache");
        let store = FileStore {
            cache_dir: Some(cache_dir),
        };
        let stats = store.clear().unwrap();
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
        let stats = CacheStats { entries: 1, total_bytes: 500 };
        assert_eq!(stats.human_size(), "500 B");
    }

    #[test]
    fn human_size_kib() {
        let stats = CacheStats { entries: 1, total_bytes: 2048 };
        assert_eq!(stats.human_size(), "2.0 KiB");
    }

    #[test]
    fn human_size_mib() {
        let stats = CacheStats { entries: 1, total_bytes: 2 * 1024 * 1024 };
        assert_eq!(stats.human_size(), "2.0 MiB");
    }
}
