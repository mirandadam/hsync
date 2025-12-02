use crate::db::Database;
use crate::utils::Logger;
use anyhow::{Context, Result};
use clap::ValueEnum;
use crossbeam_channel::{Receiver, Sender};
use filetime::{set_file_times, FileTime};
use indicatif::{ProgressBar, ProgressStyle};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

pub const BLOCK_SIZE: usize = 5 * 1024 * 1024; // 5MB

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
}

#[derive(Debug)]
pub struct Block {
    pub data: Vec<u8>,
    pub offset: u64,
    pub dest_path: PathBuf,
    pub source_path: PathBuf, // Needed for DB
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub permissions: u32,
    pub is_last_block: bool,
    pub file_hash: Option<String>,
    pub file_size: u64,
}

#[derive(Clone)]
pub struct PipelineConfig {
    pub source_dir: PathBuf,
    pub dest_dir: PathBuf,
    pub bw_limit: Option<u64>, // bytes per second
    #[allow(dead_code)]
    pub db_path: String,
    #[allow(dead_code)]
    pub log_path: String,
    pub hash_algo: HashAlgorithm,
}

trait DynDigest: Send {
    fn update(&mut self, data: &[u8]);
    fn finalize_hex(&self) -> String;
}

struct Md5Wrapper(Md5);
impl DynDigest for Md5Wrapper {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    fn finalize_hex(&self) -> String {
        hex::encode(self.0.clone().finalize())
    }
}

struct Sha1Wrapper(Sha1);
impl DynDigest for Sha1Wrapper {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    fn finalize_hex(&self) -> String {
        hex::encode(self.0.clone().finalize())
    }
}

struct Sha256Wrapper(Sha256);
impl DynDigest for Sha256Wrapper {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    fn finalize_hex(&self) -> String {
        hex::encode(self.0.clone().finalize())
    }
}

fn create_hasher(algo: HashAlgorithm) -> Box<dyn DynDigest> {
    match algo {
        HashAlgorithm::Md5 => Box::new(Md5Wrapper(Md5::new())),
        HashAlgorithm::Sha1 => Box::new(Sha1Wrapper(Sha1::new())),
        HashAlgorithm::Sha256 => Box::new(Sha256Wrapper(Sha256::new())),
    }
}

/// Formats byte count in human-readable form (e.g., "1.5 GB")
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Formats a duration as human-readable time (e.g., "2h 15m 30s")
fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {:02}m", hours, mins)
    } else if secs >= 60 {
        let mins = secs / 60;
        let secs_rem = secs % 60;
        format!("{}m {:02}s", mins, secs_rem)
    } else {
        format!("{}s", secs)
    }
}

/// Producer that reads files from the database backlog (pending files).
pub fn run_producer(
    config: PipelineConfig,
    sender: Sender<Block>,
    db: std::sync::Arc<std::sync::Mutex<Database>>,
    logger: std::sync::Arc<Logger>,
) -> Result<()> {
    let mut total_bytes_sent = 0u64;
    let mut files_transferred = 0u64;
    let transfer_start = Instant::now();

    // Get pending files and total bytes from database
    let (pending_files, total_pending_bytes) = {
        let db_guard = db.lock().unwrap();
        (
            db_guard.get_pending_files()?,
            db_guard.pending_total_bytes()?,
        )
    };

    let total_files = pending_files.len();
    if total_files == 0 {
        println!("No files to transfer.");
        return Ok(());
    }

    println!(
        "Transferring {} files ({})...",
        total_files,
        format_bytes(total_pending_bytes)
    );

    // Per-file progress bar for ETA and bandwidth display
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} {msg}\n[{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA: {eta})")?
            .progress_chars("=>-"),
    );

    for file_record in pending_files {
        let source_path = PathBuf::from(&file_record.source_path);
        let dest_path = PathBuf::from(&file_record.dest_path);

        // Compute relative path for display
        let relative_path = source_path
            .strip_prefix(&config.source_dir)
            .unwrap_or(&source_path);

        // Get fresh metadata from source (file may have changed since scan)
        let metadata = match fs::metadata(&source_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = logger.log(&format!("Skipping (read error): {:?} - {}", source_path, e));
                continue;
            }
        };

        let mtime = FileTime::from_last_modification_time(&metadata).unix_seconds();
        let atime = FileTime::from_last_access_time(&metadata).unix_seconds();
        let ctime = mtime;
        let size = metadata.len();

        #[cfg(unix)]
        let permissions = std::os::unix::fs::MetadataExt::mode(&metadata);
        #[cfg(not(unix))]
        let permissions = 0u32;

        // Reset progress bar for this file
        pb.set_length(size);
        pb.set_position(0);

        // Calculate backlog ETA based on transfer rate so far
        let backlog_eta = if total_bytes_sent > 0 {
            let elapsed = transfer_start.elapsed();
            let rate = total_bytes_sent as f64 / elapsed.as_secs_f64();
            let remaining = total_pending_bytes.saturating_sub(total_bytes_sent);
            if rate > 0.0 {
                Some(Duration::from_secs_f64(remaining as f64 / rate))
            } else {
                None
            }
        } else {
            None
        };

        let eta_str = backlog_eta
            .map(|d| format!(" Backlog ETA: {}", format_duration(d)))
            .unwrap_or_default();

        pb.set_message(format!(
            "[{}/{} Total: {}{}] {}",
            files_transferred + 1,
            total_files,
            format_bytes(total_bytes_sent),
            eta_str,
            relative_path.display()
        ));

        let mut file = match File::open(&source_path) {
            Ok(f) => f,
            Err(e) => {
                let _ = logger.log(&format!("Skipping (open error): {:?} - {}", source_path, e));
                continue;
            }
        };

        let mut hasher = create_hasher(config.hash_algo);
        let mut offset = 0u64;
        let mut file_bytes_sent = 0u64;
        let mut buffer = vec![0u8; BLOCK_SIZE];

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                // Handle empty file case
                if size == 0 {
                    let block = Block {
                        data: vec![],
                        offset: 0,
                        dest_path: dest_path.clone(),
                        source_path: source_path.clone(),
                        atime,
                        mtime,
                        ctime,
                        permissions,
                        is_last_block: true,
                        file_hash: Some(hasher.finalize_hex()),
                        file_size: 0,
                    };
                    sender.send(block).context("Failed to send block")?;
                }
                break;
            }

            total_bytes_sent += bytes_read as u64;
            file_bytes_sent += bytes_read as u64;

            pb.set_position(file_bytes_sent);

            // Update backlog ETA during transfer
            let backlog_eta = {
                let elapsed = transfer_start.elapsed();
                let rate = total_bytes_sent as f64 / elapsed.as_secs_f64();
                let remaining = total_pending_bytes.saturating_sub(total_bytes_sent);
                if rate > 0.0 {
                    Some(Duration::from_secs_f64(remaining as f64 / rate))
                } else {
                    None
                }
            };

            let eta_str = backlog_eta
                .map(|d| format!(" Backlog ETA: {}", format_duration(d)))
                .unwrap_or_default();

            pb.set_message(format!(
                "[{}/{} Total: {}{}] {}",
                files_transferred + 1,
                total_files,
                format_bytes(total_bytes_sent),
                eta_str,
                relative_path.display()
            ));

            let chunk_data = buffer[0..bytes_read].to_vec();
            hasher.update(&chunk_data);

            let is_last = (offset + bytes_read as u64) == size;
            let file_hash = if is_last {
                Some(hasher.finalize_hex())
            } else {
                None
            };

            let block = Block {
                data: chunk_data,
                offset,
                dest_path: dest_path.clone(),
                source_path: source_path.clone(),
                atime,
                mtime,
                ctime,
                permissions,
                is_last_block: is_last,
                file_hash,
                file_size: size,
            };

            sender.send(block).context("Failed to send block")?;
            offset += bytes_read as u64;

            if is_last {
                break;
            }
        }

        files_transferred += 1;
    }

    pb.finish_with_message(format!(
        "Finished. {} files transferred, {}",
        files_transferred,
        format_bytes(total_bytes_sent)
    ));
    Ok(())
}

pub fn run_consumer(
    receiver: Receiver<Block>,
    db: std::sync::Arc<std::sync::Mutex<Database>>,
    logger: std::sync::Arc<Logger>,
    bw_limit: Option<u64>,
) -> Result<()> {
    let start_time = Instant::now();
    let mut total_bytes_written = 0u64;

    while let Ok(block) = receiver.recv() {
        if let Some(parent) = block.dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut options = OpenOptions::new();
        options.write(true).create(true);

        // Truncate if writing from the beginning (new file or overwrite)
        if block.offset == 0 {
            options.truncate(true);
        }

        let mut file = options.open(&block.dest_path)?;

        file.seek(SeekFrom::Start(block.offset))?;
        file.write_all(&block.data)?;

        // Rate Limiting on the write side to enable full-duplex streaming
        let bytes_written = block.data.len() as u64;
        total_bytes_written += bytes_written;
        if let Some(limit) = bw_limit {
            let expected_duration =
                Duration::from_secs_f64(total_bytes_written as f64 / limit as f64);
            let elapsed = start_time.elapsed();
            if expected_duration > elapsed {
                thread::sleep(expected_duration - elapsed);
            }
        }

        if block.is_last_block {
            // Metadata Sync
            let mtime = FileTime::from_unix_time(block.mtime, 0);
            let atime = FileTime::from_unix_time(block.atime, 0);
            set_file_times(&block.dest_path, atime, mtime)?;

            // Persistence - mark as synced with hash
            let db_guard = db.lock().unwrap();
            db_guard.mark_synced(
                block.source_path.to_str().unwrap(),
                block.file_hash.as_deref().unwrap_or(""),
            )?;

            // Audit
            logger.log(&format!(
                "Transferred: {:?} -> {:?} (Hash: {})",
                block.source_path,
                block.dest_path,
                block.file_hash.as_deref().unwrap_or("?")
            ))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_hasher() {
        let mut h = create_hasher(HashAlgorithm::Md5);
        h.update(b"hello");
        assert_eq!(h.finalize_hex(), "5d41402abc4b2a76b9719d911017c592");

        let mut h = create_hasher(HashAlgorithm::Sha1);
        h.update(b"hello");
        assert_eq!(h.finalize_hex(), "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");

        let mut h = create_hasher(HashAlgorithm::Sha256);
        h.update(b"hello");
        assert_eq!(
            h.finalize_hex(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
