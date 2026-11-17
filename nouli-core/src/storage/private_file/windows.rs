//! Native creation-time DACLs and handle-based checks; no permission repair.
use super::Error;
use std::{
    ffi::c_void,
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_NO_TOKEN, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, GetLastError,
        INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_FILE_OBJECT,
        },
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
        NO_PROPAGATE_INHERIT_ACE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSID,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_ALWAYS, OPEN_EXISTING,
        READ_CONTROL,
    },
    System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
};

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut name: Vec<_> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    name.push(0);
    Ok(name)
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: the caller supplies a SID obtained from a successful Win32 query.
    unsafe {
        check(ConvertSidToStringSidW(sid, &raw mut text))?;
        let _allocation = Local(text.cast());
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(
            text, length,
        )))
    }
}

fn user_sid() -> io::Result<String> {
    // SAFETY: returned handles and token buffers stay live through SID conversion.
    unsafe {
        let mut token = ptr::null_mut();
        if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) == 0 {
            if GetLastError() != ERROR_NO_TOKEN {
                return Err(io::Error::last_os_error());
            }
            check(OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY,
                &raw mut token,
            ))?;
        }
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
        check(GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &raw mut bytes,
        ))?;
        sid_text((*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid)
    }
}

fn descriptor() -> io::Result<Local> {
    let user = user_sid()?;
    let sddl: Vec<u16> = format!("O:{user}D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: SDDL is NUL terminated; Win32 allocates the descriptor with LocalAlloc.
    unsafe {
        check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &raw mut descriptor,
            ptr::null_mut(),
        ))?;
    }
    Ok(Local(descriptor))
}

fn attributes(descriptor: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .expect("SECURITY_ATTRIBUTES fits u32"),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

fn validate(file: &File, path: &Path, directory: bool) -> Result<(), Error> {
    // SAFETY: the file handle remains live and all output pointers have storage.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = zeroed();
        check(GetFileInformationByHandle(
            file.as_raw_handle(),
            &raw mut info,
        ))?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && info.nNumberOfLinks > 1)
        {
            return Err(Error::Insecure(path.into()));
        }
        // SQLite can delete a rollback journal after this handle is opened.
        // Treat that stale, unlinked object as absent and never read from it.
        if !directory && info.nNumberOfLinks == 0 {
            return Err(io::Error::from(io::ErrorKind::NotFound).into());
        }
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut sd = ptr::null_mut();
        let status = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut acl,
            ptr::null_mut(),
            &raw mut sd,
        );
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status.cast_signed()).into());
        }
        let _allocation = Local(sd);
        let user = user_sid()?;
        if owner.is_null() || acl.is_null() {
            return Err(Error::Insecure(path.into()));
        }
        let owner = sid_text(owner)?;
        if owner != user && owner != "S-1-5-18" && owner != "S-1-5-32-544" {
            return Err(Error::Insecure(path.into()));
        }
        let mut inherits_files = false;
        let mut inherits_directories = false;
        for index in 0..u32::from((*acl).AceCount) {
            let mut entry = ptr::null_mut();
            check(GetAce(acl, index, &raw mut entry))?;
            let header = &*entry.cast::<ACE_HEADER>();
            match header.AceType {
                0 => {
                    let allowed = &*entry.cast::<ACCESS_ALLOWED_ACE>();
                    let principal = sid_text(ptr::addr_of!(allowed.SidStart).cast_mut().cast())?;
                    if allowed.Mask != 0
                        && principal != user
                        && principal != "S-1-5-18"
                        && principal != "S-1-5-32-544"
                    {
                        return Err(Error::Insecure(path.into()));
                    }
                    if allowed.Mask != 0
                        && u32::from(header.AceFlags) & NO_PROPAGATE_INHERIT_ACE == 0
                    {
                        inherits_files |= u32::from(header.AceFlags) & OBJECT_INHERIT_ACE != 0;
                        inherits_directories |=
                            u32::from(header.AceFlags) & CONTAINER_INHERIT_ACE != 0;
                    }
                }
                1 => {} // Deny ACEs cannot widen access.
                _ => return Err(Error::Insecure(path.into())),
            }
        }
        if directory && (!inherits_files || !inherits_directories) {
            return Err(Error::Insecure(path.into()));
        }
    }
    Ok(())
}

fn open_handle(path: &Path, access: u32, disposition: u32, directory: bool) -> Result<File, Error> {
    let descriptor = descriptor()?;
    let security = attributes(&descriptor);
    let path_wide = wide(path)?;
    // Directory handles deliberately omit FILE_SHARE_DELETE to pin the root.
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | if directory { 0 } else { FILE_SHARE_DELETE };
    // SAFETY: path/security buffers remain live for CreateFileW; ownership is transferred once.
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            access | READ_CONTROL,
            share,
            &raw const security,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    FILE_ATTRIBUTE_NORMAL
                },
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error().into());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    validate(&file, path, directory)?;
    Ok(file)
}

pub(super) fn create_directory(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            && !parent.exists()
        {
            create_directory(parent)?;
        }
        let sd = descriptor()?;
        let security = attributes(&sd);
        // SAFETY: the NUL-terminated name and security descriptor outlive the call.
        if unsafe { CreateDirectoryW(wide(path)?.as_ptr(), &raw const security) } == 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
    }
    open_directory(path)?;
    Ok(())
}
pub(super) fn open_directory(path: &Path) -> Result<File, Error> {
    open_handle(path, FILE_READ_ATTRIBUTES, OPEN_EXISTING, true)
}
pub(super) fn open(path: &Path, write: bool, exclusive: bool) -> Result<File, Error> {
    open_handle(
        path,
        GENERIC_READ | if write { GENERIC_WRITE } else { 0 },
        if exclusive { CREATE_NEW } else { OPEN_EXISTING },
        false,
    )
}
pub(super) fn open_lock(path: &Path) -> Result<File, Error> {
    open_handle(path, GENERIC_READ | GENERIC_WRITE, OPEN_ALWAYS, false)
}
pub(super) fn publish(source: &Path, destination: &Path, replace: bool) -> Result<(), Error> {
    // SAFETY: both paths are NUL-terminated and live for the call. No delete gap.
    if unsafe {
        MoveFileExW(
            wide(source)?.as_ptr(),
            wide(destination)?.as_ptr(),
            MOVEFILE_WRITE_THROUGH
                | if replace {
                    MOVEFILE_REPLACE_EXISTING
                } else {
                    0
                },
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        if replace || error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        identity::Id,
        storage::{Ledger, OpenMode, StorageError, private_file},
    };
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Security::{
        Authorization::SetNamedSecurityInfoW, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let root = Self(std::env::temp_dir().join(format!("nouli-acl-{}", Id::generate())));
            create_directory(&root.0).unwrap();
            root
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn set_dacl(path: &Path, sddl: &str) {
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: all buffers are live; the descriptor and ACL come from Win32.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &raw mut descriptor,
                    ptr::null_mut()
                ),
                0
            );
            let _allocation = Local(descriptor);
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
            assert_ne!(present, 0);
            let mut name = wide(path).unwrap();
            assert_eq!(
                SetNamedSecurityInfoW(
                    name.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut()
                ),
                ERROR_SUCCESS
            );
        }
    }

    fn dacl(path: &Path) -> Vec<u8> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: successful GetSecurityInfo owns a valid ACL until LocalFree.
        unsafe {
            assert_eq!(
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &raw mut acl,
                    ptr::null_mut(),
                    &raw mut descriptor
                ),
                ERROR_SUCCESS
            );
            let _allocation = Local(descriptor);
            assert!(!acl.is_null());
            std::slice::from_raw_parts(acl.cast::<u8>(), usize::from((*acl).AclSize)).to_vec()
        }
    }

    #[test]
    fn new_children_are_private_even_under_a_broad_parent() {
        let parent = Directory::new();
        set_dacl(
            &parent.0,
            "D:P(A;OICI;FA;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
        );
        let child = parent.0.join("private");
        private_file::publish(&child.join("auth.toml"), b"fixture-value").unwrap();
        open_directory(&child).unwrap();
        private_file::open_read(&child.join("auth.toml")).unwrap();
    }

    #[test]
    fn broad_existing_files_are_refused_without_rewriting_acl_or_contents() {
        let root = Directory::new();
        let path = root.0.join("auth.toml");
        private_file::publish(&path, b"original").unwrap();
        set_dacl(&path, "D:P(A;;FA;;;WD)(A;;FA;;;SY)(A;;FA;;;BA)");
        let before = dacl(&path);
        assert!(matches!(
            private_file::open_read(&path),
            Err(Error::Insecure(_))
        ));
        assert!(matches!(
            private_file::publish(&path, b"replacement"),
            Err(Error::Insecure(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(dacl(&path), before);
    }

    #[test]
    fn directories_need_private_inheritable_permissions() {
        let root = Directory::new();
        let user = user_sid().unwrap();
        set_dacl(
            &root.0,
            &format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)"),
        );
        let before = dacl(&root.0);
        assert!(matches!(create_directory(&root.0), Err(Error::Insecure(_))));
        assert_eq!(dacl(&root.0), before);
    }

    #[test]
    fn multiply_linked_files_are_refused_without_modifying_the_original() {
        let root = Directory::new();
        let file = root.0.join("original");
        private_file::publish(&file, b"unchanged").unwrap();
        let alias = root.0.join("alias");
        std::fs::hard_link(&file, &alias).unwrap();
        for path in [&file, &alias] {
            assert!(matches!(open(path, false, false), Err(Error::Insecure(_))));
            assert!(matches!(
                private_file::publish(path, b"replacement"),
                Err(Error::Insecure(_))
            ));
        }
        assert_eq!(std::fs::read(file).unwrap(), b"unchanged");
    }

    #[test]
    #[ignore = "requires Windows Developer Mode or an elevated account with symlink privilege"]
    fn final_reparse_points_are_refused() {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let root = Directory::new();
        let file = root.0.join("original");
        private_file::publish(&file, b"unchanged").unwrap();
        let alias = root.0.join("alias");
        symlink_file(&file, &alias).unwrap();
        assert!(matches!(
            open(&alias, false, false),
            Err(Error::Insecure(_))
        ));
        let directory = root.0.join("linked-directory");
        symlink_dir(&root.0, &directory).unwrap();
        assert!(matches!(
            open_directory(&directory),
            Err(Error::Insecure(_))
        ));
        assert!(matches!(
            create_directory(&directory),
            Err(Error::Insecure(_))
        ));
        assert_eq!(std::fs::read(file).unwrap(), b"unchanged");
    }

    #[test]
    fn ledger_journals_and_writer_guards_have_private_acls() {
        const SQL: &str = "CREATE TABLE security_probe(value TEXT);\n\
                          INSERT INTO security_probe VALUES ('fixture');";
        let root = Directory::new();
        let mut ledger = Ledger::open(&root.0, OpenMode::ReadWriteCreate).unwrap();
        let run = Id::generate();
        let guard = ledger.run_guard(run).unwrap();
        open_directory(&root.0.join("locks")).unwrap();
        private_file::open_read(&root.0.join(format!("locks/run-{run}.lock"))).unwrap();
        let tx = ledger.connection.transaction().unwrap();
        tx.execute_batch(SQL).unwrap();
        private_file::open_read(&root.0.join("ledger.sqlite3-journal")).unwrap();
        tx.rollback().unwrap();
        drop(guard);
        drop(ledger);
    }

    #[test]
    fn unsafe_store_and_lock_directories_are_refused_in_both_open_modes() {
        for unsafe_child in [false, true] {
            let root = Directory::new();
            drop(Ledger::open(&root.0, OpenMode::ReadWriteCreate).unwrap());
            let unsafe_path = if unsafe_child {
                root.0.join("locks")
            } else {
                root.0.clone()
            };
            create_directory(&unsafe_path).unwrap();
            set_dacl(
                &unsafe_path,
                "D:P(A;OICI;FA;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
            );
            let before = dacl(&unsafe_path);
            let count = std::fs::read_dir(&root.0).unwrap().count();
            for mode in [OpenMode::ReadOnlyExisting, OpenMode::ReadWriteCreate] {
                assert!(matches!(
                    Ledger::open(&root.0, mode),
                    Err(StorageError::Insecure { .. })
                ));
            }
            assert_eq!(dacl(&unsafe_path), before);
            assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), count);
        }
    }

    #[test]
    fn wal_is_rejected_before_any_sqlite_recovery() {
        let root = Directory::new();
        drop(Ledger::open(&root.0, OpenMode::ReadWriteCreate).unwrap());
        let writer = rusqlite::Connection::open(root.0.join("ledger.sqlite3")).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer
            .execute_batch("CREATE TABLE wal_probe(value TEXT);")
            .unwrap();
        let before = std::fs::read(root.0.join("ledger.sqlite3-wal")).unwrap();
        for mode in [OpenMode::ReadOnlyExisting, OpenMode::ReadWriteCreate] {
            assert!(matches!(
                Ledger::open(&root.0, mode),
                Err(StorageError::WrongStore)
            ));
        }
        assert_eq!(
            std::fs::read(root.0.join("ledger.sqlite3-wal")).unwrap(),
            before
        );
        drop(writer);
    }

    #[test]
    fn nul_paths_never_open_or_modify_the_prefix_file() {
        let root = Directory::new();
        let path = root.0.join("prefix");
        private_file::publish(&path, b"original").unwrap();
        let mut invalid = path.as_os_str().to_owned();
        invalid.push("\0suffix");
        let invalid = Path::new(&invalid);
        assert!(
            matches!(private_file::open_read(invalid), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
        );
        assert!(private_file::publish(invalid, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn failed_replacement_preserves_contents_and_removes_staging_files() {
        let root = Directory::new();
        let path = root.0.join("auth.toml");
        private_file::publish(&path, b"original").unwrap();
        let held = private_file::lock_beside(&path).unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        assert!(private_file::publish(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 2);
        drop(reader);
        drop(held);
        let _lock = private_file::lock_beside(&path).unwrap();
        private_file::publish(&path, b"replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }

    #[test]
    fn refusing_an_unsafe_existing_lock_directory_does_not_create_a_database() {
        let root = Directory::new();
        let locks = root.0.join("locks");
        create_directory(&locks).unwrap();
        set_dacl(
            &locks,
            "D:P(A;OICI;FA;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
        );
        assert!(matches!(
            Ledger::open(&root.0, OpenMode::ReadWriteCreate),
            Err(StorageError::Insecure { .. })
        ));
        assert!(!root.0.join("ledger.sqlite3").exists());
    }
}
