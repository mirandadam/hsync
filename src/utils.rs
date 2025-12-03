use anyhow::Result;
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;

/// Formats byte count in human-readable form (e.g., "1.5 GB")
pub fn format_bytes(bytes: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;

    #[test]
    fn test_logger() -> Result<()> {
        let log_path = "test_log.txt";
        let _ = fs::remove_file(log_path); // Ensure clean start

        let logger = Logger::new(log_path);
        logger.log("Test message 1")?;
        logger.log("Test message 2")?;

        let mut content = String::new();
        fs::File::open(log_path)?.read_to_string(&mut content)?;

        assert!(content.contains("Test message 1"));
        assert!(content.contains("Test message 2"));
        assert!(content.contains("[")); // Timestamp check

        fs::remove_file(log_path)?;
        Ok(())
    }
}

// Global logger instance could be used, or passed around.
// For simplicity in this PoC, we might just instantiate it where needed or pass it.
