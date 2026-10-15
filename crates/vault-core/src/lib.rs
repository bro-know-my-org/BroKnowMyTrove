//! Shared vault core. The experimental format is not a stable product format.

mod codec;
pub mod crypto;
mod error;
mod keyfile;
mod ledger;
#[allow(dead_code)] // Schema is consumed by the following persistence stage.
mod model;
pub mod operation;
#[allow(dead_code)]
mod store;
#[allow(dead_code)]
mod stream;
mod vault;

pub use error::{Error, Result};
pub use keyfile::{CreateOptions, Credential};
pub use ledger::{CommitInfo, Inspection, Issue};
pub use model::{Entry, EntryKind, Id};
pub use vault::{OpenMode, Vault};

/// Product name shared by both application entry points.
pub const PRODUCT_NAME: &str = "BroKnowMyTrove";

/// Explicitly describes the capabilities of this engineering milestone.
pub const DEVELOPMENT_STATUS: &str =
    "Experimental development build. No stable format or independent security audit yet.";
