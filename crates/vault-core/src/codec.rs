use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
};

use serde::{Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use crate::{
    Error, Id, Result,
    crypto::{Secret, seal, unseal},
};

pub(crate) const KEY_LIMIT: usize = 16 * 1024;
pub(crate) const COMMIT_LIMIT: usize = 4096;
pub(crate) const SNAPSHOT_LIMIT: usize = 64 * 1024 * 1024;

pub(crate) fn open_regular(path: &Path) -> Result<File> {
    if !path.symlink_metadata()?.is_file() {
        return Err(Error::InvalidInput);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::InvalidInput);
    }
    Ok(file)
}

pub(crate) fn read_limited(path: &Path, max: usize) -> Result<Zeroizing<Vec<u8>>> {
    let file = open_regular(path)?;
    if file.metadata()?.len() > max as u64 {
        return Err(Error::ResourceLimit);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(Error::ResourceLimit);
    }
    Ok(bytes)
}

pub(crate) fn encode<T: Serialize>(value: &T, max: usize) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::InvalidFormat)?);
    if bytes.len() > max {
        return Err(Error::ResourceLimit);
    }
    Ok(bytes)
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T> {
    if bytes.len() > max {
        return Err(Error::ResourceLimit);
    }
    serde_json::from_slice(bytes).map_err(|_| Error::InvalidFormat)
}

pub(crate) fn aad(purpose: u8, vault: Id, object: Id) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(b"BKMTFMT1");
    bytes.push(purpose);
    bytes.extend_from_slice(&vault.0);
    bytes.extend_from_slice(&object.0);
    bytes
}

pub(crate) fn seal_json<T: Serialize>(
    key: &Secret,
    aad: &[u8],
    value: &T,
    max: usize,
) -> Result<Vec<u8>> {
    seal(key, aad, &encode(value, max - 40)?)
}

pub(crate) fn unseal_json<T: DeserializeOwned>(
    key: &Secret,
    aad: &[u8],
    bytes: &[u8],
    max: usize,
) -> Result<T> {
    if bytes.len() > max {
        return Err(Error::ResourceLimit);
    }
    decode(&unseal(key, aad, bytes)?, max - 40)
}
