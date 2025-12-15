use anyhow::Result;
use filetime::FileTime;
use hsync::{pipeline::HashAlgorithm, run, Args};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

#[test]
fn test_integration_full_flow() -> Result<()> {
    let source_dir = PathBuf::from("test_int_source");
    let dest_dir = PathBuf::from("test_int_dest");
    let db_path = "test_int.db";
    let log_path = "test_int.log";

    // Cleanup
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)?;
    }
    if dest_dir.exists() {
        fs::remove_dir_all(&dest_dir)?;
    }
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path)?;
    }
    if std::path::Path::new(log_path).exists() {
        fs::remove_file(log_path)?;
    }

    fs::create_dir_all(&source_dir)?;

    // 1. Create files
    let mut f1 = File::create(source_dir.join("file1.txt"))?;
    f1.write_all(b"Hello World")?;

    let mut f2 = File::create(source_dir.join("large.bin"))?;
    f2.write_all(&vec![0u8; 1024 * 1024])?; // 1MiB

    // Empty file
    File::create(source_dir.join("empty.txt"))?;

    // 2. Run Sync (fresh scan, then transfer)
    let args = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: false,
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args.clone())?;

    // Verify files transferred
    assert!(dest_dir.join("file1.txt").exists());
    assert!(dest_dir.join("large.bin").exists());
    assert!(dest_dir.join("empty.txt").exists());
    assert_eq!(fs::metadata(dest_dir.join("large.bin"))?.len(), 1024 * 1024);
    assert_eq!(fs::metadata(dest_dir.join("empty.txt"))?.len(), 0);

    // Verify log contains transfer entries
    let log_content = fs::read_to_string(log_path)?;
    assert!(log_content.contains("Transferred"));

    // 3. Run again - should rescan (no pending files) and find everything synced
    run(args.clone())?;

    // 4. Truncation/Update Test
    // Shrink source file - this should be detected on next scan
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut f2 = File::create(source_dir.join("large.bin"))?; // Truncates source
    f2.write_all(b"Small")?;

    // Run again - should detect changed file via scan and transfer it
    run(args.clone())?;

    assert_eq!(fs::metadata(dest_dir.join("large.bin"))?.len(), 5);

    // 5. Mirroring Test
    File::create(dest_dir.join("extra.txt"))?;
    let args_mirror = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: true,
        rescan: false,
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args_mirror)?;
    assert!(!dest_dir.join("extra.txt").exists());

    // 6. Forced Rescan Test - use --rescan to force re-evaluation
    let args_rescan = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: true,
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args_rescan)?;

    // 7. Bandwidth Limit Test
    // Modify a file to force transfer
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut f1 = File::create(source_dir.join("file1.txt"))?;
    f1.write_all(b"Hello World Updated")?;

    let args_bw = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: Some("10M".to_string()), // 10MiB/s
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: true, // Force rescan to detect the change
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args_bw)?;

    // Cleanup
    fs::remove_dir_all(source_dir)?;
    fs::remove_dir_all(dest_dir)?;
    fs::remove_file(db_path)?;
    fs::remove_file(log_path)?;

    Ok(())
}

/// Test that file is re-synced when size changes but mtime stays the same.
/// This is an edge case that can occur if a file is modified and then
/// the mtime is manually restored (or on some filesystems).
#[test]
fn test_size_change_without_mtime_change() -> Result<()> {
    let source_dir = PathBuf::from("test_size_source");
    let dest_dir = PathBuf::from("test_size_dest");
    let db_path = "test_size.db";
    let log_path = "test_size.log";

    // Cleanup
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)?;
    }
    if dest_dir.exists() {
        fs::remove_dir_all(&dest_dir)?;
    }
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path)?;
    }
    if std::path::Path::new(log_path).exists() {
        fs::remove_file(log_path)?;
    }

    fs::create_dir_all(&source_dir)?;

    // Create initial file
    let file_path = source_dir.join("testfile.txt");
    {
        let mut f = File::create(&file_path)?;
        f.write_all(b"Original content")?;
    }

    // Get original mtime
    let original_mtime = FileTime::from_last_modification_time(&fs::metadata(&file_path)?);

    // First sync
    let args = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: false,
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args.clone())?;

    // Verify initial transfer
    assert!(dest_dir.join("testfile.txt").exists());
    assert_eq!(
        fs::read_to_string(dest_dir.join("testfile.txt"))?,
        "Original content"
    );

    // Modify file content (changes size), then restore original mtime
    {
        let mut f = File::create(&file_path)?;
        f.write_all(b"New content with different size!")?;
    }
    // Restore original mtime - now size differs but mtime is the same
    filetime::set_file_mtime(&file_path, original_mtime)?;

    // Run sync with rescan to detect change
    let args_rescan = Args {
        rescan: true,
        ..args.clone()
    };
    run(args_rescan)?;

    // Verify the file was re-synced due to size difference
    assert_eq!(
        fs::read_to_string(dest_dir.join("testfile.txt"))?,
        "New content with different size!"
    );

    // Cleanup
    fs::remove_dir_all(source_dir)?;
    fs::remove_dir_all(dest_dir)?;
    fs::remove_file(db_path)?;
    fs::remove_file(log_path)?;

    Ok(())
}

#[test]
fn test_resume_from_backlog() -> Result<()> {
    // Test that resuming works when there are pending files in the DB
    let source_dir = PathBuf::from("test_resume_source");
    let dest_dir = PathBuf::from("test_resume_dest");
    let db_path = "test_resume.db";
    let log_path = "test_resume.log";

    // Cleanup
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)?;
    }
    if dest_dir.exists() {
        fs::remove_dir_all(&dest_dir)?;
    }
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path)?;
    }
    if std::path::Path::new(log_path).exists() {
        fs::remove_file(log_path)?;
    }

    fs::create_dir_all(&source_dir)?;

    // Create files
    let mut f1 = File::create(source_dir.join("file1.txt"))?;
    f1.write_all(b"Content 1")?;
    let mut f2 = File::create(source_dir.join("file2.txt"))?;
    f2.write_all(b"Content 2")?;

    // First run - scan and transfer
    let args = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: false,
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args.clone())?;

    // Both files should be transferred
    assert!(dest_dir.join("file1.txt").exists());
    assert!(dest_dir.join("file2.txt").exists());

    // Cleanup
    fs::remove_dir_all(source_dir)?;
    fs::remove_dir_all(dest_dir)?;
    fs::remove_file(db_path)?;
    fs::remove_file(log_path)?;

    Ok(())
}

/// Test that --rescan with an existing database performs a fresh filesystem scan.
///
/// This test exposes a bug where --rescan would:
/// 1. Mark all existing DB records as pending
/// 2. Check pending_count() which was now > 0
/// 3. Skip the scan and enter "resume mode" with stale records
///
/// The correct behavior: --rescan should always perform a fresh filesystem scan,
/// detecting new files that weren't in the database before.
#[test]
fn test_rescan_detects_new_files() -> Result<()> {
    let source_dir = PathBuf::from("test_rescan_new_source");
    let dest_dir = PathBuf::from("test_rescan_new_dest");
    let db_path = "test_rescan_new.db";
    let log_path = "test_rescan_new.log";

    // Cleanup
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)?;
    }
    if dest_dir.exists() {
        fs::remove_dir_all(&dest_dir)?;
    }
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path)?;
    }
    if std::path::Path::new(log_path).exists() {
        fs::remove_file(log_path)?;
    }

    fs::create_dir_all(&source_dir)?;

    // Create initial file
    let mut f1 = File::create(source_dir.join("file1.txt"))?;
    f1.write_all(b"Content 1")?;

    // First run - creates database with file1.txt
    let args = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: false,
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 10,
        retry_interval_seconds: 60,
    };
    run(args.clone())?;

    assert!(dest_dir.join("file1.txt").exists());

    // Add a NEW file to source (not in database yet)
    let mut f2 = File::create(source_dir.join("file2_new.txt"))?;
    f2.write_all(b"New file content")?;

    // Run with --rescan: should perform fresh scan and detect the new file
    // BUG (before fix): would skip scan because DB had records, missing the new file
    let args_rescan = Args {
        rescan: true,
        ..args.clone()
    };
    run(args_rescan)?;

    // The new file MUST be transferred - this would fail with the bug
    assert!(
        dest_dir.join("file2_new.txt").exists(),
        "New file should be detected and transferred when using --rescan"
    );
    assert_eq!(
        fs::read_to_string(dest_dir.join("file2_new.txt"))?,
        "New file content"
    );

    // Cleanup
    fs::remove_dir_all(source_dir)?;
    fs::remove_dir_all(dest_dir)?;
    fs::remove_file(db_path)?;
    fs::remove_file(log_path)?;

    Ok(())
}

/// Test that files missing from source are skipped during transfer
/// and logged appropriately (not treated as errors).
///
/// This test simulates a scenario where:
/// 1. Files are scanned and added to the pending backlog
/// 2. The database is manipulated to add a "ghost" pending file
/// 3. On resume, the ghost file (no longer in source) should be skipped
#[test]
fn test_missing_source_file_skipped() -> Result<()> {
    use hsync::db::{Database, FileStatus};
    use std::sync::{Arc, Mutex};

    let source_dir = PathBuf::from("test_missing_source");
    let dest_dir = PathBuf::from("test_missing_dest");
    let db_path = "test_missing.db";
    let log_path = "test_missing.log";

    // Cleanup
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)?;
    }
    if dest_dir.exists() {
        fs::remove_dir_all(&dest_dir)?;
    }
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path)?;
    }
    if std::path::Path::new(log_path).exists() {
        fs::remove_file(log_path)?;
    }

    fs::create_dir_all(&source_dir)?;
    fs::create_dir_all(&dest_dir)?;

    // Create one file
    let mut f1 = File::create(source_dir.join("file1.txt"))?;
    f1.write_all(b"File 1 content")?;

    // Create database and manually add a pending file that doesn't exist
    let db = Arc::new(Mutex::new(Database::new(db_path)?));
    {
        let db_guard = db.lock().unwrap();
        // Add a real file
        db_guard.upsert_file(
            source_dir.join("file1.txt").to_str().unwrap(),
            dest_dir.join("file1.txt").to_str().unwrap(),
            0, // created
            0, // changed
            0, // modified
            0o644,
            14, // size
            FileStatus::Pending,
        )?;
        // Add a "ghost" file that doesn't exist in source
        db_guard.upsert_file(
            source_dir.join("ghost_file.txt").to_str().unwrap(),
            dest_dir.join("ghost_file.txt").to_str().unwrap(),
            0, // created
            0, // changed
            0, // modified
            0o644,
            100, // size
            FileStatus::Pending,
        )?;
    }
    drop(db);

    // Run with resume (database has pending files)
    let args = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: None,
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: false, // Resume from backlog
        block_size: "5M".to_string(),
        queue_capacity: 20,
        retry_attempts: 1,
        retry_interval_seconds: 0,
    };
    run(args)?;

    // file1 should be transferred
    assert!(dest_dir.join("file1.txt").exists());
    // ghost_file should NOT exist in dest (was skipped)
    assert!(!dest_dir.join("ghost_file.txt").exists());

    // Log should contain "source file no longer exists" message
    let log_content = fs::read_to_string(log_path)?;
    assert!(
        log_content.contains("source file no longer exists"),
        "Log should mention skipped file: {}",
        log_content
    );

    // Cleanup
    fs::remove_dir_all(source_dir)?;
    fs::remove_dir_all(dest_dir)?;
    fs::remove_file(db_path)?;
    fs::remove_file(log_path)?;

    Ok(())
}
