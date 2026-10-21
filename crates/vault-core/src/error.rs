use std::{fmt, io};

/// Errors deliberately exclude filesystem paths, plaintext names, and secret input.
#[derive(Debug)]
pub enum Error {
    Io(io::ErrorKind),
    Crypto,
    Authentication,
    InvalidFormat,
    ResourceLimit,
    InvalidInput,
    Conflict,
    Locked,
    ReadOnly,
    Cancelled,
    SourceChanged,
    InsufficientSpace,
    CommitUncertain,
    ExportUncertain,
    ExportPrivacy,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

impl From<alkali::AlkaliError> for Error {
    fn from(_: alkali::AlkaliError) -> Self {
        Self::Crypto
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Io(_) => "filesystem operation failed",
            Self::Crypto => "cryptographic operation failed",
            Self::Authentication => "authentication failed (wrong credential or damaged data)",
            Self::InvalidFormat => "invalid or unsupported vault format",
            Self::ResourceLimit => "format or resource limit exceeded",
            Self::InvalidInput => "invalid input",
            Self::Conflict => "name, destination, or commit conflict",
            Self::Locked => "vault is in use by another local process",
            Self::ReadOnly => "vault requires inspection or recovery; writes are disabled",
            Self::Cancelled => "operation cancelled",
            Self::SourceChanged => "source changed while being imported",
            Self::InsufficientSpace => "insufficient space or filesystem file-size limit",
            Self::CommitUncertain => "commit outcome uncertain; close and inspect the vault",
            Self::ExportUncertain => {
                "authenticated export published; directory durability uncertain"
            }
            Self::ExportPrivacy => {
                "destination cannot protect temporary plaintext; choose a private local filesystem"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}
