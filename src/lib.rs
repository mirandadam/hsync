pub mod cleanup;
pub mod db;
pub mod pipeline;
pub mod scan;
pub mod utils;

use anyhow::Result;
use clap::Parser;
use crossbeam_channel::bounded;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cleanup::run_cleanup;
use db::Database;
use pipeline::{run_consumer, run_producer, Block, HashAlgorithm, PipelineConfig};
use scan::run_scan;
use utils::{parse_bandwidth, Logger};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to source directory
    #[arg(long)]
    pub source: PathBuf,

    /// Path to destination directory
    #[arg(long)]
    pub dest: PathBuf,

    /// Local database file path
    #[arg(long, default_value = "hsync.db")]
    pub db: String,

    /// Audit log file path
    #[arg(long, default_value = "hsync.log")]
    pub log: String,

    /// Maximum transfer speed (e.g., 20M, 512K, 1G, or raw bytes)
    #[arg(long)]
    pub bwlimit: Option<String>,

    /// Checksum algorithm to use
    #[arg(long, value_enum, default_value_t = HashAlgorithm::Sha256)]
    pub checksum: HashAlgorithm,

    /// Enable deletion of extra files in destination
    #[arg(long)]
    pub delete_extras: bool,

    /// Force a full rescan, ignoring any existing backlog
    #[arg(long)]
    pub rescan: bool,

    /// Block size for file transfer (e.g., 1M, 512K)
    #[arg(long, default_value = "5M")]
    pub block_size: String,

    /// Number of block queue slots for pipeline buffering
    #[arg(long, default_value_t = 20)]
    pub queue_capacity: usize,

    /// Total transfer attempts (including initial attempt)
    #[arg(long, default_value_t = 10)]
    pub retry_attempts: u32,

    /// Seconds to wait between retry attempts
    #[arg(long, default_value_t = 60)]
    pub retry_interval_seconds: u64,
}

pub fn run(args: Args) -> Result<()> {
    // Parse bandwidth limit if provided
    let bw_limit = args
        .bwlimit
        .as_ref()
        .map(|s| parse_bandwidth(s))
        .transpose()?;

    // Parse block size
    let block_size = parse_bandwidth(&args.block_size)? as usize;

    let queue_capacity = args.queue_capacity;

    let db = Arc::new(Mutex::new(Database::new(&args.db)?));
    let logger = Arc::new(Logger::new(&args.log));

    // Determine mode: resume from backlog or perform fresh scan
    let should_scan = if args.rescan {
        println!("Forcing full rescan...");
        true
    } else {
        let pending_count = {
            let db_guard = db.lock().unwrap();
            db_guard.pending_count()?
        };
        if pending_count > 0 {
            println!("Resuming: {} files pending transfer.", pending_count);
            false
        } else {
            true
        }
    };

    if should_scan {
        println!("Scanning source and destination directories...");
        let pending = run_scan(&args.source, &args.dest, &db)?;

        if pending == 0 {
            println!("All files are already synced.");
            if args.delete_extras {
                let config = PipelineConfig {
                    source_dir: args.source.clone(),
                    dest_dir: args.dest.clone(),
                    bw_limit,
                    db_path: args.db.clone(),
                    log_path: args.log.clone(),
                    hash_algo: args.checksum,
                    block_size,
                };
                run_cleanup(&config, &logger)?;
            }
            println!("Sync completed.");
            return Ok(());
        }
    }

    // Transfer phase: process the backlog with retry logic
    let config = PipelineConfig {
        source_dir: args.source.clone(),
        dest_dir: args.dest.clone(),
        bw_limit,
        db_path: args.db.clone(),
        log_path: args.log.clone(),
        hash_algo: args.checksum,
        block_size,
    };

    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=args.retry_attempts {
        // Check if there are still pending files
        let pending_count = {
            let db_guard = db.lock().unwrap();
            db_guard.pending_count()?
        };

        if pending_count == 0 {
            // All files transferred successfully
            break;
        }

        if attempt > 1 {
            // Log retry attempt
            let msg = format!(
                "Retry attempt {}/{}: {} (waiting {}s before retry)",
                attempt,
                args.retry_attempts,
                last_error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_default(),
                args.retry_interval_seconds
            );
            eprintln!("{}", msg);
            let _ = logger.log(&msg);
            thread::sleep(Duration::from_secs(args.retry_interval_seconds));
        }

        let (sender, receiver) = bounded::<Block>(queue_capacity);

        let producer_db = db.clone();
        let producer_logger = logger.clone();
        let producer_config = config.clone();
        let producer_handle = thread::spawn(move || -> Result<()> {
            run_producer(producer_config, sender, producer_db, producer_logger)
        });

        let consumer_db = db.clone();
        let consumer_logger = logger.clone();
        let bw_limit = config.bw_limit;
        let consumer_handle = thread::spawn(move || -> Result<()> {
            run_consumer(receiver, consumer_db, consumer_logger, bw_limit)
        });

        let producer_result = producer_handle.join().unwrap();
        let consumer_result = consumer_handle.join().unwrap();

        // Check for errors from either thread
        match (producer_result, consumer_result) {
            (Ok(()), Ok(())) => {
                last_error = None;
            }
            (Err(e), _) => {
                last_error = Some(e);
            }
            (_, Err(e)) => {
                last_error = Some(e);
            }
        }

        if last_error.is_none() {
            break;
        }
    }

    // Check if retries were exhausted with an error
    if let Some(e) = last_error {
        let pending_count = {
            let db_guard = db.lock().unwrap();
            db_guard.pending_count()?
        };
        if pending_count > 0 {
            let msg = format!(
                "Transfer failed after {} attempts: {}",
                args.retry_attempts, e
            );
            eprintln!("{}", msg);
            let _ = logger.log(&msg);
            return Err(anyhow::anyhow!(msg));
        }
    }

    if args.delete_extras {
        run_cleanup(&config, &logger)?;
    }

    println!("Sync completed.");
    Ok(())
}
