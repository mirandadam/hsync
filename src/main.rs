use anyhow::Result;
use clap::Parser;
use hsync::{run, Args};

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}
