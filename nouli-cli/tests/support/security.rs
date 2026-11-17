//! Native permission fixtures; product tests remain entirely inside Cargo.
use std::path::Path;

pub fn is_private(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) => metadata.permissions().mode().trailing_zeros() >= 6,
            // Atomic publication can rename a listed temporary file before stat.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        }
    }
    #[cfg(windows)]
    {
        let result = if path.is_dir() {
            nouli_core::storage::private_file::open_directory(path)
        } else {
            nouli_core::storage::private_file::open_read(path)
        };
        match result {
            Ok(_) => true,
            Err(error) if error.is_not_found() => true,
            Err(_) => false,
        }
    }
}

pub fn set_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if path.is_dir() { 0o700 } else { 0o600 }),
        )
        .unwrap();
    }
    #[cfg(windows)]
    set_acl(path, true);
}

pub fn set_public(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if path.is_dir() { 0o777 } else { 0o640 }),
        )
        .unwrap();
    }
    #[cfg(windows)]
    set_acl(path, false);
}

pub fn repair_hint() -> &'static str {
    #[cfg(unix)]
    {
        "chmod 600"
    }
    #[cfg(windows)]
    {
        "icacls"
    }
}

#[cfg(windows)]
fn set_acl(path: &Path, private: bool) {
    use std::{
        mem::size_of,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        System::Threading::*,
    };
    // SAFETY: pointers reference live Win32-owned descriptors or aligned buffers.
    unsafe {
        let mut token = ptr::null_mut();
        assert_ne!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token),
            0
        );
        let token = OwnedHandle::from_raw_handle(token);
        let mut bytes = 0;
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &raw mut bytes,
        );
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        assert_ne!(
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &raw mut bytes
            ),
            0
        );
        let sid = (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        let mut text = ptr::null_mut();
        assert_ne!(ConvertSidToStringSidW(sid, &raw mut text), 0);
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        let user = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        let principal = if private { user.as_str() } else { "WD" };
        let sddl: Vec<_> = format!("D:P(A;OICI;FA;;;{principal})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &raw mut descriptor,
                ptr::null_mut()
            ),
            0
        );
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        assert_ne!(
            GetSecurityDescriptorDacl(
                descriptor,
                &raw mut present,
                &raw mut acl,
                &raw mut defaulted
            ),
            0
        );
        let mut name: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let result = SetNamedSecurityInfoW(
            name.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        );
        LocalFree(descriptor);
        assert_eq!(result, ERROR_SUCCESS);
    }
}
