//! SC#4 [BLOCKING] (DAEMON-02): a different-uid client is rejected at the
//! Unix IPC boundary.
//!
//! **Evidence tiers:**
//!
//! - **Always-on, offline (this file's non-`#[ignore]` tests):** exercises
//!   `authorize_uid` — the pure decision function
//!   `ipc::unix::accept_and_authorize` calls on every accepted connection
//!   — directly. It asserts the decision REJECTS a mismatched uid (not
//!   merely that the socket directory has the right mode — research
//!   explicitly warns a stat-only test is necessary but not sufficient
//!   evidence, Pitfall 5). The accompanying `0700` mode assertion is
//!   present alongside this, not in place of it.
//! - **Real cross-account (gated, `#[ignore]`):** `cross_account_peer_is_rejected_end_to_end`
//!   performs a genuine different-OS-account connection attempt via
//!   `sudo -n -u $RDPILOT_SECOND_UID`, exercising `accept_and_authorize`
//!   end-to-end against a real mismatched peer uid. Gated behind the
//!   `RDPILOT_SECOND_UID` env var (a second local username with
//!   passwordless-sudo access configured) so CI/dev machines without a
//!   second account never fail — only an explicit
//!   `cargo test -- --ignored` run with that env var set exercises it.
//!   The full cross-account proof is re-confirmed on Windows
//!   (DACL/other-account) in the live-gate Plan 12-07.

// Unix-only: `authorize_uid`/`geteuid()`/`MetadataExt` are all
// Unix-specific -- the Windows cross-account proof lives in
// `live_daemon_windows_dacl.rs` instead (live-VM-confirmed compile
// fix, Plan 15-05: this file never had a cfg gate before, which
// broke `cargo test -p rdpilot-daemon` on a Windows target).
#![cfg(unix)]

use std::os::unix::fs::MetadataExt;

use rdpilot_daemon::authorize_uid;

/// The daemon's own effective uid, for constructing "our uid" / "a
/// different uid" test inputs. A direct `libc::geteuid()` call is fine
/// here: this file is its own separate crate (an integration test target)
/// and is not bound by the library crate's `#![deny(unsafe_code)]`.
#[allow(unsafe_code)]
fn our_uid() -> u32 {
    // SAFETY: `geteuid()` takes no arguments, cannot fail, and has no
    // aliasing/lifetime hazards — always safe to call.
    unsafe { libc::geteuid() }
}

/// Always-on offline evidence: a peer uid one greater than the daemon's
/// own effective uid is rejected with `PermissionDenied`.
#[test]
fn authorize_uid_rejects_a_different_uid() {
    let our = our_uid();
    let err = authorize_uid(our.wrapping_add(1), our).expect_err("a mismatched uid must be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

/// Control case: a matching peer uid is authorized.
#[test]
fn authorize_uid_accepts_a_matching_uid() {
    let our = our_uid();
    assert!(authorize_uid(our, our).is_ok(), "a matching uid must be authorized");
}

/// Accompanying (not sole) evidence, per research Pitfall 5: the real
/// socket directory is created with mode `0700`. This alone is
/// insufficient proof of DAEMON-02 (a misconfigured directory alongside a
/// missing uid check would still be a vulnerability) — it accompanies,
/// rather than replaces, the `authorize_uid` decision-logic assertions
/// above.
#[test]
fn the_socket_directory_is_mode_0700() {
    let path = rdpilot_daemon::socket_path().expect("socket_path should resolve on this host");
    let dir = path.parent().expect("socket_path always has a parent directory");
    let meta = std::fs::metadata(dir).expect("stat should succeed — bind()/socket_path() create the dir eagerly");
    assert_eq!(meta.mode() & 0o777, 0o700, "expected the socket dir to be mode 0700, got {:o}", meta.mode() & 0o777);
}

/// Real cross-account proof (gated): a genuinely different local OS
/// account's connection attempt is rejected end-to-end by
/// `accept_and_authorize`.
///
/// Opt-in only: requires `RDPILOT_SECOND_UID` (a second local username
/// with passwordless `sudo -u` access configured) AND an explicit
/// `cargo test -- --ignored` invocation. Never runs in a default
/// `cargo test` pass, so a dev/CI machine without a second account is
/// unaffected.
#[test]
#[ignore = "requires RDPILOT_SECOND_UID (a second local username, passwordless sudo -u) and --ignored"]
fn cross_account_peer_is_rejected_end_to_end() {
    let Ok(second_user) = std::env::var("RDPILOT_SECOND_UID") else {
        panic!("RDPILOT_SECOND_UID must be set to run this explicitly-ignored test");
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build a current-thread tokio runtime for this test");

    rt.block_on(async {
        let listener = rdpilot_daemon::bind().await.expect("bind should succeed as the primary account");
        let sock_path = rdpilot_daemon::socket_path().expect("socket_path should resolve");

        // Connect as `second_user` via a `sudo -n -u` Python3 one-shot
        // probe, in the background — the assertion that matters is the
        // daemon-side `accept_and_authorize` call below; this probe only
        // needs to establish the raw socket connection (which, like TCP,
        // succeeds at the kernel/backlog level independent of uid — the
        // uid check happens only once the daemon calls `peer_cred()`).
        let script = format!(
            "import socket,time; s=socket.socket(socket.AF_UNIX); s.connect('{}'); time.sleep(3)",
            sock_path.display()
        );
        let mut probe = std::process::Command::new("sudo")
            .args(["-n", "-u", &second_user, "python3", "-c", &script])
            .spawn()
            .expect("failed to spawn the sudo -u probe (check passwordless sudo config for RDPILOT_SECOND_UID)");

        let accepted = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            rdpilot_daemon::accept_and_authorize(&listener),
        )
        .await
        .expect("timed out waiting for the cross-account probe to connect");

        assert!(
            accepted.is_err(),
            "a different-uid peer must be rejected by accept_and_authorize — DAEMON-02 regression"
        );

        let _ = probe.wait();
        let _ = std::fs::remove_file(&sock_path);
    });
}
