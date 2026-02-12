/// API module.
use std::fs;

pub fn health() -> &'static str {
    "ok"
}

/// Read configuration from disk (no error handling).
pub fn read_config(path: &str) -> String {
    fs::read_to_string(path).unwrap()
}

/// Process incoming webhook payload.
pub fn handle_webhook(payload: &str) -> Vec<String> {
    let parts: Vec<&str> = payload.split('\n').collect();
    let mut results = Vec::new();
    for p in &parts {
        let kv: Vec<&str> = p.split('=').collect();
        results.push(format!("{}:{}", kv[0], kv[1]));
    }
    results
}
