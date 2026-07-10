use super::engine::{DepStep, DepStepAction, execute_dep_step, uninstall_dep};
use leyen_model::deps::get_prefix_deps_state_path;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tempfile::tempdir;

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
            sha256: valid_sha,
        },
    };

    let cancel = Arc::new(AtomicBool::new(false));

    // Should succeed
    let result = execute_dep_step(&step, "/tmp", "/tmp", &cache_path, &cancel).await;
    assert!(result.is_ok());

    // Corrupt the file
    fs::write(&test_file, "corrupted").unwrap();
    let result = execute_dep_step(&step, "/tmp", "/tmp", &cache_path, &cancel).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Checksum mismatch"));
    assert!(!test_file.exists()); // Should have been deleted
}

#[tokio::test]
async fn test_concurrent_downloads_of_same_file_both_succeed() {
    let cache_dir = tempdir().unwrap();
    let cache_path = cache_dir.path().to_string_lossy().to_string();
    let test_file = cache_dir.path().join("concurrent.txt");
    fs::write(&test_file, "hello world").unwrap();

    // SHA256 of "hello world"
    let valid_sha = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    let step = DepStep {
        description: "Concurrent claim test",
        action: DepStepAction::DownloadFile {
            url: "https://example.com/concurrent.txt", // not downloaded, file already exists
            file_name: "concurrent.txt",
            sha256: valid_sha,
        },
    };

    let cancel = Arc::new(AtomicBool::new(false));

    // Two operations racing for the same cache file (as would happen for
    // different prefixes sharing the global cache dir) must serialize via the
    // claim guard rather than stepping on each other, and both must succeed.
    let (a, b) = tokio::join!(
        execute_dep_step(&step, "/tmp", "/tmp", &cache_path, &cancel),
        execute_dep_step(&step, "/tmp", "/tmp", &cache_path, &cancel),
    );

    assert!(a.is_ok(), "first concurrent call failed: {:?}", a.err());
    assert!(b.is_ok(), "second concurrent call failed: {:?}", b.err());
}

#[tokio::test]
async fn test_uninstall_dep_rejects_corrupt_state() {
    let prefix_dir = tempdir().unwrap();
    let prefix_path = prefix_dir.path().to_string_lossy().to_string();

    let state_path = get_prefix_deps_state_path(&prefix_path);
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    fs::write(&state_path, [0xFFu8, 0xFE, 0x00, 0x01, 0x02, 0x03]).unwrap();

    let cancel = Arc::new(AtomicBool::new(false));
    let result = uninstall_dep("nonexistent-dep", &prefix_path, "", cancel, |_, _, _| {}).await;

    let err = result.expect_err("corrupt state must error, not report 'no longer tracked'");
    assert!(err.contains("corrupt"), "unexpected error message: {err}");
}
