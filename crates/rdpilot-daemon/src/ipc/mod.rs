//! Local-user-scoped IPC transport — cfg-gated Unix/Windows listener +
//! shared framing (Plan 12-04; DAEMON-02).
//!
//! Stub for now (Plan 12-02) — filled in by Plan 12-04.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

mod framing;
