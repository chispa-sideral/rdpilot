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

/// The second account's password, needed to register the one-shot
/// Scheduled Task that runs the cross-account probe AS that account (see
/// [`cross_account_connection_is_rejected_by_the_owner_only_dacl`]'s body
/// doc for why a Scheduled Task, not `runas`, is the correct mechanism
/// here -- live-VM-confirmed, Plan 15-05).
const SECOND_ACCOUNT_PASSWORD_ENV: &str = "RDPILOT_SECOND_WINDOWS_PASSWORD";
const TASK_NAME_ENV: &str = "RDPILOT_DACL_TASK_NAME";

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
        // `second_account`, via a one-shot Scheduled Task -- NOT `runas`
        // (the originally-authored mechanism, offline-plausible but never
        // live-compiled/run against this project's actual execution
        // topology). Live-VM-confirmed root cause (Plan 15-05): `runas`
        // internally calls `CreateProcessWithLogonW`, which requires an
        // interactive window station to attach the new logon session to.
        // This whole test suite runs via `az vm run-command invoke`, i.e.
        // as `NT AUTHORITY\SYSTEM` in the non-interactive services session
        // (Session 0) -- `runas` fails there IMMEDIATELY (exit code 1,
        // Secondary Logon service state irrelevant) WITHOUT ever spawning
        // the second-account process, so the daemon-side accept below
        // would time out for the WRONG reason (no probe ever ran) and
        // silently report a false-positive PASS. This file's own original
        // module doc already anticipated exactly this failure mode and
        // named the fix: "a scheduled task trigger is the documented
        // fallback" -- Task Scheduler creates its own logon session
        // without needing an interactive desktop, independently verified
        // live (via a manual `whoami` probe) to genuinely run as the
        // second account and receive a real `ERROR_ACCESS_DENIED` from
        // the owner-only DACL. The probe writes its outcome to a result
        // file (a scheduled task has no direct stdout channel back to
        // this process) which is the actual ground-truth assertion below
        // -- not merely the daemon-side timeout, which alone cannot
        // distinguish "genuinely rejected" from "probe never ran".
        let task_name = std::env::var(TASK_NAME_ENV).unwrap_or_else(|_| "RdpilotDaclCrossAccountProbe".to_owned());
        // NOT `std::env::temp_dir()`: under this test's execution context
        // (`az vm run-command` runs as `NT AUTHORITY\SYSTEM`), that
        // resolves to SYSTEM's own profile-scoped temp directory
        // (`...\systemprofile\AppData\Local\Temp`), which the second
        // account has no ACL access to at all (live-VM-confirmed, Plan
        // 15-05). `C:\Windows\Temp` is the shared location instead --
        // but even THIS is not automatically readable/writable by every
        // account on a hardened image: this VM's `Configure-Target.ps1`
        // in-guest hardening (Plan 1) leaves files written there by
        // `NT AUTHORITY\SYSTEM` with an inherited DACL scoped to
        // `BUILTIN\Administrators`/`NT AUTHORITY\SYSTEM` ONLY (verified
        // live via `icacls`) -- a non-admin second account can neither
        // read the inner probe script nor write the result file, so the
        // scheduled task fails before ever reaching the pipe-open call,
        // producing the exact same "MISSING" result as "the probe never
        // ran at all". The explicit `icacls /grant` calls below close
        // that gap precisely for the two files this probe creates
        // (least-privilege: NOT a blanket `C:\Windows\Temp` ACL change).
        let shared_temp = PathBuf::from(r"C:\Windows\Temp");
        let result_path = shared_temp.join("rdpilot-dacl-probe-result.txt");
        let inner_script_path = shared_temp.join("rdpilot-dacl-probe-inner.ps1");
        let _ = std::fs::remove_file(&result_path);

        let inner_script = format!(
            "$out = try {{ [System.IO.File]::Open('{pipe}', 'Open', 'ReadWrite').Close(); 'opened' }} catch {{ 'denied: ' + $_.Exception.Message }}
$out | Out-File -FilePath '{result}' -Encoding utf8 -Force
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
            "[FAIL] {name}: icacls grant on the inner probe script failed: {}",
            String::from_utf8_lossy(&grant_read.stderr)
        );
        let grant_write = Command::new("icacls")
            .args([result_path.to_str().expect("result path should be valid UTF-8"), "/grant", &format!("{second_account}:(M)")])
            .output()
            .expect("failed to invoke icacls granting the second account write access on the probe result file");
        assert!(
            grant_write.status.success(),
            "[FAIL] {name}: icacls grant on the probe result file failed: {}",
            String::from_utf8_lossy(&grant_write.stderr)
        );

        let _ = Command::new("schtasks").args(["/delete", "/tn", &task_name, "/f"]).output();
        let create = Command::new("schtasks")
            .args([
                "/create",
                "/tn",
                &task_name,
                "/tr",
                &format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File {}", inner_script_path.display()),
                "/sc",
                "once",
                "/st",
                "23:59",
                "/ru",
                &second_account,
                "/rp",
                &second_password,
                "/f",
            ])
            .output()
            .expect("failed to invoke schtasks /create (check RDPILOT_SECOND_WINDOWS_ACCOUNT/RDPILOT_SECOND_WINDOWS_PASSWORD provisioning)");
        assert!(
            create.status.success(),
            "[FAIL] {name}: schtasks /create failed: {}",
            String::from_utf8_lossy(&create.stderr)
        );

        let run = Command::new("schtasks").args(["/run", "/tn", &task_name]).output().expect("failed to invoke schtasks /run");
        assert!(run.status.success(), "[FAIL] {name}: schtasks /run failed: {}", String::from_utf8_lossy(&run.stderr));

        let accepted = tokio::time::timeout(Duration::from_secs(5), rdpilot_daemon::accept_and_authorize(&listener)).await;

        // Give the scheduled task a moment to finish writing its result
        // file even if the accept above already timed out first.
        tokio::time::sleep(Duration::from_secs(2)).await;
        // `Out-File -Encoding utf8` (the inner script above) writes a
        // leading UTF-8 BOM (U+FEFF) by PowerShell's own default -- NOT
        // whitespace, so `str::trim_start()` alone leaves it in place and
        // a naive `starts_with("denied:")` would false-negative even on a
        // genuine rejection (live-VM-confirmed, Plan 15-05: the probe DID
        // correctly receive "Access to the path ... is denied", but the
        // BOM-prefixed string failed the original un-stripped check).
        let probe_result = std::fs::read_to_string(&result_path)
            .unwrap_or_else(|_| "MISSING: probe never wrote a result".to_owned())
            .trim_start_matches('\u{feff}')
            .to_owned();

        let _ = Command::new("schtasks").args(["/delete", "/tn", &task_name, "/f"]).output();
        let _ = std::fs::remove_file(&result_path);
        let _ = std::fs::remove_file(&inner_script_path);

        // The scheduled task's own result file is the ground truth: it
        // must report a genuine "denied: ..." (ERROR_ACCESS_DENIED from
        // the owner-only DACL), never "opened" and never a missing
        // result (a missing result means the probe never genuinely ran as
        // the second account at all -- exactly the false-positive `runas`
        // silently produced before this fix).
        assert!(
            probe_result.trim_start().starts_with("denied:"),
            "[FAIL] {name}: expected the cross-account probe to be denied by the owner-only DACL, got: {probe_result:?}"
        );
        println!("[PASS] {name}: cross-account connection rejected at the pipe boundary: {}", probe_result.trim());

        match accepted {
            Ok(Ok(_)) => panic!("[FAIL] {name}: a different-account peer must never be accepted by the pipe DACL -- DAEMON-02 regression"),
            Ok(Err(err)) => println!("[PASS] {name}: daemon-side accept_and_authorize also observed rejection: {err}"),
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
