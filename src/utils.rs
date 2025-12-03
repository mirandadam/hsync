use anyhow::{anyhow, Result};
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;

/// Parses a human-readable bandwidth string (e.g., "20M", "512K") into bytes per second.
/// Supports suffixes: K/k (1024), M/m (1024²), G/g (1024³). No suffix means bytes.
pub fn parse_bandwidth(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("Bandwidth value cannot be empty"));
    }

    // Check if the last character is a suffix
    let last_char = s.chars().last().unwrap();
    let (num_str, multiplier) = match last_char {
        'K' | 'k' => (&s[..s.len() - 1], 1024u64),
        'M' | 'm' => (&s[..s.len() - 1], 1024u64 * 1024),
        'G' | 'g' => (&s[..s.len() - 1], 1024u64 * 1024 * 1024),
        _ => (s, 1u64),
    };

    let num: f64 = num_str
        .trim()
        .parse()
        .map_err(|_| anyhow!("Invalid bandwidth value: '{}'", s))?;

    if num < 0.0 {
        return Err(anyhow!("Bandwidth value cannot be negative"));
    }

    let result = (num * multiplier as f64).round() as u64;
    if result == 0 && num > 0.0 {
        return Err(anyhow!("Bandwidth value too small"));
    }

    Ok(result)
}

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
    fn test_parse_bandwidth_with_suffixes() {
        // Basic suffixes
        assert_eq!(parse_bandwidth("1K").unwrap(), 1024);
        assert_eq!(parse_bandwidth("1k").unwrap(), 1024);
        assert_eq!(parse_bandwidth("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_bandwidth("1m").unwrap(), 1024 * 1024);
        assert_eq!(parse_bandwidth("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_bandwidth("1g").unwrap(), 1024 * 1024 * 1024);

        // Larger values
        assert_eq!(parse_bandwidth("20M").unwrap(), 20 * 1024 * 1024);
        assert_eq!(parse_bandwidth("512K").unwrap(), 512 * 1024);

        // Raw bytes (no suffix)
        assert_eq!(parse_bandwidth("1000000").unwrap(), 1000000);
        assert_eq!(parse_bandwidth("20000000").unwrap(), 20000000);

        // Decimal values
        assert_eq!(
            parse_bandwidth("1.5M").unwrap(),
            (1.5 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(
            parse_bandwidth("0.5G").unwrap(),
            (0.5 * 1024.0 * 1024.0 * 1024.0) as u64
        );

        // Whitespace handling
        assert_eq!(parse_bandwidth(" 10M ").unwrap(), 10 * 1024 * 1024);
    }

    #[test]
    fn test_parse_bandwidth_errors() {
        assert!(parse_bandwidth("").is_err());
        assert!(parse_bandwidth("abc").is_err());
        assert!(parse_bandwidth("M").is_err());
        assert!(parse_bandwidth("-10M").is_err());
    }

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
