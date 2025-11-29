use anyhow::Result;
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

pub struct Logger {
    file_path: String,
}

impl Logger {
    pub fn new(file_path: &str) -> Self {
        Self {
            file_path: file_path.to_string(),
        }
    }

    pub fn log(&self, message: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)?;

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        writeln!(file, "[{}] {}", timestamp, message)?;
        Ok(())
    }
}

// Global logger instance could be used, or passed around.
// For simplicity in this PoC, we might just instantiate it where needed or pass it.
