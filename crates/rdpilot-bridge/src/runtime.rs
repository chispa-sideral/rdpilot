use rdpilot_bridge_protocol::{
    encode, BundleManifest, Envelope, Message, CUA_DRIVER_EXE_NAME, MANIFEST_NAME, MAX_FRAME_BYTES,
    PROTOCOL_VERSION, QUEUE_DEPTH,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{self, Instant},
};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const PIPE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_PENDING: usize = 256;
const STDERR_TAIL_BYTES: usize = 8192;

#[derive(Clone)]
pub struct Config {
    pub generation: u64,
    /// Identity of the installed bundle; Hello must name the same bundle.
    pub bundle_id: String,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub transfer_root: PathBuf,
    pub share_root: PathBuf,
    pub request_timeout: Duration,
}
impl Config {
    /// Run the Cua driver installed next to this executable, identified by
    /// the adjacent manifest.
    pub fn adjacent(generation: u64) -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        let executable = exe.with_file_name(CUA_DRIVER_EXE_NAME);
        let manifest: BundleManifest =
            serde_json::from_slice(&std::fs::read(exe.with_file_name(MANIFEST_NAME))?)?;
        manifest.validate().map_err(error)?;
        Ok(Self {
            generation,
            bundle_id: manifest.bundle_id,
            executable,
            args: vec!["mcp".into(), "--direct".into()],
            transfer_root: std::env::temp_dir().join("rdpilot-transfer-root"),
            share_root: PathBuf::from(r"\\tsclient\RDPILOT"),
            request_timeout: REQUEST_TIMEOUT,
        })
    }
}

fn error(reason: impl Into<String>) -> io::Error {
    io::Error::other(reason.into())
}
fn emit(
    output: &mpsc::Sender<Envelope>,
    generation: u64,
    control_id: u64,
    message: Message,
) -> io::Result<()> {
    let envelope = Envelope::new(generation, control_id, message);
    encode(&envelope)?; // Outer overhead counts toward the limit, including opaque MCP strings.
    output
        .try_send(envelope)
        .map_err(|_| error("carrier output closed or saturated"))
}

struct Attachment {
    id: u64,
    generation: u64,
    input: mpsc::Sender<(Instant, Value)>,
    output: mpsc::Receiver<Value>,
    task: JoinHandle<io::Result<()>>,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One independently scheduled control loop per RDP connection. No desktop logic.
/// Input EOF or outbound congestion retires the carrier; the caller must explicitly reattach.
pub async fn run(
    config: Config,
    mut input: mpsc::Receiver<Envelope>,
    output: mpsc::Sender<Envelope>,
) -> io::Result<()> {
    let mut ready = false;
    let mut active: Option<Attachment> = None;
    let mut runtime_generation = 0u64;
    let mut last_attachment_id = 0u64;
    let mut last_control = Instant::now();
    let mut heartbeat = time::interval(Duration::from_millis(100));
    let mut transfer: Option<(
        u64,
        Instant,
        Arc<AtomicBool>,
        JoinHandle<rdpilot_bridge_protocol::FileTransferResponse>,
    )> = None;
    let result = async {
        loop {
            tokio::select! {
                biased;
                incoming = input.recv() => {
                    let Some(envelope) = incoming else { return Ok(()); };
                    if envelope.generation != config.generation { continue; }
                    if envelope.version != PROTOCOL_VERSION { return Err(error("unsupported bridge version")); }
                    last_control = Instant::now();
                    let id = envelope.control_id;
                    match envelope.message {
                        Message::Hello { bundle_id } => {
                            if bundle_id != config.bundle_id {
                                return Err(error(format!("Cua bundle mismatch: installed {}, daemon wants {bundle_id}", config.bundle_id)));
                            }
                            ready = true;
                            emit(&output,config.generation,id,Message::Ready {bundle_id:config.bundle_id.clone(),bridge_version:env!("CARGO_PKG_VERSION").into()})?;
                        }
                        Message::Ping if ready => emit(&output,config.generation,id,Message::Pong)?,
                        Message::Open {attachment_id} if ready => {
                            if active.is_some() {
                                emit(&output,config.generation,id,Message::Error {reason:"Cua attachment busy".into()})?;
                                continue;
                            }
                            if attachment_id <= last_attachment_id {
                                emit(&output,config.generation,id,Message::Error {reason:"attachment ID must increase within bridge generation".into()})?;
                                continue;
                            }
                            last_attachment_id = attachment_id;
                            runtime_generation = runtime_generation.checked_add(1).ok_or_else(|| error("generation exhausted"))?;
                            // spawn is synchronous and establishes containment before resuming Windows execution.
                            match spawn(&config) {
                                Ok((child,guard)) => {
                                    let (tx,rx) = mpsc::channel(QUEUE_DEPTH);
                                    let (out_tx,out_rx) = mpsc::channel(QUEUE_DEPTH);
                                    let timeout = config.request_timeout;
                                    let task = tokio::spawn(child_loop(child,guard,rx,out_tx,timeout));
                                    active=Some(Attachment {id:attachment_id,generation:runtime_generation,input:tx,output:out_rx,task});
                                    emit(&output,config.generation,id,Message::Opened {attachment_id,runtime_generation})?;
                                }
                                Err(e) => emit(&output,config.generation,id,Message::Error {reason:format!("Cua spawn/containment failed: {e}")})?,
                            }
                        }
                        Message::Mcp {attachment_id,runtime_generation,message} if ready => {
                            if let Some(a) = active.as_ref() {
                                if a.id != attachment_id || a.generation != runtime_generation { continue; }
                                if a.input.try_send((Instant::now(),message)).is_err() {
                                    retire(&mut active).await;
                                    emit(&output,config.generation,0,Message::Closed {attachment_id,reason:"Cua input queue saturated".into()})?;
                                }
                            }
                        }
                        Message::Close {attachment_id,reason} if ready => {
                            if active.as_ref().is_some_and(|a| a.id == attachment_id) {
                                retire(&mut active).await;
                                emit(&output,config.generation,id,Message::Closed {attachment_id,reason})?;
                            }
                        }
                        Message::FileTransfer(request) if ready => {
                            if transfer.is_some() {
                                emit(&output,config.generation,id,Message::Error{reason:"file transfer busy".into()})?;
                            } else {
                                let root=config.transfer_root.clone(); let share=config.share_root.clone();
                                let cancel=Arc::new(AtomicBool::new(false)); let worker_cancel=cancel.clone();
                                let task=tokio::task::spawn_blocking(move || crate::transfer::execute(&root,&share,&request,&worker_cancel));
                                transfer=Some((id,Instant::now(),cancel,task));
                            }
                        }
                        _ => emit(&output,config.generation,id,Message::Error {reason:"unexpected bridge message or Hello required".into()})?,
                    }
                }
                message = async { match active.as_mut() { Some(a)=>a.output.recv().await, None=>std::future::pending().await } } => {
                    if let Some(message)=message {
                        let a=active.as_ref().unwrap();
                        emit(&output,config.generation,0,Message::Mcp {attachment_id:a.id,runtime_generation:a.generation,message})?;
                    } else {
                        let mut a=active.take().unwrap(); let id=a.id;
                        let result=(&mut a.task).await; // completion reason is diagnostic only; never an MCP result
                        let reason=match result { Ok(Err(e))=>e.to_string(), Ok(Ok(()))=>"Cua exited".into(), Err(e)=>e.to_string() };
                        emit(&output,config.generation,0,Message::Closed{attachment_id:id,reason})?;
                    }
                }
                _ = heartbeat.tick() => {
                    if last_control.elapsed() > HEARTBEAT_TIMEOUT { return Err(error("RDP carrier heartbeat expired")); }
                    if transfer.as_ref().is_some_and(|(_,start,_,_)|start.elapsed()>REQUEST_TIMEOUT) { return Err(error("file transfer deadline expired")); }
                    if transfer.as_ref().is_some_and(|(_,_,_,task)|task.is_finished()) {
                        let (id,_,_,task)=transfer.take().unwrap();
                        let result=task.await.map_err(|e|error(e.to_string()))?;
                        emit(&output,config.generation,id,Message::FileTransferResult(result))?;
                    }
                }
            }
        }
    }.await;
    retire(&mut active).await;
    if let Some((_, _, cancel, task)) = transfer.take() {
        cancel.store(true, Ordering::Relaxed);
        task.abort();
    }
    result
}

async fn retire(active: &mut Option<Attachment>) {
    if let Some(mut a) = active.take() {
        // Abort the supervisor: its guard kills the complete child tree immediately.
        a.task.abort();
        let _ = time::timeout(CLOSE_TIMEOUT, &mut a.task).await;
    }
}

#[cfg(unix)]
struct ProcessGuard(u32);
#[cfg(unix)]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
#[cfg(windows)]
type ProcessGuard = crate::windows::Job;
#[cfg(not(any(unix, windows)))]
struct ProcessGuard;

fn spawn(config: &Config) -> io::Result<(Child, ProcessGuard)> {
    let mut command = Command::new(&config.executable);
    command
        .args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(
            windows_sys::Win32::System::Threading::CREATE_SUSPENDED
                | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
        );
    }
    let child = command.spawn()?;
    #[cfg(unix)]
    let guard = ProcessGuard(child.id().unwrap());
    #[cfg(windows)]
    let guard = unsafe {
        // Child owns this handle and was created suspended immediately above.
        crate::windows::Job::contain_and_resume(
            child
                .raw_handle()
                .ok_or_else(|| error("missing Cua process handle"))?,
        )?
    };
    #[cfg(not(any(unix, windows)))]
    let guard = ProcessGuard;
    Ok((child, guard))
}

/// Read newline JSON without allowing read_line to allocate an unbounded string.
pub async fn read_json<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> io::Result<Option<Value>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(error("truncated Cua JSON line"))
            };
        }
        let end = buf.iter().position(|&b| b == b'\n').map(|i| i + 1);
        let take = end.unwrap_or(buf.len());
        if line.len() + take > MAX_FRAME_BYTES {
            return Err(error("Cua JSON line exceeds frame ceiling"));
        }
        line.extend_from_slice(&buf[..take]);
        reader.consume(take);
        if end.is_some() {
            return serde_json::from_slice(&line).map(Some).map_err(Into::into);
        }
    }
}

struct Tasks(Vec<JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

async fn child_loop(
    mut child: Child,
    _guard: ProcessGuard,
    mut input: mpsc::Receiver<(Instant, Value)>,
    output: mpsc::Sender<Value>,
    request_timeout: Duration,
) -> io::Result<()> {
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (lines_tx, mut lines_rx) = mpsc::channel::<io::Result<Value>>(QUEUE_DEPTH);
    let (writes_tx, mut writes_rx) = mpsc::channel::<Value>(QUEUE_DEPTH);
    let (failure_tx, mut failure_rx) = mpsc::channel::<String>(1);
    let failure_write = failure_tx.clone();
    let mut tasks = Tasks(Vec::new());
    tasks.0.push(tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_json(&mut reader).await {
                Ok(Some(value)) => {
                    if lines_tx.try_send(Ok(value)).is_err() {
                        let _ = failure_tx.try_send("Cua output queue saturated".into());
                        return;
                    }
                }
                Ok(None) => {
                    let _ = failure_tx.try_send("Cua stdout EOF".into());
                    return;
                }
                Err(e) => {
                    let _ = failure_tx.try_send(e.to_string());
                    return;
                }
            }
        }
    }));
    tasks.0.push(tokio::spawn(async move {
        while let Some(value) = writes_rx.recv().await {
            let mut line = match serde_json::to_vec(&value) {
                Ok(line) => line,
                Err(e) => {
                    let _ = failure_write.try_send(e.to_string());
                    return;
                }
            };
            line.push(b'\n');
            let result = time::timeout(PIPE_TIMEOUT, stdin.write_all(&line)).await;
            if !matches!(result, Ok(Ok(()))) {
                let _ = failure_write.try_send("Cua stdin write failed or timed out".into());
                return;
            }
        }
    }));
    tasks.0.push(tokio::spawn(async move {
        let mut tail = std::collections::VecDeque::with_capacity(STDERR_TAIL_BYTES);
        let mut buffer = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            for &byte in &buffer[..n] {
                if tail.len() == STDERR_TAIL_BYTES {
                    tail.pop_front();
                }
                tail.push_back(byte);
            }
        }
        // Deliberately never copy child stderr onto MCP stdout or an unbounded logger.
    }));
    let mut pending: HashMap<(bool, String), Instant> = HashMap::new();
    let mut timer = time::interval(Duration::from_millis(20));
    let result = loop {
        tokio::select! {
            _=timer.tick()=> {
                if pending.values().any(|at|at.elapsed()>request_timeout) {break Err(error("MCP request deadline expired; completion unknown, not replayed"));}
            }
            reason=failure_rx.recv()=>{break Err(error(reason.unwrap_or_else(||"Cua pipes closed".into())));}
            status=child.wait()=>{break Err(error(format!("Cua process exited: {status:?}")));}
            incoming=input.recv()=> {
                let Some((at,message))=incoming else {break Ok(());};
                if let Err(e)=track(&mut pending,false,&message,at) {break Err(e);}
                if at.elapsed()>request_timeout {break Err(error("queued MCP request deadline expired"));}
                if writes_tx.try_send(message).is_err() {break Err(error("Cua stdin queue saturated"));}
            }
            incoming=lines_rx.recv()=> {
                let Some(message)=incoming else {break Err(error("Cua stdout closed"));};
                let message=match message {Ok(v)=>v,Err(e)=>break Err(e)};
                if let Err(e)=track(&mut pending,true,&message,Instant::now()) {break Err(e);}
                if output.try_send(message).is_err() {break Err(error("MCP caller output saturated"));}
            }
        }
    };
    drop(tasks);
    drop(_guard); // kill contained descendants even when root process already exited
    let _ = child.start_kill();
    let _ = time::timeout(CLOSE_TIMEOUT, child.wait()).await;
    result
}

fn track(
    pending: &mut HashMap<(bool, String), Instant>,
    from_child: bool,
    message: &Value,
    at: Instant,
) -> io::Result<()> {
    if let Some(id) = message.get("id") {
        let id = serde_json::to_string(id)?;
        if message.get("method").is_some() {
            if pending.len() >= MAX_PENDING || pending.insert((from_child, id), at).is_some() {
                return Err(error("too many or duplicate pending MCP requests"));
            }
        } else if message.get("result").is_some() || message.get("error").is_some() {
            pending.remove(&(!from_child, id));
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };

    #[tokio::test]
    async fn job_closes_root_and_descendant_created_after_resume() {
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let config=Config {
            generation:1, bundle_id:"test".into(), executable:powershell,
            args:vec!["-NoProfile".into(),"-NonInteractive".into(),"-Command".into(),r#"$p=Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 60' -WindowStyle Hidden -PassThru; @{method='pids';params=@($PID,$p.Id)} | ConvertTo-Json -Compress; Start-Sleep -Seconds 60"#.into()],
            transfer_root:std::env::temp_dir(),share_root:std::env::temp_dir(),request_timeout:REQUEST_TIMEOUT,
        };
        let (mut child, job) = spawn(&config).unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let message = time::timeout(Duration::from_secs(15), read_json(&mut reader))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let pids = message["params"].as_array().unwrap();
        let handles: Vec<_> = pids
            .iter()
            .map(|id| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, id.as_u64().unwrap() as u32) })
            .collect();
        assert!(handles.iter().all(|handle| !handle.is_null()));
        drop(job);
        for handle in handles {
            unsafe {
                assert_eq!(WaitForSingleObject(handle, 3000), WAIT_OBJECT_0);
                CloseHandle(handle);
            }
        }
        time::timeout(CLOSE_TIMEOUT, child.wait())
            .await
            .unwrap()
            .unwrap();
    }
}
