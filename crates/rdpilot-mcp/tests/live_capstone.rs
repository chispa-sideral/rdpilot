//! PROOF-04 -- the live-LLM capstone (gated, live-only, D-25/D-15.1).
//!
//! Per binding direction 1 (developer decision, superseding this phase's
//! own `15-RESEARCH.md` reqwest/Anthropic-API sketch): the LLM driving this
//! capstone is the LOCAL, already-authenticated Claude Code CLI in headless
//! print mode -- `claude -p "<task>" --mcp-config <rendered config>`. NO
//! `ANTHROPIC_API_KEY`, NO `reqwest`, NO Anthropic SDK, NO hardcoded model
//! id anywhere in this file: the local CLI resolves its own auth and model
//! exactly as an interactive session would. `crates/rdpilot-mcp/Cargo.toml`
//! already documents this split next to the PROOF-03 `[dev-dependencies]`
//! block.
//!
//! Drives the REAL compiled `rdpilot-mcp` binary as an MCP server FOR
//! `claude -p` (via `--mcp-config`, `tests/fixtures/capstone-mcp-config.json`
//! rendered with real connection secrets at run time) against a REAL remote
//! Windows target (`.secrets/connection.json`) -- the same 7-Zip File
//! Manager (`class_name` `"7-Zip::FM"`) every other Phase 15 proof targets,
//! for continuity with the Phase 9/PROOF-03 proofs.
//!
//! # Two-part success assertion (binding direction 1 -- never self-report)
//!
//! 1. **Transcript evidence:** the captured `claude -p --output-format
//!    stream-json` output is scanned for `tool_use` blocks and MUST show at
//!    least one read/inspect call (`rdpilot_uia` or `computer`) AND at
//!    least one file-transfer call (`rdpilot_put` or `rdpilot_get`).
//! 2. **Independent side-effect verification:** AFTER `claude -p` exits,
//!    this test opens its OWN fresh `rmcp` client subprocess (Pattern 2,
//!    reusing `tests/live_proof.rs`'s exact pattern) against the SAME
//!    compiled `rdpilot-mcp` binary, downloads the file the capstone prompt
//!    told the model to upload (a fixed, test-authored local seed file,
//!    fixed remote name, fixed session name -- see the `CAPSTONE_*`
//!    constants), and compares the downloaded bytes BYTE-FOR-BYTE against
//!    the original seed bytes this test wrote to disk itself. This never
//!    trusts the model's prose, and never even trusts the checksum the
//!    model's own `rdpilot_put`/`rdpilot_get` tool calls reported in the
//!    transcript -- it is a ground-truth local file comparison driven by a
//!    SEPARATE MCP client connection, exactly mirroring what `rdpilot_get`'s
//!    own metadata-only design intends `bytes_transferred`/`checksum` to
//!    prove, but without needing to reimplement the daemon's SHA-256
//!    algorithm here (no new dependency, see `Cargo.toml`'s T-15-SC note).
//!
//! FAILS (never a silent pass) if either half of the assertion is missing.
//!
//! # Gating (D-18) -- fail LOUD once armed, never a silent skip once armed
//!
//! `#[ignore]`-gated, and additionally early-returns cleanly (a legitimate,
//! quiet skip, mirroring every other Phase 15 harness) when `RDPILOT_LIVE`
//! is unset or `.secrets/connection.json` is absent -- that combination
//! means the suite was never opted into. But once BOTH are present (the
//! developer explicitly armed the live gate), a missing `claude` binary on
//! `PATH` is a HARD, loud failure (`panic!`/`assert!`), never a quiet
//! `eprintln!` + `return` that could be mistaken for a pass -- this is the
//! plan's explicit "fail fast ... never a silent skip that reads like a
//! pass" requirement for the `claude`-CLI half of the gate specifically.
//!
//! `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_capstone -- --ignored`
//! runs it against a provisioned VM + an authenticated local `claude` CLI.
//! Plain `cargo test -p rdpilot-mcp` lists it (`--list`) but never executes
//! it -- nothing here runs offline or in CI.
//!
//! # Non-interactive tool-permission flag -- TODO-for-15-08 (execution-time detail)
//!
//! See [`PERMISSION_FLAGS`]'s doc comment: this is the ONE place the exact
//! flag(s) live. If the locally installed `claude` CLI's headless MCP
//! tool-approval behavior differs from what is documented there, 15-08
//! changes ONLY that constant -- nothing else in this file encodes the
//! choice.
//!
//! # Security
//!
//! Reads the gitignored `.secrets/connection.json` and hands the
//! credentials to the spawned `rdpilot-mcp` server subprocess's OWN
//! environment ONLY (never as a `claude`/tool-call argument, never printed,
//! never included in the rendered `--mcp-config` temp file's path that gets
//! logged) -- see `tests/fixtures/capstone-mcp-config.json`'s doc comment
//! for the full substitution/redaction contract (T-15-08). The rendered
//! config's temp file is deleted after the run. Nothing in this file's
//! `println!`/`format!`/`panic!` calls ever includes `target.password`.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command as TokioCommand;

/// Name of the opt-in env var that arms this live suite (D-18), mirroring
/// `tests/live_proof.rs`'s identical convention.
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// The EXACT session name the capstone task prompt instructs the model to
/// use for `rdpilot_connect`/every subsequent call. Fixed (not model-chosen)
/// so this test's independent verification step can address the session by
/// name without needing to first `rdpilot_list`-discover it.
const CAPSTONE_SESSION_NAME: &str = "capstone-04";

/// The EXACT remote transfer-root name the prompt instructs the model to
/// use for both `rdpilot_put` and the independent verification's
/// `rdpilot_get`.
const CAPSTONE_REMOTE_NAME: &str = "rdpilot-capstone-04-upload.bin";

/// The known seed content this test writes to a local file BEFORE spawning
/// `claude -p`, and that the prompt tells the model to upload verbatim.
/// The independent verification step downloads `CAPSTONE_REMOTE_NAME` back
/// to a SEPARATE local path and asserts its bytes equal this constant
/// exactly -- never re-deriving trust from the model's own report.
const CAPSTONE_SEED_CONTENT: &[u8] = b"rdpilot PROOF-04 capstone payload -- independently verified\n";

/// The full, individually quoted 7-Zip File Manager install path (mirrors
/// `crates/rdpilot/tests/support/proof_harness.rs::SEVEN_ZIP_EXE`,
/// human-confirmed live by 09-01) -- a bare `"7zFM.exe"` fails live with
/// `LastError=2`, only the complete quoted path works.
const SEVEN_ZIP_EXE: &str = "\"C:\\Program Files\\7-Zip\\7zFM.exe\"";

/// Seed argument pre-navigating 7zFM to a known, always-present folder
/// (mirrors `proof_harness.rs::SEVEN_ZIP_SEED_ARGS`) so the read/inspect
/// step's menu bar walk has a deterministic window to target.
const SEVEN_ZIP_SEED_ARGS: &str = "\"C:\\Program Files\"";

/// The exact, human-confirmed (09-01) window class for 7-Zip File Manager
/// (mirrors `proof_harness.rs::SEVEN_ZIP_CLASS`). Matched EXACTLY, never a
/// title substring.
const SEVEN_ZIP_CLASS: &str = "7-Zip::FM";

/// Bounded wall-clock budget for the WHOLE `claude -p` subprocess run
/// (T-15-10: a hung/malformed transcript must never stall the test
/// indefinitely). `claude`'s locally-installed CLI (this session's
/// `--help`) exposes no `--max-turns` flag to cap agent turns directly, so
/// this wall-clock bound is the primary safety net; the task prompt itself
/// also asks the model to work efficiently. Generous for a real RDP-driven
/// multi-tool-call task against a live VM -- adjust here (not scattered
/// elsewhere) if 15-08 finds it too tight/loose.
const CAPSTONE_TIMEOUT: Duration = Duration::from_secs(300);

/// TODO-for-15-08 (execution-time detail, plan binding constraint): the
/// exact flag(s) that make `claude -p` auto-permit the `rdpilot` MCP
/// server's tool calls without an interactive approval prompt. Verified
/// against the LOCALLY INSTALLED `claude` CLI's own `--help` this
/// authoring session (v2.1.207): `--permission-mode bypassPermissions`
/// bypasses per-call approval for the whole session, and
/// `--strict-mcp-config` additionally ensures ONLY the `rdpilot` server
/// from `--mcp-config` is loaded (no other ambient project/user MCP
/// servers/tools leak into this run). This is a SANE DEFAULT, not a
/// confirmed-working combination -- it has never been run live (this plan
/// is OFFLINE-AUTHOR only). If 15-08 finds `claude -p` still stalls on an
/// interactive tool-approval prompt with these flags, the documented
/// fallback is `--dangerously-skip-permissions` (paired with
/// `--allow-dangerously-skip-permissions` first, if the installed CLI gates
/// the former behind the latter). Change ONLY this constant; every other
/// use site in this file references it, never hardcodes a flag name of its
/// own. Per the plan's binding constraint: if headless MCP tool use turns
/// out to be impossible non-interactively at all, this harness must FAIL
/// LOUDLY (the transcript tool-call assertion below naturally does this --
/// zero tool calls observed is an assertion failure, never a silent pass).
const PERMISSION_FLAGS: &[&str] = &["--permission-mode", "bypassPermissions", "--strict-mcp-config"];

/// A live connection target, loaded from `.secrets/connection.json` at the
/// workspace root. Deliberately has NO `Debug`/`Display` impl (mirrors
/// `tests/live_proof.rs::LiveTarget`) -- nothing in this file can
/// accidentally format the whole struct (and thus the password) into a
/// message.
struct LiveTarget {
    host: String,
    user: String,
    password: String,
    port: u16,
}

/// Locate `.secrets/connection.json` relative to the workspace root
/// (mirrors `tests/live_proof.rs::connection_file`).
fn connection_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("connection.json")
}

/// Locate the byte-verified sensor executable relayed by Plan 15-05
/// (`.secrets/sensor-build/rdpilot-sensor.exe`), same two-levels-up
/// resolution as [`connection_file`]. **Live-diagnosed here (Plan 15-08,
/// mirroring the identical Plan 15-06/15-07 finding for
/// `live_cli_verbs.rs`/both `live_proof.rs` harnesses):** without pointing
/// the spawned `rdpilot-mcp` subprocess's `RDPILOT_SENSOR_BINARY_PATH` at
/// this path, the real `Connect` path never deploys a sensor at all, so
/// every sensor-backed tool call this capstone drives (`rdpilot_launch`,
/// `rdpilot_window_list`, `rdpilot_uia`, `rdpilot_put`/`rdpilot_get`) times
/// out identically to what those three prior plans already found and fixed.
fn sensor_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".secrets")
        .join("sensor-build")
        .join("rdpilot-sensor.exe")
}

/// Load the live target, or `None` if the suite is not armed (D-18 gate:
/// `RDPILOT_LIVE` unset, or the secrets file absent) -- a legitimate,
/// quiet skip. A present-but-malformed file panics with a descriptive
/// message that never includes the password (mirrors
/// `tests/live_proof.rs::load_live_target`).
fn load_live_target() -> Option<LiveTarget> {
    std::env::var_os(LIVE_ENV)?;

    let path = connection_file();
    if !path.exists() {
        return None;
    }

    let raw = std::fs::read_to_string(&path)
        .expect("RDPILOT_LIVE is set and .secrets/connection.json exists but could not be read");
    let json: serde_json::Value = serde_json::from_str(&raw).expect(".secrets/connection.json is not valid JSON");

    let host =
        json.get("host").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `host`").to_owned();
    let user =
        json.get("user").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `user`").to_owned();
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `password`")
        .to_owned();
    let port: u16 =
        json.get("rdpPort").and_then(serde_json::Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(3389);

    Some(LiveTarget { host, user, password, port })
}

/// `RDPILOT_LIVE` is armed (a [`LiveTarget`] was loaded) but is the local
/// `claude` CLI actually resolvable? A short `claude --version` probe --
/// `--version` never touches MCP config/tools, so no long-lived detached
/// grandchild risk (unlike the full capstone run below), a plain
/// `.output()` is safe here.
fn claude_cli_available() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Record one D-9.4-style `PASS`/`FAIL` step: print it immediately (so a
/// long live run streams progress) and append it to `steps` for the final
/// summary/assert. Mirrors `tests/live_proof.rs::record`.
fn record(steps: &mut Vec<(String, bool, String)>, name: &str, passed: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("  [{}] {name}: {detail}", if passed { "PASS" } else { "FAIL" });
    steps.push((name.to_owned(), passed, detail));
}

/// Render `tests/fixtures/capstone-mcp-config.json`'s template into a
/// concrete `--mcp-config` file at `dest`, substituting the documented
/// tokens with `mcp_bin`/`target`'s real values, and stripping the
/// template's own `_comment` key (claude only needs `mcpServers`; dropping
/// the comment keeps the rendered file minimal and avoids depending on
/// unknown-top-level-key tolerance in the CLI's own parser). NEVER logs the
/// rendered contents (they carry `target.password`).
fn render_mcp_config(mcp_bin: &Path, target: &LiveTarget, dest: &Path) {
    let template_raw = include_str!("fixtures/capstone-mcp-config.json");
    let template: serde_json::Value = serde_json::from_str(template_raw).expect("fixture template must be valid JSON");

    let mut servers =
        template.get("mcpServers").cloned().expect("fixture template must have a top-level `mcpServers` object");
    let rdpilot = servers
        .get_mut("rdpilot")
        .expect("fixture template must define a `mcpServers.rdpilot` entry");

    rdpilot["command"] = serde_json::Value::String(mcp_bin.to_string_lossy().into_owned());
    let env = rdpilot.get_mut("env").expect("fixture template's `rdpilot` entry must have an `env` object");
    env["RDPILOT_HOST"] = serde_json::Value::String(target.host.clone());
    env["RDPILOT_PORT"] = serde_json::Value::String(target.port.to_string());
    env["RDPILOT_USERNAME"] = serde_json::Value::String(target.user.clone());
    env["RDPILOT_PASSWORD"] = serde_json::Value::String(target.password.clone());
    env["RDPILOT_SENSOR_BINARY_PATH"] = serde_json::Value::String(sensor_binary_path().to_string_lossy().into_owned());

    let rendered = serde_json::json!({ "mcpServers": servers });
    let mut file = std::fs::File::create(dest).expect("create the rendered --mcp-config temp file");
    file.write_all(rendered.to_string().as_bytes()).expect("write the rendered --mcp-config temp file");
}

/// The exact, deterministic task prompt given to the model. Every
/// address/name it touches (`session`, `local upload path`, `remote_name`)
/// is fixed here, not left to the model's discretion, so the independent
/// side-effect verification step (below) can address the same session/file
/// without needing to discover it first. Deliberately never mentions a
/// host/username/password -- the server already has those from its own
/// process environment (D-27), and the prompt says so explicitly so the
/// model does not go hunting for credentials to pass itself.
fn capstone_prompt(local_upload_path: &Path) -> String {
    format!(
        "You are operating a remote Windows desktop through the \"rdpilot\" MCP server's tools. \
         Complete the following steps, IN ORDER, using ONLY the rdpilot MCP tools. Do not ask \
         clarifying questions and do not wait for confirmation -- just execute every step, then \
         report what happened.\n\
         \n\
         1. Call rdpilot_connect with name=\"{session}\". Do NOT pass host/username/password \
            yourself and do NOT ask the user for them -- the server already has its target and \
            credentials configured via its own environment.\n\
         2. Launch the 7-Zip File Manager on session \"{session}\" with rdpilot_launch: \
            exe={exe}, args={args}.\n\
         3. Call rdpilot_window_list on session \"{session}\" and find the window whose class is \
            exactly \"{class}\"; bring it to the foreground with rdpilot_foreground.\n\
         4. Call rdpilot_uia on session \"{session}\" against that window's hwnd (scope: children \
            is enough) and report the menu bar item names it returns.\n\
         5. Upload the local file at \"{local_upload}\" to the session's remote transfer root as \
            \"{remote_name}\" using rdpilot_put on session \"{session}\". Report the checksum and \
            bytes_transferred it returns.\n\
         6. Download it back with rdpilot_get on session \"{session}\" (remote_name=\"{remote_name}\") \
            to a local path of your own choosing. Report the checksum and bytes_transferred it \
            returns.\n\
         7. State in one sentence whether the checksums from steps 5 and 6 matched.\n\
         \n\
         Use the EXACT session name \"{session}\" and the EXACT remote_name \"{remote_name}\" given \
         above for every call -- do not invent your own names.",
        session = CAPSTONE_SESSION_NAME,
        exe = serde_json::Value::String(SEVEN_ZIP_EXE.to_owned()),
        args = serde_json::Value::String(SEVEN_ZIP_SEED_ARGS.to_owned()),
        class = SEVEN_ZIP_CLASS,
        local_upload = local_upload_path.display(),
        remote_name = CAPSTONE_REMOTE_NAME,
    )
}

/// One `claude -p` subprocess invocation's captured result.
struct ClaudeRun {
    /// `true` if the process exited zero WITHIN the timeout. `false` for
    /// either a nonzero exit or a killed-on-timeout run -- the transcript
    /// assertion (not this flag) is what ultimately decides PROOF-04's
    /// pass/fail, since a model that completed every tool call but then hit
    /// an unrelated late-turn CLI hiccup should still be judged on what it
    /// actually did.
    exited_zero: bool,
    stdout: String,
    stderr: String,
}

/// Spawn `claude -p <prompt> --mcp-config <mcp_config_path> <PERMISSION_FLAGS> --output-format
/// stream-json --verbose`, capturing stdout/stderr to REAL FILES (never a
/// piped `output()`/`wait_with_output()`) -- mirrors
/// `crates/rdpilot-cli/tests/cli_lifecycle.rs::run_cli`'s stdio-capture
/// discipline: the MCP server subprocess `claude` spawns
/// (`rdpilot-mcp`) itself auto-starts a detached `rdpilot-daemon`
/// grandchild (`connect.rs`) that would inherit a piped stdout's write end
/// and never close it, hanging `wait_with_output()` until the daemon itself
/// exits. Reading a file back after `claude` (the immediate child) exits
/// does not have that problem. Bounded by [`CAPSTONE_TIMEOUT`]
/// (T-15-10): on timeout the child is killed and this function returns a
/// non-zero-exit result rather than hanging forever, but a killed run is
/// still handed to the transcript parser below (a model that made several
/// tool calls before running out of time should still get partial credit
/// against the transcript assertion, per this struct's own doc comment).
async fn run_claude_p(prompt: &str, mcp_config_path: &Path, capture_dir: &Path) -> ClaudeRun {
    let stdout_path = capture_dir.join("claude-stdout.jsonl");
    let stderr_path = capture_dir.join("claude-stderr.log");
    let stdout_file = std::fs::File::create(&stdout_path).expect("create the claude stdout capture file");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create the claude stderr capture file");

    let mut child = TokioCommand::new("claude")
        .arg("-p")
        .arg(prompt)
        .arg("--mcp-config")
        .arg(mcp_config_path)
        .args(PERMISSION_FLAGS)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .expect("spawn the local `claude` CLI");

    let exited_zero = match tokio::time::timeout(CAPSTONE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            eprintln!("claude -p: error waiting on the subprocess: {e}");
            false
        }
        Err(_elapsed) => {
            eprintln!(
                "claude -p exceeded the {CAPSTONE_TIMEOUT:?} bounded time budget (T-15-10) -- killing it; \
                 whatever it managed to do before now is still evaluated against the transcript below"
            );
            let _ = child.start_kill();
            let _ = child.wait().await;
            false
        }
    };

    ClaudeRun {
        exited_zero,
        stdout: std::fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

/// Recursively walk a parsed `stream-json` value (or any nested JSON
/// structure) collecting the `name` of every object shaped like an
/// Anthropic/Claude-Code `tool_use` content block (`{"type": "tool_use",
/// "name": "...", ...}`). Deliberately schema-tolerant (walks EVERY
/// object/array, not a hardcoded envelope path) -- `--output-format
/// stream-json`'s exact message envelope was not runnable/verifiable
/// offline this plan (OFFLINE-AUTHOR), so this avoids hardcoding a message
/// shape that might not match the installed CLI's actual JSONL output;
/// 15-08 discovers any real mismatch immediately via the transcript
/// assertion below (zero names collected -> hard failure, never a silent
/// pass).
fn collect_tool_use_names(value: &serde_json::Value, names: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                if let Some(name) = map.get("name").and_then(serde_json::Value::as_str) {
                    names.insert(name.to_owned());
                }
            }
            for v in map.values() {
                collect_tool_use_names(v, names);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_tool_use_names(v, names);
            }
        }
        _ => {}
    }
}

/// Parse `stdout` as `stream-json` JSONL (one JSON value per line; blank
/// lines and any non-JSON line are skipped, never a hard parse failure --
/// a partial/killed run can leave a truncated final line) and collect every
/// `tool_use` block's `name` across the whole transcript.
fn tool_use_names_from_transcript(stdout: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            collect_tool_use_names(&value, &mut names);
        }
    }
    names
}

/// `true` if `names` contains a tool whose name ENDS WITH one of
/// `suffixes` -- Claude Code's MCP tool-naming convention prefixes
/// externally-supplied MCP tools with `mcp__<server>__` (e.g.
/// `mcp__rdpilot__rdpilot_uia`), but that exact prefix is unverified
/// offline (OFFLINE-AUTHOR) and could differ by CLI version, so this
/// matches by suffix rather than hardcoding the full prefixed name.
fn any_tool_called(names: &HashSet<String>, suffixes: &[&str]) -> bool {
    names.iter().any(|n| suffixes.iter().any(|s| n.ends_with(s)))
}

/// `value` MUST be a JSON object -- the shape every `rdpilot_*` tool's
/// arguments take (mirrors `tests/live_proof.rs::args_object`).
fn args_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        other => panic!("expected a JSON object for tool-call arguments, got {other:?}"),
    }
}

/// Join every text content block's text (mirrors
/// `tests/live_proof.rs::text_of`).
fn text_of(result: &CallToolResult) -> String {
    result.content.iter().filter_map(ContentBlock::as_text).map(|t| t.text.clone()).collect::<Vec<_>>().join(" ")
}

/// Call `name` with `arguments` against the independent verification
/// client and classify the outcome (mirrors `tests/live_proof.rs::call`).
async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &'static str,
    arguments: serde_json::Value,
) -> Result<CallToolResult, String> {
    client
        .call_tool(CallToolRequestParams::new(name).with_arguments(args_object(arguments)))
        .await
        .map_err(|e| format!("{name} call failed: {e}"))
        .and_then(|result| {
            if result.is_error == Some(true) {
                Err(format!("{name} returned is_error=true: {}", text_of(&result)))
            } else {
                Ok(result)
            }
        })
}

#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1, an authenticated local claude CLI, and a live Azure VM"]
async fn capstone_llm_drives_read_inspect_and_file_transfer_through_mcp() {
    let Some(target) = load_live_target() else {
        eprintln!("skipping PROOF-04: RDPILOT_LIVE unset or .secrets/connection.json absent");
        return;
    };

    // From here on, RDPILOT_LIVE is armed -- a missing `claude` CLI is a
    // hard, loud failure (never a silent skip that reads like a pass, the
    // plan's explicit fail-fast requirement for this specific gate half).
    assert!(
        claude_cli_available(),
        "RDPILOT_LIVE is set but the `claude` CLI is not resolvable on PATH (or `claude --version` \
         failed) -- install/authenticate Claude Code before running PROOF-04. This is a hard \
         failure, not a skip: RDPILOT_LIVE signals explicit opt-in to run this live capstone. Note: \
         this harness deliberately never checks ANTHROPIC_API_KEY (binding direction 1) -- `claude` \
         auth is entirely the local CLI's own concern."
    );

    let mut steps: Vec<(String, bool, String)> = Vec::new();
    println!("=== rdpilot PROOF-04 live-LLM capstone (claude -p + independent side-effect verification) ===");

    let capture_root = std::env::temp_dir().join(format!("rdpilot-mcp-capstone-04-{}", std::process::id()));
    std::fs::create_dir_all(&capture_root).expect("create the capstone capture/temp root");

    // --- seed the local upload file BEFORE spawning claude -- this test
    // owns the ground-truth bytes the independent verification step below
    // compares against ---
    let local_upload = capture_root.join("capstone-04-upload-seed.bin");
    std::fs::write(&local_upload, CAPSTONE_SEED_CONTENT).expect("seed the local upload file");

    // --- render the Task-1 fixture into a temp --mcp-config pointed at the
    // REAL compiled rdpilot-mcp binary + the real connection secrets
    // (never logged) ---
    let mcp_bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-mcp"));
    let mcp_config_path = capture_root.join("mcp-config.json");
    render_mcp_config(&mcp_bin, &target, &mcp_config_path);

    // --- Step 1: spawn `claude -p` bounded by CAPSTONE_TIMEOUT, driving
    // the read/inspect + file-transfer task purely through the rdpilot MCP
    // tools ---
    let prompt = capstone_prompt(&local_upload);
    let claude_run = run_claude_p(&prompt, &mcp_config_path, &capture_root).await;
    record(
        &mut steps,
        "claude -p subprocess run",
        true, // informational only -- pass/fail is decided by the transcript/side-effect
              // assertions below, not the process exit code (see ClaudeRun's doc comment)
        format!(
            "exited_zero={} stdout_bytes={} stderr_tail={:?}",
            claude_run.exited_zero,
            claude_run.stdout.len(),
            claude_run.stderr.chars().rev().take(200).collect::<String>().chars().rev().collect::<String>()
        ),
    );

    // --- Step 2: TRANSCRIPT evidence -- at least one read/inspect call AND
    // at least one file-transfer call, parsed from the captured
    // stream-json transcript, never from the model's closing prose ---
    let tool_names = tool_use_names_from_transcript(&claude_run.stdout);
    let read_inspect_called = any_tool_called(&tool_names, &["rdpilot_uia", "computer"]);
    let file_transfer_called = any_tool_called(&tool_names, &["rdpilot_put", "rdpilot_get"]);
    record(
        &mut steps,
        "transcript: read/inspect tool call observed",
        read_inspect_called,
        format!("observed tool names: {tool_names:?}"),
    );
    record(
        &mut steps,
        "transcript: file-transfer tool call observed",
        file_transfer_called,
        format!("observed tool names: {tool_names:?}"),
    );

    // --- Step 3: INDEPENDENT side-effect verification -- a FRESH rmcp
    // client subprocess (Pattern 2, tests/live_proof.rs), never the
    // claude-driven server, downloads CAPSTONE_REMOTE_NAME from
    // CAPSTONE_SESSION_NAME and the bytes are compared verbatim against
    // CAPSTONE_SEED_CONTENT -- independent of BOTH the model's prose AND
    // its own tool results' self-reported checksum ---
    let transport =
        TokioChildProcess::new(TokioCommand::new(&mcp_bin)).expect("spawn a SECOND, independent rdpilot-mcp subprocess");
    let verify_client = ()
        .serve(transport)
        .await
        .expect("complete the MCP initialize handshake against the independent rdpilot-mcp subprocess");

    let local_download = capture_root.join("capstone-04-download-verify.bin");
    let get_args = serde_json::json!({
        "session": CAPSTONE_SESSION_NAME,
        "remote_name": CAPSTONE_REMOTE_NAME,
        "local_path": local_download.to_string_lossy(),
    });
    let get_result = call(&verify_client, "rdpilot_get", get_args).await;
    let side_effect_ok = match &get_result {
        Ok(_) => match std::fs::read(&local_download) {
            Ok(downloaded) => downloaded == CAPSTONE_SEED_CONTENT,
            Err(e) => {
                eprintln!("independent verification: failed to read the downloaded file back: {e}");
                false
            }
        },
        Err(_) => false,
    };
    record(
        &mut steps,
        "independent side-effect: downloaded bytes match the known local seed file",
        side_effect_ok,
        match &get_result {
            Ok(r) => format!(
                "rdpilot_get succeeded ({}); byte-for-byte match against the {}-byte seed file = {side_effect_ok}",
                text_of(r),
                CAPSTONE_SEED_CONTENT.len()
            ),
            Err(e) => e.clone(),
        },
    );

    // --- cleanup: disconnect the capstone's session via the independent
    // client (best-effort -- not itself a pass/fail signal) ---
    let _ = call(&verify_client, "rdpilot_disconnect", serde_json::json!({ "session": CAPSTONE_SESSION_NAME })).await;
    let _ = verify_client.cancel().await;

    let _ = std::fs::remove_file(&local_upload);
    let _ = std::fs::remove_file(&local_download);
    let _ = std::fs::remove_file(&mcp_config_path); // never leave the secret-bearing rendered config on disk
    let _ = std::fs::remove_dir_all(&capture_root);

    // Only the two REQUIRED assertions decide PROOF-04's outcome (binding
    // direction 1): transcript evidence of both tool-call kinds, AND the
    // independent side-effect. The subprocess exit-code step above is
    // informational only.
    let required = [&steps[1], &steps[2], &steps[3]];
    let all_passed = required.iter().all(|(_, passed, _)| *passed);
    println!("PROOF: {}", if all_passed { "PASS" } else { "FAIL" });

    assert!(
        all_passed,
        "PROOF-04 requires BOTH transcript evidence of a read/inspect + file-transfer tool call \
         AND an independently-verified side-effect (byte-for-byte match) -- see the PASS/FAIL trace \
         above. A model that only talks about success without actually calling the tools, or a tool \
         call that did not actually land the file on the target, both FAIL here by design (binding \
         direction 1: never assert success from the model's self-report alone)."
    );
}
