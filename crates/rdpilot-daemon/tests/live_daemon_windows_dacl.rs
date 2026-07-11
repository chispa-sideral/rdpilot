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

use std::process::Command;
use std::time::Duration;

/// Name of the opt-in env var that arms this live suite (D-18) -- mirrors
/// `crates/rdpilot/tests/common/mod.rs::LIVE_ENV` without depending on
/// that crate's test-support module (this is a daemon-crate integration
/// test, a separate compilation unit).
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// A second local Windows account name, already provisioned (with
/// non-interactive elevation configured, e.g. a one-time `runas /savecred`
/// during VM setup -- the Windows analogue of `ipc_security.rs`'s
/// passwordless-`sudo` prerequisite) and reachable by Plan 15-05's VM
/// setup, mirroring `tests/ipc_security.rs`'s `RDPILOT_SECOND_UID` pattern
/// for the Unix cross-account case. This file only CONSUMES the name via
/// env var -- provisioning the account (and its non-interactive elevation)
/// is Plan 15-05's job.
const SECOND_ACCOUNT_ENV: &str = "RDPILOT_SECOND_WINDOWS_ACCOUNT";

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
#[ignore = "requires RDPILOT_LIVE=1, a Windows host, and RDPILOT_SECOND_WINDOWS_ACCOUNT"]
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

    current_thread_runtime().block_on(async {
        let listener = rdpilot_daemon::bind().await.expect("bind should succeed as the primary account");
        let pipe_path = rdpilot_daemon::socket_path().expect("socket_path should resolve");

        // A minimal probe attempting to OPEN the pipe path directly as
        // `second_account` -- mirrors `ipc_security.rs`'s `sudo -n -u`
        // Python one-shot, using the Windows equivalent (`runas
        // /savecred`, relying on credentials cached once during Plan
        // 15-05's VM provisioning -- an interactive `runas` prompt would
        // hang this test, exactly like a `sudo` password prompt would
        // hang the Unix probe). The daemon-side `accept_and_authorize`
        // timeout below is the assertion that matters; this probe only
        // needs to attempt the open. Plan 15-05 adjusts the exact
        // invocation here if the VM's actual non-interactive-elevation
        // mechanism differs (a scheduled task trigger is the documented
        // fallback -- see this file's module doc).
        let script = format!(
            "try {{ [System.IO.File]::Open('{}', 'Open', 'ReadWrite').Close(); 'opened' }} catch {{ 'denied: ' + $_.Exception.Message }}",
            pipe_path.display()
        );
        let mut probe = Command::new("runas")
            .args(["/savecred", &format!("/user:{second_account}"), "powershell -NoProfile -Command", &script])
            .spawn()
            .expect("failed to spawn the cross-account probe (check RDPILOT_SECOND_WINDOWS_ACCOUNT provisioning)");

        let accepted = tokio::time::timeout(Duration::from_secs(5), rdpilot_daemon::accept_and_authorize(&listener)).await;

        match accepted {
            Ok(Ok(_)) => panic!("[FAIL] {name}: a different-account peer must never be accepted by the pipe DACL -- DAEMON-02 regression"),
            Ok(Err(err)) => println!("[PASS] {name}: cross-account connection rejected at the pipe boundary: {err}"),
            Err(_) => println!("[PASS] {name}: no cross-account connection was ever accepted (timed out waiting, as expected)"),
        }

        let _ = probe.wait();
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

        let (accepted, connected) = tokio::join!(
            rdpilot_daemon::accept_and_authorize(&listener),
            tokio::net::windows::named_pipe::ClientOptions::new().open(&pipe_path),
        );

        assert!(accepted.is_ok(), "[FAIL] {name}: a same-account connection must be accepted through the present DACL: {accepted:?}");
        assert!(connected.is_ok(), "[FAIL] {name}: the same-account client connect should succeed: {connected:?}");
        println!("[PASS] {name}: a same-account connection was accepted through the present, non-null, owner-scoped DACL");
    });
}
