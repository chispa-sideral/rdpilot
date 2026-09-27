# Cua Driver 0.28.2 distribution notices

We redistribute the unchanged official Windows x86_64 binary ZIP with SHA256
`1f4bfceeab64cb7f56be7aad774c3dc2d2910d1427e4be1d79939c706e8029ba`.
Its six entries are the driver, cursor helper, UIA helper, SDK DLL, Node runtime
and ABI header. The bare ZIP contains no license/notice files. These accompanying
files supply the Cua license and collected third-party notices; keep them with
the distribution.

Cua's MIT text is `Cua-MIT.txt`. Versioned source and release workflow:

- https://github.com/trycua/cua/tree/cua-driver-rs-v0.28.2/libs/cua-driver
- https://github.com/trycua/cua/blob/cua-driver-rs-v0.28.2/.github/workflows/cd-rust-cua-driver.yml

## Locked Rust artifacts

The workflow builds `cua-driver`, `cursor-theme-cli`, `cua-driver-uia` and
`cua-driver-sdk` with `cargo build --locked` for Windows. `Cua-Cargo-inventory.json`
records 360 registry packages in a conservative normal/build dependency closure
for `x86_64-pc-windows-msvc`; dev-only dependency edges are excluded. It includes
package versions, declared license expressions, original source archive URLs,
Cargo.lock SHA256 checksums and notice-text hashes.

`Cua-Cargo-NOTICES.txt` reproduces the collected license/notice/copyright files,
including nested notices for bundled native code. Identical texts are stored
once with all applicable packages listed. Keeping all alternative license texts
does not remove any copyright attribution or select a more restrictive option.
The graph may include unused features or build-only dependencies. It is a
conservative source-derived notice inventory, not a binary SBOM or a claim to
have independently reconstructed every upstream build input.

Collection evidence: the exact release-tag Rust Cargo manifests and Cargo.lock
were read with `cargo metadata --locked --filter-platform x86_64-pc-windows-msvc`.
Only temporary target source placeholders were used to let Cargo read manifests;
no upstream binary was rebuilt or changed. Package source archives were resolved
through Cargo's checksum-verified registry cache. The four roots' non-dev edges
were traversed, retaining normal and build dependencies. License files absent
from published crate archives were recovered from the exact `.cargo_vcs_info.json`
source commit (clipboard-win, jsonschema-regex, UniFFI and Nugine SIMD); those
source URLs are recorded in the inventory. Notice collection occurred 2026-09-27.

The represented licenses are MIT/Apache-2.0 and their alternatives, BSD, ISC,
Zlib, Boost, Unicode, CDLA-Permissive and MPL-2.0. Their copyright/license texts
remain intact. This distribution keeps Cua as a separate unmodified process;
none of these declarations requires replacing rdpilot's LGPL-3.0-or-later
license. No incompatible declared license was found in this inspected closure.
This finding is limited to the source-derived graph described above.

The UniFFI 0.31.0 packages declare MPL-2.0. Their unchanged corresponding Source
Code Form is available at each exact-version source archive URL in the inventory,
and at https://github.com/mozilla/uniffi-rs/tree/309762f55db3f0548194a9ceba3027fa64b18a93.
The full MPL license is included in `MPL-2.0.txt`. Recipients may obtain, modify
and redistribute that source under the MPL; rdpilot places no additional
restriction on those rights.

## Bundled Node component

`Cua-Node-runtime-NOTICE.md` is the unchanged upstream notice for
`cua_driver_node_runtime.node`. Its corresponding Source Code Form consists of
the pinned npm development package's runtime sources plus Cua's deterministic
transformations. Obtain both here (no payment required):

- Original source: https://registry.npmjs.org/uniffi-bindgen-react-native/-/uniffi-bindgen-react-native-0.31.0-3.tgz
- Transformation/build script: https://github.com/trycua/cua/blob/cua-driver-rs-v0.28.2/libs/cua-driver/scripts/build-node-runtime.mjs
- Original notice: https://github.com/trycua/cua/blob/cua-driver-rs-v0.28.2/libs/cua-driver/scripts/node-runtime-NOTICE.md

The script copies `runtimes/core` and `runtimes/napi` from that npm package,
patches the two N-API RustBuffer boundaries and compiles the result. Those
covered sources remain MPL-2.0. The included MPL text and these instructions
inform recipients how to obtain the corresponding source under that license.
The Node component is not needed by rdpilot's direct MCP launch, but is retained
because this package ships the approved unchanged upstream distribution.

**Provenance limitation:** this Node build is not Cargo-locked. The npm source
package contains no runtime Cargo.lock, and the build script runs `cargo build`
without `--locked`. Therefore the release tag does not establish the exact
transitive versions compiled into the published Node binary. Supplemental
attributions in `Cua-Node-supplemental-NOTICES.txt` cover the currently resolved
runtime families, with several versions corroborated by binary source-path
strings. They must not be presented as an exact historical dependency audit.
An upstream build lock/SBOM or equivalent provenance is needed to close that
remaining gap before claiming comprehensive release dependency compliance.

## rdpilot

rdpilot is LGPL-3.0-or-later; the repository LICENSE applies to our binaries.
These notices do not claim that rdpilot owns or signs upstream components.
Preserve upstream signed bytes and notices, and refresh this inventory whenever
the Cua version changes. This inventory covers Cua's dependencies only; it does
not replace the release's own rdpilot dependency/source obligations.
