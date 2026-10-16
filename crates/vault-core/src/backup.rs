use crate::{
    Credential, EntryKind, Error, Id, OpenMode, Result, Vault, codec,
    files::{ensure_capacity, same_metadata},
    ledger,
    model::object_size,
    operation::{Operation, Point},
    store::{Kind, Store, sync_directory},
    stream,
    vault::read_snapshot,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, Write},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupVerification {
    /// Read back and compare ciphertext; authenticate metadata, not content streams.
    Copy,
    /// Additionally authenticate every referenced content stream, including trash.
    Full,
}

#[derive(Debug)]
pub struct BackupReport {
    pub commit: Id,
    pub copied_files: usize,
    pub ciphertext_bytes: u64,
    pub verification: BackupVerification,
    pub verified_objects: u64,
    /// The complete copy was published, but final directory durability is uncertain.
    /// Keep it and inspect it; do not report this as an unqualified success.
    pub publication_failure: Option<Error>,
}

impl Vault {
    /// Copies the current state to a new directory while holding the source lock.
    /// Keeps all commit records, only the current snapshot and its objects (including
    /// trash). Historical recovery snapshots and unreferenced objects are not backed up.
    /// Before keys.json is published, failure/cancellation attempts to remove the destination.
    /// A killed process can leave an incomplete directory; never reuse/overwrite it.
    pub fn backup_to(
        &self,
        destination: &Path,
        verification: BackupVerification,
        operation: &Operation,
    ) -> Result<BackupReport> {
        operation.check()?;
        let graph =
            ledger::scan_with_operation(&self.store, &self.master, self.vault_id, operation)?;
        let current = self.current.ok_or(Error::ReadOnly)?;
        if graph.report.head != Some(current)
            || !graph.report.issues.is_empty()
            || !self.issues.is_empty()
        {
            return Err(Error::ReadOnly);
        }
        let snapshot_id = graph.records[&current].snapshot;
        let snapshot = read_snapshot(&self.store, &self.master, self.vault_id, snapshot_id)?;
        operation.check()?;
        let mut objects = BTreeMap::new();
        for entry in &snapshot.entries {
            operation.check()?;
            if let EntryKind::File { object, length, .. } = entry.kind
                && objects
                    .insert(object, length)
                    .is_some_and(|previous| previous != length)
            {
                return Err(Error::InvalidFormat);
            }
        }
        let mut files = Vec::new();
        for (&id, &length) in &objects {
            operation.check()?;
            files.push((Kind::Object, id, object_size(length)?));
        }
        for (kind, id, limit) in
            std::iter::once((Kind::Snapshot, snapshot_id, codec::SNAPSHOT_LIMIT)).chain(
                graph
                    .records
                    .keys()
                    .map(|&id| (Kind::Commit, id, codec::COMMIT_LIMIT)),
            )
        {
            operation.check()?;
            let size = codec::open_regular(&self.store.path(kind, id))?
                .metadata()?
                .len();
            if size > limit as u64 {
                return Err(Error::ResourceLimit);
            }
            files.push((kind, id, size));
        }
        let total = files
            .iter()
            .try_fold(self.key_bytes.len() as u64, |sum, (_, _, size)| {
                sum.checked_add(*size).ok_or(Error::ResourceLimit)
            })?;
        let name = destination.file_name().ok_or(Error::InvalidInput)?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        if parent.starts_with(&self.store.root) {
            return Err(Error::InvalidInput);
        }
        ensure_capacity(
            &parent,
            total.checked_add(1024 * 1024).ok_or(Error::ResourceLimit)?,
            files
                .iter()
                .map(|(_, _, size)| *size)
                .max()
                .unwrap_or(0)
                .max(self.key_bytes.len() as u64),
        )?;
        // Zero random bytes reserves precisely the requested name with exclusive mkdir.
        // The guard removes only the directory it created, never an existing target.
        let owned = tempfile::Builder::new()
            .prefix(name)
            .rand_bytes(0)
            .tempdir_in(&parent)?;
        let target = Store::initialize(owned.path())?;
        let mut report = BackupReport {
            commit: current,
            copied_files: files.len() + 1,
            ciphertext_bytes: 0,
            verification,
            verified_objects: 0,
            publication_failure: None,
        };
        for (kind, id, size) in files {
            copy_verified(&self.store, &target, kind, id, size, operation)?;
            report.ciphertext_bytes += size;
        }
        let copied = ledger::scan_with_operation(&target, &self.master, self.vault_id, operation)?;
        if copied.report.head != Some(current) || !copied.report.issues.is_empty() {
            return Err(Error::InvalidFormat);
        }
        read_snapshot(&target, &self.master, self.vault_id, snapshot_id)?;
        operation.check()?;
        if verification == BackupVerification::Full {
            for (&object, &length) in &objects {
                operation.check()?;
                stream::decrypt(
                    &self.master,
                    self.vault_id,
                    object,
                    length,
                    &mut codec::open_regular(&target.path(Kind::Object, object))?,
                    &mut std::io::sink(),
                    operation,
                )?;
                report.verified_objects += 1;
            }
        }
        let latest =
            ledger::scan_with_operation(&self.store, &self.master, self.vault_id, operation)?;
        if latest.report.head != Some(current)
            || !latest.report.issues.is_empty()
            || *codec::read_limited(&self.store.root.join("keys.json"), codec::KEY_LIMIT)?
                != *self.key_bytes
        {
            return Err(Error::SourceChanged);
        }
        let mut keys = tempfile::Builder::new()
            .prefix(".tmp-key-")
            .tempfile_in(&target.root)?;
        operation.writer(&mut keys).write_all(&self.key_bytes)?;
        keys.as_file().sync_all()?;
        keys.as_file_mut().rewind()?;
        let mut copied_keys = Vec::new();
        keys.read_to_end(&mut copied_keys)?;
        if copied_keys != *self.key_bytes {
            return Err(Error::InvalidFormat);
        }
        operation.checkpoint(Point::BackupPrepared)?;
        operation.check()?;
        keys.persist_noclobber(target.root.join("keys.json"))
            .map_err(|error| Error::from(error.error))?;
        let _ = owned.keep();
        // Publication is the cancellation boundary. Finish syncing even if cancelled.
        report.publication_failure =
            finish_publication(&target.root, &parent, operation, sync_directory);
        report.ciphertext_bytes = total;
        Ok(report)
    }

    /// Restores an independently openable backup into a new directory. Full mode is
    /// recommended for recovery; this never overlays or modifies an existing vault.
    pub fn restore_to(
        source: &Path,
        credential: Credential<'_>,
        destination: &Path,
        verification: BackupVerification,
        operation: &Operation,
    ) -> Result<BackupReport> {
        operation.check()?;
        Self::open_with_operation(source, credential, OpenMode::ReadOnly, operation)?.backup_to(
            destination,
            verification,
            operation,
        )
    }
}

fn finish_publication(
    target: &Path,
    parent: &Path,
    operation: &Operation,
    mut sync: impl FnMut(&Path) -> Result<()>,
) -> Option<Error> {
    let published = operation.checkpoint(Point::BackupPublished);
    let target_synced = sync(target);
    let parent_synced = sync(parent);
    let finished = operation.checkpoint(Point::BackupDirectorySynced);
    published
        .and(target_synced)
        .and(parent_synced)
        .and(finished)
        .err()
}

fn copy_verified(
    source: &Store,
    target: &Store,
    kind: Kind,
    id: Id,
    size: u64,
    operation: &Operation,
) -> Result<()> {
    let path = source.path(kind, id);
    let mut input = codec::open_regular(&path)?;
    let before = input.metadata()?;
    if before.len() != size {
        return Err(Error::SourceChanged);
    }
    // The destination is new and the plan is bounded; avoid rescanning its growing
    // inventory for each file during a bulk copy.
    let mut output = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempfile_in(target.root.join(kind.directory()))?;
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = size;
    let mut writer = operation.writer(&mut output);
    while remaining > 0 {
        operation.check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..count])?;
        writer.write_all(&buffer[..count])?;
        remaining -= count as u64;
        operation.progress(size - remaining, size);
    }
    output.as_file().sync_all()?;
    input.rewind()?;
    output.as_file_mut().rewind()?;
    let mut compared = [0u8; 64 * 1024];
    remaining = size;
    while remaining > 0 {
        operation.check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..count])?;
        output.read_exact(&mut compared[..count])?;
        if buffer[..count] != compared[..count] {
            return Err(Error::SourceChanged);
        }
        remaining -= count as u64;
    }
    if !same_metadata(&before, &input.metadata()?)
        || !same_metadata(&before, &path.symlink_metadata()?)
        || same_file::Handle::from_file(input.try_clone()?)? != same_file::Handle::from_path(&path)?
    {
        return Err(Error::SourceChanged);
    }
    target.publish(kind, id, output, operation)
}

#[cfg(test)]
mod tests;
