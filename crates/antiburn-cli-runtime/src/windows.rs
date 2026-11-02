use std::fs::Metadata;
use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, GetAce, IsWellKnownSid, LookupAccountNameW,
    WinBuiltinAdministratorsSid, WinLocalSystemSid,
};

use anyhow::{Context, Result, ensure};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorControl, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub fn check_owned(path: &Path, metadata: &Metadata) -> Result<()> {
    check_policy(path, metadata, false)
}

pub fn check_ancestor(path: &Path, metadata: &Metadata) -> Result<()> {
    check_policy(path, metadata, true)
}

fn check_policy(path: &Path, metadata: &Metadata, ancestor: bool) -> Result<()> {
    ensure!(
        metadata.file_attributes() & 0x400 == 0,
        "CLI path is a Windows reparse point"
    );
    ensure!(
        ancestor || !metadata.permissions().readonly(),
        "CLI path is read-only; its policy was preserved"
    );
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    let mut dacl = null_mut();
    // Windows allocates the descriptor. Release it after the SID comparison.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    ensure!(status == 0, "Cannot inspect CLI path ownership ({status})");
    let result = (|| -> Result<()> {
        let mut token = null_mut();
        ensure!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0,
            "Cannot inspect CLI installation user"
        );
        let result = (|| -> Result<()> {
            let mut length = 0;
            unsafe {
                GetTokenInformation(token, TokenUser, null_mut(), 0, &mut length);
            }
            ensure!(length > 0 && length < 65536, "Invalid Windows token size");
            let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
            ensure!(
                unsafe {
                    GetTokenInformation(
                        token,
                        TokenUser,
                        buffer.as_mut_ptr().cast(),
                        length,
                        &mut length,
                    )
                } != 0,
                "Cannot read CLI installation user"
            );
            let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            let installer = trusted_installer_sid();
            let trusted = |sid| unsafe {
                EqualSid(sid, user.User.Sid) != 0
                    || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                    || installer
                        .as_ref()
                        .is_some_and(|value| EqualSid(sid, value.as_ptr().cast_mut().cast()) != 0)
            };
            let owner_matches = !owner.is_null()
                && if ancestor {
                    trusted(owner)
                } else {
                    unsafe { EqualSid(owner, user.User.Sid) != 0 }
                };
            ensure!(owner_matches, "CLI path belongs to another user");
            ensure!(
                !dacl.is_null(),
                "CLI path has an unrestricted ACL; it was preserved"
            );
            for index in 0..unsafe { (*dacl).AceCount } as u32 {
                let mut ace = null_mut();
                ensure!(
                    unsafe { GetAce(dacl, index, &mut ace) } != 0,
                    "Cannot inspect CLI path permissions"
                );
                let header = unsafe { &*ace.cast::<ACE_HEADER>() };
                // An inherit-only ACE does not grant access to this directory.
                if header.AceFlags & 0x8 != 0 {
                    continue;
                }
                if header.AceType == 0 {
                    let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                    let sid = std::ptr::addr_of!(allowed.SidStart).cast_mut().cast();
                    // Creating a new child cannot replace an existing checked ancestor.
                    let write_mask = if ancestor { 0x500d0150 } else { 0x500d0156 };
                    ensure!(
                        trusted(sid) || allowed.Mask & write_mask == 0,
                        "CLI path allows unrelated users to change it; it was preserved"
                    );
                } else {
                    ensure!(
                        header.AceType == 1 || header.AceType == 2,
                        "CLI path has an unsupported ACL; it was preserved"
                    );
                }
            }
            let mut control = 0;
            let mut revision = 0;
            ensure!(
                unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
                    != 0,
                "Cannot inspect CLI path policy"
            );
            ensure!(
                ancestor || control & SE_DACL_PROTECTED == 0,
                "CLI path has a protected ACL; its policy was preserved"
            );
            Ok(())
        })();
        unsafe {
            CloseHandle(token);
        }
        result
    })();
    unsafe {
        LocalFree(descriptor);
    }
    result
}

fn trusted_installer_sid() -> Option<Vec<usize>> {
    let name: Vec<u16> = "NT SERVICE\\TrustedInstaller"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut sid_length = 0;
    let mut domain_length = 0;
    let mut sid_type = 0;
    unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name.as_ptr(),
            null_mut(),
            &mut sid_length,
            null_mut(),
            &mut domain_length,
            &mut sid_type,
        );
    }
    if sid_length == 0 || sid_length > 65536 || domain_length > 65536 {
        return None;
    }
    let mut sid = vec![0usize; (sid_length as usize).div_ceil(size_of::<usize>())];
    let mut domain = vec![0u16; domain_length as usize];
    let result = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut sid_type,
        )
    };
    (result != 0).then_some(sid)
}

pub fn replace_preserving_policy(destination: &Path, source: &Path) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            source.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).context(
            "CLI replacement deferred; destination may be locked or policy prevents replacement",
        );
    }
    Ok(())
}
