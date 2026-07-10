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

// ---------------------------------------------------------------------------
// Path validation + FileStream copy-with-inline-SHA256 handler. BCL-only
// (System.IO + System.Security.Cryptography), NativeAOT-safe, no new NuGet
// (D-10.5, T-10-SC).
// ---------------------------------------------------------------------------

internal static class FileTransfer
{
    /// The RDPDR-redirected UNC path the RDPILOT drive is mounted at inside
    /// the RDP session (drive name `"RDPILOT"`, `connect.rs` line ~131,
    /// proven live by `session.rs`'s `launch_command()` bootstrap convention
    /// — 10-RESEARCH Item 3). NOT validated by this module — the Rust-side
    /// `RdpilotDriveBackend::resolve_under_root` (Plan 10-01) is the
    /// validator for every path that crosses this UNC boundary.
    private const string UncShareBase = @"\\tsclient\RDPILOT";

    /// The FIXED sensor-side remote transfer root every `RemotePath`
    /// candidate is validated against (D-10.2's C#-side half of the
    /// symmetric defense-in-depth).
    ///
    /// This is a fixed constant, NOT caller-configurable (WARNING-1 fold-in,
    /// recorded in the plan's <output> instruction for Phase 13/14's future
    /// CLI/MCP put/get wiring): allowing the far-side (Rust SDK) caller to
    /// dictate the root the validator checks the destination against would
    /// let an attacker/bug on that side simply supply a root that already
    /// contains the traversal target, defeating the entire point of
    /// validating "under a root" in the first place. The root must be a
    /// value ONLY the sensor process itself controls. `Path.GetTempPath()`
    /// resolves to the current user's `%TEMP%` on the real Windows target —
    /// a directory that already exists and is writable by the same user
    /// context the sensor process runs under (the interactive RDP session
    /// user, per D-5.7's Session > 0 constraint) — a fixed, always-available
    /// sensor-owned location requiring no extra deployment step.
    internal static readonly string ShareRoot = Path.Combine(Path.GetTempPath(), "rdpilot-transfer-root");

    /// Best-effort creates <see cref="ShareRoot"/> if it does not already
    /// exist. Called once per `Transfer` invocation rather than at process
    /// startup — mirrors the fire-and-forget, no-persistent-init discipline
    /// of the other sensor handlers (e.g. `ProcessLaunch`, no static
    /// constructor side effects).
    internal static void EnsureShareRootExists() => Directory.CreateDirectory(ShareRoot);

    /// Unconditionally normalizes BOTH separator styles to
    /// `Path.DirectorySeparatorChar` — mirrors the Rust-side Pitfall-1 fix
    /// (10-01-SUMMARY.md `resolve_under_root`/`looks_rooted`): on the real
    /// Windows target both `\` and `/` are valid separators, but this must
    /// be deterministic regardless of which host actually executes the
    /// validator (this offline linux-x64 dotnet run vs. the real win-x64
    /// NativeAOT binary) for the FILE-03 selftest to be a faithful proxy.
    private static string NormalizeSeparators(string path) =>
        path.Replace('\\', Path.DirectorySeparatorChar).Replace('/', Path.DirectorySeparatorChar);

    /// A host-independent "does this look like an absolute/rooted path"
    /// check, run BEFORE any `Path`/`GetFullPath` call — mirrors the
    /// Rust-side `looks_rooted()` fix (10-01-SUMMARY.md Decision #1).
    ///
    /// `Path.IsPathRooted` alone is NOT sufficient here: it only recognizes
    /// a drive-letter prefix (`"C:"`) as rooted when actually running on
    /// Windows. On this offline linux-x64 host, `Path.IsPathRooted("C:/Windows/System32")`
    /// returns `false` (Linux path semantics have no concept of drive
    /// letters), which would silently let the FILE-03 Windows-drive-absolute
    /// adversarial case be nested harmlessly under the share root instead of
    /// rejected — a real gap masked by running the selftest offline, not a
    /// hypothetical one. This explicit check closes it deterministically on
    /// both platforms: a leading separator (POSIX-style absolute) OR an
    /// ASCII-letter-plus-colon prefix (Windows drive-style absolute) is
    /// always treated as rooted, regardless of host OS.
    private static bool LooksRooted(string normalizedCandidate)
    {
        if (normalizedCandidate.Length == 0)
        {
            return false;
        }

        if (normalizedCandidate[0] == Path.DirectorySeparatorChar)
        {
            return true;
        }

        return normalizedCandidate.Length >= 2
            && char.IsAsciiLetter(normalizedCandidate[0])
            && normalizedCandidate[1] == ':';
    }

    /// Validates `candidate` resolves to a genuine descendant of `root`
    /// (D-10.2, Pitfall 3): NEVER `string.StartsWith` (sibling-prefix
    /// bypass — `"/shared-evil"` string-prefix-matches `"/shared"`) and
    /// NEVER `Contains("..")` (substring matching — the exact FreeRDP
    /// off-by-one bug class, CVE-2025-48817).
    ///
    /// Algorithm: normalize separators unconditionally (see
    /// <see cref="NormalizeSeparators"/>), reject outright if the candidate
    /// itself looks rooted (see <see cref="LooksRooted"/> — every legitimate
    /// `RemotePath` is expected to be relative to <see cref="ShareRoot"/>,
    /// never an absolute real-machine path; this design intentionally walls
    /// the sensor's remote-side I/O off from the rest of the remote disk,
    /// symmetric with the Rust side's own D-10.1 share-root confinement),
    /// then `Path.GetFullPath(candidate, root)` (the 2-arg overload — a
    /// relative candidate resolves under root, an absolute one is returned
    /// as-is, which the following `GetRelativePath` check catches) and
    /// `Path.GetRelativePath(root, full)`: REJECT if the result starts with
    /// `".."` or is itself rooted (an absolute/unrelated-root result means
    /// `GetRelativePath` could not express it as a descendant, e.g. no
    /// common ancestor at all).
    internal static bool ValidateRemotePath(string root, string candidate, out string? resolvedPath)
    {
        string normalizedRoot = NormalizeSeparators(root);
        string normalizedCandidate = NormalizeSeparators(candidate);

        if (LooksRooted(normalizedCandidate))
        {
            resolvedPath = null;
            return false;
        }

        string fullRoot = Path.GetFullPath(normalizedRoot);
        string fullCandidate = Path.GetFullPath(normalizedCandidate, fullRoot);
        string relative = Path.GetRelativePath(fullRoot, fullCandidate);

        if (relative.StartsWith("..", StringComparison.Ordinal) || Path.IsPathRooted(relative))
        {
            resolvedPath = null;
            return false;
        }

        resolvedPath = fullCandidate;
        return true;
    }

    /// Copies bytes between the validated real remote path and the
    /// RDPDR-redirected `\\tsclient\RDPILOT\<share_name>` UNC path in a
    /// single `FileStream` pass, computing SHA-256 inline via
    /// `IncrementalHash` (D-10.5 — never `File.Copy`-then-rehash, which
    /// would double the I/O for zero benefit).
    ///
    /// Direction (10-RESEARCH Item 3): for `Upload`, the source is the UNC
    /// path (bytes already staged there by the Rust SDK's own
    /// `RdpilotDriveBackend` write path, Plan 10-02) and the destination is
    /// the validated real remote path — driving Read IRPs into the
    /// already-proven `handle_read`. For `Download`, the source is the
    /// validated real remote path and the destination is the UNC path —
    /// driving the NEW Write/SetInformation IRPs Plan 10-02 implements.
    ///
    /// Two-validator reason-mapping (BLOCKER fix, D-10.4, second half): a
    /// `ValidateRemotePath` rejection here is the AUTHORITATIVE caller-facing
    /// `error_kind = "path_traversal"` producer — this is the C# sensor's
    /// own remote-destination validator rejecting untrusted input BEFORE any
    /// `System.IO` call touches it. This is structurally distinct from the
    /// Rust-side `RdpilotDriveBackend::resolve_under_root` rejection
    /// (Plan 10-01), which protects the OTHER trust boundary (the local
    /// share root) and already returns a structurally-distinct
    /// `NtStatus::NO_SUCH_FILE` at the IRP level — that rejection manifests
    /// HERE (if it manifests at all) only as an ordinary `FileStream` open
    /// failure against `\\tsclient\RDPILOT\<name>`, which this method
    /// reports as `error_kind = "io"` (NOT a false `path_traversal`, so the
    /// two independent validators' rejections are never conflated). Any
    /// other exception (missing file, access denied, disk full, etc.) also
    /// degrades to `error_kind = "io"` — never throws out of this method
    /// (API-01, D-6.4).
    internal static FileTransferResponse Transfer(FileTransferRequest request)
    {
        try
        {
            EnsureShareRootExists();
        }
        catch (Exception ex)
        {
            return new FileTransferResponse
            {
                Success = false,
                Data = null,
                Error = $"failed to create sensor transfer root: {ex.Message}",
                ErrorKind = "io",
            };
        }

        if (!ValidateRemotePath(ShareRoot, request.RemotePath, out string? realPath) || realPath is null)
        {
            return new FileTransferResponse
            {
                Success = false,
                Data = null,
                Error = "remote_path failed ancestry validation under the sensor transfer root",
                ErrorKind = "path_traversal",
            };
        }

        string uncPath = Path.Combine(UncShareBase, request.ShareName);
        string sourcePath = request.Op == FileTransferOp.Upload ? uncPath : realPath;
        string destPath = request.Op == FileTransferOp.Upload ? realPath : uncPath;

        try
        {
            using FileStream source = new(sourcePath, FileMode.Open, FileAccess.Read);
            using FileStream dest = new(destPath, FileMode.Create, FileAccess.Write);
            using IncrementalHash hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);

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

            return new FileTransferResponse
            {
                Success = true,
                Data = new FileTransferData
                {
                    BytesTransferred = total,
                    Sha256 = Convert.ToHexString(digest).ToLowerInvariant(),
                },
                Error = null,
                ErrorKind = null,
            };
        }
        catch (Exception ex)
        {
            return new FileTransferResponse
            {
                Success = false,
                Data = null,
                Error = ex.Message,
                ErrorKind = "io",
            };
        }
    }
}
