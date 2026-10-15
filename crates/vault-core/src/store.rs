use crate::{
    Error, Id, Result, codec,
    model::MAX_ENTRIES,
    operation::{Operation, Point},
};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub(crate) struct Store {
    pub root: PathBuf,
    _lock: File,
}

pub(crate) struct Inventory {
    pub ids: Vec<Id>,
    pub unexpected: bool,
    pub temporary: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Object,
    Snapshot,
    Commit,
}

impl Kind {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Object => "objects",
            Self::Snapshot => "snapshots",
            Self::Commit => "commits",
        }
    }

    fn points(self) -> [Point; 4] {
        match self {
            Self::Object => [
                Point::ObjectWritten,
                Point::ObjectSynced,
                Point::ObjectPublished,
                Point::ObjectDirectorySynced,
            ],
            Self::Snapshot => [
                Point::SnapshotWritten,
                Point::SnapshotSynced,
                Point::SnapshotPublished,
                Point::SnapshotDirectorySynced,
            ],
            Self::Commit => [
                Point::CommitWritten,
                Point::CommitSynced,
                Point::CommitPublished,
                Point::CommitDirectorySynced,
            ],
        }
    }
}

impl Store {
    pub fn create(root: &Path) -> Result<Self> {
        #[cfg(unix)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(root)?;
        let store = Self::lock(root)?;
        for directory in ["objects", "snapshots", "commits"] {
            fs::create_dir(store.root.join(directory))?;
        }
        sync_directory(&store.root)?;
        if let Some(parent) = store.root.parent() {
            sync_directory(parent)?;
        }
        Ok(store)
    }

    pub fn open(root: &Path) -> Result<Self> {
        validate_directory(root)?;
        for directory in ["objects", "snapshots", "commits"] {
            validate_directory(&root.join(directory))?;
        }
        // Do not create a lock in an arbitrary non-vault directory.
        let _ = codec::open_regular(&root.join("keys.json"))?;
        Self::lock(root)
    }

    fn lock(root: &Path) -> Result<Self> {
        validate_directory(root)?;
        let root = root.canonicalize()?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000);
        }
        let lock = options.open(root.join("write.lock"))?;
        if !lock.metadata()?.is_file() {
            return Err(Error::InvalidInput);
        }
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => Error::Locked,
            std::fs::TryLockError::Error(error) => error.into(),
        })?;
        Ok(Self { root, _lock: lock })
    }

    pub fn path(&self, kind: Kind, id: Id) -> PathBuf {
        self.root.join(kind.directory()).join(id.to_string())
    }

    pub fn temporary(&self, kind: Kind) -> Result<NamedTempFile> {
        let inventory = self.inventory(kind)?;
        if inventory.unexpected {
            return Err(Error::ReadOnly);
        }
        if inventory.ids.len() + inventory.temporary >= MAX_ENTRIES {
            return Err(Error::ResourceLimit);
        }
        Ok(tempfile::Builder::new()
            .prefix(".tmp-")
            .tempfile_in(self.root.join(kind.directory()))?)
    }

    pub fn read(&self, kind: Kind, id: Id, max: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        codec::read_limited(&self.path(kind, id), max)
    }

    pub fn publish_bytes(
        &self,
        kind: Kind,
        id: Id,
        bytes: &[u8],
        operation: &Operation,
    ) -> Result<()> {
        let mut temporary = self.temporary(kind)?;
        operation.writer(&mut temporary).write_all(bytes)?;
        self.publish(kind, id, temporary, operation)
    }

    pub fn publish(
        &self,
        kind: Kind,
        id: Id,
        temporary: NamedTempFile,
        operation: &Operation,
    ) -> Result<()> {
        let [written, synced, published, directory_synced] = kind.points();
        operation.checkpoint(written)?;
        temporary.as_file().sync_all()?;
        operation.checkpoint(synced)?;
        operation.check()?;
        let result = temporary
            .persist_noclobber(self.path(kind, id))
            .map_err(|error| Error::from(error.error));
        let after_publication = || -> Result<()> {
            result?;
            operation.checkpoint(published)?;
            sync_directory(&self.root.join(kind.directory()))?;
            operation.checkpoint(directory_synced)
        };
        after_publication().map_err(|error| {
            if matches!(kind, Kind::Commit) {
                Error::CommitUncertain
            } else {
                error
            }
        })
    }

    pub fn inventory(&self, kind: Kind) -> Result<Inventory> {
        let mut result = Inventory {
            ids: Vec::new(),
            unexpected: false,
            temporary: 0,
        };
        for (count, entry) in fs::read_dir(self.root.join(kind.directory()))?.enumerate() {
            if count >= MAX_ENTRIES {
                return Err(Error::ResourceLimit);
            }
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                result.unexpected = true;
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                result.unexpected = true;
                continue;
            };
            if name.starts_with(".tmp-") {
                result.temporary += 1;
                continue;
            }
            match name.parse() {
                Ok(id) => result.ids.push(id),
                Err(_) => result.unexpected = true,
            }
        }
        result.ids.sort_unstable();
        Ok(result)
    }

    pub fn unexpected_root_entry(&self) -> Result<bool> {
        for (count, entry) in fs::read_dir(&self.root)?.enumerate() {
            if count >= MAX_ENTRIES {
                return Err(Error::ResourceLimit);
            }
            let entry = entry?;
            let name = entry.file_name();
            let kind = entry.file_type()?;
            let expected = match name.to_str() {
                Some("keys.json" | "write.lock") => kind.is_file(),
                Some("objects" | "snapshots" | "commits") => kind.is_dir(),
                _ => false,
            };
            if !expected {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn validate_directory(path: &Path) -> Result<()> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path; // Windows directory durability is explicitly unverified in the platform record.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_lock_is_held_until_session_drop_and_publication_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault");
        let store = Store::create(&path).unwrap();
        fs::write(path.join("keys.json"), b"fixture").unwrap();
        assert!(matches!(Store::open(&path), Err(Error::Locked)));
        let id = Id([1; 16]);
        store
            .publish_bytes(Kind::Object, id, b"first", &Operation::default())
            .unwrap();
        assert!(
            store
                .publish_bytes(Kind::Object, id, b"second", &Operation::default())
                .is_err()
        );
        assert_eq!(&**store.read(Kind::Object, id, 20).unwrap(), b"first");
        drop(store);
        assert!(Store::open(&path).is_ok());
    }

    #[test]
    fn commit_publication_failures_have_explicit_uncertainty() {
        for point in [
            Point::CommitWritten,
            Point::CommitSynced,
            Point::CommitPublished,
            Point::CommitDirectorySynced,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::create(&directory.path().join("vault")).unwrap();
            let operation = Operation::default().failing_at(point);
            let result = store.publish_bytes(Kind::Commit, Id([1; 16]), b"record", &operation);
            if matches!(point, Point::CommitWritten | Point::CommitSynced) {
                assert!(matches!(result, Err(Error::Io(_))));
            } else {
                assert!(matches!(result, Err(Error::CommitUncertain)));
            }
        }
    }
}
