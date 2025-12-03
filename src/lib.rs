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

use cleanup::run_cleanup;
use db::Database;
use pipeline::{run_consumer, run_producer, Block, HashAlgorithm, PipelineConfig};
use scan::run_scan;
use utils::Logger;

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

    /// Maximum transfer speed in bytes per second (e.g., 20000000 for 20MB/s)
    #[arg(long)]
    pub bwlimit: Option<u64>,

    /// Checksum algorithm to use
    #[arg(long, value_enum, default_value_t = HashAlgorithm::Sha256)]
    pub checksum: HashAlgorithm,

    /// Enable deletion of extra files in destination
    #[arg(long)]
    pub delete_extras: bool,

    /// Force a full rescan, ignoring any existing backlog
    #[arg(long)]
    pub rescan: bool,
}

pub fn run(args: Args) -> Result<()> {
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
                    bw_limit: args.bwlimit,
                    db_path: args.db.clone(),
                    log_path: args.log.clone(),
                    hash_algo: args.checksum,
                };
                run_cleanup(&config, &logger)?;
            }
            println!("Sync completed.");
            return Ok(());
        }
    }

    // Transfer phase: process the backlog
    let (sender, receiver) = bounded::<Block>(20); // Fixed-size FIFO queue (20 slots)

    let config = PipelineConfig {
        source_dir: args.source.clone(),
        dest_dir: args.dest.clone(),
        bw_limit: args.bwlimit,
        db_path: args.db.clone(),
        log_path: args.log.clone(),
        hash_algo: args.checksum,
    };

    let producer_db = db.clone();
    let producer_logger = logger.clone();
    let producer_config = config.clone();
    let producer_handle = thread::spawn(move || {
        if let Err(e) = run_producer(producer_config, sender, producer_db, producer_logger) {
            eprintln!("Producer error: {}", e);
        }
    });

    let consumer_db = db.clone();
    let consumer_logger = logger.clone();
    let bw_limit = config.bw_limit;
    let consumer_handle = thread::spawn(move || {
        if let Err(e) = run_consumer(receiver, consumer_db, consumer_logger, bw_limit) {
            eprintln!("Consumer error: {}", e);
        }
    });

    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();

    if args.delete_extras {
        run_cleanup(&config, &logger)?;
    }

    println!("Sync completed.");
    Ok(())
}
