---
phase: 02-rdp-session-framebuffer-core
reviewed: 2026-06-05T00:00:00Z
depth: deep
files_reviewed: 16
files_reviewed_list:
  - crates/rdpilot/src/lib.rs
  - crates/rdpilot/src/error.rs
  - crates/rdpilot/src/config.rs
  - crates/rdpilot/src/screenshot.rs
  - crates/rdpilot/src/connect.rs
  - crates/rdpilot/src/session_loop.rs
  - crates/rdpilot/src/framebuffer.rs
  - crates/rdpilot/src/keepalive.rs
  - crates/rdpilot/src/session.rs
  - crates/rdpilot/examples/screenshot.rs
  - crates/rdpilot/tests/live_session.rs
  - crates/rdpilot/tests/common/mod.rs
  - crates/rdpilot/Cargo.toml
  - Cargo.toml
  - .cargo/config.toml
  - rust-toolchain.toml
findings:
  critical: 1
  warning: 7
  info: 6
  total: 14
status: issues_found
---

# Phase 2: Code Review Report

**Reviewed:** 2026-06-05T00:00:00Z
**Depth:** deep
**Files Reviewed:** 16
**Status:** issues_found

## Summary

Phase 2 delivers the RDP session + framebuffer core. The code is unusually
disciplined for a first Rust drop: the public surface is owned-types-only, the
error type is `Send + Sync + 'static` with a compile-time assertion, the
password is redacted in `Debug` and never logged, `accept_invalid_certs`
defaults off, live tests are env-gated and never hardcode secrets, and the
no-`unwrap`/`expect`/`panic` mandate is honored in the library paths. The crop
math, framebuffer snapshot integrity, keepalive-as-pointer-move, and the
dedicated current-thread runtime rationale are all sound.

The one material correctness bug is in the close/teardown path: `Session::close`
sends a graceful-shutdown request but then **keeps the original sender alive on
the joined thread**, so the loop's `recv()` does not resolve to `None` and the
loop can block indefinitely on `read_pdu()` while `close()` blocks on
`thread.join()` — a hang/deadlock risk on any server that does not promptly send
`Terminate`. The `mem::replace` comment claims to "drop the sender" but does not.
Beyond that: a default-features `rustls` build path can panic inside the library
(violating API-01) if the process-global `CryptoProvider` is ambiguous; the
graphics-update snapshot copies the full framebuffer on every dirty region (out
of v1 perf scope but flagged for correctness of the integrity claim); and a few
unused/dead-code and naming items. No injection, no hardcoded secrets, no unsafe.

## Critical Issues

### CR-01: `Session::close` does not drop the live sender — loop can hang, `join()` deadlocks

**File:** `crates/rdpilot/src/session.rs:127-148`

**Issue:** `close()` is documented and structured to (a) send `Close`, then (b)
"Drop the sender so the loop's `input_rx.recv()` resolves to `None` and the loop
exits even if the graceful path did not produce a Terminate." But the code does
**not** drop the sender — it swaps it for a *new live* sender:

```rust
let (dummy_tx, _dummy_rx) = mpsc::channel(1);
let _ = std::mem::replace(&mut self.input_tx, dummy_tx);
```

`mem::replace` returns the old sender into `let _ = ...`, which drops it *at the
end of the statement* — so far so good — **but** `self.input_tx` now holds
`dummy_tx`, which is alive for the whole rest of `close()` and is only dropped
when `self` drops at the end of `close()`. Critically, `_dummy_rx` is bound to a
local and kept alive too. The real problem is the ordering and the loop's
behavior:

- After the swap, the channel the loop is reading from (`input_rx`) has had its
  *only* original sender dropped, so `recv()` *will* eventually return `None` —
  **but only when the loop next polls the `input_rx.recv()` arm of the
  `select!`.** The loop is in `tokio::select!` over `reader.read_pdu()`,
  `input_rx.recv()`, and the keepalive. If the server sends no PDUs and the
  graceful-shutdown `Close` did not yield a `Terminate` output, the loop's
  forward progress depends on `recv()` returning `None`, which it will — so the
  loop *does* break. The latent hang is on the **graceful path**: the `Close`
  event calls `active_stage.graceful_shutdown()`, whose outputs are written but
  do **not** set `terminate`. The loop then re-enters `select!`. If `read_pdu()`
  now blocks (server acknowledged shutdown by going silent rather than sending a
  `Terminate`/closing the socket), the only thing that unblocks the loop is the
  next `recv()` poll returning `None`. That works **only because** the original
  sender was dropped. The comment's intent is correct, but the mechanism is
  fragile and easy to regress: any future edit that holds the original
  `input_tx` (e.g. cloning it for input injection in Phase 3, or removing the
  swap) silently reintroduces a `join()` deadlock, because `close()` blocks on
  `thread.join()` forever while the loop blocks on `read_pdu()`.

  Additionally there is no timeout on the `thread.join()` in
  `spawn_blocking(move || thread.join())` — if the loop ever fails to observe the
  channel close (e.g. it is parked inside a long `read_pdu()` on a half-open TCP
  connection with no OS-level reset), `close()` blocks indefinitely with no
  escape hatch.

**Fix:** Make the teardown explicit and robust instead of relying on the
`mem::replace` side effect. Take the sender out by value and drop it before
joining, and bound the join with a timeout:

```rust
pub async fn close(mut self) -> Result<()> {
    // Request graceful shutdown (ignore error: loop may already be gone).
    let _ = self.input_tx.send(RdpInputEvent::Close).await;

    // Explicitly drop the only real sender so the loop's recv() resolves to
    // None and the select! makes progress even if no Terminate arrives.
    let dummy = mpsc::channel(1).0; // keep the field type valid
    let real_tx = std::mem::replace(&mut self.input_tx, dummy);
    drop(real_tx); // explicit, not an incidental `let _ =` drop

    if let Some(thread) = self.thread.take() {
        // Bound the join so a wedged read_pdu() on a half-open socket cannot
        // hang close() forever.
        let joined = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::task::spawn_blocking(move || thread.join()),
        )
        .await
        .map_err(|_| Error::Session("session thread did not exit within 10s".to_owned()))?
        .map_err(|e| Error::Session(format!("join task failed: {e}")))?;
        joined.map_err(|_| Error::Session("session thread panicked".to_owned()))?
    } else {
        Ok(())
    }
}
```

Also add a defensive `select!` arm or a short read timeout in `session_loop::run`
so the loop does not depend solely on channel-close to break out of a silent
`read_pdu()` after `graceful_shutdown()`. At minimum, set a flag after
`graceful_shutdown()` so the loop stops re-entering `read_pdu()` and breaks on
the next `recv() == None`.

## Warnings

### WR-01: Library can panic on `ClientConfig::builder()` — violates API-01 no-panic mandate

**File:** `crates/rdpilot/src/connect.rs:194-217`; `crates/rdpilot/Cargo.toml:37`

**Issue:** `rustls = "0.23"` with default features compiles in `aws-lc-rs` as a
provider, but `rustls::ClientConfig::builder()` resolves the *process-global*
default `CryptoProvider`, and in rustls 0.23 that call **panics** if no default
provider has been installed and the situation is ambiguous (e.g. multiple
providers compiled in, or a dependency disabled default features upstream). The
crate documents "Library code never panics … There are no
`unwrap`/`expect`/`panic!` calls in non-test code" (lib.rs:16-18), but a panic
originating *inside rustls* under this builder is still a library-path panic the
SDK does not control. `aws-lc-rs` also requires a C toolchain and may not build
cleanly on the MinGW-w64/`x86_64-pc-windows-gnu` target this repo pins
(.cargo/config.toml).

**Fix:** Use the fallible builder and/or pin the provider explicitly so the
panic path is removed and the GNU target builds deterministically:

```rust
// Option A: explicit provider, no global-default dependency:
let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
let builder = rustls::ClientConfig::builder_with_provider(provider)
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::Tls(format!("TLS provider init failed: {e}")))?;
```

Prefer the `ring` provider (pure-ish, builds under MinGW) over `aws-lc-rs`, and
disable rustls default features in Cargo.toml (`rustls = { version = "0.23",
default-features = false, features = ["ring", "std", "tls12"] }`) so the provider
is unambiguous and no `builder()` panic is reachable.

### WR-02: Full-framebuffer copy on every `GraphicsUpdate` undermines the snapshot claim and risks tearing semantics

**File:** `crates/rdpilot/src/session_loop.rs:120-128`

**Issue:** On every `GraphicsUpdate` the loop does
`frame.write(width, height, image.data().to_vec())` — a full `width*height*4`
copy for any dirty region, however small. This is documented as intentional for
integrity (always a complete frame). The correctness concern: the doc on
`SharedFrame::write` (framebuffer.rs:55-64) asserts a reader "can never observe a
partially-updated frame," which is true for the *mutex* but the **screenshot a
caller gets may be from before the just-decoded region was applied to a later
update**, because the snapshot is taken on *every* update unconditionally. That
is fine, but the comment "snapshot the whole framebuffer … so a reader always
gets a complete frame (Pitfall 3)" conflates two things: completeness (guaranteed
by the mutex) vs. freshness (best-effort). Not a bug, but the integrity contract
is overstated. (Perf is out of v1 scope, so the per-update full copy itself is
not flagged.)

**Fix:** Reword the comments to separate "self-consistent" (guaranteed) from
"latest decoded region included" (best-effort, latest-wins). No code change
required for v1.

### WR-03: `extract_server_public_key` returns the BIT STRING body, not necessarily what CredSSP expects

**File:** `crates/rdpilot/src/connect.rs:255-270`

**Issue:** The function decodes the cert and returns
`subject_public_key_info.subject_public_key.as_bytes()` — i.e. the *inner*
public-key BIT STRING bytes. CredSSP's "public key authentication" / TSRequest
`pubKeyAuth` is computed over the server's **SubjectPublicKeyInfo** in some
stacks and over the **subjectPublicKey** in others; IronRDP's
`connect_finalize` expects the `server_public_key` argument in a specific
encoding. If the wrong granularity (SPKI DER vs. the raw key bits) is passed,
CredSSP will fail authentication on a real target — and Phase 2's offline tests
cannot catch it because they never reach `connect_finalize`. This is exactly the
kind of boundary the unit tests do not cover.

**Fix:** Verify against IronRDP 0.15's `connect_finalize`/`screenshot.rs` example
what `server_public_key` must contain. The canonical IronRDP example extracts the
public key via `x509_cert` and passes
`cert.tbs_certificate.subject_public_key_info.subject_public_key.as_bytes()` —
confirm this matches 0.15 and add a comment citing the exact upstream line, since
the live suite is the only thing that exercises it.

### WR-04: `rustls_native_certs::load_native_certs()` errors are silently discarded

**File:** `crates/rdpilot/src/connect.rs:202-213`

**Issue:** `load_native_certs()` (0.8) returns a `CertificateResult` carrying both
`.certs` and `.errors`. The code reads only `.certs` and checks emptiness, but
never inspects `.errors`. On a misconfigured store you can get *some* certs plus
errors, and you silently proceed with a partial trust anchor set — a real cert
might be rejected later for a confusing reason, or a partial store masks a
platform misconfiguration. Per-cert `roots.add(cert)` failures are also
swallowed via `let _ =`.

**Fix:** If `native.certs.is_empty()` you already error; additionally, when
`native.certs` is empty *and* `!native.errors.is_empty()`, include the error
detail in the `Error::Tls` message (without leaking secrets — these are load
errors, safe to surface) so a misconfigured root store is diagnosable. Optionally
`debug!`-log the count of skipped malformed roots.

### WR-05: `serial_test` dev-dependency is declared but never used

**File:** `crates/rdpilot/Cargo.toml:48`

**Issue:** `serial_test = "3"` is a dev-dependency, but no test uses
`#[serial]`/`#[serial_test::serial]` anywhere in the crate. The live suite relies
on `--test-threads=1` (documented in live_session.rs:9-11) for serialization
instead. Dead dependency: adds compile time, supply-chain surface, and confuses
the next reader into thinking serialization is enforced in-code when it is not.

**Fix:** Either remove `serial_test` from `[dev-dependencies]`, or actually
annotate the live tests with `#[serial]` so single-threaded execution is enforced
by the tests rather than relying on the operator remembering `--test-threads=1`.
The latter is more robust against a forgotten flag.

### WR-06: `Error::Config` variant is dead — never constructed

**File:** `crates/rdpilot/src/error.rs:55-57`, `:102`

**Issue:** `Error::Config(String)` is declared and handled in `category()` but is
**never constructed** anywhere in the crate (confirmed by grep: only the
declaration and the `category()` match arm reference it). `ConnectionConfig` has
no `validate()` and `new()`/builders never fail. Dead variant on a
`#[non_exhaustive]` public enum is harmless API-wise but signals an unfinished
validation story (e.g. empty host, zero dimensions, port 0 are all silently
accepted today).

**Fix:** Either add the intended validation (e.g. a `ConnectionConfig::validate`
or validation inside `Session::connect` that returns `Error::Config` for empty
host / zero width-or-height) and wire it up, or remove the variant until it is
needed. Note that `dimensions(0, 0)` currently flows straight into
`DesktopSize { width: 0, height: 0 }` (connect.rs:160-163) — a `validate()`
returning `Error::Config` would be the right home for rejecting it.

### WR-07: Inconsistent getter naming on the public `ConnectionConfig` API

**File:** `crates/rdpilot/src/config.rs:110-151`

**Issue:** Getters mix two conventions on the same public type: `host()`,
`username()`, `width()`, `height()` (no prefix) vs. `get_port()`, `get_domain()`,
`get_accept_invalid_certs()` (get-prefixed). Rust API guidelines (C-GETTER)
prefer the unprefixed form (`port()`, `domain()`, `accept_invalid_certs()`). The
`get_`-prefixed names also collide conceptually with the builder setters of the
same root name (`port(self, …)` builder vs. `get_port(&self)` getter), which is
the only reason the `get_` prefix was introduced — but that is a smell on a
public SDK surface that wants to be stable for PyO3/MCP marshalling.

**Fix:** Settle on one convention before 0.1 ships (public API churn later is
costly). Recommended: rename setters to `with_port`/`with_domain`/
`with_dimensions`/`with_accept_invalid_certs` (builder convention) and make all
getters unprefixed (`port()`, `domain()`, `accept_invalid_certs()`). Do this now
while the only consumers are the in-repo example and tests.

## Info

### IN-01: `to_socket_addrs().next()` silently picks the first resolved address

**File:** `crates/rdpilot/src/connect.rs:128-137`

**Issue:** `resolve_addr` takes only the first address from DNS. On a dual-stack
host whose first record is an unreachable IPv6 (or vice-versa), connect fails even
though another resolved address would succeed. Acceptable for v1 lab targets, but
worth noting.

**Fix:** Consider iterating all resolved addresses and attempting connect to each
until one succeeds, or document the first-address behavior.

### IN-02: Magic numbers in the connector config lack named constants

**File:** `crates/rdpilot/src/connect.rs:154-183`

**Issue:** `keyboard_functional_keys_count: 12`, `client_build: 0`, hardcoded
`"C:\\Windows\\System32\\mstscax.dll"` client_dir, etc. are inline literals. These
mirror the upstream example so they are intentional, but the `client_dir` string
in particular is an opaque magic value.

**Fix:** Add a one-line comment on `client_dir` explaining it is the conventional
mstscax.dll path RDP servers expect, or hoist to a named const.

### IN-03: Example and test config loaders are near-duplicates

**File:** `crates/rdpilot/examples/screenshot.rs:96-137`;
`crates/rdpilot/tests/common/mod.rs:65-112`

**Issue:** Both parse the same `.secrets/connection.json` schema with the same
field extraction and `accept_invalid_certs(true)`, diverging only in error
handling (`Result<_, String>` vs. `expect`). Duplication will drift.

**Fix:** Acceptable for v1 (example must not depend on `tests/`), but note the
duplication so a future schema change updates both. A shared `examples`-visible
helper is not worth it yet.

### IN-04: `capture_when_ready` panics with a bare `panic!` after a fixed 10s budget

**File:** `crates/rdpilot/tests/live_session.rs:259-267`

**Issue:** Test-only (allowed), but the 100-iteration × 100ms = 10s budget is a
magic value and the failure is a `panic!` rather than a test assertion message
tied to a criterion. Fine for tests; flagged only for the hardcoded timing.

**Fix:** Extract the poll budget to a named const for clarity. No functional
change.

### IN-05: `is_blank` treats a <4-byte buffer as blank, but `from_rgba` already forbids that

**File:** `crates/rdpilot/tests/live_session.rs:80-86`

**Issue:** `is_blank` guards `rgba.len() < 4 -> true`, but any `Screenshot` from
`from_rgba`/`crop` is dimension-validated, so a sub-4-byte buffer is unreachable
for a real screenshot. Harmless dead guard in test code.

**Fix:** None required; optionally drop the guard.

### IN-06: `_ = RDPILOT_SENSOR;` anchor and the unused `svc` feature are scaffolding

**File:** `crates/rdpilot/src/connect.rs:94`; `crates/rdpilot/Cargo.toml:17-24`

**Issue:** `let _ = RDPILOT_SENSOR;` is a deliberate "anchor" so the reserved name
is not dropped, and the `svc` feature plus `DrdynvcClient::new()` register an
empty host channel for Phase 4. This is intentional seam scaffolding, clearly
documented. Flagged only so reviewers know it is load-bearing-for-later, not
accidental dead code. The `dvc_seam_name_is_reserved` test (connect.rs:361-364)
is a tautology (`assert_eq!(RDPILOT_SENSOR, "RDPILOT_SENSOR")`) that only guards
against a rename — low value but harmless.

**Fix:** None for v1. Consider removing the tautological test or replacing it with
a check that the channel is actually registered on the connector.

---

_Reviewed: 2026-06-05T00:00:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: deep_
