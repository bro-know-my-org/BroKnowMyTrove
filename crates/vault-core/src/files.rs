use crate::{
    Entry, EntryKind, Error, Id, Result, Vault, codec,
    model::{self, object_size, validate_name},
    operation::{Operation, Point},
    store::{Kind, sync_directory},
    stream,
};
use std::{
    fs::{self, Metadata},
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

impl Vault {
    /// Copies a regular file into a new immutable object. The source is never removed.
    pub fn import_file(
        &mut self,
        parent: Option<Id>,
        name: &str,
        source: &Path,
        operation: &Operation,
    ) -> Result<Id> {
        self.write_file(None, parent, name, source, operation)
    }

    pub fn replace_file(&mut self, id: Id, source: &Path, operation: &Operation) -> Result<()> {
        let entry = self
            .entry(id)
            .filter(|entry| !entry.trashed)
            .ok_or(Error::InvalidInput)?
            .clone();
        if !matches!(entry.kind, EntryKind::File { .. }) {
            return Err(Error::InvalidInput);
        }
        self.write_file(Some(id), entry.parent, &entry.name, source, operation)?;
        Ok(())
    }

    fn write_file(
        &mut self,
        replace: Option<Id>,
        parent: Option<Id>,
        name: &str,
        source: &Path,
        operation: &Operation,
    ) -> Result<Id> {
        self.check_writable()?;
        operation.check()?;
        validate_name(name)?;
        if let Some(parent) = parent
            && !self
                .entry(parent)
                .is_some_and(|entry| !entry.trashed && matches!(entry.kind, EntryKind::Directory))
        {
            return Err(Error::InvalidInput);
        }
        let before = source.symlink_metadata()?;
        if !before.is_file() {
            return Err(Error::InvalidInput);
        }
        let mut input = codec::open_regular(source)?;
        let opened = input.metadata()?;
        if !same_metadata(&before, &opened) {
            return Err(Error::SourceChanged);
        }
        let length = opened.len();
        let object = Id::random()?;
        let id = match replace {
            Some(id) => id,
            None => Id::random()?,
        };
        let mut snapshot = self.snapshot.clone();
        let kind = EntryKind::File {
            object,
            length,
            modified_ns: opened
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| duration.as_nanos().try_into().ok()),
        };
        if let Some(entry) = snapshot.entries.iter_mut().find(|entry| entry.id == id) {
            entry.kind = kind;
        } else {
            snapshot.entries.push(Entry {
                id,
                parent,
                name: name.into(),
                trashed: false,
                kind,
            });
        }
        snapshot.validate()?;
        let metadata_bytes = codec::encode(&snapshot, codec::SNAPSHOT_LIMIT - 40)?.len() as u64;
        let required = object_size(length)?
            .checked_add(metadata_bytes)
            .and_then(|size| size.checked_add(1024 * 1024))
            .ok_or(Error::ResourceLimit)?;
        ensure_capacity(&self.store.root, required, object_size(length)?)?;
        let mut temporary = self.store.temporary(Kind::Object)?;
        stream::encrypt(
            &self.master,
            self.vault_id,
            object,
            length,
            &mut input,
            &mut temporary,
            operation,
        )?;
        let after = source.symlink_metadata()?;
        if !after.is_file()
            || !same_metadata(&opened, &input.metadata()?)
            || !same_metadata(&opened, &after)
            || same_file::Handle::from_file(input.try_clone()?)?
                != same_file::Handle::from_path(source)?
        {
            return Err(Error::SourceChanged);
        }
        operation.check()?;
        self.store
            .publish(Kind::Object, object, temporary, operation)?;
        self.commit(snapshot, operation)?;
        Ok(id)
    }

    /// Publishes only after the entire stream authenticates, with no silent overwrite.
    pub fn export_file(&self, id: Id, destination: &Path, operation: &Operation) -> Result<()> {
        let entry = self.entry(id).ok_or(Error::InvalidInput)?;
        let EntryKind::File {
            object,
            length,
            modified_ns,
        } = entry.kind
        else {
            return Err(Error::InvalidInput);
        };
        operation.check()?;
        let destination = export_destination(destination)?;
        let parent = destination.parent().ok_or(Error::InvalidInput)?;
        ensure_capacity(
            parent,
            length.checked_add(4096).ok_or(Error::ResourceLimit)?,
            length,
        )?;
        let mut input = codec::open_regular(&self.store.path(Kind::Object, object))?;
        if input.metadata()?.len() != object_size(length)? {
            return Err(Error::InvalidFormat);
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".bkm-export-")
            .tempfile_in(parent)?;
        stream::decrypt(
            &self.master,
            self.vault_id,
            object,
            length,
            &mut input,
            &mut temporary,
            operation,
        )?;
        if let Some(timestamp) = modified_ns {
            temporary
                .as_file()
                .set_modified(UNIX_EPOCH + std::time::Duration::from_nanos(timestamp))?;
        }
        temporary.as_file().sync_all()?;
        operation.checkpoint(Point::ExportSynced)?;
        operation.check()?;
        export_destination(&destination)?;
        temporary
            .persist_noclobber(&destination)
            .map_err(|error| Error::from(error.error))?;
        operation
            .checkpoint(Point::ExportPublished)
            .and_then(|_| sync_directory(parent))
            .map_err(|_| Error::ExportUncertain)
    }

    pub fn verify_content(&self, operation: &Operation) -> Result<ContentReport> {
        let mut report = ContentReport::default();
        let mut seen = std::collections::HashSet::new();
        for entry in self.entries() {
            if let EntryKind::File { object, length, .. } = entry.kind {
                if !seen.insert(object) {
                    continue;
                }
                operation.check()?;
                let result = codec::open_regular(&self.store.path(Kind::Object, object)).and_then(
                    |mut input| {
                        stream::decrypt(
                            &self.master,
                            self.vault_id,
                            object,
                            length,
                            &mut input,
                            &mut io::sink(),
                            operation,
                        )
                    },
                );
                match result {
                    Ok(()) => {
                        report.verified_objects += 1;
                        report.plaintext_bytes = report
                            .plaintext_bytes
                            .checked_add(length)
                            .ok_or(Error::ResourceLimit)?;
                    }
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(_) => report.failed_objects.push(object),
                }
            }
        }
        Ok(report)
    }
}

#[derive(Default, Debug)]
pub struct ContentReport {
    pub verified_objects: u64,
    pub plaintext_bytes: u64,
    pub failed_objects: Vec<Id>,
}

fn ensure_capacity(path: &Path, required: u64, largest_file: u64) -> Result<()> {
    let capacity = vault_platform::capacity(path)?;
    if capacity.available < required || capacity.max_file_size.is_some_and(|max| largest_file > max)
    {
        return Err(Error::InsufficientSpace);
    }
    Ok(())
}

fn export_destination(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::InvalidInput)?;
    validate_name(name)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let destination = parent.join(name);
    let text = destination.to_str().ok_or(Error::InvalidInput)?;
    if text.len() > 4096 || text.encode_utf16().count() > 240 {
        return Err(Error::ResourceLimit);
    }
    let key = model::name_key(name);
    for entry in fs::read_dir(&parent)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| model::name_key(name) == key)
        {
            return Err(Error::Conflict);
        }
    }
    Ok(destination)
}

fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    let common = a.len() == b.len() && a.modified().ok() == b.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        common
            && a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        common
    }
}

#[cfg(test)]
mod tests;
