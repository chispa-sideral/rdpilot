# Phase 10: SDK File-Transfer Extension - Research

**Researched:** 2026-07-10
**Domain:** MS-RDPEFS (RDPDR) drive-redirection extension + sensor-mediated file transfer trigger
**Confidence:** MEDIUM (protocol/crate facts HIGH; real-Windows chunking behavior is empirical/unverifiable offline — see Item 1)

## Summary

This phase generalizes the already-live `RdpilotDriveBackend` (Phase 5) from a single
read-only served file to a bidirectional, allow-listed share, and adds a sensor-mediated
trigger so the remote Windows OS actually issues the Read/Write IRPs the hybrid design
requires. All three of CONTEXT's flagged open-research items were resolved by reading the
pinned `ironrdp-rdpdr-0.6.0` crate source directly (it ships on this machine's Cargo
registry cache) plus the existing `connect.rs`/`session.rs`/`sensor.rs`/`Program.cs` code.

**Item 1 (chunk size) resolves to a negative-but-actionable finding**: neither MS-RDPEFS
nor the pinned crate define a fixed/negotiated maximum per-IRP `Length` for
`DeviceReadRequest`/`DeviceWriteRequest` — the field is a raw `u32` and the
`GeneralCapabilitySet` the crate encodes carries no max-transfer-size sub-field. The real
per-IRP chunk size is dictated entirely by the remote Windows OS's own redirected-drive
I/O stack (out of scope of the crate and the spec's own capability negotiation), and is
empirically observable only at a live gate, exactly like Phase 5's `rdpsnd`-stub and
`QueryInformation`/`QueryVolumeInformation` findings were. **This does not block planning**
— D-10.3's design (bounded per-IRP seek+write, loop until `Close`) already handles any
chunk size the OS chooses; FILE-04's test should pick a file comfortably larger than any
plausible chunk (low hundreds of KB to a few MB) and assert the loop actually iterated
more than once (by IRP count or byte-offset progression), rather than hardcoding an
assumed byte constant.

**Item 2 (piggyback gating) is CONFIRMED, no edge case found**: `connect.rs` lines 127-145
gate RDPDR registration (drive backend + `rdpsnd` stub) behind a single
`if let Some(sensor_path) = cfg.get_sensor_binary_path()` block; there is no other RDPDR
registration path anywhere in the crate. `config.rs`'s `sensor_binary_path`/
`get_sensor_binary_path` builder pair is a plain `Option<PathBuf>` with no other mutator —
the gate cannot be bypassed or left half-registered.

**Item 3 (remote-side byte I/O) resolves to plain .NET file I/O against the SAME UNC path
the bootstrap already uses**: `session.rs`'s `launch_command()` proves the live-verified
convention `\\tsclient\RDPILOT\<name>` (drive name `"RDPILOT"` from `connect.rs` line 131).
The C# sensor process (a full Win32 process, not a typed shell command) can open that UNC
path directly via `System.IO.FileStream`/`File` APIs — no drive-letter mapping (`net use`)
exists or is needed anywhere in this codebase. For upload, the sensor reads FROM
`\\tsclient\RDPILOT\<name>` (driving Read IRPs into the already-working `handle_read` path)
and writes TO a validated real remote-machine path. For download, it reads a validated real
remote-machine path and writes TO the UNC path (driving the NEW Write/SetInformation IRPs
this phase implements). See "Code Examples" and "Common Pitfalls" for the recommended
stream-with-inline-hash shape.

**Primary recommendation:** Implement `DeviceWriteRequest`/`ServerDriveSetInformationRequest`
per D-10.3's staged-temp-file design; canonicalize with a manual `\`→`/` normalization step
BEFORE any `Path`/`canonicalize` call (a genuine cross-platform gap — see Pitfall 1); use
`sha2` (already resolved transitively in `Cargo.lock` at 0.11.0) and BCL `SHA256` for D-10.5;
drive the C# sensor's remote-side I/O via `FileStream` copy loops (not `File.Copy`) so the
checksum is computed in the same pass.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Local-disk file I/O (share root read/write) | RDP client / SDK (Rust, `RdpilotDriveBackend`) | — | Owns the local filesystem; the only code with direct `std::fs` access to the local share root |
| RDPDR IRP protocol handling (Create/Read/Write/SetInformation) | RDP client / SDK (Rust, `ironrdp-rdpdr` + `RdpilotDriveBackend`) | — | Static virtual channel is client-side terminated per MS-RDPEFS; server (remote Windows) only issues IRPs, never processes them |
| Remote-disk file I/O (real remote path read/write) | Remote sensor (C#, Windows target) | — | Only code running ON the remote machine can touch its real local filesystem outside the redirected-drive abstraction |
| Transfer trigger / orchestration (which file, which direction, when) | Remote sensor (C#) via DVC command | SDK (Rust, `Session::upload_file`/`download_file`) | RDPDR IRPs are server-initiated only (MS-RDPEFS); nothing on the client side can make Windows open/read/write a file on demand — the sensor DVC trigger is the only way to originate the action remotely |
| Public API surface (`upload_file`/`download_file`, error taxonomy) | SDK (Rust, `session.rs`/`error.rs`) | — | D-09 crate-internal-only: callers never see `ironrdp-rdpdr` types |
| Checksum computation | Both sides independently (Rust `sha2`, C# `SHA256`) | — | D-10.5: symmetric verification, neither side trusts the other's reported hash without recomputing |

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| FILE-01 | User can upload a file local→remote to a named session | Item 3 (remote sensor writes real path via `FileStream`, reads staged bytes from `\\tsclient\RDPILOT\<name>`); D-10.3 write path |
| FILE-02 | User can download a file remote→local from a named session | Item 3 (remote sensor reads real path, writes to `\\tsclient\RDPILOT\<name>` driving new Write IRPs); D-10.5 checksum |
| FILE-03 | Transfer validates remote paths via canonicalization to prevent path traversal / arbitrary write (BLOCKING) | Pitfall 1 (cross-platform separator normalization), Pitfall 2 (canonicalize-of-nonexistent-path), Pitfall 3 (C# `StartsWith` prefix bypass), FreeRDP CVE-2025-48817 finding |
| FILE-04 | Large files transfer reliably (chunked); partial-transfer failure is detected and surfaced | Item 1 (no fixed chunk size — design for arbitrary OS-chosen chunking); D-10.3 staged-rename gives free interrupted-transfer detection |

## Standard Stack

### Core (already pinned — no version change needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ironrdp-rdpdr` | 0.6.0 (pinned, `Cargo.toml`) | MS-RDPEFS PDU types + SVC dispatch | Already load-bearing since Phase 5; this phase only implements two more of its 11 `ServerDriveIoRequest` variants |
| `sha2` | 0.11.0 | SHA-256 checksum (Rust side, D-10.5) | Already present in `Cargo.lock` as a resolved transitive dependency (verified — see below); RustCrypto's reference pure-Rust implementation |

**Version verification:**
```
$ grep -A2 'name = "sha2"' Cargo.lock
name = "sha2"
version = "0.11.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
```
`[VERIFIED: Cargo.lock]` — `sha2` 0.11.0 is ALREADY resolved in this workspace's lockfile
(a transitive dependency, most likely pulled in by the `x509-cert`/certificate-parsing
chain used in `connect.rs`'s TLS handshake). Promoting it to a direct dependency in
`crates/rdpilot/Cargo.toml` does not introduce a new supply-chain node — the exact version
already exists in the dependency graph and its checksum is already pinned. `rust-version`
declared by the `sha2-0.11.0` crate manifest is `1.85`; the workspace's
`stable-x86_64-pc-windows-gnu` toolchain (unpinned to an exact version, tracks current
stable) is well above this floor.

### Supporting (C# / BCL — no new NuGet)

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `System.Security.Cryptography.SHA256` | BCL (.NET 8+, in-box) | SHA-256 checksum (C# side, D-10.5) | Always — zero new NuGet, confirmed NativeAOT-safe (BCL crypto primitives are part of the officially-supported NativeAOT trimming/AOT feature set; no reflection-based crypto provider lookup is used by the static `SHA256.HashData`/`IncrementalHash` APIs) `[CITED: learn.microsoft.com — Native AOT deployment compatibility]` |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `sha2` (Rust) | `blake3` | Rejected per D-10.5 — unvetted C# AOT NuGet equivalent breaks symmetry; `sha2` already resolved in the graph, zero marginal cost |
| BCL `SHA256` (C#) | `System.Security.Cryptography.MD5`/CRC32 | Rejected per D-10.5 — non-cryptographic/weak, inappropriate for a security-adjacent transfer verifying integrity across an untrusted-remote boundary |
| Staged-temp-file + atomic rename (D-10.3) | Direct in-place write | Rejected — an interrupted direct write leaves silently-corrupted output indistinguishable from a complete file; staging is what makes FILE-04's "clean, detectable failure" free |

**Installation:**
```bash
# Rust side — promotes the already-resolved transitive dep to a direct one
cd crates/rdpilot && cargo add sha2
# C# side — no installation, BCL-only
```

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|-------------|
| `sha2` | crates.io | Years (RustCrypto `hashes` monorepo, long-established) | Very high (foundational crypto crate, transitively pulled by countless crates incl. this workspace's own TLS stack) | github.com/RustCrypto/hashes | `[OK]` | Approved |

**Packages removed due to slopcheck `[SLOP]` verdict:** none
**Packages flagged as suspicious `[SUS]`:** none

`slopcheck install sha2` ran successfully against the live crates.io registry (its
registry-check phase, independent of the follow-on `cargo add` invocation which failed only
because this Linux research host has no `cargo` binary on `PATH` — an environment gap, not
a slopcheck finding) and returned `[OK] sha2 (crates.io)`. No C# package is installed in
this phase (BCL-only, D-10.5) so no Node/npm/pip-equivalent audit applies there.

## Architecture Patterns

### System Architecture Diagram

```
 LOCAL (rdpilot SDK, Rust)                          REMOTE (Windows target)
┌──────────────────────────┐                       ┌──────────────────────────┐
│ Session::upload_file()   │                       │                          │
│ Session::download_file() │                       │                          │
│        │                 │                       │                          │
│        ▼                 │   RDPILOT_SENSOR DVC   │                          │
│ sensor_request(           │──────────────────────▶│  Program.cs dispatch    │
│  MsgType::FileTransfer,   │   {op, remote_name,    │  new "else if" arm      │
│  {op, remote_name,        │    local_share_name}  │        │                 │
│   local_share_name})      │                       │        ▼                 │
│        ▲                 │◀──────────────────────│  FileTransfer handler    │
│        │  {success,       │   {success, bytes,     │  (new .cs file)          │
│        │   bytes,         │    sha256, error}     │        │                 │
│        │   sha256}        │                       │        ▼                 │
│        │                 │                       │  FileStream copy loop:   │
│        │                 │                       │  read real remote path   │
│        │                 │                       │  <-> UNC share path,     │
│        │                 │                       │  hashing inline          │
│        │                 │                       │        │                 │
│  RdpilotDriveBackend      │◀═══ MS-RDPEFS IRPs ═══│  (Windows redirected-    │
│  (Create/Read/Write/      │  (Create, Read/Write,  │   drive I/O manager      │
│   SetInformation)          │   SetInformation,      │   issues these IRPs     │
│        │                 │   Close — static SVC) │   as a SIDE EFFECT of    │
│        ▼                 │                       │   the FileStream copy    │
│  <share_root>/            │                       │   above touching         │
│  .rdpilot-staging/         │                       │   \\tsclient\RDPILOT\…) │
│   <uuid>.part              │                       │                          │
│        │ atomic rename     │                       │                          │
│        ▼ (on clean Close)  │                       │                          │
│  <share_root>/<name>       │                       │                          │
└──────────────────────────┘                       └──────────────────────────┘
```

Read left-to-right/top-to-bottom for the primary use case: a caller invokes
`Session::download_file`, which round-trips a NEW sensor command over the existing DVC
(the only new wire surface in this phase); the sensor, entirely independently, performs
ordinary `FileStream` I/O against a UNC path — and it is THAT ordinary file I/O, not
anything the Rust SDK does directly, which causes Windows to emit the RDPDR IRPs that
`RdpilotDriveBackend` answers. The two channels (DVC command/reply, and RDPDR IRP
stream) are concurrent and uncorrelated at the wire level; correctness is entirely a
property of the staged-file design (D-10.3) plus the sensor waiting for its own
`FileStream` operation to complete before replying success.

### Recommended Project Structure

```
crates/rdpilot/src/
├── rdpdr_backend.rs       # RdpilotDriveBackend: generalize served_path/served_name to
│                          # share_root; implement handle_write, handle_set_information
├── config.rs              # ConnectionConfig: add share_root builder (D-10.1)
├── session.rs              # Session::upload_file/download_file (D-10.4)
├── sensor.rs                # MsgType::FileTransfer (+ payload/response DTOs)
└── error.rs                  # Error::PathTraversal, Error::ChecksumMismatch (D-10.4)

sensor/
├── Program.cs                 # new dispatch arm, BuildFileTransferReplyEnvelope
└── FileTransfer.cs             # NEW: path validation + FileStream copy-with-hash handler
```

### Pattern 1: Canonicalize-of-nonexistent-path (both languages need a workaround)

**What:** `std::fs::canonicalize` (Rust) and, less severely, path validation logic in
general must handle the fact that the FINAL destination path frequently does not exist
yet at validation time (a brand-new upload, or the staged `.part` file).

**When to use:** Every path validation in this phase — both the Rust `RdpilotDriveBackend`
side (D-10.2) and the C# sensor side.

**Example (Rust):**
```rust
// Source: std::fs::canonicalize docs (doc.rust-lang.org/std/fs/fn.canonicalize.html)
// — "This function ... will return an error if `path` does not exist."
// Canonicalize the PARENT (which always exists — it's under the fixed, pre-created
// share root / staging dir), then re-join the untrusted leaf component, then do the
// ancestry check against the canonicalized root — never canonicalize the untrusted
// full path directly when the target may not exist yet.
fn resolve_under_root(root_canonical: &Path, untrusted_relative: &str) -> Result<PathBuf, Error> {
    // Normalize BOTH separator styles before any Path operation (Pitfall below) —
    // this must happen regardless of host OS.
    let normalized = untrusted_relative.replace('\\', "/");
    let candidate = root_canonical.join(normalized.trim_start_matches('/'));
    let parent = candidate.parent().ok_or_else(|| Error::path_traversal("no parent"))?;
    let parent_canonical = std::fs::canonicalize(parent)
        .map_err(|_| Error::path_traversal("parent does not resolve under share root"))?;
    if !parent_canonical.starts_with(root_canonical) {
        return Err(Error::path_traversal("escapes share root"));
    }
    // Re-join the (still untrusted) leaf filename onto the CANONICAL parent —
    // component-wise, not string concatenation.
    let leaf = candidate.file_name().ok_or_else(|| Error::path_traversal("no file name"))?;
    Ok(parent_canonical.join(leaf))
}
```

### Anti-Patterns to Avoid

- **Substring/`Contains("..")` matching:** exactly the FreeRDP `contains_dotdot()` bug
  class this phase's FILE-03 criterion cites (see Pitfall 3 below for the precise
  off-by-one). Never do this in either language.
- **Canonicalizing the full untrusted path when the leaf may not exist:** `std::fs::
  canonicalize`/`Path.GetFullPath` semantics differ here — `GetFullPath` does NOT require
  existence (Windows), `std::fs::canonicalize` DOES require existence (Rust). This
  asymmetry means the two D-10.2 validators are not simply mirror images of each other —
  see Pitfall 2.
- **String-prefix `StartsWith` without a trailing separator guard (C#):** `"/shared"
  .StartsWith` matches `/sharedEVIL` — see Pitfall 3.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| SHA-256 checksum | Custom hash loop | `sha2::Sha256` (Rust) / `System.Security.Cryptography.SHA256` (C#) | Cryptographic hash implementations are exactly the kind of code where hand-rolling introduces subtle bugs (padding, endianness) that don't show up in normal testing; both are BCL/RustCrypto reference implementations |
| Path traversal defense | Regex/substring `..` filtering | Canonicalize + component-wise ancestry check | This is literally the CVE class (FreeRDP GHSA-3xpj-m4hx-8vmx / CVE-2025-48817) this phase's BLOCKING criterion exists to avoid — substring approaches have a documented, exploited off-by-one history |
| Atomic "did the whole file arrive" detection | Manual byte-counter + flag file | Staged temp file + atomic rename (already D-10.3) | Atomic rename on the same filesystem is a single syscall on both POSIX (`rename(2)`) and Windows (`MoveFileEx`) — no window where a reader can observe a partially-written destination filename |

**Key insight:** every hand-roll risk in this phase clusters around the same root cause —
untrusted input (a remote-supplied path string) crossing a trust boundary. The existing
D-10.1/D-10.2 decisions already avoid the two biggest hand-roll traps (single hard-coded
allow-list → configurable canonicalized root; substring matching → real ancestry check).
The one NEW hand-roll risk this research surfaces is the cross-platform separator
handling in Pitfall 1 below, which is not covered by either D-10.2's Rust or C# validator
description as written and should be called out explicitly to the planner.

## Common Pitfalls

### Pitfall 1: Backslash is not a path separator on the (Linux) test host — FLAGS a gap in D-10.2 as written

**What goes wrong:** D-10.2 specifies "Rust `RdpilotDriveBackend` validates every
RDPDR-supplied path against the LOCAL share root using `std::fs::canonicalize` + `Path`
ancestry check." On the real Windows production target, `\` IS a path separator to
`std::path` (Rust's `std::path` on `cfg(windows)` splits on both `\` and `/`). But this
codebase's own established pattern (STATE.md, repeated across Phases 6/7/9) is to compile
and unit-test on the native `x86_64-unknown-linux-gnu` host because the pinned
`x86_64-pc-windows-gnu` MinGW toolchain is not installed here — and on Linux, `std::path`
treats `\` as an ordinary filename character, NOT a separator. A test string like
`"..\\..\\Windows\\System32"` becomes ONE opaque path component on Linux, not four —
`Path::ancestors()`/`.starts_with()` behave completely differently than they will on the
real Windows target, and the FILE-03 adversarial suite's "mixed `/`/`\` separator" case
could spuriously pass offline while exercising none of the real traversal-defense logic.

**Why it happens:** `std::path`'s separator behavior is `cfg(windows)`-gated by design
(this is correct/intentional Rust stdlib behavior, not a bug) — but it means offline
Linux-host tests of THIS SPECIFIC validator are not a faithful proxy for the real target
the way they have been for previous phases' pure-logic code (`session.rs`/`sensor.rs`
have no `cfg(windows)` code, per multiple STATE.md notes — but path-separator semantics
ARE platform-conditional stdlib behavior, an important distinction).

**How to avoid:** Manually normalize `\` → `/` (or vice versa) on the untrusted RDPDR
`path` string BEFORE any `Path`/`PathBuf` construction, unconditionally, regardless of
host OS. This makes the validator's behavior deterministic and identical on both the
Linux test host and the Windows production target, and makes the offline FILE-03 suite a
faithful proxy again. (The C# side does not have this problem — `Path.GetFullPath` on the
.NET runtime that ships with the sensor always runs ON Windows, where both separators are
native.)

**Warning signs:** A FILE-03 test case with a `\`-separated adversarial string passes
offline but the plan has no live-gate re-confirmation of that specific case on the real
Windows target — flag this combination for the plan-checker.

### Pitfall 2: `std::fs::canonicalize` requires existence; `Path.GetFullPath` does not — D-10.2's two validators are NOT mirror images

**What goes wrong:** For the write/upload path, the DESTINATION file frequently does not
exist yet (that's the point of a `Write`/`Create` IRP with `FILE_CREATE`/`FILE_OPEN_IF`
disposition). Rust's `std::fs::canonicalize` returns `Err(NotFound)` for a nonexistent
path — canonicalizing the untrusted full destination path directly will always fail for
new files, forcing an incorrect design (e.g., "reject all new-file writes") unless the
code canonicalizes the PARENT directory instead (see Code Example above). C#'s
`Path.GetFullPath` has no such existence requirement and will happily lexically resolve a
nonexistent path — which means the two validators, despite both being described as
"canonicalize + ancestry check" in D-10.2, need structurally different implementations.

**Why it happens:** This is standard, documented behavior of both APIs, not a bug —
but D-10.2 as written could be read as "the same algorithm on both sides," which it
cannot literally be.

**How to avoid:** Rust side: canonicalize the parent directory (always exists — it is
either the share root itself or `<share_root>/.rdpilot-staging/`, both created by the
SDK at connect/first-use time), then join+ancestry-check. C# side: `Path.GetFullPath`
directly on the full candidate path is fine (no existence requirement), but MUST be
followed by an ancestry check that is component-aware, not a raw string prefix check
(see Pitfall 3).

**Warning signs:** A "create a brand-new file at a legitimate nested path" test case
failing where it should succeed is the symptom of over-applying canonicalize-requires-
existence to the untrusted leaf rather than just the parent.

### Pitfall 3: C# `string.StartsWith` prefix bypass — sibling-directory escape (`/shared` vs `/sharedEvil`)

**What goes wrong:** `Path.GetFullPath(candidate).StartsWith(Path.GetFullPath(root))` is a
classic, well-documented .NET path-traversal footgun: if the share root is
`C:\rdpilot-share` and an attacker's resolved path is `C:\rdpilot-share-evil\secret.txt`,
the raw string `"C:\rdpilot-share-evil\secret.txt".StartsWith("C:\rdpilot-share")` is
`true` even though the resolved path is a SIBLING directory, not a descendant.

**Why it happens:** `string.StartsWith` has no concept of path components; it is a pure
character-sequence check.

**How to avoid:** Either (a) ensure the canonical root string used for comparison always
ends with `Path.DirectorySeparatorChar` before the `StartsWith` call (so
`"C:\rdpilot-share\"` does not prefix-match `"C:\rdpilot-share-evil\..."`), or (b) use
`Path.GetRelativePath(root, candidate)` and reject if the result starts with `".."` or is
rooted (an absolute path means `GetRelativePath` could not express it as a descendant).
Option (b) is the more robust of the two and is the recommended approach — it is also
what most current .NET path-traversal guidance (OWASP .NET cheat sheet class of advice)
recommends over raw `StartsWith`. `[ASSUMED]` — general .NET security guidance from
training knowledge; not verified against a specific current Microsoft doc page this
session, but the underlying `StartsWith` prefix-bypass class of bug is extremely well
established and low-risk to state as fact.

**Warning signs:** A FILE-03 test case using a share root name that is a strict prefix of
a sibling directory name will only catch this if such a test case is explicitly included —
recommend the planner add one.

### Pitfall 4: MS-RDPEFS device-list filter can silently swallow file-transfer IRPs after a stale drive registration

**What goes wrong:** `ironrdp-rdpdr`'s `handle_device_io_request` (lib.rs, quoted above)
silently drops (`Ok(vec![])`, no response sent) any IRP whose `device_id` is not in the
currently-announced device list ("If a request is received that contains a DeviceId field
that was not announced by the client ... the request SHOULD be ignored"). This is existing
crate behavior, not new in this phase, but the NEW write/staging code path introduces more
opportunities for a request to arrive after some kind of drive re-announcement or
reconnect race, so a silently-dropped IRP (rather than an explicit error) is a plausible
symptom during implementation.

**Why it happens:** Per-spec silent-ignore behavior for unknown device IDs, intentional
per MS-RDPEFS.

**How to avoid:** If a file-transfer test hangs (sensor times out waiting for the copy to
finish) rather than failing fast, suspect a dropped IRP due to device-list mismatch before
suspecting the new write-path logic itself.

**Warning signs:** Sensor-side `FileTransfer` handler timeout with no corresponding
`NtStatus` ever observed on the wire (vs. an explicit `NOT_SUPPORTED`/`ACCESS_DENIED`,
which would indicate the IRP DID reach the backend).

## Code Examples

### `DeviceWriteRequest` / `ServerDriveSetInformationRequest` shapes (D-10.3 implementation targets)

```rust
// Source: pinned crate source, ~/.cargo/registry/src/.../ironrdp-rdpdr-0.6.0/src/pdu/efs.rs
// (this exact file/line is what CONTEXT's rdpdr_backend.rs:507-508 currently rejects)

// efs.rs:3526-3530
pub struct DeviceWriteRequest {
    pub device_io_request: DeviceIoRequest,
    pub offset: u64,          // seek position for this IRP's write
    pub write_data: Vec<u8>,  // the bytes to write — length is NOT protocol-capped (Item 1)
}

// efs.rs:3567-3570 — the response you must build
pub struct DeviceWriteResponse {
    pub device_io_reply: DeviceIoResponse,
    pub length: u32,          // MUST echo the number of bytes actually written
}

// efs.rs:3598-3601 — the second currently-NOT_SUPPORTED IRP
pub struct ServerDriveSetInformationRequest {
    pub device_io_request: DeviceIoRequest,
    pub set_buffer: FileInformationClass,  // decodes to one of 5 variants — see below
}

// efs.rs:3611-3616 — decode() only accepts these 5 FileInformationClassLevel values for
// SetInformation (anything else is a hard decode error, not something you branch on):
//   FILE_BASIC_INFORMATION | FILE_END_OF_FILE_INFORMATION | FILE_DISPOSITION_INFORMATION
//   | FILE_RENAME_INFORMATION | FILE_ALLOCATION_INFORMATION
//
// For D-10.3's staged-write path, the one you actually need is FILE_END_OF_FILE_INFORMATION
// (efs.rs:3639-3654):
pub struct FileEndOfFileInformation {
    pub end_of_file: i64,  // Windows sends this to declare/truncate final file size —
                            // this is your authoritative "expected total bytes" signal
                            // for deciding when a staged .part file is complete.
}

// efs.rs:3747-3761 — the response you must build for ANY SetInformation, regardless of
// which of the 5 sub-variants was set:
pub struct ClientDriveSetInformationResponse {
    device_io_reply: DeviceIoResponse,
    length: u32,  // "MUST be equal to the Length field in the ... Request" — a helper
                  // constructor already exists: ClientDriveSetInformationResponse::new(&req, io_status)
}
```

### Recommended C# sensor-side transfer shape (inline hash, single I/O pass)

```csharp
// Not sourced from an existing file — new code for this phase, following the
// BuildXReplyEnvelope / try-catch-never-throw dispatch discipline of
// BuildLaunchProcessReplyEnvelope / BuildUiaTreeReplyEnvelope (Program.cs:475-524).
//
// Recommendation: use FileStream + IncrementalHash in one pass instead of File.Copy
// followed by a separate hashing read — File.Copy alone would mean the RDPDR channel's
// bytes get walked twice (once for the copy, once to re-open and hash), doubling
// wire/IO time for large files with zero benefit.
using var source = new FileStream(sourcePath, FileMode.Open, FileAccess.Read);
using var dest = new FileStream(destPath, FileMode.Create, FileAccess.Write);
using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
byte[] buffer = new byte[65536];
long total = 0;
int read;
while ((read = source.Read(buffer, 0, buffer.Length)) > 0)
{
    dest.Write(buffer, 0, read);
    hasher.AppendData(buffer, 0, read);
    total += read;
}
byte[] digest = hasher.GetHashAndReset();
// digest -> Convert.ToHexString(digest) for the wire reply payload
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Single hard-coded served file (Phase 5 `RdpilotDriveBackend`) | Configurable share root + staged writes (this phase, D-10.1/D-10.3) | This phase | Generalizes the drive backend from a bootstrap-only mechanism to a general-purpose transfer channel |
| Substring `..` filtering for RDPDR path traversal (historical FreeRDP `contains_dotdot()`) | Full canonicalize + component-wise ancestry check (D-10.2) | FreeRDP fixed in 3.25.0 (per GHSA-3xpj-m4hx-8vmx, off-by-one on trailing `..` with no separator) | Exactly the CVE class this phase's FILE-03 BLOCKING criterion exists to preempt — this project adopts the fix pattern from day one rather than the vulnerable pattern |

**Deprecated/outdated:**
- FreeRDP versions ≤ 3.24.2's `drive_file.c` `contains_dotdot()` — off-by-one on
  trailing `..` with no following separator (e.g. `/shared/..`), fixed in FreeRDP 3.25.0.
  Not applicable to this codebase directly (different implementation, IronRDP not
  FreeRDP) but directly informs why D-10.2 mandates canonicalization over substring
  matching, and gives a concrete adversarial test case worth adding to FILE-03's suite
  (a trailing `..` with NO separator after it, matching the exact off-by-one — CONTEXT
  already lists "trailing `..` with no separator" as a required case, this confirms it's
  not a hypothetical).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The real per-IRP chunk size for Read/Write IRPs against a redirected drive is small enough that a "few hundred KB to a few MB" test file reliably exercises multiple IRPs | Summary / Item 1 / FILE-04 | If the real Windows chunk size for FileStream-driven sequential copy is unexpectedly large (e.g. single-IRP transfers up to several MB), a test file sized in the low hundreds of KB might complete in ONE IRP, silently failing to exercise the "loop actually loops" behavior FILE-04 requires — mitigate by asserting IRP/offset progression directly (log offsets seen) rather than only asserting final correctness, and re-confirm the real number at the live gate exactly like Phase 5 did for `rdpsnd`/`QueryInformation` |
| A2 | .NET's BCL `SHA256`/`IncrementalHash` APIs are fully Native AOT compatible with zero trim warnings | Standard Stack, Supporting | LOW risk — this is standard, widely-used, officially-supported .NET AOT surface (unlike, e.g., reflection-heavy serializers); if wrong, the existing NativeAOT publish step (already proven working for the sensor since Phase 5) would surface a trim/AOT warning at build time, not a silent runtime failure |
| A3 | `Path.GetRelativePath` + `".."`-prefix check is the recommended C#-side ancestry check (Pitfall 3) over a guarded `StartsWith` | Common Pitfalls, Pitfall 3 | LOW-MEDIUM risk — if a subtle edge case in `GetRelativePath`'s handling of UNC paths or drive-letter case-sensitivity differs from expectation, the planner should still require the FILE-03 BLOCKING test suite to run against whichever approach is actually implemented, which will catch this regardless of which specific API is chosen |

## Open Questions

1. **Exact real-Windows per-IRP chunk size (Item 1, cannot be resolved offline)**
   - What we know: neither the MS-RDPEFS spec nor the pinned `ironrdp-rdpdr-0.6.0` crate
     impose or negotiate a fixed maximum; the field is a raw `u32`.
   - What's unclear: what the real remote Windows OS's redirected-drive I/O stack
     actually requests per IRP when the C# sensor performs a `FileStream` sequential
     copy against `\\tsclient\RDPILOT\...` — this can only be observed on a live gate
     (same category of finding as Phase 5's `rdpsnd`-stub requirement and the two
     `QueryInformation`/`QueryVolumeInformation` IRPs, which were also NOT predictable
     from the spec alone and required live wire-trace diagnosis).
   - Recommendation: do not hardcode an assumed chunk-size constant anywhere in the
     implementation or tests. Design FILE-04's large-file test to log/assert on IRP
     count or offset progression (not just final-file correctness), and record the
     actual observed chunk size in STATE.md at the live gate — mirroring exactly how
     Phase 5 recorded its live-diagnosed findings.

2. **Staging directory cleanup policy (explicitly Claude's Discretion per CONTEXT, not re-litigated here)**
   - What we know: D-10.3 creates `<share_root>/.rdpilot-staging/<uuid>.part` per
     transfer; CONTEXT already marks the cleanup policy as discretionary.
   - What's unclear: nothing new from research — flagging only so the planner
     remembers this is still open and not accidentally treated as "resolved by
     research."
   - Recommendation: simplest viable approach per CONTEXT's own suggestion (best-effort
     cleanup of stale `.part` files on next connect) is sufficient; not a blocking
     research question.

## Environment Availability

This phase adds no new external tool/service dependency beyond what Phases 1-9 already
established (the pinned Rust toolchain, the existing `ironrdp-rdpdr` crate already in
`Cargo.lock`, and the existing NativeAOT sensor build pipeline). `cargo`/`rustc` were not
on this Linux research host's `PATH` (confirmed above when `slopcheck`'s follow-on `cargo
add` step failed) — this mirrors the exact substitution pattern STATE.md documents for
every prior phase (offline verification on native `x86_64-unknown-linux-gnu`, real
`x86_64-pc-windows-gnu` build/live-gate deferred to the pinned dev machine). No new
Environment Availability gap is introduced by this phase.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | `cargo test` (Rust, existing) |
| Config file | none — plain `cargo test` in `crates/rdpilot` |
| Quick run command | `cargo test -p rdpilot --lib rdpdr_backend::` |
| Full suite command | `cargo test -p rdpilot` (offline unit tests; `tests/live_session.rs` gated tests require a live VM per existing Phase 5-9 pattern) |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| FILE-01 | Upload local→remote, verified present | unit (offline, mocked IRPs) + live gate | `cargo test -p rdpilot --lib rdpdr_backend::` | ❌ Wave 0 — new write-path tests needed alongside the existing `rdpdr_backend.rs` `#[cfg(test)] mod tests` |
| FILE-02 | Download remote→local, size/checksum match | unit (offline) + live gate | `cargo test -p rdpilot --lib rdpdr_backend::` | ❌ Wave 0 — checksum comparison helper needed |
| FILE-03 | **BLOCKING** adversarial path-traversal suite (both validators) | unit (offline, no live VM needed — pure path logic) | `cargo test -p rdpilot --lib rdpdr_backend::path_traversal` (Rust side); a C#-side equivalent test project/harness for the sensor validator | ❌ Wave 0 — this is the highest-priority new test file; can and should be fully offline (see Pitfall 1 for why the offline result IS trustworthy once separators are normalized correctly) |
| FILE-04 | Large file, chunked loop, interrupted-transfer detection | unit (offline, synthetic multi-IRP sequence) + live gate (real chunk size, Open Question 1) | `cargo test -p rdpilot --lib rdpdr_backend::` + live VM gate | ❌ Wave 0 — needs a synthetic-multi-IRP unit test (feed the backend 3+ sequential Write IRPs directly, bypassing the real wire) as the offline proxy, since the REAL chunk count can't be controlled offline |

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot --lib rdpdr_backend::`
- **Per wave merge:** `cargo test -p rdpilot` (full offline suite)
- **Phase gate:** Full offline suite green, PLUS a live gate against a disposable Azure VM
  (same pattern as every prior phase) before `/gsd-verify-work`, given FILE-03 is BLOCKING
  and FILE-04's real chunk-size behavior is only observable live.

### Wave 0 Gaps
- [ ] Offline unit tests for `handle_write`/`handle_set_information` in
      `rdpdr_backend.rs`'s existing `#[cfg(test)] mod tests` block (follow the existing
      `write_temp_file`/`decode_io_status_and_tail` helper patterns already in that file)
- [ ] A dedicated path-traversal adversarial test module (Rust side) covering: trailing
      `..` with no separator, mixed `/`+`\`, absolute-path-as-relative, AND the
      sibling-directory-prefix case (Pitfall 3's class, mirrored on the Rust side even
      though Rust's `Path::starts_with` is not vulnerable to it, as a regression guard)
- [ ] A C#-side test harness for the sensor's path validator (no existing C# test project
      was found in `sensor/` — confirm with the planner whether one exists or needs
      creating; existing sensor validation to date has relied on the live gate only,
      per Phase 5-9 STATE.md history)
- [ ] Framework install: none — `cargo test` already fully configured

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | Out of scope — this phase adds no new auth surface |
| V3 Session Management | no | Out of scope |
| V4 Access Control | yes | Canonicalization + ancestry check under a fixed allow-listed root (D-10.2), continuing the T-05-04 posture — never substring/`Contains` matching |
| V5 Input Validation | yes | Every RDPDR-supplied path string and every sensor-command payload field is untrusted input validated before touching `std::fs`/`System.IO` (existing `reject_unsupported`/D-10.2 pattern) |
| V6 Cryptography | yes | SHA-256 via `sha2` (Rust, RustCrypto reference impl) and BCL `SHA256` (C#) — never hand-rolled (D-10.5) |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Path traversal via `..`/mixed separators/absolute-as-relative (FreeRDP GHSA-3xpj-m4hx-8vmx / CVE-2025-48817 class) | Tampering / Elevation of Privilege | Canonicalize + component-wise ancestry check on BOTH sides of the trust boundary (D-10.2), never substring matching; explicit separator normalization before any `Path` operation (Pitfall 1) |
| Unbounded per-IRP allocation (a malicious/buggy remote requesting an enormous `Length`) | Denial of Service | Bounded per-IRP read/write, never buffer the whole file in memory (D-10.3, continuing T-05-05's existing `Read::take` discipline already proven in `read_served_bytes`) |
| Silent data corruption on interrupted transfer | Tampering (integrity) | Staged temp file + atomic rename — an interrupted transfer never produces a final-named file at all (D-10.3), plus independent SHA-256 verification on both sides (D-10.5) |
| C# `string.StartsWith` prefix bypass (sibling-directory escape) | Elevation of Privilege | `Path.GetRelativePath` + `".."`-prefix/rooted check instead of raw `StartsWith` (Pitfall 3) |

## Sources

### Primary (HIGH confidence)
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ironrdp-rdpdr-0.6.0/src/pdu/efs.rs` — `DeviceReadRequest`/`DeviceWriteRequest`/`ServerDriveSetInformationRequest`/`ClientDriveSetInformationResponse`/`GeneralCapabilitySet`/`FileEndOfFileInformation` struct definitions and `decode()` bodies (read directly, this session)
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ironrdp-rdpdr-0.6.0/src/lib.rs` — `Rdpdr::handle_device_io_request`'s unknown-device-id silent-ignore behavior; `SvcProcessor::process` dispatch (read directly, this session)
- `crates/rdpilot/src/connect.rs` lines 60-64, 88-145 — RDPDR/rdpsnd-stub registration gated on `cfg.get_sensor_binary_path().is_some()`, `SENSOR_EXE_NAME`/`RDPILOT_SENSOR` constants, drive name `"RDPILOT"` (read directly, this session)
- `crates/rdpilot/src/config.rs` lines 34-228 — `ConnectionConfig`'s `sensor_binary_path`/`get_sensor_binary_path` builder-getter pair, confirmed single mutation path (read directly, this session)
- `crates/rdpilot/src/rdpdr_backend.rs` (full file) — `RdpilotDriveBackend`, existing `handle_create`/`handle_read`/`handle_query_*`, the two `reject_unsupported` call sites at lines 507-508, existing test helper patterns (read directly, this session)
- `crates/rdpilot/src/session.rs` lines 151-184, 470-540 — `launch_command()` proving the live `\\tsclient\RDPILOT\<name>` UNC convention; `sensor_request()`'s round-trip shape (read directly, this session)
- `crates/rdpilot/src/sensor.rs` lines 40-64 — `MsgType` enum, envelope shape (read directly, this session)
- `sensor/Program.cs` lines 470-545 — `BuildLaunchProcessReplyEnvelope`/`BuildUiaTreeReplyEnvelope` dispatch pattern (never-throw-out-of-dispatch discipline) (read directly, this session)
- `Cargo.lock` line 3138-3141 — `sha2` 0.11.0 already resolved as a transitive dependency (read directly, this session)
- `.planning/CONTEXT.md` (Phase 10) — D-10.1 through D-10.5, deferred open research items (read directly, this session)
- `.planning/STATE.md` Phase 5 section — `rdpsnd`-stub / `QueryInformation`/`QueryVolumeInformation` live-diagnosis history establishing the "some MS-RDPEFS behavior can only be confirmed live" pattern this research's Item 1 finding continues

### Secondary (MEDIUM confidence)
- [FreeRDP GHSA-3xpj-m4hx-8vmx security advisory](https://github.com/FreeRDP/FreeRDP/security/advisories/GHSA-3xpj-m4hx-8vmx) — `contains_dotdot()` off-by-one, trailing `..` with no separator, fixed in FreeRDP 3.25.0 — cross-verified against CVE-2025-48817 coverage (ZeroPath blog, cvedetails.com)
- [ZeroPath: CVE-2025-48817 analysis](https://zeropath.com/blog/cve-2025-48817-windows-rdp-path-traversal) — corroborates the same off-by-one mechanism from an independent write-up

### Tertiary (LOW confidence)
- General .NET `Path.GetRelativePath`-over-`StartsWith` guidance (Pitfall 3 / Assumption A3) — training-knowledge-based, not verified against a specific current Microsoft doc page this session; the underlying `StartsWith` prefix-bypass bug class itself is well-established, but the SPECIFIC recommended-API framing is `[ASSUMED]`

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — `sha2` version verified directly from `Cargo.lock`; BCL `SHA256` NativeAOT-safety is `[CITED]` general knowledge, not independently re-verified this session
- Architecture: HIGH — every claim about existing code (gating, UNC path convention, dispatch pattern, IRP structs) is read directly from source, this session
- Pitfalls: MEDIUM-HIGH — Pitfalls 1/2/4 are derived directly from verified crate/stdlib semantics (HIGH); Pitfall 3's specific C#-API recommendation is `[ASSUMED]` (LOW) though the underlying bug class is well-established
- Open Question 1 (chunk size): explicitly UNRESOLVABLE offline — flagged, not silently assumed; this is the correct, honest outcome for this specific research item, not a research gap

**Research date:** 2026-07-10
**Valid until:** 30 days (stable protocol/crate facts) — Item 1's real-Windows-behavior finding has no expiry (it must be re-confirmed live at THIS phase's gate regardless of research age, exactly like Phase 5's live-diagnosed findings were)
