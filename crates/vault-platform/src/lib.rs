//! Filesystem and process adaptation. Platform FFI is isolated from the vault core.
use std::{io, path::Path};

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
}
