//! Cross-task memoization for read-only tool calls.
//!
//! When several reviewer agents run in parallel against the same diff,
//! they often call the same tool with the same arguments — each agent
//! reads `Cargo.toml`, each greps for `unwrap`, etc. Without
//! memoization those calls hit the filesystem N times and consume
//! budget from N tasks.
//!
//! The [`ToolMemo`] cache is process-global, scoped per *run* (cleared
//! between top-level CLI invocations via [`clear`]), and keyed on
//! `(tool_name, args)` where `args` is hashed from its JSON form.
//!
//! Only **successful** results are cached. Errors (filesystem failures,
//! budget exhaustion, custom-command non-zero exits) are not memoized
//! because they are often transient or task-scoped.
//!
//! Cache hits do not consume tool-call budget — that's the whole point.
//! They are still recorded in the audit log with a `(cached)` marker so
//! the post-review summary stays honest.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, RwLock};

/// Maximum number of cached entries.
///
/// A single review rarely makes more than a few hundred distinct tool
/// calls; this cap is a safety valve against pathological inputs and
/// stale state across long-lived test processes.
const MEMO_CAPACITY: usize = 1024;

/// Process-global memo store.
///
/// Wrapped in a `LazyLock` so the static initializer is `const`-safe
/// (`HashMap::new()` is `const` on stable Rust 1.85). Reads hold the
/// `RwLock` briefly and never `.await` while the lock is held.
static MEMO: LazyLock<RwLock<HashMap<MemoKey, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::with_capacity(64)));

/// Cache key — tool name plus a 64-bit hash of the JSON-serialized args.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MemoKey {
    tool: &'static str,
    args_hash: u64,
}

/// Compute a stable 64-bit hash of `args`'s JSON encoding.
///
/// Two semantically equivalent argument structs that serialize to the
/// same JSON string produce the same hash. Tools should only memoize
/// when their args round-trip deterministically (which all our built-in
/// tools do).
fn hash_args<T: serde::Serialize>(args: &T) -> Option<u64> {
    let json = serde_json::to_string(args).ok()?;
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    Some(hasher.finish())
}

/// Look up a cached successful result for `(tool, args)`.
///
/// Returns `None` on miss, on hash failure, or when the cache is full
/// of unrelated entries (lookup is exact-match only).
pub fn lookup<T: serde::Serialize>(tool: &'static str, args: &T) -> Option<String> {
    let args_hash = hash_args(args)?;
    let key = MemoKey { tool, args_hash };
    let map = MEMO.read().ok()?;
    map.get(&key).cloned()
}

/// Cache a successful result for `(tool, args)`.
///
/// Errors are intentionally not stored — see the module docstring.
/// When the cache is full, the entry is silently dropped (no eviction);
/// in practice [`clear`] is called between runs so capacity is rarely
/// the binding constraint.
pub fn store<T: serde::Serialize>(tool: &'static str, args: &T, value: String) {
    let Some(args_hash) = hash_args(args) else {
        return;
    };
    let key = MemoKey { tool, args_hash };
    let Ok(mut map) = MEMO.write() else { return };
    if map.len() >= MEMO_CAPACITY && !map.contains_key(&key) {
        return;
    }
    map.insert(key, value);
}

/// Clear all cached entries.
///
/// Called by the orchestrator at the start of every review run so
/// stale data from a previous invocation can't leak forward.
pub fn clear() {
    if let Ok(mut map) = MEMO.write() {
        map.clear();
    }
}

/// Current cache size. Test-only observability.
#[cfg(test)]
pub fn len() -> usize {
    MEMO.read().map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serial_test::serial;

    #[derive(Serialize)]
    struct Args {
        path: String,
        n: u32,
    }

    #[test]
    #[serial(memo)]
    fn lookup_miss_returns_none() {
        clear();
        let args = Args {
            path: "src/main.rs".into(),
            n: 1,
        };
        assert!(lookup("read_file", &args).is_none());
    }

    #[test]
    #[serial(memo)]
    fn store_then_lookup_hits() {
        clear();
        let args = Args {
            path: "src/main.rs".into(),
            n: 1,
        };
        store("read_file", &args, "hello".to_string());
        assert_eq!(lookup("read_file", &args).as_deref(), Some("hello"));
        assert_eq!(len(), 1);
    }

    #[test]
    #[serial(memo)]
    fn different_args_different_keys() {
        clear();
        let a1 = Args {
            path: "a.rs".into(),
            n: 1,
        };
        let a2 = Args {
            path: "b.rs".into(),
            n: 1,
        };
        store("read_file", &a1, "AAA".to_string());
        store("read_file", &a2, "BBB".to_string());
        assert_eq!(lookup("read_file", &a1).as_deref(), Some("AAA"));
        assert_eq!(lookup("read_file", &a2).as_deref(), Some("BBB"));
        assert_eq!(len(), 2);
    }

    #[test]
    #[serial(memo)]
    fn different_tools_different_namespaces() {
        clear();
        let args = Args {
            path: "x".into(),
            n: 0,
        };
        store("read_file", &args, "from-read".to_string());
        store("search_text", &args, "from-search".to_string());
        assert_eq!(lookup("read_file", &args).as_deref(), Some("from-read"));
        assert_eq!(lookup("search_text", &args).as_deref(), Some("from-search"));
    }

    #[test]
    #[serial(memo)]
    fn clear_drops_all_entries() {
        clear();
        let args = Args {
            path: "x".into(),
            n: 0,
        };
        store("read_file", &args, "v".to_string());
        assert!(lookup("read_file", &args).is_some());
        clear();
        assert!(lookup("read_file", &args).is_none());
        assert_eq!(len(), 0);
    }
}
