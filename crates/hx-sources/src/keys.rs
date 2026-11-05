//! Source keys live only in `<state>/secrets/<id>.key`.
//!
//! Keep this directory outside working folders and exclude it from backups.
//! Missing keys after restore are represented by `Ok(None)`.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("source keys must contain at least 4 bytes")]
    KeyTooShort,
    #[error("invalid source key id")]
    InvalidId,
    #[error("source key storage must use regular files in a real directory")]
    UnsafePath,
    #[error("could not access source key ({0:?})")]
    Io(io::ErrorKind),
}

impl From<io::Error> for KeyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

pub struct KeyStore {
    directory: PathBuf,
}

impl KeyStore {
    pub fn new(state: impl AsRef<Path>) -> Result<Self, KeyError> {
        let directory = state.as_ref().join("secrets");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
        private_directory(&directory)?;
        Ok(Self { directory })
    }

    /// Atomically replace a key, creating the temporary file privately too.
    pub fn write(&self, id: &str, key: &str) -> Result<(), KeyError> {
        let path = self.path(id)?;
        crate::scrub::validate_key(key).map_err(|_| KeyError::KeyTooShort)?;
        private_directory(&self.directory)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => return Err(KeyError::UnsafePath),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(key.as_bytes())?;
        file.as_file().sync_all()?;
        // Persist errors retain the temporary file, so discard their Debug and
        // Display representations along with all paths and caller input.
        file.persist(&path)
            .map_err(|error| KeyError::from(error.error))?;
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    pub fn read(&self, id: &str) -> Result<Option<String>, KeyError> {
        let path = self.path(id)?;
        private_directory(&self.directory)?;
        let mut file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !file.metadata()?.is_file() {
            return Err(KeyError::UnsafePath);
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let mut key = String::new();
        file.read_to_string(&mut key)?;
        crate::scrub::validate_key(&key).map_err(|_| KeyError::KeyTooShort)?;
        Ok(Some(key))
    }

    fn path(&self, id: &str) -> Result<PathBuf, KeyError> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(KeyError::InvalidId);
        }
        Ok(self.directory.join(format!("{id}.key")))
    }
}

fn private_directory(path: &Path) -> Result<(), KeyError> {
    // O_NOFOLLOW prevents reading or chmod'ing a symlink target. Descriptor
    // permissions also avoid following a symlink swapped in after open.
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn round_trip_and_rotation_keep_private_file_modes() {
        let state = tempfile::tempdir().unwrap();
        let keys = KeyStore::new(state.path()).unwrap();
        assert!(keys.read("fake-source").unwrap().is_none());
        for key in ["invented-first-key", "next-key"] {
            keys.write("fake-source", key).unwrap();
            assert_eq!(keys.read("fake-source").unwrap().as_deref(), Some(key));
            assert_eq!(
                fs::metadata(state.path().join("secrets"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(state.path().join("secrets/fake-source.key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn unsafe_ids_and_short_keys_are_refused_without_echoing_inputs() {
        let state = tempfile::tempdir().unwrap();
        let keys = KeyStore::new(state.path()).unwrap();
        for id in [
            "",
            "../fake-escape",
            "/fake-absolute",
            "fake/slash",
            "fake\\slash",
        ] {
            assert!(matches!(
                keys.write(id, "invented-key"),
                Err(KeyError::InvalidId)
            ));
            assert!(matches!(keys.read(id), Err(KeyError::InvalidId)));
        }
        for key in ["", "a", "abc", "☃"] {
            assert!(matches!(
                keys.write("fake-source", key),
                Err(KeyError::KeyTooShort)
            ));
        }
        assert!(keys.read("fake-source").unwrap().is_none());
        keys.write("fake-source", "éé").unwrap();
        assert_eq!(keys.read("fake-source").unwrap().as_deref(), Some("éé"));
        // Even an I/O error supplied by a caller cannot leak its message.
        let error = KeyError::from(io::Error::other("invented-secret-error"));
        assert!(!format!("{error} {error:?}").contains("invented-secret-error"));
    }

    #[test]
    fn existing_permissions_are_restricted_before_key_access() {
        let state = tempfile::tempdir().unwrap();
        let directory = state.path().join("secrets");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        let keys = KeyStore::new(state.path()).unwrap();
        keys.write("fake-source", "invented-key").unwrap();
        let path = directory.join("fake-source.key");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        keys.read("fake-source").unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn symlinks_are_not_read_or_written_through() {
        let state = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), state.path().join("secrets")).unwrap();
        assert!(KeyStore::new(state.path()).is_err());
        fs::remove_file(state.path().join("secrets")).unwrap();
        let keys = KeyStore::new(state.path()).unwrap();
        let target = outside.path().join("target.key");
        fs::write(&target, "invented-outside-key").unwrap();
        let path = state.path().join("secrets/fake-source.key");
        symlink(&target, &path).unwrap();
        assert!(keys.read("fake-source").is_err());
        assert!(keys.write("fake-source", "invented-replacement").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "invented-outside-key");
    }
}
