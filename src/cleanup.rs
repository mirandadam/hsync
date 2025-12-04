use crate::pipeline::PipelineConfig;
use crate::utils::Logger;
use anyhow::Result;
use std::fs;
use walkdir::WalkDir;

pub fn run_cleanup(config: &PipelineConfig, logger: &Logger) -> Result<()> {
    println!("Starting cleanup phase...");
    let mut deleted_count = 0;

    for entry in WalkDir::new(&config.dest_dir) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }

        let dest_path = entry.path();
        let relative_path = dest_path.strip_prefix(&config.dest_dir)?;
        let source_path = config.source_dir.join(relative_path);

        // Live check against source
        if !source_path.exists() {
            // Double check it's not a transient error or race condition?
            // Spec says: "Only if the live check confirms absence is the file deleted."
            // Simple exists() check is the live check.

            if let Err(e) = fs::remove_file(dest_path) {
                eprintln!("Failed to delete extra file {:?}: {}", dest_path, e);
                logger.log(&format!("Failed to delete extra: {:?} ({})", dest_path, e))?;
            } else {
                println!("Deleted extra file: {:?}", relative_path);
                logger.log(&format!("Deleted extra: {:?}", dest_path))?;
                deleted_count += 1;
            }
        }
    }

    println!("Cleanup completed. Deleted {} files.", deleted_count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::HashAlgorithm;
    use std::fs::File;
    use std::path::PathBuf;

    #[test]
    fn test_cleanup() -> Result<()> {
        let source_dir = PathBuf::from("test_cleanup_source");
        let dest_dir = PathBuf::from("test_cleanup_dest");
        let log_path = "test_cleanup.log";

        let _ = fs::remove_dir_all(&source_dir);
        let _ = fs::remove_dir_all(&dest_dir);
        let _ = fs::remove_file(log_path);

        fs::create_dir_all(&source_dir)?;
        fs::create_dir_all(&dest_dir)?;

        // Create file in source
        File::create(source_dir.join("keep.txt"))?;
        // Create same file in dest
        File::create(dest_dir.join("keep.txt"))?;
        // Create extra file in dest
        File::create(dest_dir.join("extra.txt"))?;

        let config = PipelineConfig {
            source_dir: source_dir.clone(),
            dest_dir: dest_dir.clone(),
            bw_limit: None,
            db_path: "test_cleanup.db".to_string(),
            log_path: log_path.to_string(),
            hash_algo: HashAlgorithm::Sha256,
            block_size: 5 * 1024 * 1024,
        };
        let logger = Logger::new(log_path);

        run_cleanup(&config, &logger)?;

        assert!(dest_dir.join("keep.txt").exists());
        assert!(!dest_dir.join("extra.txt").exists());

        // Cleanup
        fs::remove_dir_all(source_dir)?;
        fs::remove_dir_all(dest_dir)?;
        fs::remove_file(log_path)?;
        let _ = fs::remove_file("test_cleanup.db");

        Ok(())
    }
}
