use anyhow::Result;
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;

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
