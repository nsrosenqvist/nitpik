//! Content-hash based result cache.
//!
//! Caches review results to skip redundant LLM calls when
//! the same file+agent+model combination is reviewed again.

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
}
