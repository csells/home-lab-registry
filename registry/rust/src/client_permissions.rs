use std::{fs::File, io};

#[cfg(unix)]
pub(crate) fn private(file: &File) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(file.metadata()?.permissions().mode() & 0o077 == 0)
}

#[cfg(windows)]
pub(crate) fn private(file: &File) -> io::Result<bool> {
    use std::{ffi::c_void, mem::size_of, os::windows::io::AsRawHandle, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, INHERIT_ONLY_ACE, IsWellKnownSid,
            OWNER_SECURITY_INFORMATION, WinBuiltinAdministratorsSid, WinLocalSystemSid,
        },
        System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
    };

    struct Descriptor(*mut c_void);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // The returned buffer is allocated by GetSecurityInfo with LocalAlloc.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    let mut owner = null_mut();
    let mut acl: *mut ACL = null_mut();
    let mut descriptor = Descriptor(null_mut());
    // The handle remains open and the returned pointers remain valid while
    // the security descriptor is alive.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor.0,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    if owner.is_null() || acl.is_null() {
        return Ok(false);
    }
    for index in 0..unsafe { (*acl).AceCount } {
        let mut ace = null_mut();
        if unsafe { GetAce(acl, u32::from(index), &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // GetAce returns a pointer into this security descriptor.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match u32::from(header.AceType) {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE
                if usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>() =>
            {
                let sid = unsafe { &raw mut (*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart }.cast();
                if unsafe {
                    EqualSid(sid, owner) == 0
                        && IsWellKnownSid(sid, WinLocalSystemSid) == 0
                        && IsWellKnownSid(sid, WinBuiltinAdministratorsSid) == 0
                } {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn credential_acl_rejects_an_everyone_read_grant() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("registry.json");
        std::fs::write(&path, "{}").unwrap();
        let who = Command::new("whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .unwrap();
        assert!(who.status.success());
        let user = String::from_utf8(who.stdout).unwrap();
        let sid = user.trim().rsplit(',').next().unwrap().trim_matches('"');
        assert!(sid.starts_with("S-1-"));
        let restrict = Command::new("icacls.exe")
            .arg(&path)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:(F)")])
            .output()
            .unwrap();
        assert!(restrict.status.success());
        assert!(private(&File::open(&path).unwrap()).unwrap());

        let broaden = Command::new("icacls.exe")
            .arg(&path)
            .args(["/grant", "*S-1-1-0:(R)"])
            .output()
            .unwrap();
        assert!(broaden.status.success());
        assert!(!private(&File::open(&path).unwrap()).unwrap());
    }
}
