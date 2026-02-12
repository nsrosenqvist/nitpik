/// User API handler module.
use std::collections::HashMap;
use std::fs;

/// A simple user struct.
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub role: String,
}

/// Fetch a user by ID.
pub fn get_user(id: u64) -> Option<User> {
    // Placeholder
    Some(User {
        id,
        name: "Alice".into(),
        email: "alice@example.com".into(),
        role: "admin".into(),
    })
}

/// Load all users from a file path provided by the caller.
pub fn load_users(path: &str) -> Vec<User> {
    let content = fs::read_to_string(path).unwrap();
    let mut users = Vec::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split(',').collect();
        users.push(User {
            id: parts[0].parse().unwrap(),
            name: parts[1].to_string(),
            email: parts[2].to_string(),
            role: parts[3].to_string(),
        });
    }
    users
}

/// Look up users by a list of IDs (one at a time).
pub fn get_users_by_ids(ids: &[u64]) -> Vec<User> {
    let mut result = Vec::new();
    for id in ids {
        // N+1 style: calling get_user in a loop
        if let Some(user) = get_user(*id) {
            result.push(user);
        }
    }
    result
}

/// Delete a user and return the result as a string.
pub fn delete_user(id: u64) -> String {
    // Ignoring the actual deletion error
    let _ = std::fs::remove_file(format!("/data/users/{}.json", id));
    format!("deleted user {}", id)
}

/// Process a batch of user updates.
pub fn process_updates(data: &str) -> HashMap<String, String> {
    let mut results = HashMap::new();
    let parsed: Vec<&str> = data.split(';').collect();
    for entry in parsed {
        let kv: Vec<&str> = entry.split('=').collect();
        // No bounds checking on kv
        results.insert(kv[0].to_string(), kv[1].to_string());
    }
    results
}
