//! Shared vault core. The experimental format is not a stable product format.

#[allow(dead_code)] // Staged M0.2 interface; removed when M1 consumes the primitives.
pub mod crypto;
mod error;

pub use error::{Error, Result};

/// Product name shared by both application entry points.
pub const PRODUCT_NAME: &str = "BroKnowMyTrove";

/// Explicitly describes the capabilities of this engineering milestone.
pub const DEVELOPMENT_STATUS: &str =
    "M0.1 engineering skeleton. Vault operations and encryption are not implemented.";
