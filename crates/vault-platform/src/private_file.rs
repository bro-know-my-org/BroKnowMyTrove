//! Windows export staging: protected DACL at creation, before any plaintext.
use std::{
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE,
        GetVolumeInformationByHandleW,
    },
    System::SystemServices::FILE_PERSISTENT_ACLS,
};

#[allow(unsafe_code)]
pub fn create_private_file(path: &Path) -> io::Result<File> {
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    name.push(0);
    // Protected DACL: only the object owner and SYSTEM get access. Inherited
    // directory ACEs must not grant another account a handle before validation.
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: NUL-terminated input lives across the call; on success the API
    // allocates descriptor with LocalAlloc and it is freed after CreateFileW.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: buffers and security descriptor remain valid during creation;
    // CREATE_NEW excludes existing files. Only delete sharing is allowed so
    // tempfile can rename/remove while no second data reader can open it.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let error = io::Error::last_os_error();
    // SAFETY: descriptor was allocated by the successful conversion above.
    unsafe {
        LocalFree(descriptor);
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(error);
    }
    // SAFETY: the successful CreateFileW returns a uniquely owned valid handle.
    let file = unsafe { File::from_raw_handle(handle) };
    let mut flags = 0;
    // SAFETY: file owns a live handle; flags is writable, optional outputs null.
    let checked = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        )
    };
    let error = if checked == 0 {
        Some(io::Error::last_os_error())
    } else if flags & FILE_PERSISTENT_ACLS == 0 {
        Some(io::ErrorKind::Unsupported.into())
    } else {
        None
    };
    if let Some(error) = error {
        // FAT/exFAT can ignore the DACL. Reject before writing any plaintext,
        // removing this empty file while its delete-shared handle is still open.
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}
