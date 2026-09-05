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
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
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

fn utf16z(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The exact child process launched for the cross-account probe.  Its handle
/// is consumed by `wait_terminate_inspect_close`, so no caller can forget the
/// bounded wait/termination/close sequence.
struct OwnedProbeProcess {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl OwnedProbeProcess {
    fn wait_terminate_inspect_close(self) -> Result<u32, String> {
        let first_wait = unsafe { WaitForSingleObject(self.handle, PROBE_WAIT_MILLIS) };
        if first_wait != WAIT_OBJECT_0 {
            let terminated = unsafe { TerminateProcess(self.handle, 1) } != 0;
            let terminal_wait =
                unsafe { WaitForSingleObject(self.handle, TERMINATION_WAIT_MILLIS) };
            let mut exit_code = 0;
            let exit_code_observed =
                unsafe { GetExitCodeProcess(self.handle, &mut exit_code) } != 0;
            let process_closed = unsafe { CloseHandle(self.handle) } != 0;
            return Err(format!(
                "probe did not complete (wait={first_wait}, terminated={terminated}, terminal_wait={terminal_wait}, exit_observed={exit_code_observed}, process_closed={process_closed})"
            ));
        }

        let mut exit_code = 0;
        let exit_code_observed = unsafe { GetExitCodeProcess(self.handle, &mut exit_code) } != 0;
        let process_closed = unsafe { CloseHandle(self.handle) } != 0;
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
        handle: process.hProcess,
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
        let listener = rdpilot_daemon::bind().await.expect("bind should succeed as the primary account");
        let pipe_path = rdpilot_daemon::socket_path().expect("socket_path should resolve");

        // A minimal probe attempting to OPEN the pipe path directly as
        // `second_account`. The hosted diagnostic showed that the
        // Scheduled Task remained Ready with 0x41303 (has not run), so it
        // never exercised the DACL. This hosted runner is an ordinary
        // runner account, not LocalSystem: PowerShell's credentialed
        // process launch can create the probe directly under the second
        // account. The result file remains the ground-truth assertion;
        // a timeout or missing result is failure.
        // The cross-account process needs a location shared with the hosted
        // runner account. The explicit `icacls /grant` calls below grant
        // access only to the two probe files, never to the temp directory.
        let shared_temp = PathBuf::from(r"C:\Windows\Temp");
        let result_path = shared_temp.join("rdpilot-dacl-probe-result.txt");
        let inner_script_path = shared_temp.join("rdpilot-dacl-probe-inner.ps1");
        let _ = std::fs::remove_file(&result_path);

        let inner_script = format!(
            "$out = try {{ [System.IO.File]::Open('{pipe}', 'Open', 'ReadWrite').Close(); 'opened' }} catch {{ if ($_.Exception -is [System.UnauthorizedAccessException]) {{ 'denied' }} else {{ 'unexpected-error' }} }}
[System.IO.File]::WriteAllText('{result}', $out)
",
            pipe = pipe_path.display(),
            result = result_path.display(),
        );
        std::fs::write(&inner_script_path, inner_script).expect("failed to write the cross-account probe's inner script");
        // Pre-touch the result file (empty) so `icacls` has a real target
        // to grant Modify rights on -- `icacls` cannot ACL a path that
        // does not exist yet, and the probe itself (running as the
        // second account) needs write access to CREATE/overwrite it.
        std::fs::write(&result_path, "").expect("failed to pre-touch the cross-account probe's result file");

        let grant_read = Command::new("icacls")
            .args([inner_script_path.to_str().expect("inner script path should be valid UTF-8"), "/grant", &format!("{second_account}:(RX)")])
            .output()
            .expect("failed to invoke icacls granting the second account read+execute on the probe script");
        assert!(
            grant_read.status.success(),
            "[FAIL] {name}: icacls grant on the inner probe script failed"
        );
        let grant_write = Command::new("icacls")
            .args([result_path.to_str().expect("result path should be valid UTF-8"), "/grant", &format!("{second_account}:(M)")])
            .output()
            .expect("failed to invoke icacls granting the second account write access on the probe result file");
        assert!(
            grant_write.status.success(),
            "[FAIL] {name}: icacls grant on the probe result file failed"
        );

        let owned_probe = match launch_cross_account_probe(
            &second_account,
            &second_password,
            &inner_script_path,
        ) {
            Ok(probe) => probe,
            Err(error) => {
                let _ = std::fs::remove_file(&result_path);
                let _ = std::fs::remove_file(&inner_script_path);
                panic!("[FAIL] {name}: credentialed cross-account probe failed: {error}");
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
        let probe_completion = match probe_completion.await {
            Ok(completion) => completion,
            Err(_) => Err("probe completion task failed".to_owned()),
        };

        // The owned native process above has now completed its bounded
        // completion path. The result file is still the ground-truth DACL signal;
        // poll it briefly because the child may release its final file write
        // just before process termination becomes observable.
        // The inner script emits only a fixed classification. Observe it
        // before cleanup so an absent launch cannot be mistaken for denial.
        let wrote_probe = if probe_completion.is_ok() {
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let (classification, _, _) = probe_observation(&result_path);
                    if classification != "missing" && classification != "empty" {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
            .await
            .is_ok()
        } else {
            false
        };

        let (probe_classification, _, _) = probe_observation(&result_path);

        let _ = std::fs::remove_file(&result_path);
        let _ = std::fs::remove_file(&inner_script_path);

        match probe_completion {
            Ok(0) => {}
            Ok(_) => panic!("[FAIL] {name}: credentialed cross-account probe exited unsuccessfully"),
            Err(error) => panic!("[FAIL] {name}: credentialed cross-account probe failed: {error}"),
        }

        // The cross-account probe's result file is the ground truth: it
        // must report a genuine "denied" (ERROR_ACCESS_DENIED from
        // the owner-only DACL), never "opened" and never a missing
        // result (a missing result means the probe never genuinely ran as
        // the second account at all).
        assert!(
            wrote_probe && probe_classification == "denied",
            "[FAIL] {name}: expected the cross-account probe to be denied by the owner-only DACL, observed classification: {probe_classification}"
        );
        println!("[PASS] {name}: cross-account connection rejected at the pipe boundary");

        match accepted {
            Ok(Ok(_)) => panic!("[FAIL] {name}: a different-account peer must never be accepted by the pipe DACL -- DAEMON-02 regression"),
            Ok(Err(_)) => println!("[PASS] {name}: daemon-side accept_and_authorize also observed rejection"),
            Err(_) => {
                println!("[PASS] {name}: daemon-side accept_and_authorize never completed (timed out waiting, consistent with rejection)");
            }
        }
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
