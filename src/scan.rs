//! Parallel scanning of source and destination directories.
//!
//! Scans both directories independently to build a backlog of files
//! that need to be transferred.

use crate::db::{Database, FileStatus};
use crate::utils::format_bytes;
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
/// Maps relative path to (mtime, size)
type DestinationMap = HashMap<PathBuf, (i64, u64)>;

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

    // Scan source and destination in parallel
    let dest_dir_clone = dest_dir.clone();
    let dest_pb_clone = dest_pb.clone();
    let dest_handle = thread::spawn(move || scan_destination(&dest_dir_clone, &dest_pb_clone));

    let source_dir_clone = source_dir.clone();
    let source_pb_clone = source_pb.clone();
    let source_handle = thread::spawn(move || scan_source(&source_dir_clone, &source_pb_clone));

    // Wait for both scans to complete
    let (dest_map, dest_total_size) = dest_handle.join().unwrap()?;
    dest_pb.finish_with_message(format!(
        "{} files found ({})",
        dest_map.len(),
        format_bytes(dest_total_size)
    ));

    let (source_map, source_total_size) = source_handle.join().unwrap()?;
    source_pb.finish_with_message(format!(
        "{} files found ({})",
        source_map.len(),
        format_bytes(source_total_size)
    ));

    // Compare and populate database
    println!("Updating database...");
    let pending = compare_and_populate(source_dir, dest_dir, &source_map, &dest_map, db)?;

    // Summary progress bar
    let summary_pb = multi_progress.add(ProgressBar::new_spinner());
    summary_pb.set_style(ProgressStyle::default_spinner().template("{msg}").unwrap());
    summary_pb.finish_with_message(format!(
        "Scan complete: {} source files ({}), {} destination files ({}), {} to transfer",
        source_map.len(),
        format_bytes(source_total_size),
        dest_map.len(),
        format_bytes(dest_total_size),
        pending
    ));

    Ok(pending)
}

/// Source file metadata: (mtime, atime, size, permissions)
type SourceFileInfo = (i64, i64, u64, u32);

/// Scan results from the source directory
/// Maps relative path to file metadata
type SourceMap = HashMap<PathBuf, SourceFileInfo>;

/// Scans the destination directory and returns a map of relative paths to (mtime, size)
/// along with the total size of all scanned files.
fn scan_destination(dest_dir: &PathBuf, pb: &ProgressBar) -> Result<(DestinationMap, u64)> {
    let mut dest_map = HashMap::new();
    let mut count = 0u64;
    let mut total_size = 0u64;

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
                let size = metadata.len();
                dest_map.insert(relative.to_path_buf(), (mtime, size));
                count += 1;
                total_size += size;

                if count % 1000 == 0 {
                    pb.set_message(format!(
                        "{} files scanned ({})",
                        count,
                        format_bytes(total_size)
                    ));
                }
            }
        }
    }

    pb.set_message(format!(
        "{} files scanned ({})",
        count,
        format_bytes(total_size)
    ));
    Ok((dest_map, total_size))
}

/// Scans source directory and returns a map of relative paths to file metadata
/// along with the total size of all scanned files.
fn scan_source(source_dir: &PathBuf, pb: &ProgressBar) -> Result<(SourceMap, u64)> {
    let mut source_map = HashMap::new();
    let mut count = 0u64;
    let mut total_size = 0u64;

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
        let size = metadata.len();

        #[cfg(unix)]
        let permissions = std::os::unix::fs::MetadataExt::mode(&metadata);
        #[cfg(not(unix))]
        let permissions = 0u32;

        source_map.insert(relative_path, (mtime, atime, size, permissions));
        count += 1;
        total_size += size;

        if count % 1000 == 0 {
            pb.set_message(format!(
                "{} files scanned ({})",
                count,
                format_bytes(total_size)
            ));
        }
    }

    pb.set_message(format!(
        "{} files scanned ({})",
        count,
        format_bytes(total_size)
    ));
    Ok((source_map, total_size))
}

/// Compares source and destination maps, populates the database.
/// Returns the number of pending files.
fn compare_and_populate(
    source_dir: &PathBuf,
    dest_dir: &PathBuf,
    source_map: &SourceMap,
    dest_map: &DestinationMap,
    db: &Arc<Mutex<Database>>,
) -> Result<u64> {
    let mut pending = 0u64;

    // Hold lock for entire operation and use a single transaction for performance
    let db_guard = db.lock().unwrap();
    db_guard.begin_transaction()?;

    for (relative_path, &(mtime, atime, size, permissions)) in source_map {
        let source_path = source_dir.join(relative_path);
        let dest_path = dest_dir.join(relative_path);
        let ctime = mtime; // ctime fallback

        // Determine status: check if destination exists with matching mtime and size
        let status = match dest_map.get(relative_path) {
            Some(&(dest_mtime, dest_size)) if dest_mtime == mtime && dest_size == size => {
                FileStatus::Synced
            }
            _ => {
                pending += 1;
                FileStatus::Pending
            }
        };

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
    }

    db_guard.commit_transaction()?;
    drop(db_guard);

    Ok(pending)
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
