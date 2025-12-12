use anyhow::Result;
use std::path::{Path, PathBuf};

/// Finds application entry point
pub fn find_entry_point(project_root: &Path, custom_entry: Option<&str>) -> Result<PathBuf> {
    // Canonicalize project_root for path traversal check
    let canonical_root = project_root.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "Failed to canonicalize project root {:?}: {}",
            project_root,
            e
        )
    })?;

    // If custom entry point is specified
    if let Some(entry) = custom_entry {
        let candidate = project_root.join(entry);

        // Canonicalize candidate
        let canonical_candidate = candidate.canonicalize().map_err(|e| {
            anyhow::anyhow!(
                "Entry point path does not exist or cannot be accessed: {:?}: {}",
                candidate,
                e
            )
        })?;

        // Check that candidate is inside project_root (path traversal protection)
        if !canonical_candidate.starts_with(&canonical_root) {
            anyhow::bail!(
                "Entry point path attempts to escape project root: {:?}",
                candidate
            );
        }

        // Check that it's a file, not a directory
        if !canonical_candidate.is_file() {
            anyhow::bail!("Entry point is not a regular file: {:?}", candidate);
        }

        return Ok(canonical_candidate);
    }

    // Search for standard entry points
    let candidates = ["main.py", "app.py", "__main__.py", "server.py"];

    for candidate in &candidates {
        let candidate_path = project_root.join(candidate);

        // Check that it's a file (not a directory)
        if candidate_path.is_file() {
            // Canonicalize for consistency
            let canonical = candidate_path.canonicalize().map_err(|e| {
                anyhow::anyhow!(
                    "Failed to canonicalize entry point {:?}: {}",
                    candidate_path,
                    e
                )
            })?;

            // Additional path traversal check (though this shouldn't happen for standard names)
            if !canonical.starts_with(&canonical_root) {
                anyhow::bail!(
                    "Entry point path attempts to escape project root: {:?}",
                    candidate_path
                );
            }

            return Ok(canonical);
        }
    }

    anyhow::bail!(
        "Entry point not found in {:?}. Tried: {:?}",
        project_root,
        candidates
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_find_entry_point() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        // Create app.py
        fs::write(project_root.join("app.py"), "from fastapi import FastAPI\n").unwrap();

        let entry = find_entry_point(project_root, None).unwrap();
        assert!(entry.ends_with("app.py"));
    }
}
