//! Shared vault core. The experimental format is not a stable product format.

mod backup;
mod codec;
pub mod crypto;
mod error;
mod files;
mod garbage;
mod key_update;
mod keyfile;
mod ledger;
mod model;
pub mod operation;
mod store;
mod stream;
mod tree;
mod vault;

#[cfg(test)]
mod recovery_tests;

pub use backup::{BackupReport, BackupVerification};
pub use error::{Error, Result};
pub use files::ContentReport;
pub use garbage::{CleanupReport, StorageUsage};
pub use keyfile::{CreateOptions, Credential};
pub use ledger::{CommitInfo, Inspection, Issue};
pub use model::{Entry, EntryKind, Id};
pub use vault::{OpenMode, Vault};

/// Product name shared by both application entry points.
pub const PRODUCT_NAME: &str = "BroKnowMyTrove";

/// Explicitly describes the capabilities of this engineering milestone.
pub const DEVELOPMENT_STATUS: &str =
    "Experimental development build. No stable format or independent security audit yet.";
