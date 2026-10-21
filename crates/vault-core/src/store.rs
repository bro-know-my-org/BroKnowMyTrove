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
        // Strip trailing separators and `/.` before inspecting the final component:
        // otherwise symlink_metadata follows a directory symlink on Unix.
        let root: PathBuf = root.components().collect();
        match fs::symlink_metadata(&root) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(Error::InvalidInput);
                }
                ensure_empty_directory(&root)?;
                #[cfg(unix)]
                restrict_existing_directory(&root, || ensure_empty_directory(&root))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                #[cfg(unix)]
                let mut builder = fs::DirBuilder::new();
                #[cfg(not(unix))]
                let builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&root)?;
            }
            Err(error) => return Err(error.into()),
        }
        Self::initialize(&root)
    }

    pub(crate) fn initialize(root: &Path) -> Result<Self> {
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
        self.inventory_with_operation(kind, &Operation::default())
    }

    pub fn inventory_with_operation(&self, kind: Kind, operation: &Operation) -> Result<Inventory> {
        operation.check()?;
        let mut result = Inventory {
            ids: Vec::new(),
            unexpected: false,
            temporary: 0,
        };
        for (count, entry) in fs::read_dir(self.root.join(kind.directory()))?.enumerate() {
            operation.checkpoint(Point::InventoryEntry)?;
            operation.check()?;
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
        operation.check()?;
        Ok(result)
    }

    pub fn root_state(&self, operation: &Operation) -> Result<(bool, usize)> {
        let mut temporary = 0;
        let mut unexpected = false;
        for (count, entry) in fs::read_dir(&self.root)?.enumerate() {
            operation.check()?;
            if count >= MAX_ENTRIES {
                return Err(Error::ResourceLimit);
            }
            let entry = entry?;
            let name = entry.file_name();
            let kind = entry.file_type()?;
            let expected = match name.to_str() {
                Some("keys.json" | "write.lock") => kind.is_file(),
                Some("objects" | "snapshots" | "commits") => kind.is_dir(),
                Some(name) if name.starts_with(".tmp-key-") && kind.is_file() => {
                    temporary += 1;
                    true
                }
                _ => false,
            };
            if !expected {
                unexpected = true;
            }
        }
        Ok((unexpected, temporary))
    }
}

fn ensure_empty_directory(path: &Path) -> Result<()> {
    if fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err(Error::Conflict);
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_existing_directory(
    root: &Path,
    check_empty: impl FnOnce() -> Result<()>,
) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    let metadata = directory.metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(Error::InvalidInput);
    }
    // Match newly created roots without following a replacement symlink at chmod.
    directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    // A file could have arrived before chmod. Restore the same directory's original
    // permissions if that check rejects creation, including on directory read errors.
    if let Err(error) = check_empty() {
        directory.set_permissions(metadata.permissions())?;
        return Err(error);
    }
    Ok(())
}

fn validate_directory(path: &Path) -> Result<()> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    Ok(vault_platform::sync_directory(path)?)
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
    fn create_uses_an_empty_existing_directory_and_rejects_others() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault");
        fs::create_dir(&path).unwrap();
        let store = Store::create(&path).unwrap();
        assert!(path.join("objects").is_dir());
        drop(store);

        let occupied = directory.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("file"), b"x").unwrap();
        assert!(matches!(Store::create(&occupied), Err(Error::Conflict)));

        let file = directory.path().join("file");
        fs::write(&file, b"x").unwrap();
        assert!(matches!(Store::create(&file), Err(Error::InvalidInput)));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = directory.path().join("target");
            fs::create_dir(&target).unwrap();
            let link = directory.path().join("link");
            symlink(&target, &link).unwrap();
            for path in [link.clone(), link.join(""), link.join(".")] {
                assert!(matches!(Store::create(&path), Err(Error::InvalidInput)));
                assert!(fs::read_dir(&target).unwrap().next().is_none());
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn create_restricts_existing_directory_permissions_without_changing_occupied_ones() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        for mode in [0o777, 0o755] {
            let path = directory.path().join(format!("vault-{mode:o}"));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            let _store = Store::create(&path).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }

        let occupied = directory.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("file"), b"x").unwrap();
        fs::set_permissions(&occupied, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(Store::create(&occupied), Err(Error::Conflict)));
        assert_eq!(
            fs::metadata(&occupied).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    #[cfg(unix)]
    fn rejected_directory_recheck_restores_original_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_empty_directory(path).unwrap();
        let result = restrict_existing_directory(path, || {
            // Deterministically model a file arriving between the two empty checks.
            fs::write(path.join("arrived"), b"preserve")?;
            ensure_empty_directory(path)
        });
        assert!(matches!(result, Err(Error::Conflict)));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(fs::read(path.join("arrived")).unwrap(), b"preserve");

        let result = restrict_existing_directory(path, || {
            Err(Error::Io(std::io::ErrorKind::PermissionDenied))
        });
        assert!(matches!(
            result,
            Err(Error::Io(std::io::ErrorKind::PermissionDenied))
        ));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o755
        );
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
