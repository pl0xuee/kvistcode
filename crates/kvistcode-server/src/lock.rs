//! One server per data folder. The lock is an OS file lock held for the life of the process,
//! so a crash or `kill -9` never leaves a stale one behind.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;

pub struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    /// Takes the lock, or says plainly that another server has it.
    pub fn take(data_dir: &Path) -> Result<DataDirLock, String> {
        let path = data_dir.join("server.lock");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(format!(
                    "another kvistcode-server is already using {}. Stop it first, or start this \
                     one with a different --data-dir.",
                    data_dir.display()
                ))
            }
            Err(TryLockError::Error(e)) => {
                return Err(format!("couldn't lock {}: {e}", path.display()))
            }
        }
        // Only for people looking at the file.
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        Ok(DataDirLock { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_is_refused_until_the_first_is_dropped() {
        let dir = std::env::temp_dir().join(format!("kv-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = DataDirLock::take(&dir).unwrap();
        let err = DataDirLock::take(&dir).err().expect("refused");
        assert!(err.contains("already using"), "{err}");
        drop(first);
        assert!(DataDirLock::take(&dir).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
