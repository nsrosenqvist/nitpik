//! Read a unified diff from a file.

use std::path::Path;

use super::DiffError;

/// Read a unified diff from a file path.
pub async fn read_diff_file(path: &Path) -> Result<String, DiffError> {
    if !path.exists() {
        return Err(DiffError::PathNotFound(
            path.display().to_string(),
        ));
    }

    tokio::fs::read_to_string(path)
        .await
        .map_err(DiffError::FileReadError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_existing_diff_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.diff");
        std::fs::write(&path, "diff --git a/f b/f\n").unwrap();

        let content = read_diff_file(&path).await.unwrap();
        assert!(content.contains("diff --git"));
    }

    #[tokio::test]
    async fn read_nonexistent_diff_file() {
        let result = read_diff_file(Path::new("/tmp/nitpik_no_such.diff")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
