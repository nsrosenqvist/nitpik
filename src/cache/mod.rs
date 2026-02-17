//! Content-hash based result cache.
//!
//! Caches review results to skip redundant LLM calls when
//! the same file+agent+model combination is reviewed again.
//!
//! A sidecar `.meta` file per file×agent×model triple tracks the
//! most recent cache key so that prior findings can be retrieved
//! after a content change invalidates the cache.

pub mod store;

use sha2::{Digest, Sha256};

use crate::models::finding::Finding;

/// Compute a cache key from file content, agent config, and model name.
pub fn cache_key(file_content: &str, agent_name: &str, model: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_content.as_bytes());
    hasher.update(agent_name.as_bytes());
    hasher.update(model.as_bytes());
    hex::encode(hasher.finalize())
}

/// The cache engine for review results.
pub struct CacheEngine {
    enabled: bool,
    store: store::FileStore,
}

impl CacheEngine {
    /// Create a new cache engine.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            store: store::FileStore::new(),
        }
    }

    /// Create a cache engine with a specific cache directory (useful for testing).
    pub fn new_with_dir(cache_dir: std::path::PathBuf) -> Self {
        Self {
            enabled: true,
            store: store::FileStore::new_with_dir(cache_dir),
        }
    }

    /// Look up cached findings.
    pub fn get(&self, key: &str) -> Option<Vec<Finding>> {
        if !self.enabled {
            return None;
        }
        self.store.get(key)
    }

    /// Store findings in the cache.
    pub fn put(&self, key: &str, findings: &[Finding]) {
        if !self.enabled {
            return;
        }
        self.store.put(key, findings);
    }

    /// Write the sidecar that maps a file×agent×model×scope tuple to its
    /// latest content-hash cache key.
    pub fn put_sidecar(
        &self,
        file_path: &str,
        agent_name: &str,
        model: &str,
        cache_key: &str,
        review_scope: &str,
    ) {
        if !self.enabled {
            return;
        }
        self.store.put_sidecar(file_path, agent_name, model, cache_key, review_scope);
    }

    /// Retrieve findings from the *previous* review of a file×agent×model×scope
    /// tuple, if the cache key has changed (content invalidation).
    ///
    /// Returns `None` when caching is disabled, on first run, or when
    /// the cache key hasn't changed (pure hit).
    pub fn get_previous(
        &self,
        file_path: &str,
        agent_name: &str,
        model: &str,
        current_cache_key: &str,
        review_scope: &str,
    ) -> Option<Vec<Finding>> {
        if !self.enabled {
            return None;
        }
        self.store.get_previous(file_path, agent_name, model, current_cache_key, review_scope)
    }

    /// Remove stale `.meta` sidecar files older than the given duration.
    ///
    /// Returns the number of files removed.
    pub fn cleanup_stale(&self, max_age: std::time::Duration) -> usize {
        if !self.enabled {
            return 0;
        }
        self.store.cleanup_stale_sidecars(max_age)
    }

    /// Remove all cached entries.
    pub fn clear(&self) -> Result<store::CacheStats, std::io::Error> {
        self.store.clear()
    }

    /// Compute statistics about the cache.
    pub fn stats(&self) -> Result<store::CacheStats, std::io::Error> {
        self.store.stats()
    }

    /// Return the cache directory path.
    pub fn path(&self) -> Option<&std::path::PathBuf> {
        self.store.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_deterministic() {
        let k1 = cache_key("content", "agent", "model");
        let k2 = cache_key("content", "agent", "model");
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_varies_with_content() {
        let k1 = cache_key("content1", "agent", "model");
        let k2 = cache_key("content2", "agent", "model");
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_varies_with_agent() {
        let k1 = cache_key("content", "agent1", "model");
        let k2 = cache_key("content", "agent2", "model");
        assert_ne!(k1, k2);
    }

    #[test]
    fn get_previous_returns_none_when_disabled() {
        // CacheEngine with enabled=false should never return prior findings
        let engine = CacheEngine::new(false);
        assert!(engine.get_previous("f.rs", "backend", "model", "key", "main").is_none());
    }

    #[test]
    fn put_sidecar_noop_when_disabled() {
        // Should not panic or write anything when disabled
        let engine = CacheEngine::new(false);
        engine.put_sidecar("f.rs", "backend", "model", "key", "main");
    }

    #[test]
    fn cleanup_stale_noop_when_disabled() {
        let engine = CacheEngine::new(false);
        assert_eq!(engine.cleanup_stale(std::time::Duration::from_secs(1)), 0);
    }
}
