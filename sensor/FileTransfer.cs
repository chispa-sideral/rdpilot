// FileTransfer DTOs + sensor-mediated remote-path validator + copy-with-
// inline-SHA256 handler for the RDPILOT_SENSOR wire protocol (Phase 10,
// FILE-01/FILE-02/FILE-03). Wire schema (10-03-PLAN Task 1, mirrors
// crates/rdpilot/src/sensor.rs's MsgType::FileTransfer doc comment):
//   request payload:  {"op": "Upload"|"Download", "remote_path": <string>, "share_name": <string>}
//   response payload: {"success":true,"data":{"bytes_transferred":<i64>,"sha256":<string>}}
//                   or {"success":false,"error":"...","error_kind":"path_traversal"|"io"|null}
//
// D-10.2 (this validator is the C# half of the symmetric defense-in-depth):
// every candidate destination path is validated against a FIXED sensor-side
// transfer root using Path.GetRelativePath + ".."/rooted rejection — NEVER
// `string.StartsWith` (Pitfall 3 sibling-prefix bypass) and NEVER
// `Contains("..")` (substring matching, the exact FreeRDP
// GHSA-3xpj-m4hx-8vmx/CVE-2025-48817 off-by-one bug class this phase's
// FILE-03 BLOCKING criterion exists to preempt).
//
// D-10.4 (BLOCKER fix — structured error reason): a path-validation
// rejection sets `error_kind = "path_traversal"` on the response, DISTINCT
// from a copy/IO failure's `error_kind = "io"`. This is the caller-facing
// PathTraversal producer Plan 10-04's `sensor_request` caller maps to
// `Error::PathTraversal` (see the two-validator reason-mapping note on
// `Transfer` below).
//
// D-10.5 (checksum): FileStream read/write loop with
// `IncrementalHash.CreateHash(HashAlgorithmName.SHA256)` fed inline — never
// `File.Copy` followed by a separate hashing pass (would double the I/O for
// large files with zero benefit).

using System.Security.Cryptography;
using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// Upload/download discriminant for the single `FileTransfer` `MsgType`
/// variant (10-CONTEXT Claude's Discretion: one variant with an op
/// discriminant, not two separate `MsgType` values). Serializes as the bare
/// string form ("Upload"/"Download") via the same
/// `[JsonSourceGenerationOptions(UseStringEnumConverter = true)]` applied to
/// the whole `EnvelopeJsonContext` (matches `MsgType`'s own wire shape).
internal enum FileTransferOp
{
    Upload,
    Download,
}

/// The FileTransfer request payload: `{"op": ..., "remote_path": <string>,
/// "share_name": <string>}`.
///
/// `RemotePath` is validated by <see cref="FileTransfer.ValidateRemotePath"/>
/// against the FIXED sensor-side transfer root (see
/// <see cref="FileTransfer.ShareRoot"/>) before any `System.IO` call —
/// untrusted input from the (by-design unauthenticated) DVC channel, never
/// treated as a real absolute remote-machine path outside that root.
/// `ShareName` is the filename under the RDPDR-redirected
/// `\\tsclient\RDPILOT\<name>` UNC path (drive name `"RDPILOT"`,
/// `connect.rs` line ~131) — validated independently, on the Rust side, by
/// `RdpilotDriveBackend::resolve_under_root` (Plan 10-01).
internal sealed record FileTransferRequest
{
    [JsonPropertyName("op")]
    public required FileTransferOp Op { get; init; }

    [JsonPropertyName("remote_path")]
    public required string RemotePath { get; init; }

    [JsonPropertyName("share_name")]
    public required string ShareName { get; init; }
}

/// The `data` shape of a successful FileTransfer reply:
/// `{"bytes_transferred":<i64>,"sha256":<string>}`. `Sha256` is the lowercase
/// hex digest (`Convert.ToHexString` upper-cases by default — see
/// <see cref="FileTransfer.Transfer"/> for the explicit lowercasing) computed
/// inline during the single `FileStream` copy pass (D-10.5).
internal sealed record FileTransferData
{
    [JsonPropertyName("bytes_transferred")]
    public required long BytesTransferred { get; init; }

    [JsonPropertyName("sha256")]
    public required string Sha256 { get; init; }
}

/// The FileTransfer response payload (D-6.4 success/failure envelope shape,
/// extended by D-10.4's structured `error_kind`).
///
/// `ErrorKind` is a small string sentinel (NOT a `[JsonStringEnumConverter]`
/// enum — the plan explicitly offers either shape; a plain nullable string
/// keeps the wire value symmetric with the Rust-side mapping this drives:
/// `"path_traversal"` maps to `Error::PathTraversal` in Plan 10-04, `"io"`
/// (or `null`) maps to a generic transport/semantic failure). This field is
/// what lets a path-validation rejection be distinguished from a copy IO
/// failure by Plan 10-04's `sensor_request` caller (BLOCKER fix, D-10.4,
/// CLI-03 precursor) — the free-text `Error` string alone is not enough.
internal sealed record FileTransferResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public FileTransferData? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }

    [JsonPropertyName("error_kind")]
    public string? ErrorKind { get; init; }
}
