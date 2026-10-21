//! Filesystem and process adaptation. Platform FFI is isolated from the vault core.
use std::{io, path::Path};

/// Flush directory metadata, propagating unsupported filesystems and access failures.
/// Windows needs a writable directory handle for FlushFileBuffers (`File::sync_all`).
pub fn sync_directory(path: &Path) -> io::Result<()> {
    // A trailing separator or `/.` must not make a final directory symlink traversable.
    let path: std::path::PathBuf = path.components().collect();
    #[cfg(unix)]
    let directory = std::fs::File::from(rustix::fs::open(
        &path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    #[cfg(windows)]
    let directory = {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?
    };
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    directory.sync_all()
}

#[cfg(windows)]
mod private_file;
#[cfg(windows)]
pub use private_file::create_private_file;
#[cfg(target_os = "macos")]
mod macos_acl;

/// Create a new file for unauthenticated export staging, never inheriting
/// broader data access. A destination unable to enforce privacy is rejected.
#[cfg(not(windows))]
pub fn create_private_file(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let checked = (|| {
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(io::ErrorKind::Unsupported.into());
        }
        #[cfg(target_os = "macos")]
        macos_acl::check_private(&file)?;
        Ok(())
    })();
    if let Err(error) = checked {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

/// Release a console created solely for this process (for example by Explorer).
/// Keep a shared terminal attached so GUI startup errors remain visible there.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn detach_private_console() -> io::Result<()> {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut process = 0;
    // SAFETY: the writable buffer holds one process ID and lives across the call.
    // FreeConsole only detaches this process, and takes no pointers.
    unsafe {
        if GetConsoleProcessList(&mut process, 1) == 1 && FreeConsole() == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub struct Capacity {
    pub available: u64,
    pub max_file_size: Option<u64>,
}

pub fn capacity(path: &Path) -> io::Result<Capacity> {
    Ok(Capacity {
        available: fs2::available_space(path)?,
        max_file_size: max_file_size(path)?,
    })
}

fn fat_limit(filesystem: &str) -> Option<u64> {
    if matches!(filesystem, "FAT" | "FAT32" | "msdos") {
        Some(u32::MAX as u64)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn max_file_size(path: &Path) -> io::Result<Option<u64>> {
    let info = rustix::fs::statfs(path)?;
    Ok(if info.f_type as u64 == 0x4d44 {
        fat_limit("FAT32")
    } else {
        None
    })
}

#[cfg(target_os = "macos")]
fn max_file_size(path: &Path) -> io::Result<Option<u64>> {
    let info = rustix::fs::statfs(path)?;
    let name: Vec<u8> = info
        .f_fstypename
        .iter()
        .take_while(|ch| **ch != 0)
        .map(|ch| *ch as u8)
        .collect();
    Ok(fat_limit(&String::from_utf8_lossy(&name)))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn max_file_size(path: &Path) -> io::Result<Option<u64>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};
    let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    path.push(0);
    let mut volume = vec![0u16; 32768];
    let mut filesystem = [0u16; 64];
    // SAFETY: both input strings are NUL terminated; writable buffers have the declared
    // lengths, live across each synchronous call, and all optional outputs are null.
    unsafe {
        if GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) == 0 {
            return Err(io::Error::last_os_error());
        }
        if GetVolumeInformationW(
            volume.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    let length = filesystem
        .iter()
        .position(|ch| *ch == 0)
        .ok_or(io::ErrorKind::InvalidData)?;
    Ok(fat_limit(&String::from_utf16_lossy(&filesystem[..length])))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_fat_limits_and_queries_real_destination() {
        assert_eq!(fat_limit("FAT32"), Some(4_294_967_295));
        assert_eq!(fat_limit("msdos"), Some(4_294_967_295));
        assert_eq!(fat_limit("exFAT"), None);
        assert_eq!(fat_limit("NTFS"), None);
        let _ = capacity(&std::env::temp_dir()).unwrap();
    }

    #[test]
    fn directory_flush_rejects_a_regular_file_and_missing_path() {
        let path = std::env::current_exe().unwrap();
        assert!(sync_directory(&path).is_err());
        assert!(sync_directory(&path.join("missing")).is_err());
    }

    #[test]
    fn writable_directory_flush_is_not_silently_skipped() {
        let temporary = tempfile::tempdir().unwrap();
        sync_directory(temporary.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_flush_rejects_a_directory_symlink() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        let link = temporary.path().join("link");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        for path in [link.clone(), link.join(""), link.join(".")] {
            assert!(sync_directory(&path).is_err());
        }
        sync_directory(&target).unwrap();
    }
}
