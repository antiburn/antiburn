// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows process host: launches a target inside a kill-on-close Job Object
//! with no visible console.
//!
//! The GUI process re-invokes a console-subsystem sibling binary with the
//! internal marker argument; that re-invocation runs [`run`] here. Owning the
//! target through a Job Object means dropping the direct child also tears down
//! its descendants, and allocating a windowless console (or a pseudoconsole on
//! older Windows builds) keeps console-subsystem children from flashing a
//! window.

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, GetStdHandle, HPCON, STD_ERROR_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
    STARTF_FORCEOFFFEEDBACK, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

const HOST_HEADER_LEN: usize = 3;
const PROVIDER_ENV: &str = "ANTIBURN_LOCAL_PROCESS_HOST_PROVIDER";

/// The target starts suspended so the Job Object owns it before any code runs.
/// Deliberately excludes every flag that can allocate a visible console.
fn child_creation_flags() -> u32 {
    CREATE_SUSPENDED
}

fn child_startup_flags() -> u32 {
    STARTF_FORCEOFFFEEDBACK | STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES
}

pub(super) fn prepare_host(program: &OsStr) -> (Vec<OsString>, Option<OsString>) {
    let parent_cwd = std::env::current_dir().unwrap_or_default();
    let parent_path = std::env::var_os("PATH");
    let resolved = resolve_executable_path(program, &parent_cwd, parent_path.as_deref())
        .map(PathBuf::into_os_string)
        .unwrap_or_default();
    let path_marker = format!(
        "__ANTIBURN_LOCAL_UNCHANGED_PATH_V1_{}__",
        uuid::Uuid::new_v4()
    );
    let masked_path = parent_path.map(|path| {
        let mut masked = OsString::from(&path_marker);
        masked.push(path);
        masked
    });
    (
        vec![
            OsString::from(path_marker),
            parent_cwd.into_os_string(),
            resolved,
            program.to_os_string(),
        ],
        masked_path,
    )
}

pub(super) fn target_program(args: &[OsString]) -> Option<&OsStr> {
    args.get(HOST_HEADER_LEN).map(OsString::as_os_str)
}

pub(super) fn run(args: &[OsString]) -> io::Result<u32> {
    let (header, target_args) = args.split_at_checked(HOST_HEADER_LEN).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing process host metadata")
    })?;
    let mut command_line = command_line(target_args)?;
    let parent_cwd = Path::new(&header[1]);
    let default_application = (!header[2].is_empty()).then(|| PathBuf::from(&header[2]));
    let (child_path, changed_child_path) = child_path(&header[0])?;
    let application = if changed_child_path {
        let child_path = child_path.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "changed child PATH was not available",
            )
        })?;
        resolve_from_path(&target_args[0], parent_cwd, child_path).or(default_application)
    } else {
        default_application
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "target program was not found"))?;
    let application_name = nul_terminated(&application)?;
    let job = create_kill_on_close_job()?;
    let use_conpty = use_conpty_provider()?;
    let mut startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: child_startup_flags(),
        wShowWindow: 0, // SW_HIDE, without pulling in the broad UI feature set.
        // SAFETY: These handles belong to this host and are passed only to a
        // child created with handle inheritance enabled.
        hStdInput: unsafe { GetStdHandle(STD_INPUT_HANDLE) },
        hStdOutput: unsafe { GetStdHandle(STD_OUTPUT_HANDLE) },
        hStdError: unsafe { GetStdHandle(STD_ERROR_HANDLE) },
        ..Default::default()
    };
    let mut startup_ex = STARTUPINFOEXW::default();
    let mut pseudo_console = None;
    let mut attributes = None;
    let creation_flags;
    let startup_ptr;
    if use_conpty {
        let pseudo = PseudoConsole::new()?;
        let attribute_list = ProcThreadAttributeList::for_pseudoconsole(pseudo.handle())?;
        startup.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup_ex.StartupInfo = startup;
        startup_ex.lpAttributeList = attribute_list.ptr;
        startup_ptr = &startup_ex.StartupInfo;
        creation_flags = child_creation_flags() | EXTENDED_STARTUPINFO_PRESENT;
        pseudo_console = Some(pseudo);
        attributes = Some(attribute_list);
    } else {
        startup_ptr = &startup;
        creation_flags = child_creation_flags();
    }
    let mut process = PROCESS_INFORMATION::default();

    // SAFETY: The command line is mutable and NUL-terminated, all optional
    // pointers are null, and the output structures remain valid for the call.
    let created = unsafe {
        CreateProcessW(
            application_name.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            creation_flags,
            null(),
            null(),
            startup_ptr,
            &mut process,
        )
    };
    // The attribute list is consumed only during CreateProcessW. ConPTY itself
    // stays alive until after the target exits.
    drop(attributes);
    if created == 0 {
        return Err(io::Error::last_os_error());
    }

    let process_handle = OwnedHandle(process.hProcess);
    let thread_handle = OwnedHandle(process.hThread);
    // SAFETY: Both handles came from the successful CreateProcessW call and
    // the primary thread is still suspended.
    if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0 {
        let error = io::Error::last_os_error();
        terminate_suspended(process_handle.0);
        return Err(error);
    }
    // SAFETY: The primary thread handle is valid and has not been resumed.
    if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
        let error = io::Error::last_os_error();
        terminate_suspended(process_handle.0);
        return Err(error);
    }
    drop(thread_handle);

    // SAFETY: The process handle remains owned by this function while waiting.
    let wait = unsafe { WaitForSingleObject(process_handle.0, INFINITE) };
    if wait == WAIT_FAILED {
        return Err(io::Error::last_os_error());
    }
    if wait != WAIT_OBJECT_0 {
        return Err(io::Error::other(format!(
            "unexpected process wait result {wait}"
        )));
    }

    let mut exit_code = 0;
    // SAFETY: Waiting signaled that the process exited and its handle is valid.
    if unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) } == 0 {
        return Err(io::Error::last_os_error());
    }
    drop(pseudo_console);
    Ok(exit_code)
}

fn use_conpty_provider() -> io::Result<bool> {
    let forced = std::env::var_os(PROVIDER_ENV);
    if forced.is_some() {
        // SAFETY: The host is single-threaded and removes its internal
        // control variable before creating the target process.
        unsafe {
            std::env::remove_var(PROVIDER_ENV);
        }
    }
    match forced.as_deref() {
        Some(value) if value.eq_ignore_ascii_case("conpty") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("alloc-console") => {
            allocate_windowless_console()?;
            Ok(false)
        }
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown Windows process host provider",
        )),
        None => match allocate_windowless_console() {
            Ok(()) => Ok(false),
            Err(_) => Ok(true),
        },
    }
}

struct PseudoConsole {
    handle: HPCON,
    _input: OwnedHandle,
    drain: Option<std::thread::JoinHandle<()>>,
}

impl PseudoConsole {
    fn new() -> io::Result<Self> {
        let (input_read, input_write) = create_pipe()?;
        let (output_read, output_write) = create_pipe()?;
        let mut handle = 0;
        // SAFETY: Both synchronous pipe ends remain valid for the call and the
        // output pointer references initialized writable storage.
        let status = unsafe {
            CreatePseudoConsole(
                COORD { X: 80, Y: 25 },
                input_read.0,
                output_write.0,
                0,
                &mut handle,
            )
        };
        if status < 0 {
            return Err(io::Error::other(format!(
                "CreatePseudoConsole failed with HRESULT 0x{:08x}",
                status as u32
            )));
        }

        // ConPTY duplicates its server-side pipe ends. Keep the client input
        // writer open because EOF terminates attached console processes; target
        // stdin still comes from the separate STARTF_USESTDHANDLES handle.
        drop(input_read);
        drop(output_write);
        let output_handle = output_read.into_raw() as usize;
        let drain = std::thread::spawn(move || {
            // SAFETY: Ownership of this pipe handle moved out of OwnedHandle
            // and into File, which closes it exactly once when draining ends.
            let mut output = unsafe { std::fs::File::from_raw_handle(output_handle as HANDLE) };
            let _ = io::copy(&mut output, &mut io::sink());
        });
        Ok(Self {
            handle,
            _input: input_write,
            drain: Some(drain),
        })
    }

    fn handle(&self) -> HPCON {
        self.handle
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        // SAFETY: This handle was returned by CreatePseudoConsole and is closed
        // once, after the attached target has exited or failed to launch.
        unsafe {
            ClosePseudoConsole(self.handle);
        }
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
}

struct ProcThreadAttributeList {
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}

impl ProcThreadAttributeList {
    fn for_pseudoconsole(pseudo_console: HPCON) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: A null first call reports the required opaque buffer size.
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        let ptr = storage.as_mut_ptr().cast::<c_void>();
        // SAFETY: `storage` is aligned and large enough for the requested list
        // and remains owned by this value until the list is deleted.
        if unsafe { InitializeProcThreadAttributeList(ptr, 1, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE uniquely takes the HPCON value
        // itself as lpValue, matching Microsoft's canonical ConPTY sample.
        if unsafe {
            UpdateProcThreadAttribute(
                ptr,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                pseudo_console as *const c_void,
                size_of::<HPCON>(),
                std::ptr::null_mut(),
                null(),
            )
        } == 0
        {
            // SAFETY: The preceding initialization succeeded.
            unsafe {
                DeleteProcThreadAttributeList(ptr);
            }
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr,
            _storage: storage,
        })
    }
}

impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        // SAFETY: `ptr` was initialized successfully and its backing storage is
        // still alive. Delete does not free that caller-owned storage.
        unsafe {
            DeleteProcThreadAttributeList(self.ptr);
        }
    }
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    // SAFETY: Both output pointers are valid and null security attributes make
    // these host-only handles non-inheritable.
    if unsafe { CreatePipe(&mut read, &mut write, null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((OwnedHandle(read), OwnedHandle(write)))
}

/// Allocates a console session with no HWND on Windows 11 24H2 and newer.
/// The target and all console descendants inherit this session, avoiding the
/// create-then-hide race inherent in `CREATE_NEW_CONSOLE + SW_HIDE`.
pub fn allocate_windowless_console() -> io::Result<()> {
    #[repr(C)]
    struct AllocConsoleOptions {
        mode: u32,
        use_show_window: i32,
        show_window: u16,
    }

    type AllocConsoleWithOptions =
        unsafe extern "system" fn(options: *const AllocConsoleOptions, result: *mut u32) -> i32;

    let kernel32 = "kernel32.dll\0".encode_utf16().collect::<Vec<_>>();
    // SAFETY: The module name and export name are NUL-terminated. kernel32 is
    // loaded in every Windows process and remains loaded for the host lifetime.
    let procedure = unsafe {
        let module = GetModuleHandleW(kernel32.as_ptr());
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        GetProcAddress(module, c"AllocConsoleWithOptions".as_ptr().cast())
    };
    let Some(procedure) = procedure else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "windowless consoles require Windows 11 24H2 or Windows Server 2025",
        ));
    };
    // SAFETY: GetProcAddress returned the documented AllocConsoleWithOptions
    // export, whose ABI and signature are declared above.
    let allocate: AllocConsoleWithOptions = unsafe { std::mem::transmute(procedure) };
    let options = AllocConsoleOptions {
        mode: 2, // ALLOC_CONSOLE_MODE_NO_WINDOW
        use_show_window: 0,
        show_window: 0,
    };
    let mut result = 0;
    // SAFETY: Both pointers reference initialized values for the duration of
    // the call, and this single-threaded host does not already own a console.
    let status = unsafe { allocate(&options, &mut result) };
    if status < 0 {
        return Err(io::Error::other(format!(
            "AllocConsoleWithOptions failed with HRESULT 0x{:08x}",
            status as u32
        )));
    }
    if result == 0 {
        return Err(io::Error::other(
            "AllocConsoleWithOptions did not allocate a windowless console",
        ));
    }
    Ok(())
}

fn child_path(marker: &OsStr) -> io::Result<(Option<OsString>, bool)> {
    let Some(path) = std::env::var_os("PATH") else {
        return Ok((None, false));
    };
    let prefix = marker.encode_wide().collect::<Vec<_>>();
    let units = path.encode_wide().collect::<Vec<_>>();
    if let Some(original) = units.strip_prefix(prefix.as_slice()) {
        let original = OsString::from_wide(original);
        // SAFETY: The host is single-threaded and mutates its own
        // environment before creating the only target process.
        unsafe {
            std::env::set_var("PATH", &original);
        }
        Ok((Some(original), false))
    } else {
        Ok((Some(path), true))
    }
}

fn resolve_executable_path(
    program: &OsStr,
    parent_cwd: &Path,
    parent_path: Option<&OsStr>,
) -> io::Result<PathBuf> {
    let path = Path::new(program);
    if program.is_empty() || path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "target program path has no file name",
        ));
    }
    ensure_no_nul(program)?;

    if path.file_name() != Some(program) {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            parent_cwd.join(path)
        };
        return Ok(append_exe_if_found(&path).unwrap_or(path));
    }

    let mut directories = Vec::new();
    if let Ok(mut application_dir) = std::env::current_exe() {
        application_dir.pop();
        directories.push(application_dir);
    }
    if let Some(system_dir) = system_directory(GetSystemDirectoryW) {
        directories.push(system_dir);
    }
    if let Some(windows_dir) = system_directory(GetWindowsDirectoryW) {
        directories.push(windows_dir);
    }
    if let Some(path) = parent_path {
        directories.extend(path_directories(path, parent_cwd));
    }

    let has_extension = program.encode_wide().any(|unit| unit == b'.' as u16);
    for directory in directories {
        let candidate = directory.join(program);
        let candidate = if has_extension {
            candidate
        } else {
            let mut executable = candidate.into_os_string();
            executable.push(".exe");
            PathBuf::from(executable)
        };
        if std::fs::symlink_metadata(&candidate).is_ok() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "target program was not found",
    ))
}

fn resolve_from_path(program: &OsStr, parent_cwd: &Path, path: &OsStr) -> Option<PathBuf> {
    (Path::new(program).file_name() == Some(program)).then_some(())?;
    let has_extension = program.encode_wide().any(|unit| unit == b'.' as u16);
    path_directories(path, parent_cwd).find_map(|directory| {
        let candidate = directory.join(program);
        let candidate = if has_extension {
            candidate
        } else {
            let mut executable = candidate.into_os_string();
            executable.push(".exe");
            PathBuf::from(executable)
        };
        std::fs::symlink_metadata(&candidate)
            .is_ok()
            .then_some(candidate)
    })
}

fn path_directories<'a>(
    path: &'a OsStr,
    parent_cwd: &'a Path,
) -> impl Iterator<Item = PathBuf> + 'a {
    std::env::split_paths(path)
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                parent_cwd.join(path)
            }
        })
}

fn append_exe_if_found(path: &Path) -> Option<PathBuf> {
    if has_exe_suffix(path.as_os_str()) {
        return None;
    }
    let mut executable = path.as_os_str().to_os_string();
    executable.push(".exe");
    let executable = PathBuf::from(executable);
    std::fs::symlink_metadata(&executable)
        .is_ok()
        .then_some(executable)
}

fn has_exe_suffix(value: &OsStr) -> bool {
    let units = value.encode_wide().collect::<Vec<_>>();
    units.len() >= 4
        && units[units.len() - 4] == b'.' as u16
        && ascii_eq_ignore_case(units[units.len() - 3], b'e')
        && ascii_eq_ignore_case(units[units.len() - 2], b'x')
        && ascii_eq_ignore_case(units[units.len() - 1], b'e')
}

fn ascii_eq_ignore_case(unit: u16, expected: u8) -> bool {
    unit == expected.to_ascii_lowercase() as u16 || unit == expected.to_ascii_uppercase() as u16
}

fn system_directory(
    get_directory: unsafe extern "system" fn(*mut u16, u32) -> u32,
) -> Option<PathBuf> {
    let mut buffer = vec![0; 32_768];
    // SAFETY: `buffer` is writable for the supplied capacity. Both supported
    // getters use the same Win32 buffer contract.
    let length = unsafe { get_directory(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    (length > 0 && length < buffer.len()).then(|| {
        buffer.truncate(length);
        PathBuf::from(OsString::from_wide(&buffer))
    })
}

fn nul_terminated(path: &Path) -> io::Result<Vec<u16>> {
    let normalized;
    let path = if path.as_os_str().encode_wide().count() >= 248 {
        normalized = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        &normalized
    } else {
        path
    };
    let mut units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "target program contains an embedded NUL",
        ));
    }
    units.push(0);
    Ok(units)
}

fn ensure_no_nul(value: &OsStr) -> io::Result<()> {
    if value.encode_wide().any(|unit| unit == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "target program contains an embedded NUL",
        ));
    }
    Ok(())
}

fn create_kill_on_close_job() -> io::Result<OwnedHandle> {
    // SAFETY: Null attributes and name request an unnamed, non-inheritable job.
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let handle = OwnedHandle(handle);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: The information pointer and byte count exactly describe `limits`.
    let configured = unsafe {
        SetInformationJobObject(
            handle.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

fn terminate_suspended(process: HANDLE) {
    // SAFETY: This is called only with a live process handle returned by
    // CreateProcessW. Failure is non-actionable; dropping the Job Object is the
    // second cleanup mechanism.
    unsafe {
        TerminateProcess(process, 1);
    }
}

fn command_line(args: &[OsString]) -> io::Result<Vec<u16>> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing target program",
        ));
    }

    let mut command_line = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if index != 0 {
            command_line.push(b' ' as u16);
        }
        append_quoted_arg(&mut command_line, arg)?;
    }
    command_line.push(0);
    Ok(command_line)
}

fn append_quoted_arg(output: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    let units = arg.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process argument contains an embedded NUL",
        ));
    }
    let needs_quotes = units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == b' ' as u16 || *unit == b'\t' as u16 || *unit == b'"' as u16);
    if !needs_quotes {
        output.extend(units);
        return Ok(());
    }

    output.push(b'"' as u16);
    let mut backslashes = 0;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            output.push(unit);
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            output.push(unit);
        }
        backslashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn into_raw(mut self) -> HANDLE {
        let handle = self.0;
        self.0 = std::ptr::null_mut();
        handle
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: OwnedHandle is constructed only from handles whose
            // ownership was transferred to this module and closes each once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;

    fn rendered(args: &[&str]) -> String {
        let args = args.iter().map(OsString::from).collect::<Vec<_>>();
        let wide = command_line(&args).unwrap();
        String::from_utf16(&wide[..wide.len() - 1]).unwrap()
    }

    #[test]
    fn quotes_windows_arguments() {
        assert_eq!(rendered(&["program"]), "program");
        assert_eq!(rendered(&["program", ""]), "program \"\"");
        assert_eq!(rendered(&["program", "two words"]), "program \"two words\"");
        assert_eq!(rendered(&["program", "tab\there"]), "program \"tab\there\"");
        assert_eq!(rendered(&["program", "say\"hi"]), "program \"say\\\"hi\"");
        assert_eq!(
            rendered(&["program", r"C:\Program Files\"]),
            r#"program "C:\Program Files\\""#
        );
        assert_eq!(rendered(&["program", r#"a\"b"#]), r#"program "a\\\"b""#);
        assert_eq!(rendered(&["程序.exe", "雪 λ"]), "程序.exe \"雪 λ\"");
    }

    #[test]
    fn rejects_missing_target() {
        let error = command_line(&[]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_embedded_nul() {
        let arg = OsString::from_wide(&[b'a' as u16, 0, b'b' as u16]);
        let error = command_line(&[arg]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn executable_suffix_is_case_insensitive() {
        assert!(has_exe_suffix(OsStr::new("tool.ExE")));
        assert!(!has_exe_suffix(OsStr::new("tool")));
    }

    #[test]
    fn child_launch_never_requests_a_visible_console() {
        assert_eq!(child_creation_flags(), CREATE_SUSPENDED);
    }

    #[test]
    fn child_launch_disables_the_working_in_background_cursor() {
        assert_ne!(child_startup_flags() & STARTF_FORCEOFFFEEDBACK, 0);
    }
}
