# Deferred Items — Phase 02

Out-of-scope discoveries logged during execution (not fixed; tracked here).

## From Plan 02-02 (session/framebuffer)

- **rustdoc intra-doc-link warnings in `crates/rdpilot/src/config.rs`** (pre-existing
  from Wave 1, commit `b0b6ab0`): the public `ConnectionConfig::new` / `port` doc
  comments link to `[`DEFAULT_PORT`]`, `[`DEFAULT_WIDTH`]`, `[`DEFAULT_HEIGHT`]`,
  which are `pub const`s inside the private `config` module — so rustdoc reports
  "public documentation links to private item". `cargo doc --no-deps` still exits 0
  (warnings, not errors). Not introduced by this plan; deferred per scope-discipline.
  Suggested fix (future doc pass): re-export the consts (`pub use config::{DEFAULT_PORT, ...}`)
  or change the doc links to plain code spans.
