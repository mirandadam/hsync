use anyhow::Result;
use clap::Parser;
use crossbeam_channel::bounded;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

mod db;
mod pipeline;
mod utils;

use db::Database;
use pipeline::{run_consumer, run_producer, Block, PipelineConfig};
use utils::Logger;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to source directory
    #[arg(long)]
    source: PathBuf,

    /// Path to destination directory
    #[arg(long)]
    dest: PathBuf,

    /// Local database file path
    #[arg(long, default_value = "hsync.db")]
    db: String,

    /// Audit log file path
    #[arg(long, default_value = "hsync.log")]
    log: String,

    /// Maximum transfer speed in bytes per second (e.g., 20000000 for 20MB/s)
    #[arg(long)]
    bwlimit: Option<u64>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let db = Arc::new(Mutex::new(Database::new(&args.db)?));
    let logger = Arc::new(Logger::new(&args.log));

    let (sender, receiver) = bounded::<Block>(20); // Fixed-size FIFO queue (20 slots)

    let config = PipelineConfig {
        source_dir: args.source.clone(),
        dest_dir: args.dest.clone(),
        bw_limit: args.bwlimit,
        db_path: args.db.clone(),
        log_path: args.log.clone(),
    };

    let producer_db = db.clone();
    let producer_logger = logger.clone();
    let producer_handle = thread::spawn(move || {
        if let Err(e) = run_producer(config, sender, producer_db, producer_logger) {
            eprintln!("Producer error: {}", e);
        }
    });

    let consumer_db = db.clone();
    let consumer_logger = logger.clone();
    let consumer_handle = thread::spawn(move || {
        if let Err(e) = run_consumer(receiver, consumer_db, consumer_logger) {
            eprintln!("Consumer error: {}", e);
        }
    });

    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();

    println!("Sync completed.");
    Ok(())
}
