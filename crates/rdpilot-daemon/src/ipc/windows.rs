//! Windows local-user-scoped transport: named pipe + explicit owner-only
//! DACL + `first_pipe_instance(true)` (Plan 15-01, closing `12-07-PLAN.md`'s
//! Windows half of DAEMON-02).
//!
//! `#[cfg(windows)]`-only — never compiled on the Linux development host
//! (this module's own inner `#![cfg(windows)]` plus `ipc/mod.rs`'s
//! `#[cfg(windows)] mod windows;` belt-and-braces gate it out). Its real
//! compile is confirmed on the pinned Azure Windows VM in Plan 15-05 — see
//! that plan's SUMMARY for the actual build/run evidence; everything below
//! is authored offline against the `windows-sys` 0.61.2 API (already
//! lockfile-resolved, Cargo.toml `[target.'cfg(windows)'.dependencies]`).
//!
//! ## Design: the DACL IS the access control
//!
//! Unlike the Unix path (`unix.rs`), which authorizes each accepted
//! connection AFTER the fact via `peer_cred()`, a Windows named pipe's
//! security descriptor gates access at `ConnectNamedPipe` time — the OS
//! itself denies a connecting process whose token does not satisfy the
//! DACL, before this module ever sees the connection attempt. There is no
//! Windows analogue of `authorize_uid`; the explicit, owner-only, PROTECTED
//! DACL built by [`OwnerOnlyDacl::build`] below is the entire mechanism
//! (T-15-01/DAEMON-02).
//!
//! ## The two threats this module mitigates
//!
//! - **T-15-01 (critical, Elevation of Privilege):** a null/default pipe
//!   DACL would grant broad (often Everyone) access. [`OwnerOnlyDacl`]
//!   always builds an EXPLICIT, PRESENT, non-null DACL scoped to the
//!   current process token's user SID — never `null`/default — passed to
//!   `create_with_security_attributes_raw`.
//! - **T-15-02 (high, Spoofing/Elevation — named-pipe squatting):** an
//!   attacker who pre-creates a pipe of this name before the daemon starts
//!   could otherwise have a client silently attach to the ATTACKER's
//!   instance. [`bind`] passes `first_pipe_instance(true)` on the very
//!   first instance only, which makes `CreateNamedPipeW` fail loudly
//!   (`ERROR_ACCESS_DENIED`) if a pipe of this name already exists — [`bind`]
//!   maps that specific failure to [`io::ErrorKind::AddrInUse`], mirroring
//!   `unix::bind`'s own contract exactly so `server.rs`'s single-instance
//!   "another daemon already won the bind race, exit cleanly" branch
//!   (Plan 12-06's bind-as-mutex pattern) works unchanged on Windows too.
#![cfg(windows)]

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, GetLengthSid, GetTokenInformation,
    InitializeAcl, InitializeSecurityDescriptor, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
    SetSecurityDescriptorDacl, TOKEN_USER, TokenUser,
};
// `SECURITY_DESCRIPTOR_REVISION` lives in `Win32::System::SystemServices`, not
// `Win32::Security`, in windows-sys 0.61.2 (confirmed against the real crate
// source on the Azure Windows VM, Plan 15-05 -- offline-authored code guessed
// the wrong module).
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
// `OpenProcessToken` lives in `Win32::System::Threading` (alongside
// `GetCurrentProcess`), not `Win32::Security`, in windows-sys 0.61.2 (same
// live-VM-confirmed correction).
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The daemon's well-known named-pipe path (mirrors `unix.rs`'s
/// `rdpilot_ipc::transport::socket_path`'s well-known-path role, but a
/// named pipe has no filesystem entry to stat/clean up — this is purely
/// the fixed pipe name every client and the daemon itself resolve
/// identically, T-13-02).
const PIPE_NAME: &str = r"\\.\pipe\rdpilot-daemon";

/// `TOKEN_QUERY` (WinNT.h) — the minimal access right [`current_user_token_user_buffer`]
/// needs from [`OpenProcessToken`] to read the token's user SID. A stable,
/// SDK-documented, never-renumbered access-right bit (unlike a function
/// signature, hardcoding this single well-known flag carries no real risk
/// and sidesteps an import-path guess across `windows-sys` minor versions).
const TOKEN_QUERY: u32 = 0x0008;

/// `GENERIC_ALL` (WinNT.h) — the access mask [`OwnerOnlyDacl::build`]
/// grants the owner SID's single ACE, i.e. the Win32 access-mask
/// equivalent of the "D:P(A;;GA;;;OW)" owner-only SDDL intent
/// (`12-RESEARCH.md`). Hardcoded for the same stable-constant reason as
/// [`TOKEN_QUERY`].
const GENERIC_ALL: u32 = 0x1000_0000;

/// The Windows analogue of `unix::bind`'s `UnixListener`: owns the
/// currently-pending named-pipe INSTANCE that the next
/// `accept_and_authorize` call will `.connect()` on.
///
/// Interior mutability (`tokio::sync::Mutex`) is required because
/// `accept_and_authorize` takes `&PipeListener` (mirroring the Unix
/// signature exactly, so `server.rs`'s accept loop — shared verbatim
/// across both platforms — compiles unchanged) yet must REPLACE the
/// pending instance with a freshly created one after every accepted
/// connection: each Windows named-pipe "instance" is single-client-use: a
/// fresh instance must exist for every subsequent client, or the pipe name
/// would briefly have nothing to connect to (T-15-02 — never leave the
/// name unclaimed/squattable, even momentarily).
pub struct PipeListener {
    pending: tokio::sync::Mutex<Option<NamedPipeServer>>,
}

/// Bind the daemon's Windows named pipe (DAEMON-02): create the FIRST pipe
/// instance with `first_pipe_instance(true)` (T-15-02 anti-squatting —
/// fails loudly if a pipe of this name already exists) and an explicit
/// owner-only protected DACL (T-15-01 — never null/default).
///
/// # Errors
///
/// Maps the `ERROR_ACCESS_DENIED` Windows raises when a pipe of this name
/// already exists (the OS's own anti-squatting signal for
/// `FILE_FLAG_FIRST_PIPE_INSTANCE`) to [`io::ErrorKind::AddrInUse`] —
/// mirroring `unix::bind`'s own `AddrInUse` contract exactly, so
/// `server.rs`'s single-instance-daemon "another daemon already won the
/// bind race, exit cleanly" branch (Plan 12-06's bind-as-mutex pattern)
/// works unchanged on Windows too. Any OTHER creation failure (a
/// genuinely different DACL/token/allocation error) is returned verbatim.
pub async fn bind() -> io::Result<PipeListener> {
    match create_secured_pipe_instance(true) {
        Ok(server) => Ok(PipeListener { pending: tokio::sync::Mutex::new(Some(server)) }),
        Err(err) if err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("a daemon is already listening on {PIPE_NAME} (first-instance pipe creation was denied)"),
        )),
        Err(err) => Err(err),
    }
}

/// Accept one connection from `listener` (DAEMON-02): waits for a client to
/// connect to the currently pending pipe instance, then immediately
/// prepares the NEXT instance (also owner-only-DACL-secured, but with
/// `first_pipe_instance(false)` — only the very first instance this
/// process ever creates sets that flag) so a subsequent client always has
/// an instance to connect to.
///
/// On Windows there is no post-accept peer check to run (see the module
/// doc: the DACL already gated access at `ConnectNamedPipe` time) — this
/// function's shape mirrors `unix::accept_and_authorize`'s signature
/// purely so `server.rs`'s accept loop compiles unchanged on both
/// platforms.
///
/// # Errors
///
/// Propagates the underlying `connect()`/pipe-creation I/O error verbatim.
pub async fn accept_and_authorize(listener: &PipeListener) -> io::Result<NamedPipeServer> {
    let server = {
        let mut guard = listener.pending.lock().await;
        guard
            .take()
            .ok_or_else(|| io::Error::other("no pending pipe instance -- accept_and_authorize called concurrently"))?
    };
    server.connect().await?;

    // Prepare the NEXT instance for the following client BEFORE returning
    // this one -- otherwise a second client racing in has nothing to
    // connect to (T-15-02: the pipe name must never go briefly unclaimed).
    let next = create_secured_pipe_instance(false)?;
    {
        let mut guard = listener.pending.lock().await;
        *guard = Some(next);
    }

    Ok(server)
}

/// The daemon's well-known Windows pipe path (mirrors `unix.rs`'s reliance
/// on `rdpilot_ipc::transport::socket_path` for the same role — a named
/// pipe has no directory/socket-file to stat or clean up, so this is
/// purely the fixed [`PIPE_NAME`] wrapped as a [`PathBuf`] for API parity
/// with the Unix side).
pub fn socket_path() -> io::Result<PathBuf> {
    Ok(PathBuf::from(PIPE_NAME))
}

/// Create one secured named-pipe instance: `first` is `true` only for the
/// very first instance [`bind`] creates (T-15-02 anti-squatting); every
/// subsequent per-client instance [`accept_and_authorize`] prepares passes
/// `false`. Every instance gets its OWN freshly-built owner-only DACL
/// (defense in depth — never reuses a raw pointer across instances).
#[allow(unsafe_code)] // Localized: the single Win32 pipe-creation FFI call below.
fn create_secured_pipe_instance(first: bool) -> io::Result<NamedPipeServer> {
    let mut dacl = OwnerOnlyDacl::build()?;
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: dacl.as_ptr(),
        bInheritHandle: 0,
    };
    let attrs: *mut c_void = ptr::addr_of_mut!(sa).cast::<c_void>();

    // SAFETY: `attrs` points at a stack-local `SECURITY_ATTRIBUTES` whose
    // `lpSecurityDescriptor` wraps `dacl`'s fully-initialized owner-only
    // PROTECTED security descriptor (T-15-01: explicit, never
    // null/default -- built by `OwnerOnlyDacl::build` immediately above).
    // `dacl` (and the SID/ACL buffers it owns) stays alive on this
    // function's stack for the ENTIRE duration of this call -- Windows
    // copies the descriptor into the kernel pipe object during
    // `CreateNamedPipeW` itself, so no lifetime needs to extend past this
    // call returning. `first_pipe_instance(first)` is `true` only for the
    // very first instance this process ever creates (T-15-02: a
    // pre-existing pipe of this name makes THAT call fail loudly); every
    // later per-client instance passes `false`. This is the ONE call in
    // this module to `tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw`,
    // itself an `unsafe fn` per tokio's own contract (the caller vouches
    // for `attrs`' validity, satisfied above).
    unsafe { ServerOptions::new().first_pipe_instance(first).create_with_security_attributes_raw(PIPE_NAME, attrs) }
}

/// Owns every buffer the owner-only protected DACL's pointers reference —
/// the current user's `TOKEN_USER` bytes (whose trailing bytes carry the
/// SID [`OwnerOnlyDacl::build`] reads a pointer into), the ACL bytes, and
/// the `SECURITY_DESCRIPTOR` itself — so they all outlive the
/// `create_with_security_attributes_raw` call that consumes a pointer into
/// [`OwnerOnlyDacl::as_ptr`] (T-15-01). Moving a `Vec<u8>` never moves its
/// heap allocation, so pointers derived from `_sid_buf`/`_acl_buf` before
/// they were moved into this struct stay valid.
struct OwnerOnlyDacl {
    _sid_buf: Vec<u8>,
    _acl_buf: Vec<u8>,
    sd: SECURITY_DESCRIPTOR,
}

impl OwnerOnlyDacl {
    /// Build a fresh owner-only, PROTECTED `SECURITY_DESCRIPTOR`: current
    /// process token's user SID -> a one-ACE ACL granting that SID
    /// `GENERIC_ALL` -> a `SECURITY_DESCRIPTOR` with that ACL set as its
    /// DACL, PRESENT and non-null (never defaulted) — the Win32
    /// equivalent of the "D:P(A;;GA;;;OW)" owner-only SDDL intent.
    #[allow(unsafe_code)] // Localized: each Win32 Security-API call below is individually justified.
    fn build() -> io::Result<Self> {
        let sid_buf = current_user_token_user_buffer()?;

        // SAFETY: `sid_buf` was just filled by `GetTokenInformation`
        // (`TokenUser`) in `current_user_token_user_buffer` above -- it is
        // a valid, fully-initialized `TOKEN_USER` followed by its
        // trailing variable-length SID bytes (the Win32 documented
        // variable-length-struct convention for this info class). Reading
        // the `Sid` pointer field out of it is a plain struct-field read
        // through a pointer known to be valid for at least
        // `size_of::<TOKEN_USER>()` bytes, not a dereference of unions or
        // uninitialized memory.
        let sid: PSID = unsafe { (*(sid_buf.as_ptr().cast::<TOKEN_USER>())).User.Sid };

        // SAFETY: `GetLengthSid` reads only the SID's own header bytes
        // (already fully initialized, immediately above) and returns its
        // byte length by value -- no aliasing/lifetime hazard.
        let sid_len = unsafe { GetLengthSid(sid) } as usize;

        // Classic MSDN ACL-sizing formula (ACL header + one
        // ACCESS_ALLOWED_ACE + the SID's own bytes, minus the ACE
        // struct's built-in placeholder DWORD), rounded up to the DWORD
        // alignment `InitializeAcl` requires.
        let acl_len = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_len;
        let acl_len = (acl_len + 3) & !3;
        let mut acl_buf = vec![0u8; acl_len];
        let acl_ptr = acl_buf.as_mut_ptr().cast::<ACL>();

        // SAFETY: `acl_ptr` points at an `acl_len`-byte buffer sized by
        // the formula immediately above -- `InitializeAcl` writes only
        // its own fixed-size ACL header into it and cannot overrun (Win32
        // contract for a correctly-sized buffer).
        if unsafe { InitializeAcl(acl_ptr, acl_len as u32, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `acl_ptr` is the freshly-initialized, correctly-sized
        // ACL immediately above; `sid` is the valid current-user SID read
        // above and outlives this call (borrowed from `sid_buf`, which
        // this struct retains ownership of). Grants `GENERIC_ALL` to the
        // owner SID only -- the single ACE the owner-only DACL intent
        // requires (T-15-01) -- `AddAccessAllowedAce` appends within the
        // buffer `InitializeAcl` already sized to fit exactly one such
        // ACE.
        if unsafe { AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL, sid) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // `SECURITY_DESCRIPTOR` is a fixed-size struct -- no separate
        // heap buffer needed beyond the struct's own storage.
        let mut sd: SECURITY_DESCRIPTOR = unsafe {
            // SAFETY: an all-zero `SECURITY_DESCRIPTOR` is a valid bit
            // pattern for this POD Win32 struct (it has no
            // Drop/invariant beyond what `InitializeSecurityDescriptor`
            // establishes immediately below, before this value is ever
            // read as a security descriptor).
            std::mem::zeroed()
        };
        let sd_ptr = ptr::addr_of_mut!(sd).cast::<c_void>();

        // SAFETY: `sd_ptr` points at the stack-local `SECURITY_DESCRIPTOR`
        // immediately above -- `InitializeSecurityDescriptor` writes
        // exactly `size_of::<SECURITY_DESCRIPTOR>()` bytes of its own
        // revision/control header into it (Win32 contract), which the
        // struct's own storage already provides.
        if unsafe { InitializeSecurityDescriptor(sd_ptr, SECURITY_DESCRIPTOR_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `sd_ptr` is the freshly-initialized descriptor above;
        // `acl_ptr` is the fully-built owner-only ACL above, kept alive
        // by `acl_buf` (moved into this struct's `_acl_buf` field right
        // after this call, alongside `sd`) -- `dacl_present = TRUE (1)`,
        // `dacl = acl_ptr` (non-null), `dacl_defaulted = FALSE (0)`: an
        // EXPLICIT, PRESENT, non-null DACL -- never the null/default
        // descriptor T-15-01 forbids.
        if unsafe { SetSecurityDescriptorDacl(sd_ptr, 1, acl_ptr, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(OwnerOnlyDacl { _sid_buf: sid_buf, _acl_buf: acl_buf, sd })
    }

    /// A raw pointer at this DACL's `SECURITY_DESCRIPTOR`, suitable for
    /// `SECURITY_ATTRIBUTES::lpSecurityDescriptor`. Borrows `self`
    /// mutably purely to produce a `*mut` (no interior unsafe -- taking
    /// a field's address via `addr_of_mut!` does not dereference it).
    fn as_ptr(&mut self) -> *mut c_void {
        ptr::addr_of_mut!(self.sd).cast::<c_void>()
    }
}

/// Query the current process token's `TokenUser` info class into a
/// correctly-sized heap buffer (the standard two-call
/// `GetTokenInformation` idiom: an initial size-probe call, then the real
/// read). The returned buffer's first bytes are a `TOKEN_USER` struct
/// whose `User.Sid` field points BACK INTO this same buffer's trailing
/// bytes (Win32's variable-length-struct convention) — callers must keep
/// this buffer alive for as long as they dereference that `Sid` pointer.
#[allow(unsafe_code)] // Localized: each Win32 token-query call below is individually justified.
fn current_user_token_user_buffer() -> io::Result<Vec<u8>> {
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle (never a real
    // handle needing `CloseHandle`) -- it takes no arguments and cannot
    // fail.
    let process = unsafe { GetCurrentProcess() };

    // `HANDLE` is `*mut c_void` in windows-sys 0.61.2 (it was an integer
    // type in older versions) -- a bare `0` no longer type-checks as a null
    // handle (live-VM-confirmed correction, Plan 15-05).
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is the valid pseudo-handle from immediately
    // above; `&mut token` is a valid, uniquely-owned local out-pointer
    // `OpenProcessToken` writes the opened token handle into on success.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut needed: u32 = 0;
    // SAFETY: `token` is the valid token handle just opened above; a null
    // buffer with length 0 is the documented Win32 idiom to probe the
    // required buffer size via `needed` -- this call is EXPECTED to
    // report failure (`ERROR_INSUFFICIENT_BUFFER`); only the `needed`
    // out-value is consulted afterward.
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        // SAFETY: `token` is the still-open, valid handle from above,
        // closed exactly once on this early-return path.
        unsafe { CloseHandle(token) };
        return Err(io::Error::other("GetTokenInformation(TokenUser) reported a zero required buffer size"));
    }

    let mut buf = vec![0u8; needed as usize];
    // SAFETY: `buf` is freshly allocated with EXACTLY `needed` bytes (the
    // size the probe call above reported); `GetTokenInformation` writes
    // at most that many bytes into it (Win32 contract for a
    // correctly-sized buffer).
    let ok = unsafe { GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast::<c_void>(), needed, &mut needed) };

    // SAFETY: `token` is the valid handle opened above, closed exactly
    // once here regardless of the read's success/failure.
    unsafe { CloseHandle(token) };

    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf)
}
