//! Windows containment and overlapped WTS adapter. No COM or desktop automation.
use rdpilot_bridge_protocol::{encode, Decoder, Envelope, CHANNEL_NAME, QUEUE_DEPTH};
use std::{
    ffi::c_void,
    io,
    mem::{size_of, zeroed},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        RemoteDesktop::{
            WTSFreeMemory, WTSVirtualChannelClose, WTSVirtualChannelOpenEx, WTSVirtualChannelQuery,
            WTSVirtualFileHandle,
        },
        Threading::CreateEventW,
        IO::{CancelIoEx, GetOverlappedResultEx, OVERLAPPED},
    },
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process: HANDLE) -> i32;
}

pub struct Job(HANDLE);
// Job objects are kernel handles; ownership, not a thread affinity, controls them.
unsafe impl Send for Job {}
impl Job {
    /// Caller creates suspended. No instruction of uncontained child code runs.
    ///
    /// # Safety
    /// `process` must be a live process handle created with CREATE_SUSPENDED,
    /// held open throughout this call and owned by the caller.
    pub unsafe fn contain_and_resume(process: *mut c_void) -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(handle, process) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let status = NtResumeProcess(process);
            if status < 0 {
                return Err(io::Error::other(format!(
                    "NtResumeProcess failed: {status:#x}"
                )));
            }
            Ok(job)
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Channel {
    channel: HANDLE,
    file: HANDLE,
}
unsafe impl Send for Channel {}
unsafe impl Sync for Channel {}
impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            WTSVirtualChannelClose(self.channel);
        }
    }
}
impl Channel {
    fn open() -> io::Result<Self> {
        let mut name = CHANNEL_NAME.as_bytes().to_vec();
        name.push(0);
        unsafe {
            let channel = WTSVirtualChannelOpenEx(u32::MAX, name.as_ptr(), 1);
            if channel.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut ptr = ptr::null_mut();
            let mut length = 0;
            if WTSVirtualChannelQuery(channel, WTSVirtualFileHandle, &mut ptr, &mut length) == 0 {
                let error = io::Error::last_os_error();
                WTSVirtualChannelClose(channel);
                return Err(error);
            }
            if length as usize != size_of::<HANDLE>() || ptr.is_null() {
                if !ptr.is_null() {
                    WTSFreeMemory(ptr);
                }
                WTSVirtualChannelClose(channel);
                return Err(io::Error::other(
                    "WTSVirtualFileHandle returned unexpected size",
                ));
            }
            let file = *(ptr as *const HANDLE);
            WTSFreeMemory(ptr);
            Ok(Self { channel, file })
        }
    }
    /// Each operation owns its event/OVERLAPPED/buffer until completion. Cancellation
    /// that the kernel cannot complete within a second terminates this bridge rather
    /// than freeing a still-referenced buffer or hanging shutdown indefinitely.
    fn operation(&self, buffer: &mut [u8], write: bool, timeout_ms: u32) -> io::Result<usize> {
        unsafe {
            let event = CreateEventW(ptr::null(), 1, 0, ptr::null());
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = event;
            let mut count = 0;
            let success = if write {
                WriteFile(
                    self.file,
                    buffer.as_ptr(),
                    buffer.len() as u32,
                    &mut count,
                    &mut ov,
                )
            } else {
                ReadFile(
                    self.file,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut count,
                    &mut ov,
                )
            };
            let result = if success != 0 {
                Ok(count as usize)
            } else {
                let code = GetLastError();
                if code != ERROR_IO_PENDING {
                    Err(io::Error::from_raw_os_error(code as i32))
                } else if GetOverlappedResultEx(self.file, &ov, &mut count, timeout_ms, 0) != 0 {
                    Ok(count as usize)
                } else {
                    let code = GetLastError();
                    CancelIoEx(self.file, &ov);
                    if GetOverlappedResultEx(self.file, &ov, &mut count, 1000, 0) != 0 {
                        // Completion raced cancellation: preserve the transferred bytes.
                        Ok(count as usize)
                    } else {
                        if GetLastError() != ERROR_OPERATION_ABORTED {
                            // Outstanding kernel references make returning unsafe. OS closes the job handle.
                            std::process::abort();
                        }
                        Err(if code == WAIT_TIMEOUT {
                            io::Error::new(io::ErrorKind::TimedOut, "WTS I/O deadline")
                        } else {
                            io::Error::from_raw_os_error(code as i32)
                        })
                    }
                }
            };
            CloseHandle(event);
            result
        }
    }
}

pub struct Carrier {
    pub incoming: mpsc::Receiver<Envelope>,
    pub outgoing: mpsc::Sender<Envelope>,
    stop: Arc<AtomicBool>,
}
impl Drop for Carrier {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn open_carrier() -> io::Result<Carrier> {
    let channel = Arc::new(Channel::open()?);
    let stop = Arc::new(AtomicBool::new(false));
    let (input_tx, incoming) = mpsc::channel(QUEUE_DEPTH);
    let (outgoing, mut output_rx) = mpsc::channel::<Envelope>(QUEUE_DEPTH);
    let read_channel = channel.clone();
    let read_stop = stop.clone();
    std::thread::spawn(move || {
        let mut decoder = Decoder::new();
        let mut pdu = crate::wts_framing::PduDecoder::default();
        let mut buffer = [0u8; 1608];
        while !read_stop.load(Ordering::Relaxed) {
            let n = match read_channel.operation(&mut buffer, false, 200) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                Err(e) => {
                    eprintln!("WTS read: {e}");
                    break;
                }
            };
            let data = match pdu.push(&buffer[..n]) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("WTS header: {e}");
                    break;
                }
            };
            match decoder.push(data) {
                Ok(frames) => {
                    for frame in frames {
                        if input_tx.try_send(frame).is_err() {
                            read_stop.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("WTS envelope: {e}");
                    break;
                }
            }
        }
        read_stop.store(true, Ordering::Relaxed);
    });
    let write_stop = stop.clone();
    std::thread::spawn(move || {
        while !write_stop.load(Ordering::Relaxed) {
            // Polling only this blocking worker permits close even when no caller writes.
            let frame = match output_rx.try_recv() {
                Ok(frame) => frame,
                Err(mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            };
            let mut bytes = match encode(&frame) {
                Ok(bytes) => bytes,
                Err(_) => break,
            };
            let started = std::time::Instant::now();
            for chunk in bytes.chunks_mut(1600) {
                if started.elapsed() > Duration::from_secs(5) {
                    write_stop.store(true, Ordering::Relaxed);
                    return;
                }
                match channel.operation(chunk, true, 1000) {
                    Ok(n) if n == chunk.len() => {}
                    Ok(n) => {
                        eprintln!(
                            "WTS short write: {n}/{}; attachment retired, no retry",
                            chunk.len()
                        );
                        write_stop.store(true, Ordering::Relaxed);
                        return;
                    }
                    Err(e) => {
                        eprintln!("WTS write: {e}");
                        write_stop.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
        }
        write_stop.store(true, Ordering::Relaxed);
    });
    Ok(Carrier {
        incoming,
        outgoing,
        stop,
    })
}
