use crate::{Error, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy)]
pub struct Progress {
    pub processed: u64,
    pub total: u64,
}

/// Clone the handle to cancel from another thread. Callbacks must not log sensitive data.
#[derive(Clone, Default)]
pub struct Operation {
    cancelled: Arc<AtomicBool>,
    observer: Option<Arc<dyn Fn(Progress) + Send + Sync>>,
    #[cfg(test)]
    pub(crate) fault: Option<Point>,
    #[cfg(test)]
    write_limit: Option<u64>,
    #[cfg(test)]
    cancel_at: Option<Point>,
}

impl Operation {
    #[cfg(test)]
    pub(crate) fn with_write_limit(mut self, bytes: u64) -> Self {
        self.write_limit = Some(bytes);
        self
    }

    #[cfg(test)]
    pub(crate) fn cancelling_at(mut self, point: Point) -> Self {
        self.cancel_at = Some(point);
        self
    }

    pub(crate) fn writer<'a, W: std::io::Write>(
        &self,
        output: &'a mut W,
    ) -> OperationWriter<'a, W> {
        OperationWriter {
            output,
            #[cfg(test)]
            remaining: self.write_limit,
        }
    }
    #[cfg(test)]
    pub(crate) fn failing_at(mut self, point: Point) -> Self {
        self.fault = Some(point);
        self
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn check_cancelled(&self) -> Result<()> {
        self.check()
    }

    pub fn with_progress(mut self, observer: impl Fn(Progress) + Send + Sync + 'static) -> Self {
        self.observer = Some(Arc::new(observer));
        self
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn progress(&self, processed: u64, total: u64) {
        if let Some(observer) = &self.observer {
            observer(Progress { processed, total });
        }
    }

    pub(crate) fn checkpoint(&self, point: Point) -> Result<()> {
        #[cfg(test)]
        if self.cancel_at == Some(point) {
            self.cancel();
        }
        #[cfg(test)]
        if self.fault == Some(point) {
            return Err(Error::Io(std::io::ErrorKind::StorageFull));
        }
        let _ = point;
        Ok(())
    }
}

pub(crate) struct OperationWriter<'a, W> {
    output: &'a mut W,
    #[cfg(test)]
    remaining: Option<u64>,
}

impl<W: std::io::Write> std::io::Write for OperationWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        if let Some(remaining) = &mut self.remaining {
            if *remaining == 0 {
                return Err(std::io::ErrorKind::StorageFull.into());
            }
            // Both partial successful writes and a subsequent ENOSPC must be handled.
            let count = bytes.len().min((*remaining).min(8192) as usize);
            let written = self.output.write(&bytes[..count])?;
            *remaining -= written as u64;
            return Ok(written);
        }
        self.output.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
    InventoryEntry,
    CommitRead,
    KeysDerived,
    KeysPublished,
    KeyUpdateDerived,
    KeyUpdateWritten,
    KeyUpdateSynced,
    KeyUpdatePublished,
    KeyUpdateDirectorySynced,
    ObjectWritten,
    ObjectSynced,
    ObjectPublished,
    ObjectDirectorySynced,
    SnapshotWritten,
    SnapshotSynced,
    SnapshotPublished,
    SnapshotDirectorySynced,
    CommitWritten,
    CommitSynced,
    CommitPublished,
    CommitDirectorySynced,
    RecoverySnapshotsRetired,
    BeforeObjectReclaim,
    ObjectReclaimed,
    ExportSynced,
    ExportPublished,
    BackupPrepared,
    BackupPublished,
    BackupDirectorySynced,
}
