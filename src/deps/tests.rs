#[cfg(test)]
mod tests {
    use super::super::engine::{execute_dep_step, DepStep, DepStepAction};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_snapshot_logic() {
        let prefix_dir = tempdir().unwrap();
        let cache_dir = tempdir().unwrap();
        let _prefix_path = prefix_dir.path().to_string_lossy().to_string();
        let _cache_path = cache_dir.path().to_string_lossy().to_string();
    }

    #[tokio::test]
    async fn test_sha256_verification() {
        let cache_dir = tempdir().unwrap();
        let cache_path = cache_dir.path().to_string_lossy().to_string();
        let test_file = cache_dir.path().join("test.txt");
        fs::write(&test_file, "hello world").unwrap();

        // SHA256 of "hello world" is b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let valid_sha = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        
        let step = DepStep {
            description: "Test SHA",
            action: DepStepAction::DownloadFile {
                url: "https://example.com/test.txt", // Not actually downloaded because file exists
                file_name: "test.txt",
                sha256: Some(valid_sha),
            },
        };

        // Should succeed
        let result = execute_dep_step(&step, "/tmp", "/tmp", &cache_path).await;
        assert!(result.is_ok());

        // Corrupt the file
        fs::write(&test_file, "corrupted").unwrap();
        let result = execute_dep_step(&step, "/tmp", "/tmp", &cache_path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Checksum mismatch"));
        assert!(!test_file.exists()); // Should have been deleted
    }
}
