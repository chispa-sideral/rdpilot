//! Exercises daemon autostart through the real shared transport and daemon IPC.
//!
//! Linux-only because cleanup uses pidfds to retain process identity across
//! exit and PID reuse. The production process-group change remains Unix-wide.
#![cfg(target_os = "linux")]

use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rdpilot_ipc::{
    connect_existing, connect_or_spawn, read_frame, write_frame, Request, SessionLifecycle,
    WireKey, WireKeyAction, WireResponse,
};
use tokio::net::UnixStream;

const ROLE_ENV: &str = "RDPILOT_AUTOSTART_GROUP_HELPER";
const ROLE: &str = "launch-real-daemon";
const READY_TIMEOUT: Duration = Duration::from_secs(12);

/// This test is also launched as a helper by the parent. The ordinary Cargo
/// invocation has no role marker and exits without starting any processes.
#[test]
fn launcher_helper() {
    if std::env::var(ROLE_ENV).as_deref() != Ok(ROLE) {
        return;
    }
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .and_then(|runtime| runtime.block_on(launch_real_daemon()));
    if let Err(error) = result {
        eprintln!("launcher fixture failed: {error}");
        std::process::exit(71);
    }
}

async fn launch_real_daemon() -> io::Result<()> {
    let socket = PathBuf::from(required_env("RDPILOT_AUTOSTART_SOCKET")?);
    let ready = PathBuf::from(required_env("RDPILOT_AUTOSTART_READY")?);
    let shim = PathBuf::from(required_env("RDPILOT_AUTOSTART_SHIM")?);

    let mut stream = tokio::time::timeout(READY_TIMEOUT, connect_or_spawn(&socket, &shim))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "real autostart did not connect"))?
        .map_err(|error| io::Error::other(format!("real autostart failed: {error}")))?;

    let connected = exchange(
        &mut stream,
        Request::Connect {
            name: Some("process-group-regression".to_owned()),
            host: "fake-process-group-test".to_owned(),
            port: None,
            username: "test-user".to_owned(),
            password: "fixture-only".to_owned(),
            domain: None,
            accept_invalid_certs: false,
            cua_enabled: false,
            cua_version: "latest-dev".to_owned(),
            cua_auto_download: true,
            connect_ack: false,
            record: None,
        },
    )
    .await?;
    let session = match connected {
        WireResponse::Connected { session, .. } => session,
        other => return Err(io::Error::other(format!("Connect returned {other:?}"))),
    };

    let listed = exchange(&mut stream, Request::List {}).await?;
    match listed {
        WireResponse::SessionList { sessions, .. }
            if sessions.len() == 1
                && sessions[0].id == session.as_str()
                && sessions[0].status == SessionLifecycle::Live => {}
        other => {
            return Err(io::Error::other(format!(
                "initial List was not Live: {other:?}"
            )))
        }
    }

    let screenshot = exchange(
        &mut stream,
        Request::Screenshot {
            session: session.clone(),
        },
    )
    .await?;
    match screenshot {
        WireResponse::Screenshot { png_base64 } if !png_base64.is_empty() => {}
        other => {
            return Err(io::Error::other(format!(
                "initial screenshot failed: {other:?}"
            )))
        }
    }
    drop(stream);

    atomic_write(&ready, session.as_str().as_bytes())?;
    eprintln!(
        "readiness complete: initial Live, nonempty screenshot, session {}",
        session.as_str()
    );

    // The parent destroys this launcher's owned process group after readiness.
    // This deadline is a fixture backstop if the supervisor itself fails.
    tokio::time::sleep(Duration::from_secs(30)).await;
    Ok(())
}

async fn exchange(stream: &mut UnixStream, request: Request) -> io::Result<WireResponse> {
    tokio::time::timeout(Duration::from_secs(5), async {
        write_frame(stream, &request)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        read_frame(stream)
            .await
            .map_err(|error| io::Error::other(error.to_string()))
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "IPC round trip exceeded five seconds",
        )
    })?
}

fn required_env(name: &str) -> io::Result<String> {
    std::env::var(name).map_err(|_| io::Error::new(io::ErrorKind::NotFound, name))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(temp, path)
}

struct OwnedLauncher {
    child: Child,
    pgid: libc::pid_t,
    pidfd: Option<OwnedFd>,
    finished: bool,
}

impl OwnedLauncher {
    fn terminate_group_and_reap(&mut self) -> io::Result<ExitStatus> {
        if self.pgid <= 0 {
            return Err(io::Error::other("launcher process group was not validated"));
        }
        let pidfd = self
            .pidfd
            .as_ref()
            .ok_or_else(|| io::Error::other("launcher pidfd was not acquired"))?;
        let term = signal_group(self.pgid, libc::SIGTERM);
        eprintln!("owned process-group SIGTERM result: {term:?}");
        term?;
        wait_pidfd(pidfd.as_raw_fd(), Duration::from_millis(150));
        // Keep the child unreaped through every PGID signal so its zombie
        // continues to pin this owned group identifier.
        let kill = signal_group(self.pgid, libc::SIGKILL);
        eprintln!("owned process-group SIGKILL result: {kill:?}");
        kill?;
        if !wait_pidfd(pidfd.as_raw_fd(), Duration::from_secs(5)) {
            signal_pidfd(pidfd.as_raw_fd(), libc::SIGKILL)?;
            if !wait_pidfd(pidfd.as_raw_fd(), Duration::from_secs(5)) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "launcher did not exit after bounded group and pidfd signals",
                ));
            }
        }
        let status = self.child.try_wait()?.ok_or_else(|| {
            io::Error::other("launcher pidfd reported exit but child status was unavailable")
        })?;
        self.finished = true;
        Ok(status)
    }
}

impl Drop for OwnedLauncher {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.pgid > 0 {
            let _ = signal_group(self.pgid, libc::SIGTERM);
            std::thread::sleep(Duration::from_millis(100));
            let _ = signal_group(self.pgid, libc::SIGKILL);
        }
        if let Some(pidfd) = &self.pidfd {
            if !wait_pidfd(pidfd.as_raw_fd(), Duration::from_secs(2)) {
                let _ = signal_pidfd(pidfd.as_raw_fd(), libc::SIGKILL);
            }
            if wait_pidfd(pidfd.as_raw_fd(), Duration::from_secs(2)) {
                let _ = self.child.try_wait();
            }
        } else {
            let _ = self.child.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
        self.finished = true;
    }
}

struct OwnedDaemon {
    pid: libc::pid_t,
    pidfd: OwnedFd,
    cleaned: bool,
}

impl OwnedDaemon {
    fn cleanup(&mut self) -> io::Result<()> {
        signal_pidfd(self.pidfd.as_raw_fd(), libc::SIGTERM)?;
        if !wait_pidfd(self.pidfd.as_raw_fd(), Duration::from_secs(2)) {
            signal_pidfd(self.pidfd.as_raw_fd(), libc::SIGKILL)?;
        }
        if !wait_pidfd(self.pidfd.as_raw_fd(), Duration::from_secs(3)) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("owned daemon {} did not exit after pidfd signals", self.pid),
            ));
        }
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = signal_pidfd(self.pidfd.as_raw_fd(), libc::SIGKILL);
            let _ = wait_pidfd(self.pidfd.as_raw_fd(), Duration::from_secs(2));
            self.cleaned = true;
        }
    }
}

#[tokio::test]
async fn autostart_daemon_survives_teardown_of_the_callers_process_group() {
    let root = tempfile::tempdir().expect("create private fixture directory");
    let runtime = root.path().join("runtime");
    let config_home = root.path().join("config");
    let cache_home = root.path().join("cache");
    let data_home = root.path().join("data");
    let recordings = root.path().join("recordings");
    fs::create_dir_all(&runtime).expect("create isolated runtime directory");
    fs::create_dir_all(&config_home).expect("create isolated config directory");
    fs::create_dir_all(&cache_home).expect("create isolated cache directory");
    fs::create_dir_all(&data_home).expect("create isolated data directory");
    fs::create_dir_all(&recordings).expect("create isolated recording directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        .expect("make isolated runtime private");
    let socket = runtime.join("rdpilot").join("daemon.sock");
    let ready = root.path().join("ready");
    let pid_file = root.path().join("daemon.pid");
    let ack = root.path().join("identity-acquired");
    let shim = root.path().join("daemon-shim");
    let sink = root.path().join("sessions.json");
    let launcher_log_path = root.path().join("launcher.log");
    let launcher_log = File::create(&launcher_log_path).expect("create launcher log");

    // Fixed shell text; all paths and executable names arrive through the
    // environment. Publication must complete before the daemon can exec.
    fs::write(
        &shim,
        b"#!/bin/sh\nset -eu\ntmp=\"${RDPILOT_AUTOSTART_PID_FILE}.tmp.$$\"\nprintf '%s\\n' \"$$\" > \"$tmp\"\nmv \"$tmp\" \"$RDPILOT_AUTOSTART_PID_FILE\"\ni=0\nwhile [ ! -f \"$RDPILOT_AUTOSTART_IDENTITY_ACK\" ]; do\n  i=$((i + 1))\n  [ \"$i\" -lt 1000 ] || exit 70\n  sleep 0.01\ndone\nexec \"$RDPILOT_AUTOSTART_DAEMON\"\n",
    )
    .expect("write fail-closed daemon shim");
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700))
        .expect("make daemon shim executable");

    let current_exe = std::env::current_exe().expect("resolve integration test executable");
    let daemon_exe = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-daemon"));
    let mut command = Command::new(current_exe);
    command
        .arg("--exact")
        .arg("launcher_helper")
        .arg("--nocapture")
        .env(ROLE_ENV, ROLE)
        .env("RDPILOT_AUTOSTART_RUNTIME", &runtime)
        .env("RDPILOT_AUTOSTART_SOCKET", &socket)
        .env("RDPILOT_AUTOSTART_READY", &ready)
        .env("RDPILOT_AUTOSTART_SHIM", &shim)
        .env("RDPILOT_AUTOSTART_DAEMON", &daemon_exe)
        .env("RDPILOT_AUTOSTART_PID_FILE", &pid_file)
        .env("RDPILOT_AUTOSTART_IDENTITY_ACK", &ack)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("RDPILOT_DAEMON_SINK_PATH", &sink)
        .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "3600000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "3600000")
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("RDPILOT_RECORDING__ENABLED", "false")
        .env("RDPILOT_RECORDING__DIR", &recordings)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            launcher_log.try_clone().expect("clone launcher log"),
        ))
        .stderr(Stdio::from(
            launcher_log.try_clone().expect("clone launcher log"),
        ));
    command.process_group(0);
    let child = command.spawn().expect("spawn owned launcher group");
    // Install child ownership before any identity query or assertion can fail.
    let mut launcher = OwnedLauncher {
        child,
        pgid: 0,
        pidfd: None,
        finished: false,
    };
    let launcher_pid = launcher.child.id() as libc::pid_t;
    let launcher_pgid = launcher_pid;
    let supervisor_pgid = unsafe { libc::getpgrp() };
    assert!(launcher_pgid > 0, "launcher group ID must be positive");
    let observed_launcher_pgid = unsafe { libc::getpgid(launcher_pid) };
    assert_eq!(
        observed_launcher_pgid, launcher_pgid,
        "spawned launcher must lead its exclusively owned process group"
    );
    assert_ne!(
        observed_launcher_pgid, supervisor_pgid,
        "launcher group must differ from supervisor group"
    );
    launcher.pgid = observed_launcher_pgid;
    launcher.pidfd = Some(pidfd_open(launcher_pid).expect("retain launcher identity immediately"));

    let fixture = (|| -> io::Result<(OwnedDaemon, String, libc::pid_t)> {
        wait_for_file(&pid_file, READY_TIMEOUT, &launcher)?;
        let raw_pid = fs::read_to_string(&pid_file)?;
        let pid = parse_single_pid(&raw_pid)?;
        let pidfd = pidfd_open(pid)?;
        if wait_pidfd(pidfd.as_raw_fd(), Duration::ZERO) {
            return Err(io::Error::other(
                "shim exited before its identity was acquired",
            ));
        }
        verify_shim_identity(pid, launcher_pid, &shim)?;
        if wait_pidfd(pidfd.as_raw_fd(), Duration::ZERO) {
            return Err(io::Error::other(
                "shim exited while its private process identity was checked",
            ));
        }
        let mut daemon = OwnedDaemon {
            pid,
            pidfd,
            cleaned: false,
        };
        let observed_pgid = unsafe { libc::getpgid(pid) };
        if observed_pgid <= 0 || observed_pgid == supervisor_pgid {
            return Err(io::Error::other(format!(
                "owned shim has invalid or supervisor group {observed_pgid}"
            )));
        }
        if observed_pgid != launcher_pgid && observed_pgid != pid {
            return Err(io::Error::other(format!(
                "shim group {observed_pgid} is neither launcher group {launcher_pgid} nor its own PID {pid}"
            )));
        }
        atomic_write(&ack, b"owned pidfd ready")?;
        eprintln!(
            "identity acquired: daemon pid {pid}, process group {observed_pgid}; waiting for semantic readiness"
        );
        wait_for_file(&ready, READY_TIMEOUT, &launcher)?;
        let session = fs::read_to_string(&ready)?;
        if session.trim().is_empty() || session.contains('\n') {
            daemon.cleanup()?;
            return Err(io::Error::other("readiness had malformed session identity"));
        }
        eprintln!(
            "semantic readiness complete: initial Live and nonempty screenshot; session {}",
            session.trim()
        );
        Ok((daemon, session.trim().to_owned(), observed_pgid))
    })();

    let (mut daemon, session, daemon_pgid) = match fixture {
        Ok(value) => value,
        Err(error) => {
            let _ = launcher.terminate_group_and_reap();
            let helper_log = fs::read_to_string(&launcher_log_path).unwrap_or_default();
            panic!("fixture failed before semantic readiness: {error}; helper log: {helper_log}");
        }
    };

    eprintln!("sending SIGTERM to owned launcher group {launcher_pgid}");
    let launcher_status = launcher
        .terminate_group_and_reap()
        .expect("signal owned group before reaping its launcher");
    eprintln!("launcher group teardown completed: {launcher_status}");

    let daemon_exited_after_group_teardown =
        wait_pidfd(daemon.pidfd.as_raw_fd(), Duration::from_millis(500));
    let daemon_alive_after_group_teardown = !daemon_exited_after_group_teardown;
    let after = post_teardown_requests(&socket, &session).await;
    let daemon_alive_after_operations = !wait_pidfd(daemon.pidfd.as_raw_fd(), Duration::ZERO);
    eprintln!(
        "post-teardown original daemon identity: pid={}, group={}, isolated_from_launcher={}, pidfd_alive_after_500ms={daemon_alive_after_group_teardown}, pidfd_alive_after_operations={daemon_alive_after_operations}",
        daemon.pid,
        daemon_pgid,
        daemon_pgid == daemon.pid && daemon_pgid != launcher_pgid,
    );
    eprintln!("post-teardown same-session operation result: {after:?}");
    let cleanup = daemon.cleanup();
    eprintln!("owned daemon pidfd cleanup: {cleanup:?}");

    cleanup.expect("success path and unwind cleanup must stop the exact daemon");
    let semantic_success = matches!(
        after,
        Ok(PostTeardownResult {
            key_ack: true,
            screenshot_nonempty: true,
            session_live: true,
        })
    ) && daemon_alive_after_group_teardown
        && daemon_alive_after_operations
        && daemon_pgid == daemon.pid
        && daemon_pgid != launcher_pgid;
    assert!(
        semantic_success,
        "the same session must accept input, screenshot and remain Live after caller-group teardown; see {}",
        launcher_log_path.display()
    );
    eprintln!("cleanup complete; same daemon session survived every post-teardown operation");
}

#[derive(Debug)]
struct PostTeardownResult {
    key_ack: bool,
    screenshot_nonempty: bool,
    session_live: bool,
}

async fn post_teardown_requests(
    socket: &Path,
    expected_session: &str,
) -> io::Result<PostTeardownResult> {
    let mut stream = tokio::time::timeout(Duration::from_secs(5), connect_existing(socket))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "post-teardown connect timed out"))?
        .map_err(|error| io::Error::other(format!("same-session IPC unavailable: {error}")))?;
    let session: rdpilot_ipc::SessionId = expected_session
        .parse()
        .map_err(|error| io::Error::other(format!("invalid ready session ID: {error}")))?;

    let key = exchange(
        &mut stream,
        Request::Key {
            session: session.clone(),
            action: WireKeyAction::Combo(vec![WireKey::Win, WireKey::R]),
        },
    )
    .await?;
    let key_ack = matches!(key, WireResponse::Ack);
    if !key_ack {
        return Err(io::Error::other(format!(
            "same-session input failed: {key:?}"
        )));
    }

    let screenshot = exchange(
        &mut stream,
        Request::Screenshot {
            session: session.clone(),
        },
    )
    .await?;
    let screenshot_nonempty = matches!(
        screenshot,
        WireResponse::Screenshot { ref png_base64 } if !png_base64.is_empty()
    );
    if !screenshot_nonempty {
        return Err(io::Error::other(format!(
            "same-session screenshot failed: {screenshot:?}"
        )));
    }

    let listed = exchange(&mut stream, Request::List {}).await?;
    let session_live = match listed {
        WireResponse::SessionList { sessions, .. } => sessions
            .iter()
            .any(|status| status.id == expected_session && status.status == SessionLifecycle::Live),
        other => {
            return Err(io::Error::other(format!(
                "same-session List failed: {other:?}"
            )))
        }
    };
    if !session_live {
        return Err(io::Error::other("same session did not remain Live"));
    }
    Ok(PostTeardownResult {
        key_ack,
        screenshot_nonempty,
        session_live,
    })
}

fn wait_for_file(path: &Path, timeout: Duration, launcher: &OwnedLauncher) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if path.is_file() {
            return Ok(());
        }
        if launcher
            .pidfd
            .as_ref()
            .is_some_and(|pidfd| wait_pidfd(pidfd.as_raw_fd(), Duration::ZERO))
        {
            return Err(io::Error::other(format!(
                "launcher exited before {} appeared",
                path.display()
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{} was not published before its deadline", path.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn verify_shim_identity(pid: libc::pid_t, launcher: libc::pid_t, shim: &Path) -> io::Result<()> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let parent = status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:").map(str::trim))
        .ok_or_else(|| io::Error::other("shim /proc status omitted PPid"))?
        .parse::<libc::pid_t>()
        .map_err(|error| io::Error::other(format!("invalid shim PPid: {error}")))?;
    if parent != launcher {
        return Err(io::Error::other(format!(
            "PID record belongs to process {pid} with unexpected parent {parent}"
        )));
    }
    let command_line = fs::read(format!("/proc/{pid}/cmdline"))?;
    use std::os::unix::ffi::OsStrExt;
    if !command_line
        .split(|byte| *byte == 0)
        .any(|arg| arg == shim.as_os_str().as_bytes())
    {
        return Err(io::Error::other(
            "recorded process is not executing the exact private daemon shim",
        ));
    }
    Ok(())
}

fn parse_single_pid(raw: &str) -> io::Result<libc::pid_t> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.bytes().any(|byte| !byte.is_ascii_digit()) {
        return Err(io::Error::other("daemon identity record is malformed"));
    }
    let pid = trimmed
        .parse::<libc::pid_t>()
        .map_err(|error| io::Error::other(format!("daemon PID is invalid: {error}")))?;
    if pid <= 0 {
        return Err(io::Error::other("daemon PID must be positive"));
    }
    Ok(pid)
}

fn pidfd_open(pid: libc::pid_t) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

fn signal_pidfd(pidfd: i32, signal: libc::c_int) -> io::Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd,
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0_u32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

fn signal_group(pgid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    if pgid <= 0 {
        return Err(io::Error::other("refusing to signal a nonpositive PGID"));
    }
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

fn wait_pidfd(pidfd: i32, timeout: Duration) -> bool {
    let mut pollfd = libc::pollfd {
        fd: pidfd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    let result = unsafe { libc::poll(&mut pollfd, 1, millis) };
    result > 0 && pollfd.revents & libc::POLLIN != 0
}
