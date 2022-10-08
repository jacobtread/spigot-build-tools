use log::info;
use std::io;
use std::path::Path;
use tokio::fs::{create_dir_all, remove_dir_all, remove_file};

/// Safely creates a directory ensuring that if a non directory
/// exists at the path its removed and the directory is created
pub async fn create_directory(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if path.exists() {
        if !path.is_dir() {
            remove_file(path).await?;
            create_dir_all(path).await?;
        }
    } else {
        create_dir_all(path).await?;
    }
    Ok(())
}

/// Removes any existing files or directories at the provided
/// path asynchronously
pub async fn remove_existing(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if path.exists() {
        if path.is_dir() {
            remove_dir_all(path).await?;
        } else if path.is_file() {
            remove_file(path).await?;
        }
    }
    Ok(())
}
