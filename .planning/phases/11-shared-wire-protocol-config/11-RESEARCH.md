# Phase 11: Shared Wire Protocol & Config - Research

**Researched:** 2026-07-11
**Domain:** Rust workspace crate design — serde wire-schema enforcement, layered config resolution, structural credential redaction
**Confidence:** HIGH

## Summary

Phase 11 adds two new, dependency-light workspace crates — `rdpilot-ipc` (wire DTOs + error taxonomy) and `rdpilot-config` (layered config resolution) — that sit strictly BELOW the existing `rdpilot` SDK crate in the dependency graph and are consumed by the not-yet-built daemon (Phase 12) and thin CLI/MCP clients (Phase 13/14). Both crates must be buildable and testable in complete isolation from IronRDP: `rdpilot-ipc` must never depend on `rdpilot`, and `rdpilot-config`'s resolved-config type must be its own owned struct, not `rdpilot::ConnectionConfig` — otherwise every thin CLI/MCP client binary would transitively pull in IronRDP/rustls/tokio-full just to parse a JSON response or read a TOML file. This is the single most consequential architecture decision for the planner to get right in Wave 1, and it's not spelled out verbatim in CONTEXT.md, so it is flagged prominently below.

For SESSION-02, the correct serde pattern is a `#[serde(tag = "op")]` internally-tagged `Request` enum with `session: SessionId` declared as a plain field inside **every** struct variant — not a `#[serde(flatten)]`-wrapped enum. Serde's `#[serde(flatten)]` on a field whose type is an internally-tagged enum is a known, still-open deserialize limitation (serde-rs/serde#1189): it serializes fine but fails to deserialize. Avoiding flatten entirely sidesteps this pitfall and also gives the simplest possible mental model: one struct field per variant, no macro magic, and the field's required non-`Option`-ness is what produces the hard rejection SESSION-02 demands. A companion `impl SessionScoped for Request` with an exhaustive match turns "a new variant forgot the session field" into a compile error, which is a genuine structural (not just tested) guarantee for future variants added in Phases 12-14.

For CONFIG-01/02, the `config` crate (already pinned at 0.15.25 by D-26, confirmed current on crates.io as of this research) handles the file→env layers cleanly via `Config::builder().add_source(File::...).add_source(Environment::with_prefix("RDPILOT").separator("__"))`, with later-added sources winning — but its `Source` trait is a poor fit for the flag/MCP-init layer, since those arrive as already-typed values from `clap`/`rmcp`, not textual env-style strings. The recommended design applies file+env resolution via the `config` crate, then applies flag/MCP-init as a simple third manual override pass in plain Rust (`if let Some(v) = flag_host { resolved.host = v }`) — simpler, more typesafe, and avoids fighting the crate's source abstraction for a use case it isn't built for. For CONFIG-02's platform config dir, `directories::BaseDirs::config_dir()` (NOT `ProjectDirs`) must be used and `rdpilot`/`config.toml` joined manually — `ProjectDirs::from(...).config_dir()` appends an unwanted extra `\config` subfolder on Windows that would produce `%APPDATA%\rdpilot\config\config.toml`, missing D-27's exact `%APPDATA%\rdpilot\config.toml` spec by one directory level. This is a concrete, easy-to-miss pitfall documented below.

For CONFIG-03, D-31 already resolved the "how" (structural, not string-scrubbing): `rdpilot-ipc`'s response DTOs must never define a password/credential-shaped field at all, which is enforced for free by `rdpilot-ipc` never depending on `rdpilot-config`. The planted-sentinel grep test becomes a **regression guard**: an exhaustive-variant sampler serializes every response DTO and asserts a fixed sentinel string never appears — a test pattern that is cheap to keep green forever precisely because the leak is structurally impossible, not because the test traces a live secret end-to-end (that end-to-end trace only becomes meaningful once Phase 12's registry exists).

**Primary recommendation:** Two new workspace crates, `rdpilot-ipc` (zero dependency on `rdpilot`) and `rdpilot-config` (zero dependency on `rdpilot`), both using `serde`/`serde_json` for wire types, `config` 0.15.25 (TOML-only feature set) + `directories` 6.0.0 (`BaseDirs`, not `ProjectDirs`) for config resolution, with `session: SessionId` embedded per-struct-variant (no `#[serde(flatten)]` over an enum) and an exhaustive-match `SessionScoped` trait as a compile-time forcing function for future wire verbs.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Wire request/response DTO definitions | Shared library (`rdpilot-ipc`) | — | Consumed by daemon (server) AND CLI/MCP (clients) — must be dependency-light so client binaries don't pull in IronRDP |
| Session-id-required schema enforcement | Shared library (`rdpilot-ipc`) | — | SESSION-02's "hard rejection at the wire boundary" must happen at deserialize time, before any handler runs — the wire type itself is the enforcement point |
| Wire error taxonomy (`WireError`) | Shared library (`rdpilot-ipc`) | API/Backend (daemon, Phase 12) | Type owned here; the SDK-`Error`→`WireError` mapping function is also defined here since it's a pure data transform, but daemon (Phase 12) is the only caller |
| Config file parsing (TOML) | Shared library (`rdpilot-config`) | — | Used by daemon at startup AND by CLI for a future `config show`/diagnostic verb — must not require IronRDP |
| Env var layering | Shared library (`rdpilot-config`) | — | `config` crate's `Environment` source |
| Flag / MCP-init override layering | Shared library (`rdpilot-config`, override API) | CLI/MCP (Phase 13/14, supply the typed values) | `rdpilot-config` exposes the override function; CLI/MCP own parsing flags/init-params into typed values via `clap`/`rmcp` |
| Platform config-dir resolution | Shared library (`rdpilot-config`) | — | `directories::BaseDirs`, OS-specific, belongs with the crate that owns "where is the file" |
| Credential storage in memory | Shared library (`rdpilot-config`, `Credentials`/`ResolvedConfig`) | API/Backend (daemon converts to `rdpilot::ConnectionConfig`) | Password lives here only; never crosses into `rdpilot-ipc` types |
| `ConnectionConfig` construction from resolved config | API/Backend (daemon, Phase 12) | — | Out of Phase 11 scope — Phase 11 stops at `ResolvedConfig`; the `ResolvedConfig → rdpilot::ConnectionConfig` conversion is Phase 12's first daemon-startup step |

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `serde` | 1.0.228 (already a workspace dep at `"1"`, `derive` feature) | Wire DTO (de)serialization, config struct deserialization | Already used throughout `rdpilot`; the de-facto Rust serialization framework |
| `serde_json` | 1.0.x (already a workspace dep at `"1"`) | Wire transport encoding (matches the existing sensor DVC JSON envelope convention in `crates/rdpilot/src/sensor.rs`) | Consistency with the already-proven sensor wire format; zero new dependency |
| `thiserror` | 2.0.18 (already a workspace dep at `"2"`) | `WireError`/config error Display impls, matching `rdpilot::Error`'s existing pattern | Already the project's error-derive convention (`error.rs`) |
| `config` | 0.15.25 `[VERIFIED: crates.io registry, matches D-26 pin exactly]` | Layered file→env config merging | D-26 already locks this version; confirmed current (max stable version on crates.io as of this research) |
| `directories` | 6.0.0 `[VERIFIED: crates.io registry]` | Platform config-dir resolution (`BaseDirs::config_dir()`) | Most widely used cross-platform config-path crate; simple `BaseDirs` API avoids the `ProjectDirs` nested-folder pitfall (see Pitfalls) |

**Installation:**
```bash
# rdpilot-ipc/Cargo.toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"

# rdpilot-config/Cargo.toml
[dependencies]
serde = { version = "1", features = ["derive"] }
config = { version = "0.15.25", default-features = false, features = ["toml"] }
directories = "6.0.0"
thiserror = "2"
```

**Version verification:** Confirmed live against the crates.io registry (2026-07-11, via `curl https://crates.io/api/v1/crates/<pkg>`):
- `config` → `0.15.25` (exact match to D-26's pin)
- `directories` → `6.0.0`
- `serde` → `1.0.228`
- `thiserror` → `2.0.18`
- `serde_json` → `1.x` (workspace already pins `"1"`, unchanged)

`config`'s default feature set pulls in `toml`, `json`, `yaml`, `ini`, `ron`, `json5`, `convert-case`, and `async` — all unneeded here. Use `default-features = false, features = ["toml"]` to keep the dependency tree minimal (consistent with the project's existing lean-dependency discipline, e.g. `ironrdp-tls`'s `default-features = false`).

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `etcetera` | 0.11.0 | Alternative platform-dir crate | Considered but not recommended — see Alternatives below |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `directories::BaseDirs` | `directories::ProjectDirs` | `ProjectDirs::from(q, org, app).config_dir()` appends an extra `\config` subfolder on Windows — mismatches D-27's exact `%APPDATA%\rdpilot\config.toml` spec by one path segment. `BaseDirs::config_dir()` + manual `.join("rdpilot").join("config.toml")` hits the D-27 spec exactly on both Windows and Unix. |
| `directories` | `etcetera` | Equally viable (XDG/Windows/macOS-aware), higher Context7 benchmark score, but `directories` is the more widely recognized/used crate in the Rust CLI ecosystem and its `BaseDirs` API maps more directly onto D-27's two explicit paths (Unix `~/.config`, Windows `%APPDATA%`) without needing to learn a second strategy-object API. No functional reason to prefer `etcetera` here. |
| `config` crate for the flag/MCP-init layer too | Force flags/MCP-init through `config::Source` | The `config` crate's `Source` trait is designed for textual/map-shaped sources (files, env). Flags are already typed values parsed by `clap` (Phase 13) and MCP-init params are already typed by `rmcp` (Phase 14) — shoehorning them through `Source` means re-stringifying and re-parsing values that are already correctly typed. A plain `Option<T>`-based override pass in Rust is simpler and typesafe. |
| `#[serde(flatten)]` wrapper for session+verb | Per-variant `session` field (recommended) | `#[serde(flatten)]` over an internally-tagged enum field does not deserialize (serde-rs/serde#1189, open). Avoided entirely by putting `session` inside each struct variant. |

## Package Legitimacy Audit

Ran `slopcheck install --ecosystem crates.io config directories thiserror serde_json serde` (2026-07-11). All five packages resolved `[OK]` against the crates.io registry before slopcheck's local `cargo add` verification step failed only because this research sandbox has no `cargo` on `PATH` (unrelated to package legitimacy — `cargo` is present at `~/.cargo/bin` but this sandbox's shell doesn't source it; see Environment Availability).

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|-------------|
| `config` | crates.io | Long-established (10.7M all-time downloads, in production use across the Rust ecosystem) | Very high | `github.com/rust-cli/config-rs` | [OK] | Approved (D-26 already pinned) |
| `directories` | crates.io | Long-established, maintained under `codeberg.org/dirs/directories-rs` | Very high | `codeberg.org/dirs/directories-rs` (formerly `github.com/dirs-dev/directories-rs`) | [OK] | Approved |
| `thiserror` | crates.io | Long-established (dtolnay) | Very high | `github.com/dtolnay/thiserror` | [OK] | Approved (already a workspace dependency) |
| `serde` | crates.io | Long-established | Very high | `github.com/serde-rs/serde` | [OK] | Approved (already a workspace dependency) |
| `serde_json` | crates.io | Long-established | Very high | `github.com/serde-rs/json` | [OK] | Approved (already a workspace dependency) |

**Packages removed due to slopcheck [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### System Architecture Diagram

```
                     rdpilot-ipc crate (no rdpilot/IronRDP dependency)
                     ┌──────────────────────────────────────────┐
                     │ Request { session: SessionId, ... }       │
                     │ WireResponse (credential-free DTOs)       │
                     │ WireError { code, message }               │
                     │ TransferOutcome (mirrored, owned)          │
                     └──────────────────────────────────────────┘
                              ▲ serde_json (wire encode/decode)     ▲
                              │                                     │
   ┌──────────────────────────┴───┐                 ┌───────────────┴─────────────┐
   │  CLI (Phase 13) / MCP (14)   │  local IPC       │  Daemon (Phase 12)          │
   │  thin clients — depend ONLY  │◄─────socket/─────►│  depends on BOTH            │
   │  on rdpilot-ipc              │  named pipe       │  rdpilot-ipc + rdpilot SDK  │
   └───────────────────────────────┘                 │  + rdpilot-config           │
                                                       └───────────────┬─────────────┘
                                                                       │
                                                       ┌───────────────▼─────────────┐
                                                       │  rdpilot-config crate        │
                                                       │  (no rdpilot dependency)     │
                                                       │  file → env → flag/init       │
                                                       │  ResolvedConfig{ host,       │
                                                       │  port, username, password,   │
                                                       │  domain, cert_bypass }       │
                                                       └───────────────┬─────────────┘
                                                                       │ daemon-only conversion
                                                                       ▼
                                                       rdpilot::ConnectionConfig
                                                       (IronRDP-facing, Phase 2 type)
```

A reader can trace: a CLI process builds a `Request` (embedding `session`), serializes it via `serde_json`, sends it over the not-yet-built IPC transport (Phase 12) to the daemon; the daemon deserializes — a missing `session` field fails right here, before any handler runs; on success the daemon (which alone holds `ResolvedConfig`/`ConnectionConfig` with the real password) executes the SDK call and serializes a credential-free `WireResponse` DTO back.

### Recommended Project Structure

```
crates/
├── rdpilot/            # existing SDK (Phase 1-10) — untouched by Phase 11
├── rdpilot-ipc/         # NEW — wire protocol
│   └── src/
│       ├── lib.rs       # #![deny(unsafe_code)]/#![deny(clippy::unwrap_used)]/#![deny(clippy::expect_used)] (match rdpilot's lib.rs convention)
│       ├── session_id.rs   # SessionId newtype
│       ├── request.rs      # Request enum (session-scoped verbs incl. Put/Get)
│       ├── response.rs     # WireResponse enum + status/list DTOs (credential-free)
│       ├── error.rs        # WireError, WireErrorCode, mapping from rdpilot::Error*
│       └── transfer.rs     # mirrored TransferOutcome { bytes_transferred, checksum }
└── rdpilot-config/      # NEW — layered config resolution
    └── src/
        ├── lib.rs        # same lint-gate convention
        ├── resolved.rs   # ResolvedConfig + Credentials (password-bearing, NOT Serialize)
        ├── paths.rs      # platform config-dir resolution (directories::BaseDirs)
        └── resolve.rs    # file → env → override(flag/mcp-init) pipeline
```
`Cargo.toml` (workspace root) gains:
```toml
[workspace]
members = ["crates/rdpilot", "crates/rdpilot-ipc", "crates/rdpilot-config"]
resolver = "2"
```

*Note: `error.rs`'s mapping from `rdpilot::Error` requires `rdpilot-ipc` to reference `rdpilot::Error`'s variant NAMES in doc comments, but the mapping function itself, if implemented as `impl From<&rdpilot::Error> for WireErrorCode`, would force `rdpilot-ipc` to depend on `rdpilot` — breaking the "clients never depend on rdpilot" invariant transitively for the daemon-only mapping code. Recommend the mapping function live in the DAEMON crate (Phase 12), not `rdpilot-ipc` — `rdpilot-ipc` only defines the `WireError`/`WireErrorCode` TYPES (D-28's "Phase 11 defines the enum" is satisfied by owning the type; "the SDK-`Error`→`WireError` mapping" per D-28's text can be read either way — flagged as an Open Question below since CONTEXT.md's D-28 text literally says "Phase 11 defines the enum + the SDK-Error→WireError mapping," which would require rdpilot-ipc to depend on rdpilot; the researcher recommends the type-only reading to preserve the dependency boundary, but this needs planner/discuss confirmation.)*

### Pattern 1: Per-variant required session field (SESSION-02)

**What:** An internally-tagged `Request` enum where every struct variant repeats `session: SessionId` as a plain, non-`Option` field — never via `#[serde(flatten)]`.
**When to use:** Any new session-scoped wire verb, in this phase and all of Phases 12-14.
**Example:**
```rust
// rdpilot-ipc/src/request.rs
use serde::{Deserialize, Serialize};
use crate::session_id::SessionId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    Screenshot { session: SessionId },
    Click { session: SessionId, x: i32, y: i32 },
    Put { session: SessionId, local_path: String, remote_name: String },
    Get { session: SessionId, remote_name: String, local_path: String },
    // ... every future verb MUST declare `session` here.
}

/// Compile-time forcing function: adding a new `Request` variant without a
/// `session` field is a match-arm error here, not just a runtime test gap.
pub trait SessionScoped {
    fn session(&self) -> &SessionId;
}

impl SessionScoped for Request {
    fn session(&self) -> &SessionId {
        match self {
            Request::Screenshot { session } => session,
            Request::Click { session, .. } => session,
            Request::Put { session, .. } => session,
            Request::Get { session, .. } => session,
        }
    }
}
```
```rust
// SessionId: bare-string wire representation, rejects empty strings at
// deserialize time (closes the trivial `"session": ""` bypass of the
// BLOCKING "field is required" criterion).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl std::str::FromStr for SessionId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            Err("session id must not be empty".to_owned())
        } else {
            Ok(SessionId(s.to_owned()))
        }
    }
}

impl<'de> serde::Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
```
**Test proving the BLOCKING criterion (per verb):**
```rust
#[test]
fn every_request_verb_rejects_a_missing_session_field() {
    let cases = [
        r#"{"op":"Screenshot"}"#,
        r#"{"op":"Click","x":1,"y":2}"#,
        r#"{"op":"Put","local_path":"a","remote_name":"b"}"#,
        r#"{"op":"Get","remote_name":"a","local_path":"b"}"#,
    ];
    for json in cases {
        let result: Result<Request, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected rejection for {json}");
    }
}
```

### Pattern 2: Credential-free DTOs via structural separation (D-31/CONFIG-03)

**What:** `rdpilot-ipc` response DTOs never define a password/credential-shaped field, enforced by `rdpilot-ipc` never depending on `rdpilot-config`.
**When to use:** Every `WireResponse` variant, especially the `list`/status DTO (D-30).
**Example:**
```rust
// rdpilot-ipc/src/response.rs — no password/secret field exists ANYWHERE here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub id: String,
    pub name: Option<String>,
    pub host: String,          // target host is NOT secret — fine to expose
    pub status: SessionLifecycle, // D-30 vocabulary
    pub connected_since: Option<String>, // ISO-8601; owned String, no chrono dep required
    pub last_activity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SessionLifecycle {
    Connecting,
    Live,
    Reconnecting,
    Disconnected,
}
```
```rust
// The planted-sentinel regression test (CONFIG-03 BLOCKING criterion).
const PLANTED_SECRET: &str = "RDPILOT-PLANTED-SECRET-SENTINEL";

fn sample_all_response_variants() -> Vec<WireResponse> {
    // Exhaustive match with a compile error on a missed variant keeps this
    // list honest as new verbs are added in Phases 12-14.
    vec![
        WireResponse::Status(SessionStatus {
            id: "brave-otter".into(),
            name: None,
            host: "10.0.0.5".into(),
            status: SessionLifecycle::Live,
            connected_since: None,
            last_activity: None,
        }),
        WireResponse::Transfer(TransferOutcome { bytes_transferred: 42, checksum: "deadbeef".into() }),
        WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, message: "not found".into() }),
    ]
}

#[test]
fn no_wire_response_variant_ever_carries_the_planted_secret() {
    for r in sample_all_response_variants() {
        let json = serde_json::to_string(&r).expect("dto is always serializable");
        assert!(!json.contains(PLANTED_SECRET), "leak found in {json}");
    }
}
```
This test's guarantee is honest, not theatrical: it passes because `WireResponse` variants structurally cannot reference `rdpilot-config::Credentials` (crate boundary), not because the sentinel was traced through a live pipeline. Document this explicitly in the test's doc comment so a future reader understands what it does and does not prove — a live end-to-end trace becomes possible (and should be added) once Phase 12's registry exists.

### Pattern 3: Layered config resolution (CONFIG-01)

**What:** `config` crate for file+env, plain Rust `Option` overrides for flag/MCP-init.
**When to use:** `rdpilot-config::resolve()`.
**Example:**
```rust
// rdpilot-config/src/resolved.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResolvedConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,   // deliberately NOT Serialize (see below)
    pub domain: Option<String>,
    #[serde(default)]
    pub accept_invalid_certs: bool,
}
// Deliberately NO `#[derive(Serialize)]` on ResolvedConfig — matches D-31's
// "structural not string-scrubbed" philosophy applied one layer earlier:
// if this type is never Serialize, it cannot be wired-serialized by
// accident even before the rdpilot-ipc boundary is considered.
```
```rust
// rdpilot-config/src/paths.rs
use directories::BaseDirs;
use std::path::PathBuf;

/// Resolves to `~/.config/rdpilot/config.toml` on Linux/macOS-XDG-style
/// hosts and `%APPDATA%\rdpilot\config.toml` on Windows — matching D-27's
/// literal spec exactly. Deliberately uses `BaseDirs`, NOT `ProjectDirs`
/// (see Pitfall: ProjectDirs adds an extra `\config` subfolder on Windows).
pub fn config_file_path() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.config_dir().join("rdpilot").join("config.toml"))
}
```
```rust
// rdpilot-config/src/resolve.rs
use config::{Config, Environment, File, FileFormat};

pub fn resolve_file_and_env() -> Result<ResolvedConfig, config::ConfigError> {
    let path = config_file_path();
    let mut builder = Config::builder();
    if let Some(path) = &path {
        builder = builder.add_source(File::new(&path.to_string_lossy(), FileFormat::Toml).required(false));
    }
    builder = builder.add_source(
        Environment::with_prefix("RDPILOT")
            .prefix_separator("_")
            .separator("__"),
    );
    builder.build()?.try_deserialize()
}

/// Third layer: flag/MCP-init values, already typed by clap/rmcp
/// (Phase 13/14) — deliberately NOT routed through `config::Source`.
pub fn apply_overrides(mut base: ResolvedConfig, overrides: ResolvedConfig) -> ResolvedConfig {
    if overrides.host.is_some() { base.host = overrides.host; }
    if overrides.port.is_some() { base.port = overrides.port; }
    if overrides.username.is_some() { base.username = overrides.username; }
    if overrides.password.is_some() { base.password = overrides.password; }
    if overrides.domain.is_some() { base.domain = overrides.domain; }
    if overrides.accept_invalid_certs { base.accept_invalid_certs = true; }
    base
}
```

### Anti-Patterns to Avoid

- **`#[serde(flatten)]` over an internally-tagged enum field:** deserializes silently-wrong or errors ("can only flatten structs and maps") — see Pitfalls.
- **`ProjectDirs::from(...).config_dir()` for a single-segment app name:** adds an unwanted extra path level on Windows.
- **Deriving `Serialize` on any credential-bearing type "just in case":** even if nothing currently uses it, a future accidental wire-path connection becomes a silent leak. Only derive what's actually needed.
- **Routing CLI flags through `config::Environment`-style string re-encoding:** loses type safety for no benefit, since `clap`/`rmcp` already hand you typed values.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Platform config directory resolution | Manual `#[cfg(target_os = ...)]` path logic | `directories::BaseDirs` | XDG spec / Windows Known Folder API / macOS conventions have real edge cases (env var overrides, missing HOME, etc.) already handled |
| TOML parsing + env var overlay merging | Hand-rolled `HashMap<String,String>` merge logic | `config` crate | Precedence ordering, type coercion, and nested-key env separator handling are exactly config-rs's job; already pinned by D-26 |
| Wire (de)serialization framework | Hand-rolled JSON reader/writer | `serde`/`serde_json` | Already the project's convention (see `sensor.rs`'s envelope) |
| Required-field enforcement | A custom "validate this JSON has key X" pre-check before deserializing | serde's own non-`Option` field semantics | Free, and produces a structured `serde_json::Error` with line/column, better than a hand-rolled string check |

**Key insight:** Every problem in this phase already has a battle-tested crate-level solution; the actual engineering work is in the DTO *shape* decisions (per-variant session field, credential-free-by-construction DTOs, crate dependency direction) — not in writing new parsing/merging code.

## Common Pitfalls

### Pitfall 1: `#[serde(flatten)]` over an internally-tagged enum breaks deserialize

**What goes wrong:** A tempting DRY design — `struct Envelope { session: SessionId, #[serde(flatten)] op: RequestOp }` where `RequestOp` is `#[serde(tag = "op")]`-tagged — serializes to valid-looking JSON but FAILS to deserialize with an error like "can only flatten structs and maps" (or silently mis-parses, depending on serde version).
**Why it happens:** serde's flatten implementation buffers the remaining fields into a generic `Content` representation for non-self-describing-format compatibility; this buffering is incompatible with an internally-tagged enum's own two-pass tag-then-content deserialize strategy. Tracked as serde-rs/serde#1189, open since 2018, still unresolved as of this research.
**How to avoid:** Put `session: SessionId` as a literal field inside every struct variant of the `Request` enum (Pattern 1 above). No flatten, no macro-generated wrapper.
**Warning signs:** A deserialize test that passes for `cargo test` but only because the test never actually round-trips through `serde_json::from_str` with a flattened field — always write and run the omit-session rejection test (SESSION-02's own mandated test) BEFORE assuming the flatten design works.

### Pitfall 2: `ProjectDirs` adds a hidden extra path segment on Windows

**What goes wrong:** `directories::ProjectDirs::from("", "", "rdpilot").config_dir()` returns `%APPDATA%\rdpilot\config` (note trailing `\config`), not `%APPDATA%\rdpilot` — so joining `config.toml` onto it produces `%APPDATA%\rdpilot\config\config.toml`, one directory level deeper than D-27's literal spec.
**Why it happens:** `ProjectDirs` is designed to host MULTIPLE subdirectories (config/cache/data) per app, so it nests a `config` subfolder under the app folder on Windows and macOS (though not on Linux, where `~/.config/<app>` IS the config dir directly — an inconsistency between platforms baked into the crate's design).
**How to avoid:** Use `directories::BaseDirs::new()?.config_dir()` (the OS-level config root, e.g. `%APPDATA%` or `~/.config`) and manually `.join("rdpilot").join("config.toml")`. This produces the exact D-27 paths on both platforms.
**Warning signs:** A live-manual test ("does the file actually appear where the user expects on Windows") catching a path one level too deep; or a CONFIG-02 "discoverable and self-explanatory" review noting the path looks wrong.

### Pitfall 3: D-28's literal text implies `rdpilot-ipc` depends on `rdpilot`, which breaks the thin-client invariant

**What goes wrong:** CONTEXT.md's D-28 text says "Phase 11 defines the enum + the SDK-`Error`→`WireError` mapping" in `rdpilot-ipc`. Implementing the mapping function as `impl From<&rdpilot::Error> for WireErrorCode` inside `rdpilot-ipc` forces `rdpilot-ipc` (and therefore, transitively, every CLI/MCP client binary that links it) to depend on `rdpilot`, which pulls in IronRDP/rustls/tokio-full — defeating the entire "CLI/MCP are thin clients" premise (D-17).
**Why it happens:** the cross-cutting decision was written before the crate-dependency-direction implications were fully worked through.
**How to avoid:** `rdpilot-ipc` defines ONLY the `WireError`/`WireErrorCode` types. The `rdpilot::Error → WireErrorCode` mapping function lives in the daemon crate (Phase 12), which is the only consumer that legitimately depends on both `rdpilot` and `rdpilot-ipc`. Flagged as an Open Question below for planner/discuss confirmation since it reinterprets locked decision text.
**Warning signs:** `cargo tree -p rdpilot-ipc` showing `rdpilot`/`ironrdp*` in the dependency tree — this should never happen.

### Pitfall 4: Config file permissions leaking a plaintext password to other local users

**What goes wrong:** `config.toml` at the platform config dir contains a plaintext password field (D-27 explicitly lists `password` as a config key). If created with default OS permissions, other local accounts on a shared machine could read it.
**Why it happens:** `std::fs::write`/`File::create` uses the process umask by default (typically `0644` on Unix — world-readable).
**How to avoid:** On Unix, when Phase 12/13 first WRITE this file (Phase 11 only reads it, per the CONTEXT.md boundary — no `config init`/write path is in scope here), use `std::os::unix::fs::PermissionsExt` to set `0600`. Flag this explicitly for whichever later phase adds a config-writing command (likely Phase 13's CLI `config` subcommand, not enumerated in the current CLI-01/02/03 requirements — note as a gap). Phase 11 itself should still document the expected permission mode in the crate's doc comments so downstream authors don't miss it.
**Warning signs:** A `ls -la` on the resolved config path showing group/other read bits set.

## Runtime State Inventory

Not applicable — Phase 11 is greenfield crate creation, not a rename/refactor/migration.

## Code Examples

### `WireError` taxonomy matching D-28's fixed code set

```rust
// rdpilot-ipc/src/error.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct WireError {
    pub code: WireErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireErrorCode {
    SessionNotFound,   // -> "session-not-found"
    DaemonUnreachable, // -> "daemon-unreachable"
    TransferFailed,    // -> "transfer-failed"
    PathTraversal,     // -> "path-traversal"
    ChecksumMismatch,  // -> "checksum-mismatch"
}
```
`#[serde(rename_all = "kebab-case")]` on `WireErrorCode` produces the exact five wire strings D-28 specifies verbatim (`session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`) — verified by hand-tracing serde's kebab-case transform against each PascalCase variant name; no ambiguity.

### `TransferOutcome` mirror (owned by `rdpilot-ipc`, not re-exported from `rdpilot`)

```rust
// rdpilot-ipc/src/transfer.rs — deliberately duplicates
// rdpilot::TransferOutcome's shape rather than depending on the `rdpilot`
// crate to obtain it (see Pitfall 3's dependency-direction reasoning).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOutcome {
    pub bytes_transferred: u64,
    pub checksum: String,
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| `#[serde(flatten)]` for envelope+payload composition | Per-variant repeated field, or two-stage envelope+`serde_json::Value` re-parse | Long-standing serde limitation (issue open since 2018), not a recent change | Still worth documenting since it's a natural-seeming design that silently fails only at deserialize time, easy to discover late |

No other "old vs new approach" shifts apply — `config` 0.15.25 and `directories` 6.0.0 are both current stable majors with no pending deprecation.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The SDK-`Error`→`WireError` mapping function should live in the daemon crate (Phase 12), not `rdpilot-ipc`, reinterpreting D-28's literal text to preserve the thin-client dependency boundary | Architecture Patterns, Pitfall 3 | If the planner instead follows D-28's literal text and puts the mapping in `rdpilot-ipc`, every CLI/MCP client binary transitively depends on IronRDP — a much heavier client binary and a violation of D-17's "thin client" intent. Needs explicit confirmation, not silent researcher override, since D-28 is a locked cross-cutting decision. |
| A2 | `WireErrorCode`'s fixed 5-code set has no explicit catch-all target for the ~9 other `rdpilot::Error` variants (Connect, Tls, Decode, Encode, CropOutOfBounds, Config, Session, CoordinateOutOfBounds, Dvc, Bootstrap, SensorRejected) that aren't PathTraversal/ChecksumMismatch | Code Examples, Open Questions | If unmapped errors panic or are silently dropped instead of routed to a sensible code (e.g. `TransferFailed` as a generic catch-all, or the enum needs `#[non_exhaustive]` + a 6th generic code), CLI-03's "distinct, legible errors" requirement (Phase 13) may not be satisfiable for non-file-transfer verbs |
| A3 | `SessionId` should reject empty strings at deserialize time (custom `FromStr`/`Deserialize`), closing the trivial `"session": ""` loophole around SESSION-02's required-field guarantee | Pattern 1 code example | If omitted, a client could pass an empty string and technically satisfy "field is present" while providing no real session identity — a narrow but real edge case around the BLOCKING criterion's intent |

## Open Questions

1. **Does `rdpilot-ipc` depend on `rdpilot`, or does the `Error`→`WireError` mapping live in the daemon crate?**
   - What we know: D-28's text literally assigns the mapping to "Phase 11" / `rdpilot-ipc`. The thin-client architecture (D-17) requires CLI/MCP binaries to never pull in IronRDP.
   - What's unclear: Whether D-28's phrasing was a loose summary ("Phase 11 owns this concept") or a literal crate-placement instruction.
   - Recommendation: Planner defines the mapping function in the daemon crate (Phase 12) referencing `WireErrorCode` from `rdpilot-ipc`; `rdpilot-ipc` itself has zero `rdpilot` dependency. Surface this reinterpretation explicitly in the plan's assumptions so it can be confirmed/corrected during plan review.

2. **What is the fallback `WireErrorCode` for `rdpilot::Error` variants outside the 5-code fixed set?**
   - What we know: D-28 gives exactly 5 codes, explicitly "FIXED", mapped 1:1 from `Error::PathTraversal`/`Error::ChecksumMismatch` plus two daemon-only concepts (`session-not-found`, `daemon-unreachable`) that have no SDK `Error` equivalent at all.
   - What's unclear: Whether non-file-transfer SDK errors (e.g. `Error::Connect`, `Error::CoordinateOutOfBounds`) are in scope for Phase 11's error taxonomy at all, or deferred to Phase 13/14 when those verbs' CLI/MCP surfaces are actually built.
   - Recommendation: Since Phase 11's BLOCKING criteria only cover SESSION-02 and CONFIG-03 (not a general error-taxonomy completeness criterion), recommend Phase 11 ship exactly the 5 D-28 codes with `#[non_exhaustive]` on `WireErrorCode`, and defer the non-file-transfer mapping decision to whichever phase (13 or 14) first needs to surface those errors over the wire.

3. **Is CONFIG-02's "gitignored" requirement satisfied trivially by D-27's platform-config-dir location, or does it imply a second, project-local config file layer?**
   - What we know: D-27 pins the config file to the OS platform config dir (`~/.config/rdpilot/` or `%APPDATA%\rdpilot\`), which is structurally outside the git repository — it can never be accidentally committed.
   - What's unclear: Whether "gitignored" in the original CONFIG-02 wording anticipated a DIFFERENT (project-relative, e.g. `.rdpilot.toml` in cwd) location that D-27 later superseded, or whether it's simply belt-and-suspenders phrasing that no longer needs an actual `.gitignore` entry given D-27's final location choice.
   - Recommendation: Treat CONFIG-02 as satisfied by D-27's location choice alone (no `.gitignore` entry needed since the file is never inside the repo tree). As a defensive no-cost addition, add `config.toml` and `.rdpilot.toml` to the repo root `.gitignore` anyway, in case a future contributor creates a project-local override file by convention for local dev/testing.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `cargo`/`rustc` (Windows-GNU toolchain) | Building/testing the new crates | ✗ in this research/planning sandbox | — (sandbox is `x86_64-unknown-linux-gnu`; workspace pins `stable-x86_64-pc-windows-gnu` via `rust-toolchain.toml`) | Build/test on the actual project dev host (ARM64 Windows w/ MinGW-w64, per `rust-toolchain.toml`/`.cargo/config.toml`) — this is the SAME constraint every prior phase (1-10) already operated under; not new to Phase 11 |
| Network access to crates.io | Adding new dependencies, verifying versions | ✓ | — | — |
| Live RDP target (Azure VM) | NOT required by Phase 11 | N/A | — | Phase 11's success criteria (SESSION-02, CONFIG-01/02/03) are all provable via unit tests on pure Rust types — no live Windows target needed, unlike Phase 10 |

**Missing dependencies with no fallback:** none — the Windows-GNU toolchain gap is a known, already-established constraint of this project (documented in `rust-toolchain.toml`'s own comments), not something Phase 11 introduces.

**Missing dependencies with fallback:** `cargo`/`rustc` in this sandbox — build/test happens on the actual dev host as it always has.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]`/`#[tokio::test]` (no external test framework — matches existing `rdpilot` crate convention: inline `#[cfg(test)] mod tests` per source file, e.g. `error.rs`, `config.rs`) |
| Config file | none — no `pytest.ini`/`jest.config` equivalent in a Rust workspace; test discovery is `cargo test`'s built-in behavior |
| Quick run command | `cargo test -p rdpilot-ipc -p rdpilot-config` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SESSION-02 | Every session-scoped request variant rejects a missing `session` field | unit | `cargo test -p rdpilot-ipc every_request_verb_rejects_a_missing_session_field` | ❌ Wave 0 |
| CONFIG-01 | File→env→flag precedence resolves deterministically, highest layer wins | unit | `cargo test -p rdpilot-config layered_precedence` | ❌ Wave 0 |
| CONFIG-02 | Config path resolves to the exact D-27 platform paths | unit | `cargo test -p rdpilot-config config_file_path_matches_platform_convention` | ❌ Wave 0 |
| CONFIG-03 | Serializing every wire response type never contains a planted secret sentinel | unit | `cargo test -p rdpilot-ipc no_wire_response_variant_ever_carries_the_planted_secret` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot-ipc -p rdpilot-config` (quick run — both new crates are small, full test suite runs in well under 30s)
- **Per wave merge:** `cargo test --workspace` (also re-runs the existing `rdpilot` crate's suite to confirm no accidental breakage — Phase 11 shouldn't touch `crates/rdpilot/src/`, but the workspace `Cargo.toml` member-list edit is a shared file)
- **Phase gate:** Full suite green before `/gsd-verify-work`. No live-RDP gate needed for this phase (see Environment Availability).

### Wave 0 Gaps
- [ ] `crates/rdpilot-ipc/` — new crate, no test infrastructure yet (`cargo new --lib` scaffolding)
- [ ] `crates/rdpilot-config/` — new crate, no test infrastructure yet
- [ ] Workspace `Cargo.toml` — needs both new members added
- [ ] No shared `conftest.py`-equivalent needed — Rust's `#[cfg(test)] mod tests` convention is per-file, matching the existing `rdpilot` crate's pattern exactly

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | Partial | `rdpilot-config` resolves credentials for pass-through to IronRDP's own NLA/CredSSP (Phase 2, already implemented) — Phase 11 does not implement authentication itself, only resolves the input values |
| V3 Session Management | No | RDP-protocol session management is `rdpilot`'s (existing) concern; Phase 11's `SessionId` is a wire-addressing identifier, not an auth session token |
| V4 Access Control | No (deferred) | Local-IPC access control (DACL/socket permissions) is explicitly Phase 12's DAEMON-02 scope, not Phase 11 |
| V5 Input Validation | Yes | Required non-`Option` `session` field on every `Request` variant (serde's own deserialize-time rejection); `SessionId`'s empty-string rejection (recommended, see A3) |
| V6 Cryptography | No new surface | `rdpilot-config` stores the password as plaintext in `config.toml`, matching the project's EXISTING accepted risk posture (`.secrets/connection.json`, D-06/D-14) — not a regression, but worth reiterating: this is not encrypted at rest. File permissions (Pitfall 4) are the practical mitigation, not encryption. |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Credential leak via an accidentally `Serialize`-derived credential-bearing type reaching a wire response | Information Disclosure | Structural separation: `rdpilot-ipc` never depends on `rdpilot-config` (D-31); `ResolvedConfig`/`Credentials` never derive `Serialize` |
| Missing required `session` field silently defaulting to some implicit "current session" (reintroducing the exact footgun D-18 exists to prevent) | Elevation of Privilege / Tampering | Non-`Option`, no `#[serde(default)]` on the `session` field — a missing field is a hard `serde_json::Error`, never a fallback value |
| World-readable `config.toml` exposing a plaintext password to other local accounts | Information Disclosure | `0600` permissions on Unix when the file is first written (flagged for the phase that adds config-writing, Pitfall 4) |
| A malformed/malicious wire request with an unexpected `op` tag causing a panic instead of a typed rejection | Denial of Service | serde's own unknown-variant handling for `#[serde(tag = "op")]` produces a typed `Err`, never a panic — combined with the project's workspace-wide `#![deny(clippy::unwrap_used)]`/`#![deny(clippy::expect_used)]` convention (must be replicated in the new crates' `lib.rs`), this is enforced end-to-end |

## Project Constraints (from CLAUDE.md)

- **No `unsafe`, no `unwrap()`/`expect()`/`panic!()` in library code** — enforced today via `crates/rdpilot/src/lib.rs`'s inner attributes (`#![deny(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]`). Both new crates (`rdpilot-ipc`, `rdpilot-config`) MUST add the same inner attributes to their own `lib.rs` — this is a per-crate opt-in, not inherited automatically from the existing `rdpilot` crate or a workspace-level `[lints]` table (the project deliberately avoided a workspace `[lints]` table because it would break `rdpilot`'s `tests/*.rs` integration tests' legitimate `.expect()` calls — see `lib.rs`'s own doc comment). Because these new crates keep tests INLINE (`#[cfg(test)] mod tests`, matching the existing `error.rs`/`config.rs` convention) rather than in separate `tests/*.rs` files, the inner attribute will also cover their own test code — write tests using `assert!`/`?`-propagation, not `.unwrap()`/`.expect()`.
- **Owned types only in public signatures (D-09)** — no third-party crate types (`config::ConfigError`, `toml::*`) may appear in `rdpilot-ipc`'s or `rdpilot-config`'s public API; source-erase into owned `Error`/`ConfigError` types mirroring `crates/rdpilot/src/error.rs`'s existing pattern (`thiserror`-derived, `Display`-only external detail, `category()` helper).
- **GSD Workflow Enforcement** — all implementation work must go through `/gsd-execute-phase` per the global CLAUDE.md; not directly actionable by this research document but noted for completeness.

## Sources

### Primary (HIGH confidence)
- Context7 `/rust-cli/config-rs` — layered config loading, `Environment` prefix/separator, `ConfigError` variants, feature flags
- Context7 `/git_codeberg_org/dirs_directories-rs` — `ProjectDirs`/`BaseDirs` API and computed path examples
- Context7 `/websites/serde_rs` — enum representations (internally tagged), container attributes
- crates.io registry (`curl https://crates.io/api/v1/crates/<pkg>`, 2026-07-11) — `config` 0.15.25, `directories` 6.0.0, `serde` 1.0.228, `thiserror` 2.0.18 version verification
- `slopcheck install --ecosystem crates.io config directories thiserror serde_json serde` (2026-07-11) — all 5 `[OK]`
- Local codebase: `crates/rdpilot/src/error.rs`, `crates/rdpilot/src/config.rs`, `crates/rdpilot/src/session.rs`, `crates/rdpilot/src/keepalive.rs`, `crates/rdpilot/src/lib.rs`, `crates/rdpilot/Cargo.toml`, `Cargo.toml` (workspace), `rust-toolchain.toml`, `.cargo/config.toml`, `.gitignore`
- `.planning/phases/11-shared-wire-protocol-config/11-CONTEXT.md`, `.planning/ROADMAP.md`, `.planning/DECISIONS-INDEX.md`, `.planning/REQUIREMENTS.md`, `.planning/PROJECT.md`, `.planning/phases/10-sdk-file-transfer-extension/10-CONTEXT.md`

### Secondary (MEDIUM confidence)
- WebSearch: "directories-rs ProjectDirs::from empty qualifier organization avoid nested folder Windows config_dir" — cross-verified against the WebFetch of docs.rs's `ProjectDirs` table
- WebFetch `docs.rs/directories/latest/directories/struct.ProjectDirs.html` — confirmed the exact `\config` subfolder behavior on Windows

### Tertiary (LOW confidence)
- WebSearch: "serde flatten field with internally tagged enum works JSON limitations" — corroborated by the Context7 serde docs' enum-representations page and the linked serde-rs/serde#1189 GitHub issue title/description (issue itself not fetched directly, relying on search-result summary — recommend the planner spot-check this against the live issue if the flatten pattern is ever reconsidered)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all versions verified directly against crates.io registry, `config` matches the pre-existing D-26 pin exactly
- Architecture (crate dependency direction, session-field pattern): HIGH for the serde flatten pitfall (corroborated by open GitHub issue + serde's own docs); MEDIUM for the `rdpilot-ipc`/daemon mapping-function placement (A1) since it reinterprets locked decision text rather than confirming it
- Pitfalls: HIGH — all three code-level pitfalls (flatten, ProjectDirs subfolder, dependency direction) verified against official docs or live crate inspection, not training-data guesses
- Security domain: HIGH — directly extends the project's own existing, already-live D-14/D-15/D-06 risk-acceptance posture rather than introducing new judgment calls

**Research date:** 2026-07-11
**Valid until:** 30 days (stable Rust ecosystem, no fast-moving dependencies in this phase's scope)
