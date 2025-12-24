use anyhow::Result;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use tracing_subscriber::fmt::MakeWriter;

/// Create a file writer for logging
pub fn create_file_writer(path: &PathBuf) -> Result<impl Write + Send + Sync + 'static> {
    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new().create(true).append(true).open(path)?;

    Ok(std::io::BufWriter::new(file))
}

/// File writer that implements MakeWriter trait for tracing-subscriber
pub struct FileWriter {
    path: PathBuf,
}

impl FileWriter {
    pub fn new(path: PathBuf) -> Self {
        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    parent = ?parent,
                    error = %err,
                    "Failed to create log directory, file operations may fail"
                );
            }
        }
        Self { path }
    }
}

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = Box<dyn Write + Send + Sync + 'a>;

    fn make_writer(&'a self) -> Self::Writer {
        // Attempt to create parent directory before opening
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(file) => Box::new(std::io::BufWriter::new(file)),
            Err(err) => {
                // Fallback to stderr writer on error
                tracing::error!(
                    path = ?self.path,
                    error = %err,
                    "Failed to open log file, falling back to stderr"
                );
                Box::new(std::io::BufWriter::new(std::io::stderr()))
            }
        }
    }
}
