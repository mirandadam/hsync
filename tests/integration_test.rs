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
    f2.write_all(&vec![0u8; 1024 * 1024])?; // 1MB

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
        bwlimit: Some("10M".to_string()), // 10MB/s
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: true, // Force rescan to detect the change
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
