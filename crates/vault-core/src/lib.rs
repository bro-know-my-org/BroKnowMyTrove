//! Shared vault core. The experimental format is not a stable product format.

#[allow(dead_code)] // Staged M0.2 interface; removed when M1 consumes the primitives.
pub mod crypto;
mod error;
#[allow(dead_code)] // Schema is consumed by the following persistence stage.
mod model;

pub use error::{Error, Result};
pub use model::{Entry, EntryKind, Id};

/// Product name shared by both application entry points.
pub const PRODUCT_NAME: &str = "BroKnowMyTrove";

/// Explicitly describes the capabilities of this engineering milestone.
pub const DEVELOPMENT_STATUS: &str =
    "Experimental development build. No stable format or independent security audit yet.";
