//! Windows-host-only live-gated DACL / anti-squatting / cross-account
//! rejection assertions (Plan 15-01, closing `12-07-PLAN.md`'s Windows half
//! of DAEMON-02).
//!
//! `#![cfg(windows)]`-gated: this ENTIRE file compiles to nothing on the
//! Linux offline suite (mirrors `src/ipc/windows.rs`'s own inner gate) --
//! every item below, including its `#[test]` functions, is stripped by
//! rustc's `cfg` attribute (applied here at the crate-root/file level,
//! exactly like `ipc/windows.rs`'s own `#![cfg(windows)]`) before any
//! Windows-only symbol is even name-resolved, so the offline
//! `cargo build ... --tests` / `cargo test -- --list` passes trivially
//! with zero tests discovered from this file.
//!
//! Each test additionally requires `RDPILOT_LIVE=1` (D-18) AND is
//! `#[ignore]`d, so it runs ONLY under an explicit `cargo test --
//! --ignored` invocation on a genuine Windows host -- never in a default
//! `cargo test` pass, mirroring `tests/ipc_security.rs`'s
//! `RDPILOT_SECOND_UID` opt-in pattern for the Unix cross-account case.
//! Real execution happens on the pinned Azure Windows VM in Plan 15-05.
//!
//! **12-07-PLAN.md must_haves covered by this file:**
//! - "The Windows named pipe is created with an explicit owner-only DACL
//!   (never null/default)" -- proven indirectly via (c) below (this file
//!   has no crate-internal access to `OwnerOnlyDacl`'s construction, only
//!   the public `bind`/`accept_and_authorize`/`socket_path` surface).
//! - "first_pipe_instance(true) causes creation to fail loudly against a
//!   pre-existing squatted pipe" -- (b) below.
//! - "A genuinely different Windows account is rejected at the pipe
//!   boundary" -- (a) below.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateProcessWithLogonW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
    CREATE_NO_WINDOW, LOGON_WITH_PROFILE, PROCESS_INFORMATION, STARTUPINFOW,
};

/// Name of the opt-in env var that arms this live suite (D-18) -- mirrors
/// `crates/rdpilot/tests/common/mod.rs::LIVE_ENV` without depending on
/// that crate's test-support module (this is a daemon-crate integration
/// test, a separate compilation unit).
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// A second local Windows account name, already provisioned by Plan
/// 15-05's VM setup, mirroring `tests/ipc_security.rs`'s
/// `RDPILOT_SECOND_UID` pattern for the Unix cross-account case. This file
/// only CONSUMES the name via env var -- provisioning the account is Plan
/// 15-05's job.
const SECOND_ACCOUNT_ENV: &str = "RDPILOT_SECOND_WINDOWS_ACCOUNT";

/// The second account's password, used only to launch the cross-account
/// probe through PowerShell's credentialed process API.
const SECOND_ACCOUNT_PASSWORD_ENV: &str = "RDPILOT_SECOND_WINDOWS_PASSWORD";

const PROBE_WAIT_MILLIS: u32 = 20_000;
const TERMINATION_WAIT_MILLIS: u32 = 1_000;

fn cleanup_probe_artifacts(result_path: &std::path::Path, inner_script_path: &std::path::Path) {
    let _ = std::fs::remove_file(result_path);
    let _ = std::fs::remove_file(inner_script_path);
}

fn utf16z(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The exact child process launched for the cross-account probe.  Its handle
/// is consumed by `wait_terminate_inspect_close`, so no caller can forget the
/// bounded wait/termination/close sequence.
struct OwnedProbeProcess {
    // A real same-process kernel handle is opaque: retain its integer value
    // privately so this single-owner wrapper can cross Tokio's blocking-task
    // Send boundary. Convert it back only at the Windows FFI call sites.
    handle_value: usize,
}

impl OwnedProbeProcess {
    fn wait_terminate_inspect_close(self) -> Result<u32, String> {
        let first_wait =
            unsafe { WaitForSingleObject(self.handle_value as HANDLE, PROBE_WAIT_MILLIS) };
        if first_wait != WAIT_OBJECT_0 {
            let terminated = unsafe { TerminateProcess(self.handle_value as HANDLE, 1) } != 0;
            let terminal_wait = unsafe {
                WaitForSingleObject(self.handle_value as HANDLE, TERMINATION_WAIT_MILLIS)
            };
            let mut exit_code = 0;
            let exit_code_observed =
                unsafe { GetExitCodeProcess(self.handle_value as HANDLE, &mut exit_code) } != 0;
            let process_closed = unsafe { CloseHandle(self.handle_value as HANDLE) } != 0;
            return Err(format!(
                "probe did not complete (wait={first_wait}, terminated={terminated}, terminal_wait={terminal_wait}, exit_observed={exit_code_observed}, process_closed={process_closed})"
            ));
        }

        let mut exit_code = 0;
        let exit_code_observed =
            unsafe { GetExitCodeProcess(self.handle_value as HANDLE, &mut exit_code) } != 0;
        let process_closed = unsafe { CloseHandle(self.handle_value as HANDLE) } != 0;
        if !exit_code_observed || !process_closed {
            return Err(format!(
                "probe completion inspection failed (exit_observed={exit_code_observed}, process_closed={process_closed})"
            ));
        }
        Ok(exit_code)
    }
}

/// Launch the fixed PowerShell probe directly under the supplied local
/// account and return the one owned native process handle to its caller.
///
/// The command line deliberately contains only the already-created script
/// path; the password stays in the `CreateProcessWithLogonW` UTF-16 buffer.
fn launch_cross_account_probe(
    second_account: &str,
    second_password: &str,
    inner_script_path: &std::path::Path,
) -> Result<OwnedProbeProcess, String> {
    let username = utf16z(second_account);
    let domain = utf16z(".");
    let password = utf16z(second_password);
    let application = utf16z(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut command_line = utf16z(&format!(
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
        inner_script_path.display()
    ));
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();

    let launched = unsafe {
        CreateProcessWithLogonW(
            username.as_ptr(),
            domain.as_ptr(),
            password.as_ptr(),
            LOGON_WITH_PROFILE,
            application.as_ptr(),
            command_line.as_mut_ptr(),
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if launched == 0 {
        return Err(format!(
            "launch failed (os error {})",
            std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default()
        ));
    }

    let owned_process = OwnedProbeProcess {
        handle_value: process.hProcess as usize,
    };
    let thread_closed = unsafe { CloseHandle(process.hThread) } != 0;
    if !thread_closed {
        let launch_error = std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or_default();
        let completion = owned_process.wait_terminate_inspect_close();
        return Err(match completion {
            Ok(exit_code) => format!(
                "thread-handle close failed (os error {launch_error}, exit_code={exit_code})"
            ),
            Err(error) => format!(
                "thread-handle close failed (os error {launch_error}, process cleanup failed: {error})"
            ),
        });
    }
    Ok(owned_process)
}

fn probe_observation(result_path: &std::path::Path) -> (&'static str, bool, u64) {
    match std::fs::read(result_path) {
        Ok(bytes) if bytes.is_empty() => ("empty", true, 0),
        Ok(bytes) => {
            let value = String::from_utf8_lossy(&bytes)
                .trim_start_matches('\u{feff}')
                .trim()
                .to_owned();
            let classification = match value.as_str() {
                "denied" => "denied",
                "opened" => "opened",
                _ => "unexpected-error",
            };
            (classification, true, bytes.len() as u64)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ("missing", false, 0),
        Err(_) => {
            let metadata = std::fs::metadata(result_path).ok();
            (
                "unexpected-error",
                metadata.is_some(),
                metadata.map_or(0, |value| value.len()),
            )
        }
    }
}

fn armed() -> bool {
    std::env::var_os(LIVE_ENV).is_some()
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build a current-thread tokio runtime for this test")
}

/// (a) DAEMON-02 cross-account rejection: a connection attempt from a
/// genuinely different local Windows account is rejected at the pipe
/// boundary by the owner-only DACL -- the daemon-side
/// `accept_and_authorize` call must never see it complete.
#[test]
#[ignore = "requires RDPILOT_LIVE=1, a Windows host, RDPILOT_SECOND_WINDOWS_ACCOUNT, and RDPILOT_SECOND_WINDOWS_PASSWORD"]
fn cross_account_connection_is_rejected_by_the_owner_only_dacl() {
    let name = "cross_account_connection_is_rejected_by_the_owner_only_dacl";
    if !armed() {
        println!("[SKIP] {name}: {LIVE_ENV} unset");
        return;
    }
    let Ok(second_account) = std::env::var(SECOND_ACCOUNT_ENV) else {
        println!("[SKIP] {name}: {SECOND_ACCOUNT_ENV} unset");
        return;
    };
    let Ok(second_password) = std::env::var(SECOND_ACCOUNT_PASSWORD_ENV) else {
        println!("[SKIP] {name}: {SECOND_ACCOUNT_PASSWORD_ENV} unset");
        return;
    };

    current_thread_runtime().block_on(async {
        let listener = rdpilot_daemon::bind()
            .await
            .expect("the owner-only pipe should bind");

        // These two explicit per-file grants let the credentialed account
        // read only its probe and overwrite only its answer, never the temp
        // directory or the pipe itself.
        let shared_temp = PathBuf::from(r"C:\Windows\Temp");
        let result_path = shared_temp.join("rdpilot-dacl-probe-result.txt");
        let inner_script_path = shared_temp.join("rdpilot-dacl-probe-inner.ps1");
        cleanup_probe_artifacts(&result_path, &inner_script_path);

        let inner_script = format!(
            "$resultPath = '{result}'
$out = 'unexpected-error'
try {{
    $client = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'rdpilot-daemon', [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::None)
    try {{ $client.Connect(5000); $out = 'opened' }} finally {{ $client.Dispose() }}
}} catch {{
    $exception = $_.Exception
    $denied = $false
    while ($null -ne $exception) {{
        if (($exception -is [System.UnauthorizedAccessException]) -or (($exception -is [System.ComponentModel.Win32Exception]) -and $exception.NativeErrorCode -eq 5)) {{ $denied = $true; break }}
        $exception = $exception.InnerException
    }}
    if ($denied) {{ $out = 'denied' }}
}}
[System.IO.File]::WriteAllText($resultPath, $out)
",
            result = result_path.display(),
        );
        if std::fs::write(&inner_script_path, inner_script).is_err()
            || std::fs::write(&result_path, "").is_err()
        {
            cleanup_probe_artifacts(&result_path, &inner_script_path);
            panic!("[FAIL] {name}: could not create the credentialed probe artifacts");
        }

        let grant_read = Command::new("icacls")
            .args([inner_script_path.to_str().expect("inner script path should be valid UTF-8"), "/grant", &format!("{second_account}:(RX)")])
            .output();
        let grant_write = Command::new("icacls")
            .args([result_path.to_str().expect("result path should be valid UTF-8"), "/grant", &format!("{second_account}:(M)")])
            .output();
        if !matches!(grant_read, Ok(output) if output.status.success())
            || !matches!(grant_write, Ok(output) if output.status.success())
        {
            cleanup_probe_artifacts(&result_path, &inner_script_path);
            panic!("[FAIL] {name}: could not grant probe artifact access");
        }

        let owned_probe = match launch_cross_account_probe(
            &second_account,
            &second_password,
            &inner_script_path,
        ) {
            Ok(probe) => probe,
            Err(_) => {
                cleanup_probe_artifacts(&result_path, &inner_script_path);
                panic!("[FAIL] {name}: could not launch the credentialed probe");
            }
        };
        let probe_completion = tokio::task::spawn_blocking(move || {
            owned_probe.wait_terminate_inspect_close()
        });

        let accepted = tokio::time::timeout(
            Duration::from_secs(5),
            rdpilot_daemon::accept_and_authorize(&listener),
        )
        .await;
        let completion = probe_completion.await;
        let (probe_classification, result_exists, _) = probe_observation(&result_path);
        cleanup_probe_artifacts(&result_path, &inner_script_path);

        assert!(
            matches!(completion, Ok(Ok(0))),
            "[FAIL] {name}: the credentialed probe must complete successfully"
        );
        assert_eq!(
            probe_classification,
            "denied",
            "[FAIL] {name}: the cross-account client must receive explicit access denial (result_exists={result_exists})"
        );
        assert!(
            !matches!(accepted, Ok(Ok(_))),
            "[FAIL] {name}: a cross-account client must never be accepted"
        );
        println!("[PASS] {name}: the cross-account named-pipe client was denied and never accepted");
    });
}

/// (b) T-15-02 anti-squatting: a second `first_pipe_instance(true)`
/// creation of the same pipe name (`bind()` called twice while the first
/// instance is still alive) FAILS LOUDLY rather than silently attaching.
#[test]
#[ignore = "requires RDPILOT_LIVE=1 and a Windows host"]
fn a_second_first_instance_creation_of_the_same_pipe_name_fails_loudly() {
    let name = "a_second_first_instance_creation_of_the_same_pipe_name_fails_loudly";
    if !armed() {
        println!("[SKIP] {name}: {LIVE_ENV} unset");
        return;
    }

    current_thread_runtime().block_on(async {
        let _first = rdpilot_daemon::bind().await.expect("the first pipe instance should bind successfully");

        let second = rdpilot_daemon::bind().await;
        assert!(
            second.is_err(),
            "[FAIL] {name}: a second first-instance pipe creation of the same name must fail loudly (T-15-02 anti-squatting)"
        );
        println!("[PASS] {name}: a second first-instance creation of the same pipe name failed loudly: {:?}", second.err());
    });
}

/// (c) T-15-01 control case: the pipe's DACL is present, non-null, and
/// scoped to the owner. This file has no access to the crate-internal
/// `SECURITY_DESCRIPTOR` construction (only the public
/// `bind`/`accept_and_authorize`/`socket_path` surface), so this is proven
/// BEHAVIORALLY: a same-account connection through the just-created pipe
/// succeeds (the owner-only DACL always grants its own creator) -- paired
/// with (a)'s cross-account rejection, together they prove the DACL is
/// present (a null/default DACL would also let this control case through,
/// so this test alone is not sufficient evidence -- exactly (a) and (c)
/// TOGETHER are what distinguish "present and owner-scoped" from
/// "null/default", mirroring `ipc_security.rs`'s own "accompanying, not
/// sole, evidence" pattern for its `0700`-mode assertion).
#[test]
#[ignore = "requires RDPILOT_LIVE=1 and a Windows host"]
fn a_same_account_connection_is_accepted_through_the_present_dacl() {
    let name = "a_same_account_connection_is_accepted_through_the_present_dacl";
    if !armed() {
        println!("[SKIP] {name}: {LIVE_ENV} unset");
        return;
    }

    current_thread_runtime().block_on(async {
        let listener = rdpilot_daemon::bind().await.expect("bind should succeed as the primary account");
        let pipe_path = rdpilot_daemon::socket_path().expect("socket_path should resolve");

        // `ClientOptions::open` is SYNCHRONOUS (`io::Result<NamedPipeClient>`,
        // not a `Future`) -- live-VM-confirmed against the real tokio
        // 1.52.3 source, Plan 15-05: the original offline-authored
        // `tokio::join!` of this call alongside the async
        // `accept_and_authorize` future never compiled (E0277: "Result<..>
        // is not a future"). Since `bind()` above already created this
        // pipe instance (making it immediately connectable per the
        // documented `CreateNamedPipeW`/`CreateFile` contract -- a client
        // `open()` against an existing, non-busy instance succeeds without
        // waiting on the server's own `ConnectNamedPipe`/`.connect()`
        // call), the correct sequencing is a synchronous `open()` first,
        // then awaiting the accept -- not a concurrent join of two
        // differently-shaped values.
        let connected = tokio::net::windows::named_pipe::ClientOptions::new().open(&pipe_path);
        let accepted = rdpilot_daemon::accept_and_authorize(&listener).await;

        assert!(accepted.is_ok(), "[FAIL] {name}: a same-account connection must be accepted through the present DACL: {accepted:?}");
        assert!(connected.is_ok(), "[FAIL] {name}: the same-account client connect should succeed: {connected:?}");
        println!("[PASS] {name}: a same-account connection was accepted through the present, non-null, owner-scoped DACL");
    });
}
