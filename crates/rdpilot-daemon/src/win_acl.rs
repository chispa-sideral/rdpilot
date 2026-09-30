//! Owner-only Windows security descriptors: the named pipe's DACL and the
//! protected, inheritable DACL of recording directories.
#![cfg(windows)]

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    AddAccessAllowedAceEx, GetLengthSid, GetTokenInformation, InitializeAcl,
    InitializeSecurityDescriptor, SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
    TokenUser, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE,
    PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SE_DACL_PROTECTED, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
// `SECURITY_DESCRIPTOR_REVISION` lives in `Win32::System::SystemServices`,
// not `Win32::Security`, in windows-sys 0.61.2.
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
// `OpenProcessToken` lives in `Win32::System::Threading` (alongside
// `GetCurrentProcess`) in windows-sys 0.61.2.
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `TOKEN_QUERY` (WinNT.h): the access right needed to read the token's
/// user SID.
const TOKEN_QUERY: u32 = 0x0008;

/// `GENERIC_ALL` (WinNT.h): the access mask of the single owner ACE, the
/// equivalent of the "D:P(A;;GA;;;OW)" owner-only SDDL intent.
const GENERIC_ALL: u32 = 0x1000_0000;

/// Create directory `path` (not its parents) with a protected DACL that
/// grants only the current user, inherited by everything created inside.
/// Fails with `AlreadyExists` when it exists.
#[allow(unsafe_code)] // Localized: the single CreateDirectoryW call below.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut dacl = OwnerOnlyDacl::build_inheritable()?;
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: dacl.as_ptr(),
        bInheritHandle: 0,
    };
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call;
    // `sa` points at the initialized descriptor owned by `dacl`, which also
    // outlives the call (Windows copies the descriptor into the new object).
    if unsafe { CreateDirectoryW(wide.as_ptr(), ptr::addr_of_mut!(sa)) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Owns every buffer the owner-only protected DACL's pointers reference —
/// the current user's `TOKEN_USER` bytes (whose trailing bytes carry the
/// SID [`OwnerOnlyDacl::build`] reads a pointer into), the ACL bytes, and
/// the `SECURITY_DESCRIPTOR` itself — so they all outlive the
/// `create_with_security_attributes_raw` call that consumes a pointer into
/// [`OwnerOnlyDacl::as_ptr`]. Moving a `Vec<u8>` never moves its
/// heap allocation, so pointers derived from `_sid_buf`/`_acl_buf` before
/// they were moved into this struct stay valid.
pub(crate) struct OwnerOnlyDacl {
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
    pub(crate) fn build() -> io::Result<Self> {
        Self::build_with(false)
    }

    /// Like [`OwnerOnlyDacl::build`], for a directory: the single ACE is
    /// inherited by every file and directory created inside it, and the
    /// DACL is protected, so no ACE of the parent directory is inherited.
    pub(crate) fn build_inheritable() -> io::Result<Self> {
        Self::build_with(true)
    }

    #[allow(unsafe_code)] // Localized: each Win32 Security-API call below is individually justified.
    fn build_with(inheritable: bool) -> io::Result<Self> {
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
        let acl_len =
            size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_len;
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
        // requires -- `AddAccessAllowedAce` appends within the
        // buffer `InitializeAcl` already sized to fit exactly one such
        // ACE.
        let ace_flags = if inheritable {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        };
        if unsafe { AddAccessAllowedAceEx(acl_ptr, ACL_REVISION, ace_flags, GENERIC_ALL, sid) } == 0
        {
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
        // descriptor that would grant broad access.
        if unsafe { SetSecurityDescriptorDacl(sd_ptr, 1, acl_ptr, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `sd_ptr` is the initialized descriptor above. Setting
        // `SE_DACL_PROTECTED` only changes its control bits: the parent
        // directory's inheritable ACEs are then not merged into the DACL of
        // an object created with this descriptor.
        if inheritable
            && unsafe { SetSecurityDescriptorControl(sd_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED) }
                == 0
        {
            return Err(io::Error::last_os_error());
        }

        Ok(OwnerOnlyDacl {
            _sid_buf: sid_buf,
            _acl_buf: acl_buf,
            sd,
        })
    }

    /// A raw pointer at this DACL's `SECURITY_DESCRIPTOR`, suitable for
    /// `SECURITY_ATTRIBUTES::lpSecurityDescriptor`. Borrows `self`
    /// mutably purely to produce a `*mut` (no interior unsafe -- taking
    /// a field's address via `addr_of_mut!` does not dereference it).
    pub(crate) fn as_ptr(&mut self) -> *mut c_void {
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
    // handle.
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
        return Err(io::Error::other(
            "GetTokenInformation(TokenUser) reported a zero required buffer size",
        ));
    }

    let mut buf = vec![0u8; needed as usize];
    // SAFETY: `buf` is freshly allocated with EXACTLY `needed` bytes (the
    // size the probe call above reported); `GetTokenInformation` writes
    // at most that many bytes into it (Win32 contract for a
    // correctly-sized buffer).
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };

    // SAFETY: `token` is the valid handle opened above, closed exactly
    // once here regardless of the read's success/failure.
    unsafe { CloseHandle(token) };

    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf)
}
