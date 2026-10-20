//! Darwin allow ACLs can grant access independently of mode 0600.
use std::{ffi::c_void, fs::File, io, os::fd::AsRawFd};

#[allow(unsafe_code)]
pub(super) fn check_private(file: &File) -> io::Result<()> {
    // Signatures and Darwin-specific empty-list behavior are defined in Apple's
    // Libc include/sys/acl.h and posix1e/{acl_file,acl_entry}.c.
    unsafe extern "C" {
        fn acl_get_fd(fd: i32) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: i32, entry: *mut *mut c_void) -> i32;
        fn acl_free(object: *mut c_void) -> i32;
    }
    // SAFETY: file supplies a live descriptor. A nonnull result is an owned ACL
    // allocation, which is freed below; no file permissions are changed.
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        // Darwin filesec_get_property reports ENOENT when no ACL is present.
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    let mut entry = std::ptr::null_mut();
    // SAFETY: acl is valid, entry is writable; ACL_FIRST_ENTRY is 0 on Darwin.
    let found = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = io::Error::last_os_error();
    // SAFETY: acl is owned and neither it nor entry is used after this call.
    unsafe { acl_free(acl) };
    if found == -1 && error.kind() == io::ErrorKind::InvalidInput {
        // Unlike POSIX, Darwin returns -1/EINVAL for an empty valid ACL.
        Ok(())
    } else if found == 0 {
        // Reject every nonempty ACL before writing. Removing an inherited ACE
        // would not revoke another user's handle opened while the file was empty.
        Err(io::ErrorKind::Unsupported.into())
    } else {
        Err(error)
    }
}
