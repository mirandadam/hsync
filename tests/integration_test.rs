use anyhow::Result;
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

    // 2. Run Sync
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

    // Verify
    assert!(dest_dir.join("file1.txt").exists());
    assert!(dest_dir.join("large.bin").exists());
    assert!(dest_dir.join("empty.txt").exists());
    assert_eq!(fs::metadata(dest_dir.join("large.bin"))?.len(), 1024 * 1024);
    assert_eq!(fs::metadata(dest_dir.join("empty.txt"))?.len(), 0);

    // 3. Skipping Test (Run again)
    // We expect "Skipping" in the log or just fast execution.
    // To verify skipping, we can check if the log contains "Skipping" if we were logging it there,
    // but the logger might only log transfers.
    // Let's check the code: logger logs "Transferred: ...".
    // The skipping logic logs "Skipping: ..." to the logger.
    run(args)?;

    let log_content = fs::read_to_string(log_path)?;
    assert!(log_content.contains("Skipping"));

    // 4. Truncation Test
    // Shrink source file
    let mut f2 = File::create(source_dir.join("large.bin"))?; // Truncates source
    f2.write_all(b"Small")?;

    // Run with rescan to force update
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

    // 6. Bandwidth Limit Test
    // We just want to exercise the code path, not strictly verify the timing as it can be flaky.
    let args_bw = Args {
        source: source_dir.clone(),
        dest: dest_dir.clone(),
        db: db_path.to_string(),
        log: log_path.to_string(),
        bwlimit: Some(1024 * 1024 * 10), // 10MB/s
        checksum: HashAlgorithm::Sha256,
        delete_extras: false,
        rescan: true, // Force re-transfer to trigger rate limiting
    };
    run(args_bw)?;

    // Cleanup
    fs::remove_dir_all(source_dir)?;
    fs::remove_dir_all(dest_dir)?;
    fs::remove_file(db_path)?;
    fs::remove_file(log_path)?;

    Ok(())
}
