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
}

impl Operation {
    #[cfg(test)]
    pub(crate) fn failing_at(mut self, point: Point) -> Self {
        self.fault = Some(point);
        self
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
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

    #[allow(dead_code)]
    pub(crate) fn checkpoint(&self, point: Point) -> Result<()> {
        #[cfg(test)]
        if self.fault == Some(point) {
            return Err(Error::Io(std::io::ErrorKind::StorageFull));
        }
        let _ = point;
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
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
    ExportSynced,
    ExportPublished,
}
