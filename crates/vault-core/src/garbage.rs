use crate::{
    EntryKind, Error, Result, Vault, ledger,
    operation::{Operation, Point},
    store::{Kind, sync_directory},
};
use std::{collections::HashSet, fs};

#[derive(Default, Debug)]
pub struct StorageUsage {
    pub active_object_bytes: u64,
    pub trash_object_bytes: u64,
    /// Requires retiring all historical local recovery snapshots.
    pub reclaimable_object_bytes: u64,
}

#[derive(Default, Debug)]
pub struct CleanupReport {
    pub trash_committed: bool,
    pub retired_snapshots: usize,
    pub reclaimed_object_bytes: u64,
    pub removed_temporary_bytes: u64,
    /// Some namespace removals may have happened even when their final sync failed.
    pub failure: Option<Error>,
}

impl Vault {
    pub fn storage_usage(&self) -> Result<StorageUsage> {
        let referenced = self.snapshot.referenced_objects();
        let active: HashSet<_> = self
            .entries()
            .filter(|entry| !entry.is_trashed())
            .filter_map(|entry| match entry.kind {
                EntryKind::File { object, .. } => Some(object),
                EntryKind::Directory => None,
            })
            .collect();
        let inventory = self.store.inventory(Kind::Object)?;
        if inventory.unexpected {
            return Err(Error::ReadOnly);
        }
        let mut usage = StorageUsage::default();
        for id in inventory.ids {
            let size = self.store.path(Kind::Object, id).symlink_metadata()?.len();
            let total = if active.contains(&id) {
                &mut usage.active_object_bytes
            } else if referenced.contains(&id) {
                &mut usage.trash_object_bytes
            } else {
                &mut usage.reclaimable_object_bytes
            };
            *total = total.checked_add(size).ok_or(Error::ResourceLimit)?;
        }
        Ok(usage)
    }

    /// First commits removal of trash metadata, then retires local recovery snapshots.
    /// A successful commit followed by failed cleanup is returned in the report.
    pub fn empty_trash(&mut self, operation: &Operation) -> Result<CleanupReport> {
        let mut next = self.snapshot.clone();
        next.entries.retain(|entry| !entry.is_trashed());
        self.commit(next, operation)?;
        let mut report = CleanupReport {
            trash_committed: true,
            ..CleanupReport::default()
        };
        report.failure = self.reclaim(operation, &mut report).err();
        Ok(report)
    }

    /// Explicitly discards all historical local recovery snapshots, never external backups.
    pub fn collect_garbage(&mut self, operation: &Operation) -> Result<CleanupReport> {
        self.check_writable()?;
        operation.check()?;
        let mut report = CleanupReport::default();
        report.failure = self.reclaim(operation, &mut report).err();
        Ok(report)
    }

    fn reclaim(&mut self, operation: &Operation, report: &mut CleanupReport) -> Result<()> {
        self.check_writable()?;
        operation.check()?;
        let graph = ledger::scan(&self.store, &self.master, self.vault_id)?;
        let head = self.current.ok_or(Error::ReadOnly)?;
        let current_snapshot = graph.records.get(&head).ok_or(Error::ReadOnly)?.snapshot;
        // Build the deletion set before retiring anything. Unknown entries stop cleanup.
        let snapshots = self.store.inventory(Kind::Snapshot)?;
        let objects = self.store.inventory(Kind::Object)?;
        if snapshots.unexpected || objects.unexpected {
            return Err(Error::ReadOnly);
        }
        let referenced = self.snapshot.referenced_objects();
        for id in snapshots
            .ids
            .into_iter()
            .filter(|id| *id != current_snapshot)
        {
            operation.check()?;
            fs::remove_file(self.store.path(Kind::Snapshot, id))?;
            report.retired_snapshots += 1;
        }
        operation.checkpoint(Point::RecoverySnapshotsRetired)?;
        sync_directory(&self.store.root.join(Kind::Snapshot.directory()))?;
        operation.checkpoint(Point::BeforeObjectReclaim)?;
        for id in objects
            .ids
            .into_iter()
            .filter(|id| !referenced.contains(id))
        {
            operation.check()?;
            let path = self.store.path(Kind::Object, id);
            let metadata = path.symlink_metadata()?;
            if !metadata.is_file() {
                return Err(Error::ReadOnly);
            }
            let total = report
                .reclaimed_object_bytes
                .checked_add(metadata.len())
                .ok_or(Error::ResourceLimit)?;
            fs::remove_file(path)?;
            report.reclaimed_object_bytes = total;
            operation.checkpoint(Point::ObjectReclaimed)?;
        }
        sync_directory(&self.store.root.join(Kind::Object.directory()))?;
        // Stale temporary files never participate in references. The lock excludes local tasks.
        for kind in [Kind::Object, Kind::Snapshot, Kind::Commit] {
            let directory = self.store.root.join(kind.directory());
            for entry in fs::read_dir(&directory)? {
                operation.check()?;
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(".tmp-"))
                {
                    if !entry.file_type()?.is_file() {
                        return Err(Error::ReadOnly);
                    }
                    let total = report
                        .removed_temporary_bytes
                        .checked_add(entry.metadata()?.len())
                        .ok_or(Error::ResourceLimit)?;
                    fs::remove_file(entry.path())?;
                    report.removed_temporary_bytes = total;
                }
            }
            sync_directory(&directory)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
