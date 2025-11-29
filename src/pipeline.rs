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
use walkdir::WalkDir;

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
    pub rescan: bool,
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

pub fn run_producer(
    config: PipelineConfig,
    sender: Sender<Block>,
    db: std::sync::Arc<std::sync::Mutex<Database>>,
    logger: std::sync::Arc<Logger>,
) -> Result<()> {
    let start_time = Instant::now();
    let mut total_bytes_sent = 0u64;

    // Count total files and size for progress bar (optional but nice)
    // For now, we'll just use a spinner or simple progress since walking twice is expensive on 200TB
    // But we can track total bytes transferred.

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} [{elapsed_precise}] {msg} ({bytes}/s)")?,
    );
    pb.set_message("Scanning...");

    for entry in WalkDir::new(&config.source_dir) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }

        let source_path = entry.path();
        let relative_path = source_path.strip_prefix(&config.source_dir)?;
        let dest_path = config.dest_dir.join(relative_path);

        let metadata = fs::metadata(source_path)?;
        let mtime = FileTime::from_last_modification_time(&metadata).unix_seconds();
        let atime = FileTime::from_last_access_time(&metadata).unix_seconds();
        // ctime is not standard in std::fs::Metadata on all platforms, using mtime as fallback or 0 if not available easily without platform specific ext
        // We'll use mtime for ctime or 0 if not available.
        let ctime = mtime;

        // Unix permissions
        #[cfg(unix)]
        let permissions = std::os::unix::fs::MetadataExt::mode(&metadata);
        #[cfg(not(unix))]
        let permissions = 0;

        let size = metadata.len();

        // Skipping Logic
        if !config.rescan && dest_path.exists() {
            if let Ok(dest_meta) = fs::metadata(&dest_path) {
                let dest_mtime = FileTime::from_last_modification_time(&dest_meta).unix_seconds();
                if dest_mtime == mtime {
                    // Skip
                    let _ = logger.log(&format!("Skipping: {:?}", source_path));
                    // Update DB with null hash
                    let db_guard = db.lock().unwrap();
                    let _ = db_guard.upsert_file_state(
                        source_path.to_str().unwrap(),
                        dest_path.to_str().unwrap(),
                        0, // created
                        ctime,
                        mtime,
                        permissions,
                        None,
                        size,
                    );
                    continue;
                }
            }
        }

        // Process File
        pb.set_message(format!("Processing: {:?}", relative_path));
        let mut file = File::open(source_path)?;
        let mut hasher = create_hasher(config.hash_algo);
        let mut offset = 0u64;
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
                        source_path: source_path.to_path_buf(),
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

            // Rate Limiting
            if let Some(limit) = config.bw_limit {
                total_bytes_sent += bytes_read as u64;
                let expected_duration =
                    Duration::from_secs_f64(total_bytes_sent as f64 / limit as f64);
                let elapsed = start_time.elapsed();
                if expected_duration > elapsed {
                    thread::sleep(expected_duration - elapsed);
                }
            } else {
                total_bytes_sent += bytes_read as u64;
            }

            pb.set_position(total_bytes_sent);

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
                source_path: source_path.to_path_buf(),
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
    }
    pb.finish_with_message("Producer finished.");
    Ok(())
}

pub fn run_consumer(
    receiver: Receiver<Block>,
    db: std::sync::Arc<std::sync::Mutex<Database>>,
    logger: std::sync::Arc<Logger>,
) -> Result<()> {
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

        if block.is_last_block {
            // Metadata Sync
            let mtime = FileTime::from_unix_time(block.mtime, 0);
            let atime = FileTime::from_unix_time(block.atime, 0);
            set_file_times(&block.dest_path, atime, mtime)?;

            // Persistence
            let db_guard = db.lock().unwrap();
            db_guard.upsert_file_state(
                block.source_path.to_str().unwrap(),
                block.dest_path.to_str().unwrap(),
                0, // created
                block.ctime,
                block.mtime,
                block.permissions,
                block.file_hash.as_deref(),
                block.file_size,
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
