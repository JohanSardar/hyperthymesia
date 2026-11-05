use super::*;
use std::fs::{File, OpenOptions, TryLockError};

/// An OS lock is released by closing the file, including on process death.
/// Never unlink lock files: replacing the inode would allow two live holders.
pub(crate) struct FileLock {
    _file: File,
}

impl Store {
    pub(crate) async fn try_file_lock(&self, name: &str) -> Result<Option<FileLock>> {
        let dir = self.data_dir.join("locks");
        tokio::fs::create_dir_all(&dir).await?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(name))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(FileLock { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}
