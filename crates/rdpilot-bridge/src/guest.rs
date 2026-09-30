//! Windows-only guest helpers for `install`, `run` and `cleanup`: the
//! per-generation launch mutex, stopping this user's rdpilot processes, and
//! detached child processes. Nothing here writes outside the per-user
//! rdpilot directory.
use std::{
    ffi::OsStr,
    io,
    os::windows::{ffi::OsStrExt, ffi::OsStringExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{
            CreateMutexW, GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW,
            TerminateProcess, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        },
    },
};

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// Owns `Local\rdpilot-cua-<generation>` for the life of the process, so a
/// repeated launch for the same RDP session generation is a no-op.
pub struct GenerationMutex(HANDLE);

impl Drop for GenerationMutex {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// `Ok(None)` when another process of this user session already runs this
/// generation.
pub fn acquire_generation(generation: u64) -> io::Result<Option<GenerationMutex>> {
    let name = wide(OsStr::new(&format!("Local\\rdpilot-cua-{generation}")));
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return Ok(None);
        }
        Ok(Some(GenerationMutex(handle)))
    }
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

fn image_of(process: HANDLE) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size)
    };
    (ok != 0).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..size as usize])))
}

fn lower(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// Terminate every other process of this user session whose image lives
/// under `base`. Returns how many were stopped.
pub fn stop_processes_under(base: &Path) -> io::Result<usize> {
    let own_pid = unsafe { GetCurrentProcessId() };
    let own_session = session_of(own_pid).ok_or_else(io::Error::last_os_error)?;
    let prefix = format!("{}\\", lower(base).trim_end_matches('\\'));
    let mut stopped = 0;
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let pid = entry.th32ProcessID;
            if pid != own_pid && session_of(pid) == Some(own_session) {
                let process = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                    0,
                    pid,
                );
                if !process.is_null() {
                    if image_of(process).is_some_and(|image| lower(&image).starts_with(&prefix))
                        && TerminateProcess(process, 1) != 0
                    {
                        stopped += 1;
                    }
                    CloseHandle(process);
                }
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    Ok(stopped)
}

/// Start `command` without a console window, independent of this process.
pub fn spawn_detached(command: &mut Command) -> io::Result<()> {
    command
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// After this process exits, remove `base` (which holds the running image).
pub fn remove_after_exit(base: &Path) -> io::Result<()> {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let cmd = PathBuf::from(system_root).join("System32").join("cmd.exe");
    let mut command = Command::new(cmd);
    command.raw_arg(format!(
        "/d /c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
        base.display()
    ));
    spawn_detached(&mut command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generation_can_be_held_once() {
        let generation = u64::from(std::process::id()) << 20 | 0x5a5a;
        let first = acquire_generation(generation).unwrap();
        assert!(first.is_some());
        assert!(acquire_generation(generation).unwrap().is_none());
        drop(first);
        assert!(acquire_generation(generation).unwrap().is_some());
    }
}
