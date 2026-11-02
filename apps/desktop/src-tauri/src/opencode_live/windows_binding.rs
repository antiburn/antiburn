//! Proves that a Windows process has the selected local SQLite file open.

#![cfg(windows)]

use std::{
    os::windows::ffi::OsStrExt,
    path::{Component, Path, Prefix},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, GENERIC_READ, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileIdInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetVolumePathNameW, OPEN_EXISTING,
    },
    System::{
        RestartManager::{
            CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
            RmStartSession,
        },
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        WindowsProgramming::DRIVE_FIXED,
    },
};

const MAX_RESTART_MANAGER_PROCESSES: usize = 4096;
const MAX_RESTART_MANAGER_ATTEMPTS: usize = 4;

fn file_time_ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume_serial_number: u64,
    pub file_id: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseBindingProof {
    pub pid: u32,
    pub process_creation_time: u64,
    pub database_identity: FileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseBindingError {
    UnsupportedPath,
    Io,
    RestartManager,
    IncompleteProcessList,
    ProcessNotUsingDatabase,
    ProcessMismatch,
    FileChanged,
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct RestartManagerSession(u32);

impl RestartManagerSession {
    fn start() -> Result<Self, DatabaseBindingError> {
        let mut session = 0;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        let status = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
        if status != ERROR_SUCCESS {
            return Err(DatabaseBindingError::RestartManager);
        }
        Ok(Self(session))
    }

    fn register_file(&self, path: &[u16]) -> Result<(), DatabaseBindingError> {
        let files = [path.as_ptr()];
        let status = unsafe {
            RmRegisterResources(self.0, 1, files.as_ptr(), 0, ptr::null(), 0, ptr::null())
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(DatabaseBindingError::RestartManager)
        }
    }

    fn process_list(&self) -> Result<Vec<RM_PROCESS_INFO>, DatabaseBindingError> {
        let mut capacity = 0usize;
        for _ in 0..MAX_RESTART_MANAGER_ATTEMPTS {
            let mut needed = 0u32;
            let mut written = capacity as u32;
            let mut reboot_reasons = 0u32;
            let mut processes = vec![RM_PROCESS_INFO::default(); capacity];
            let process_buffer = if processes.is_empty() {
                ptr::null_mut()
            } else {
                processes.as_mut_ptr()
            };
            let status = unsafe {
                RmGetList(
                    self.0,
                    &mut needed,
                    &mut written,
                    process_buffer,
                    &mut reboot_reasons,
                )
            };
            if status == ERROR_SUCCESS {
                if written as usize > processes.len() {
                    return Err(DatabaseBindingError::IncompleteProcessList);
                }
                processes.truncate(written as usize);
                return Ok(processes);
            }
            if status != ERROR_MORE_DATA
                || needed == 0
                || needed as usize > MAX_RESTART_MANAGER_PROCESSES
            {
                return Err(DatabaseBindingError::IncompleteProcessList);
            }
            capacity = needed as usize;
        }
        Err(DatabaseBindingError::IncompleteProcessList)
    }
}

impl Drop for RestartManagerSession {
    fn drop(&mut self) {
        unsafe {
            RmEndSession(self.0);
        }
    }
}

/// Prove that `pid` has the exact local database file open.
pub fn prove_database_binding(
    pid: u32,
    database_path: &Path,
) -> Result<DatabaseBindingProof, DatabaseBindingError> {
    if pid == 0 {
        return Err(DatabaseBindingError::ProcessMismatch);
    }

    let canonical_path = canonical_local_path(database_path)?;
    let wide_path = nul_terminated(canonical_path.as_os_str().encode_wide());
    let identity_before = file_identity(&wide_path)?;

    let session = RestartManagerSession::start()?;
    session.register_file(&wide_path)?;
    let processes = session.process_list()?;
    let mut matches = processes
        .iter()
        .filter(|process| process.Process.dwProcessId == pid);
    let registered_process = matches
        .next()
        .ok_or(DatabaseBindingError::ProcessNotUsingDatabase)?;
    if matches.next().is_some() {
        return Err(DatabaseBindingError::ProcessMismatch);
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(DatabaseBindingError::ProcessMismatch);
    }
    let _process_guard = HandleGuard(process);
    let mut creation_time = FILETIME::default();
    let mut exit_time = FILETIME::default();
    let mut kernel_time = FILETIME::default();
    let mut user_time = FILETIME::default();
    let got_times = unsafe {
        GetProcessTimes(
            process,
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        )
    };
    if got_times == 0
        || file_time_ticks(creation_time)
            != file_time_ticks(registered_process.Process.ProcessStartTime)
    {
        return Err(DatabaseBindingError::ProcessMismatch);
    }

    let identity_after = file_identity(&wide_path)?;
    if identity_before != identity_after {
        return Err(DatabaseBindingError::FileChanged);
    }

    Ok(DatabaseBindingProof {
        pid,
        process_creation_time: file_time_ticks(creation_time),
        database_identity: identity_after,
    })
}

fn canonical_local_path(path: &Path) -> Result<std::path::PathBuf, DatabaseBindingError> {
    require_local_disk_path(path)?;
    let canonical = path.canonicalize().map_err(|_| DatabaseBindingError::Io)?;
    require_local_disk_path(&canonical)?;
    let mut volume_root = vec![0u16; 32_768];
    if unsafe {
        GetVolumePathNameW(
            canonical
                .as_os_str()
                .encode_wide()
                .chain([0])
                .collect::<Vec<_>>()
                .as_ptr(),
            volume_root.as_mut_ptr(),
            volume_root.len() as u32,
        )
    } == 0
    {
        return Err(DatabaseBindingError::UnsupportedPath);
    }
    let root_len = volume_root
        .iter()
        .position(|character| *character == 0)
        .ok_or(DatabaseBindingError::UnsupportedPath)?;
    volume_root.truncate(root_len + 1);
    if unsafe { GetDriveTypeW(volume_root.as_ptr()) } != DRIVE_FIXED {
        return Err(DatabaseBindingError::UnsupportedPath);
    }
    if !canonical.is_file() {
        return Err(DatabaseBindingError::UnsupportedPath);
    }
    Ok(canonical)
}

fn require_local_disk_path(path: &Path) -> Result<(), DatabaseBindingError> {
    if path.is_absolute()
        && matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        )
    {
        Ok(())
    } else {
        Err(DatabaseBindingError::UnsupportedPath)
    }
}

fn file_identity(path: &[u16]) -> Result<FileIdentity, DatabaseBindingError> {
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(DatabaseBindingError::Io);
    }
    let _handle_guard = HandleGuard(handle);
    let mut information = FILE_ID_INFO::default();
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut information as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(DatabaseBindingError::Io);
    }
    Ok(FileIdentity {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    })
}

fn nul_terminated(wide: impl Iterator<Item = u16>) -> Vec<u16> {
    wide.chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};
    use windows_sys::Win32::System::RestartManager::RM_UNIQUE_PROCESS;

    #[test]
    fn restart_manager_output_uses_full_process_info_records() {
        assert_eq!(size_of::<RM_UNIQUE_PROCESS>(), 12);
        assert_eq!(size_of::<RM_PROCESS_INFO>(), 668);
        assert_eq!(align_of::<RM_PROCESS_INFO>(), 4);
        assert_eq!(offset_of!(RM_PROCESS_INFO, Process), 0);
        assert_eq!(offset_of!(RM_PROCESS_INFO, strAppName), 12);
        assert_eq!(offset_of!(RM_PROCESS_INFO, strServiceShortName), 524);
        assert_eq!(offset_of!(RM_PROCESS_INFO, ApplicationType), 652);
        assert_eq!(offset_of!(RM_PROCESS_INFO, bRestartable), 664);
    }

    #[test]
    fn accepts_absolute_local_disk_and_verbatim_disk_paths() {
        for path in [
            r"C:\data\opencode.db",
            r"c:\data\opencode.db",
            "C:/data/opencode.db",
            r"\\?\C:\data\opencode.db",
        ] {
            assert_eq!(require_local_disk_path(Path::new(path)), Ok(()), "{path}");
        }
    }

    #[test]
    fn rejects_unc_device_and_relative_paths_before_file_access() {
        for path in [
            r"\\server\share\opencode.db",
            r"\\?\UNC\server\share\opencode.db",
            r"\\.\C:\data\opencode.db",
            r"\\.\PhysicalDrive0",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\opencode.db",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\opencode.db",
            r"C:data\opencode.db",
            r"\data\opencode.db",
            r"data\opencode.db",
        ] {
            assert_eq!(
                canonical_local_path(Path::new(path)),
                Err(DatabaseBindingError::UnsupportedPath),
                "{path}"
            );
        }
    }

    #[test]
    fn binds_an_open_sqlite_database_with_a_canonical_windows_path() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("binding.sqlite");
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE binding_test (id INTEGER PRIMARY KEY);
                 INSERT INTO binding_test VALUES (1);
                 BEGIN IMMEDIATE;",
            )
            .unwrap();
        let canonical_path = database_path.canonicalize().unwrap();
        assert!(matches!(
            canonical_path.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_))
        ));

        let wide_path = nul_terminated(canonical_path.as_os_str().encode_wide());
        let session = RestartManagerSession::start().unwrap();
        session.register_file(&wide_path).unwrap();
        let processes = session.process_list().unwrap();
        let current_process = processes
            .iter()
            .find(|process| process.Process.dwProcessId == std::process::id())
            .expect("Restart Manager must report the open SQLite database");
        let proof = prove_database_binding(std::process::id(), &canonical_path).unwrap();
        assert_eq!(proof.pid, std::process::id());
        assert_ne!(proof.process_creation_time, 0);
        assert_eq!(
            proof.process_creation_time,
            file_time_ticks(current_process.Process.ProcessStartTime)
        );
        assert_eq!(proof.database_identity, file_identity(&wide_path).unwrap());
    }
}
