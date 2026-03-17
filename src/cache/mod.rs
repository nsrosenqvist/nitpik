//! Content-hash based result cache.
//!
//! # Bounded Context: Result Caching
//!
//! Owns hash computation, cache-hit lookup, result persistence,
//! and sidecar metadata for prior findings. Operates purely on
//! content hashes and serialized findings — has no knowledge of
//! LLM providers or diff parsing.
//!
//! A sidecar `.meta` file per file×agent×model triple tracks the
//! most recent cache key so that prior findings can be retrieved
//! after a content change invalidates the cache.

pub mod store;

use crate::models::finding::Finding;

/// Compute a cache key from file content, agent config, and model name.
///
/// Uses xxHash3-128 for speed — ~10× faster than SHA-256 on the 50KB+
/// prompts that typically feed into this. The collision risk is acceptable
/// for content-addressable cache keys (not security-critical).
pub fn cache_key(file_content: &str, agent_name: &str, model: &str) -> String {
    let mut data = Vec::with_capacity(file_content.len() + agent_name.len() + model.len());
    data.extend_from_slice(file_content.as_bytes());
    data.extend_from_slice(agent_name.as_bytes());
    data.extend_from_slice(model.as_bytes());
    let hash = xxhash_rust::xxh3::xxh3_128(&data);
    format!("{hash:032x}")
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
    pub async fn get(&self, key: &str) -> Option<Vec<Finding>> {
        if !self.enabled {
            return None;
        }
        self.store.get(key).await
    }

    /// Store findings in the cache.
    pub async fn put(&self, key: &str, findings: &[Finding]) {
        if !self.enabled {
            return;
        }
        self.store.put(key, findings).await;
    }

    /// Write the sidecar that maps a file×agent×model×scope tuple to its
    /// latest content-hash cache key.
    pub async fn put_sidecar(
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
        self.store
            .put_sidecar(file_path, agent_name, model, cache_key, review_scope)
            .await;
    }

    /// Retrieve findings from the *previous* review of a file×agent×model×scope
    /// tuple, if the cache key has changed (content invalidation).
    ///
    /// Returns `None` when caching is disabled, on first run, or when
    /// the cache key hasn't changed (pure hit).
    pub async fn get_previous(
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
        self.store
            .get_previous(
                file_path,
                agent_name,
                model,
                current_cache_key,
                review_scope,
            )
            .await
    }

    /// Remove stale `.meta` sidecar files older than the given duration.
    ///
    /// Returns the number of files removed.
    pub async fn cleanup_stale(&self, max_age: std::time::Duration) -> usize {
        if !self.enabled {
            return 0;
        }
        self.store.cleanup_stale_sidecars(max_age).await
    }

    /// Remove all cached entries.
    pub async fn clear(&self) -> Result<store::CacheStats, std::io::Error> {
        self.store.clear().await
    }

    /// Compute statistics about the cache.
    pub async fn stats(&self) -> Result<store::CacheStats, std::io::Error> {
        self.store.stats().await
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

    #[tokio::test]
    async fn get_previous_returns_none_when_disabled() {
        // CacheEngine with enabled=false should never return prior findings
        let engine = CacheEngine::new(false);
        assert!(
            engine
                .get_previous("f.rs", "backend", "model", "key", "main")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn put_sidecar_noop_when_disabled() {
        // Should not panic or write anything when disabled
        let engine = CacheEngine::new(false);
        engine
            .put_sidecar("f.rs", "backend", "model", "key", "main")
            .await;
    }

    #[tokio::test]
    async fn cleanup_stale_noop_when_disabled() {
        let engine = CacheEngine::new(false);
        assert_eq!(
            engine
                .cleanup_stale(std::time::Duration::from_secs(1))
                .await,
            0
        );
    }
}
