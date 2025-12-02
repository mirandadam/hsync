//! Parallel scanning of source and destination directories.
//!
//! Scans both directories independently to build a backlog of files
//! that need to be transferred.

use crate::db::{Database, FileStatus};
use anyhow::Result;
use filetime::FileTime;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;

/// Scan results from the destination directory
type DestinationMap = HashMap<PathBuf, i64>; // relative path -> mtime

/// Runs the parallel scan phase, populating the database with file states.
/// Returns the number of files pending transfer.
pub fn run_scan(
    source_dir: &PathBuf,
    dest_dir: &PathBuf,
    db: &Arc<Mutex<Database>>,
) -> Result<u64> {
    let multi_progress = MultiProgress::new();

    // Create progress bars for source and destination scans
    let source_pb = multi_progress.add(ProgressBar::new_spinner());
    source_pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {prefix}: {msg}")
            .unwrap(),
    );
    source_pb.set_prefix("Source");
    source_pb.enable_steady_tick(Duration::from_millis(100));

    let dest_pb = multi_progress.add(ProgressBar::new_spinner());
    dest_pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.blue} {prefix}: {msg}")
            .unwrap(),
    );
    dest_pb.set_prefix("Destination");
    dest_pb.enable_steady_tick(Duration::from_millis(100));

    // Scan destination first to build a lookup map
    let dest_dir_clone = dest_dir.clone();
    let dest_pb_clone = dest_pb.clone();
    let dest_handle = thread::spawn(move || scan_destination(&dest_dir_clone, &dest_pb_clone));

    // Scan source directory
    let source_dir_clone = source_dir.clone();
    let dest_dir_clone = dest_dir.clone();
    let db_clone = db.clone();
    let source_pb_clone = source_pb.clone();

    // Wait for destination scan to complete
    let dest_map = dest_handle.join().unwrap()?;
    dest_pb.finish_with_message(format!("{} files found", dest_map.len()));

    // Now scan source and compare with destination
    let (scanned, pending) = scan_source_and_compare(
        &source_dir_clone,
        &dest_dir_clone,
        &dest_map,
        &db_clone,
        &source_pb_clone,
    )?;

    source_pb.finish_with_message(format!("{} files scanned, {} pending", scanned, pending));

    // Summary progress bar
    let summary_pb = multi_progress.add(ProgressBar::new_spinner());
    summary_pb.set_style(ProgressStyle::default_spinner().template("{msg}").unwrap());
    summary_pb.finish_with_message(format!(
        "Scan complete: {} source files, {} destination files, {} to transfer",
        scanned,
        dest_map.len(),
        pending
    ));

    Ok(pending)
}

/// Scans the destination directory and returns a map of relative paths to mtimes.
fn scan_destination(dest_dir: &PathBuf, pb: &ProgressBar) -> Result<DestinationMap> {
    let mut dest_map = HashMap::new();
    let mut count = 0u64;

    for entry in WalkDir::new(dest_dir) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // Skip inaccessible entries
        };

        if entry.file_type().is_dir() {
            continue;
        }

        let path = entry.path();
        if let Ok(relative) = path.strip_prefix(dest_dir) {
            if let Ok(metadata) = fs::metadata(path) {
                let mtime = FileTime::from_last_modification_time(&metadata).unix_seconds();
                dest_map.insert(relative.to_path_buf(), mtime);
                count += 1;

                if count % 1000 == 0 {
                    pb.set_message(format!("{} files scanned", count));
                }
            }
        }
    }

    pb.set_message(format!("{} files scanned", count));
    Ok(dest_map)
}

/// Scans source directory, compares with destination, and populates the database.
/// Returns (total_scanned, pending_count).
fn scan_source_and_compare(
    source_dir: &PathBuf,
    dest_dir: &PathBuf,
    dest_map: &DestinationMap,
    db: &Arc<Mutex<Database>>,
    pb: &ProgressBar,
) -> Result<(u64, u64)> {
    let mut scanned = 0u64;
    let mut pending = 0u64;

    for entry in WalkDir::new(source_dir) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().is_dir() {
            continue;
        }

        let source_path = entry.path();
        let relative_path = match source_path.strip_prefix(source_dir) {
            Ok(p) => p.to_path_buf(),
            Err(_) => continue,
        };

        let metadata = match fs::metadata(source_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let mtime = FileTime::from_last_modification_time(&metadata).unix_seconds();
        let atime = FileTime::from_last_access_time(&metadata).unix_seconds();
        let ctime = mtime; // ctime fallback
        let size = metadata.len();

        #[cfg(unix)]
        let permissions = std::os::unix::fs::MetadataExt::mode(&metadata);
        #[cfg(not(unix))]
        let permissions = 0u32;

        let dest_path = dest_dir.join(&relative_path);

        // Determine status: check if destination exists with matching mtime
        let status = match dest_map.get(&relative_path) {
            Some(&dest_mtime) if dest_mtime == mtime => FileStatus::Synced,
            _ => {
                pending += 1;
                FileStatus::Pending
            }
        };

        // Update database
        let db_guard = db.lock().unwrap();
        db_guard.upsert_file(
            source_path.to_str().unwrap(),
            dest_path.to_str().unwrap(),
            atime, // Using atime as created (not available on all platforms)
            ctime,
            mtime,
            permissions,
            size,
            status,
        )?;
        drop(db_guard);

        scanned += 1;
        if scanned % 1000 == 0 {
            pb.set_message(format!("{} files scanned, {} pending", scanned, pending));
        }
    }

    Ok((scanned, pending))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_scan_empty_dirs() -> Result<()> {
        let source = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;
        let db = Arc::new(Mutex::new(Database::new(":memory:")?));

        let pending = run_scan(
            &source.path().to_path_buf(),
            &dest.path().to_path_buf(),
            &db,
        )?;

        assert_eq!(pending, 0);
        Ok(())
    }

    #[test]
    fn test_scan_source_only() -> Result<()> {
        let source = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;

        // Create file in source only
        let mut f = File::create(source.path().join("file1.txt"))?;
        f.write_all(b"hello")?;

        let db = Arc::new(Mutex::new(Database::new(":memory:")?));

        let pending = run_scan(
            &source.path().to_path_buf(),
            &dest.path().to_path_buf(),
            &db,
        )?;

        assert_eq!(pending, 1);
        assert_eq!(db.lock().unwrap().pending_count()?, 1);
        Ok(())
    }

    #[test]
    fn test_scan_matching_files() -> Result<()> {
        let source = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;

        // Create matching files
        let mut sf = File::create(source.path().join("file1.txt"))?;
        sf.write_all(b"hello")?;
        drop(sf);

        let mut df = File::create(dest.path().join("file1.txt"))?;
        df.write_all(b"hello")?;
        drop(df);

        // Set same mtime
        let src_meta = fs::metadata(source.path().join("file1.txt"))?;
        let src_mtime = FileTime::from_last_modification_time(&src_meta);
        filetime::set_file_mtime(dest.path().join("file1.txt"), src_mtime)?;

        let db = Arc::new(Mutex::new(Database::new(":memory:")?));

        let pending = run_scan(
            &source.path().to_path_buf(),
            &dest.path().to_path_buf(),
            &db,
        )?;

        assert_eq!(pending, 0);
        assert_eq!(db.lock().unwrap().pending_count()?, 0);
        Ok(())
    }
}
