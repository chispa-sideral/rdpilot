// rdpilot-sensor — the SERVER side of the RDPILOT_SENSOR DVC envelope
// protocol (Version/Ping/Pong only, D-5.4, NO COM). Re-derives
// tests/fixtures/sensor-responder.ps1's proven WTS open/read/write/close
// shape idiomatically in C# (D-5.7) — NOT a transliteration:
//
//   - [LibraryImport], never the older attribute-based P/Invoke marshalling,
//     for AOT-safe marshalling of the string-parameter WTSVirtualChannelOpenEx
//     call (RESEARCH Pitfall 1, T-05-02).
//   - Bounded retry-poll around the channel open, tolerating the empirically
//     proven ERROR_GEN_FAILURE / 0x31 timing race (D-5.7 constraint #1,
//     04-03-SUMMARY.md).
//   - Must run inside the interactive RDP session (Session > 0) — the launch
//     mechanism (Plans 03/04) guarantees this; on total open failure this
//     process names that requirement in its diagnostic (D-5.7 constraint #2).
//   - Every read scans forward for the first '{' byte and discards any
//     leading binary DVC framing prefix before parsing JSON — the
//     empirically observed 6-byte prefix from 04-03-SUMMARY.md (D-5.7
//     constraint #3).
//   - Malformed/truncated JSON is dropped (never crashes Main) — mirrors the
//     Rust processor's drop-never-panic discipline (T-05-01, ASVS V5).

using System.Runtime.InteropServices;
using System.Text.Json;

namespace RdpilotSensor;

internal static class Program
{
    /// Must match crates/rdpilot/src/connect.rs's RDPILOT_SENSOR const
    /// byte-for-byte (D-5.7's key_links contract).
    private const string ChannelName = "RDPILOT_SENSOR";

    private const int OpenRetries = 20;
    private const int OpenRetryDelayMs = 500;
    private const uint ReadTimeoutMs = 5000;
    private const int ReadBufferSize = 8192;

    /// Argument that selects the AOT source-gen risk-gate smoke test
    /// (RESEARCH Pitfall 1 / Pattern 6, 06-03-PLAN Task 1) instead of the
    /// normal WTS channel loop — see <see cref="RunAotSmokeTest"/>.
    private const string SmokeTestArg = "--smoke-test";

    private static int Main(string[] args)
    {
        if (args.Length > 0 && args[0] == SmokeTestArg)
        {
            return RunAotSmokeTest();
        }

        nint handle = OpenChannelWithRetry();
        if (handle == 0)
        {
            // Diagnostic already printed by OpenChannelWithRetry.
            return 1;
        }

        try
        {
            RunHandshakeAndPingPongLoop(handle);
        }
        finally
        {
            Wts.WTSVirtualChannelClose(handle);
        }

        return 0;
    }

    /// The single highest-risk unknown of Phase 6 (RESEARCH A1 / Pitfall 1 /
    /// Pattern 6, 06-03-PLAN Task 1), proven BEFORE any Win32 window-
    /// enumeration code is written: a real typed DTO (<see cref="WindowListResponse"/>)
    /// round-trips through the retyped `Envelope.Payload` (now
    /// `JsonElement?`, was `object?`) with zero reflection fallback, under
    /// an ACTUAL `dotnet publish -p:PublishAot=true` binary — not just a
    /// `dotnet build`. Every (de)serialize call below goes through the
    /// source-generated `EnvelopeJsonContext.Default.*` overloads only,
    /// matching the same discipline `ReadEnvelope`/`WriteEnvelope` use, so a
    /// broken source-gen registration surfaces as an assertion failure here
    /// (or a `JsonException`/`NotSupportedException` under
    /// `JsonSerializerIsReflectionEnabledByDefault=false`) instead of
    /// silently later inside the real WindowList handler (Task 2).
    ///
    /// Exit 0 + "PASS" on success, exit 1 + "FAIL: ..." on any mismatch or
    /// exception — never throws out of Main (mirrors the T-05-01 drop-
    /// never-crash discipline used elsewhere in this file).
    private static int RunAotSmokeTest()
    {
        try
        {
            WindowListResponse original = new()
            {
                Success = true,
                Data =
                [
                    new WindowRecord
                    {
                        Hwnd = 0x1234_5678_9ABC,
                        Title = "Notepad",
                        Rect = new WindowRect { X = 10, Y = 20, W = 800, H = 600 },
                        ZOrder = 0,
                        State = "normal",
                        ClassName = "Notepad",
                        Pid = 4242,
                    },
                ],
                Error = null,
            };

            // serialize DTO -> JsonElement -> attach to Envelope.Payload
            JsonElement payload = JsonSerializer.SerializeToElement(original, EnvelopeJsonContext.Default.WindowListResponse);
            Envelope envelope = new()
            {
                Version = ProtocolVersion.Value,
                ReqId = 1,
                Type = MsgType.WindowList,
                Payload = payload,
            };

            // Envelope -> wire bytes -> Envelope (the exact ReadEnvelope/WriteEnvelope path)
            byte[] wireBytes = JsonSerializer.SerializeToUtf8Bytes(envelope, EnvelopeJsonContext.Default.Envelope);
            Envelope? roundTripped = JsonSerializer.Deserialize(wireBytes, EnvelopeJsonContext.Default.Envelope);

            if (roundTripped is not { Payload: { } roundTrippedPayload })
            {
                Console.Error.WriteLine("[smoke-test] FAIL: round-tripped envelope has a null payload");
                return 1;
            }

            // JsonElement -> DTO (the exact shape a future request-payload read would use)
            WindowRecord[]? decodedData = roundTrippedPayload.Deserialize(EnvelopeJsonContext.Default.WindowListResponse)?.Data;

            bool matches = roundTripped.Type == MsgType.WindowList
                && roundTripped.ReqId == envelope.ReqId
                && decodedData is [{ } record]
                && record.Hwnd == original.Data![0].Hwnd
                && record.Title == original.Data[0].Title
                && record.Rect.W == original.Data[0].Rect.W
                && record.Rect.H == original.Data[0].Rect.H
                && record.State == original.Data[0].State
                && record.ClassName == original.Data[0].ClassName
                && record.Pid == original.Data[0].Pid;

            if (!matches)
            {
                Console.Error.WriteLine("[smoke-test] FAIL: round-tripped payload does not match the original DTO");
                return 1;
            }

            Console.WriteLine(
                "[smoke-test] PASS: WindowListResponse round-tripped through Envelope.Payload " +
                "(JsonElement?) via EnvelopeJsonContext with no reflection fallback");
            return 0;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[smoke-test] FAIL: unexpected exception: {ex}");
            return 1;
        }
    }

    /// Open the RDPILOT_SENSOR channel, retrying up to <see cref="OpenRetries"/>
    /// times (D-5.7 constraint #1: survives the ERROR_GEN_FAILURE / 0x31
    /// timing race while the client-side DVC listener is still negotiating).
    private static nint OpenChannelWithRetry()
    {
        nint handle = 0;
        for (int attempt = 0; attempt < OpenRetries && handle == 0; attempt++)
        {
            handle = Wts.WTSVirtualChannelOpenEx(Wts.WTS_CURRENT_SESSION, ChannelName, Wts.WTS_CHANNEL_OPTION_DYNAMIC);
            if (handle == 0)
            {
                Thread.Sleep(OpenRetryDelayMs);
            }
        }

        if (handle == 0)
        {
            int lastError = Marshal.GetLastPInvokeError();
            Console.Error.WriteLine(
                $"[rdpilot-sensor] WTSVirtualChannelOpenEx failed after {OpenRetries} retries " +
                $"(LastError={lastError}). This process MUST run inside the interactive RDP " +
                "session (Session > 0) — a WinRM-launched process (Session 0) will never see " +
                "the channel. Confirm the launch mechanism targets the interactive session " +
                "(D-5.7 constraint #2).");
        }

        return handle;
    }

    /// Read the SDK's Version handshake (always the FIRST message on the
    /// wire — RdpilotSensorProcessor::start() fires unconditionally the
    /// instant the channel is created), echo this sensor's own version back
    /// in the identical envelope shape, then loop answering every Ping with
    /// a same-req_id Pong until the channel closes.
    private static void RunHandshakeAndPingPongLoop(nint handle)
    {
        Envelope? version = null;
        while (version is null)
        {
            Envelope? candidate = ReadEnvelope(handle);
            if (candidate is { Type: MsgType.Version })
            {
                version = candidate;
            }
            // Anything read before the handshake (malformed bytes, or a
            // non-Version message) is dropped and we keep waiting — the
            // protocol guarantees Version is first, but never trust the
            // wire over that guarantee (T-05-01).
        }

        WriteEnvelope(handle, new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = 0,
            Type = MsgType.Version,
            Payload = null,
        });

        while (true)
        {
            Envelope? msg = ReadEnvelope(handle);
            if (msg is null)
            {
                continue;
            }

            if (msg.Type == MsgType.Ping)
            {
                WriteEnvelope(handle, new Envelope
                {
                    Version = ProtocolVersion.Value,
                    ReqId = msg.ReqId,
                    Type = MsgType.Pong,
                    Payload = null,
                });
            }
            else if (msg.Type == MsgType.WindowList)
            {
                WriteEnvelope(handle, BuildWindowListReplyEnvelope(msg.ReqId));
            }
            // Any other message type this v1 protocol doesn't define is
            // silently ignored — never crashes the loop (T-05-01, mirrors
            // the Rust processor's own unknown-type handling).
        }
    }

    /// Build the WindowList reply envelope (T-06-01 / D-6.4): enumerates
    /// top-level windows via <see cref="WindowEnumeration.BuildWindowListResponse"/>
    /// and wraps it `success:true`; on ANY exception from the handler,
    /// degrades to a `success:false` reply carrying the exception message
    /// instead of throwing out of the dispatch loop (mirrors the
    /// ReadEnvelope/T-05-01 drop-never-crash discipline for the response
    /// side too).
    private static Envelope BuildWindowListReplyEnvelope(ulong reqId)
    {
        WindowListResponse response;
        try
        {
            response = WindowEnumeration.BuildWindowListResponse();
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] WindowList handler failed: {ex}");
            response = new WindowListResponse { Success = false, Data = null, Error = ex.Message };
        }

        JsonElement payload = JsonSerializer.SerializeToElement(response, EnvelopeJsonContext.Default.WindowListResponse);
        return new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.WindowList,
            Payload = payload,
        };
    }

    /// Read one WTS message, strip the leading binary DVC framing prefix by
    /// scanning forward for the first '{' byte (D-5.7 constraint #3 — do NOT
    /// assume the JSON body starts at offset 0), then parse it via the
    /// AOT-safe source-generated context. Malformed/truncated JSON, or a
    /// read with no '{' at all, is dropped (returns null) — never throws out
    /// of this method (T-05-01, ASVS V5).
    private static Envelope? ReadEnvelope(nint handle)
    {
        byte[] buf = new byte[ReadBufferSize];
        bool ok = Wts.WTSVirtualChannelRead(handle, ReadTimeoutMs, buf, (uint)buf.Length, out uint bytesRead);
        if (!ok || bytesRead == 0)
        {
            return null;
        }

        int jsonStart = Array.IndexOf(buf, (byte)'{', 0, (int)bytesRead);
        if (jsonStart < 0)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] dropped malformed envelope: no '{{' found in {bytesRead} byte(s)");
            return null;
        }

        ReadOnlySpan<byte> jsonSlice = buf.AsSpan(jsonStart, (int)bytesRead - jsonStart);
        try
        {
            return JsonSerializer.Deserialize(jsonSlice, EnvelopeJsonContext.Default.Envelope);
        }
        catch (JsonException ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] dropped malformed envelope: {ex.Message}");
            return null;
        }
    }

    private static void WriteEnvelope(nint handle, Envelope envelope)
    {
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(envelope, EnvelopeJsonContext.Default.Envelope);
        Wts.WTSVirtualChannelWrite(handle, bytes, (uint)bytes.Length, out _);
    }
}

/// The WTS virtual-channel P/Invoke surface, source-generated via
/// [LibraryImport] (NOT the older attribute-based P/Invoke marshalling —
/// that older mechanism is incompatible with Native AOT for the string-
/// marshalled open call, RESEARCH Pitfall 1 / T-05-02).
///
/// Live-tuned (Plan 04 live gate, RESEARCH Open Question #3 resolved):
/// StringMarshalling.Utf16 was tried first per RESEARCH Assumption A3, but
/// the live gate showed the sensor process starting, retrying its bounded
/// open loop for its full ~10s budget, and exiting -- WTSVirtualChannelOpenEx
/// never succeeded and the RDPILOT_SENSOR DVC never even appeared on the
/// client-side wire trace (confirmed via tracing on the Rust side: zero DVC
/// Create Request for that name ever arrived). WTSVirtualChannelOpenEx is
/// documented as an ANSI-only Win32 API (no "W" wide-string export exists in
/// wtsapi32.dll) -- StringMarshalling.Utf16 caused the source generator to
/// target a "WTSVirtualChannelOpenExW" entry point that does not exist,
/// silently failing every call. Flipped to StringMarshalling.Utf8 (the ANSI
/// 'A' entry point), matching the Phase-4 PowerShell fixture's proven-live
/// CharSet.Ansi convention -- this was the exact fallback this doc comment
/// already anticipated (RESEARCH Open Question #3).
internal static partial class Wts
{
    internal const uint WTS_CURRENT_SESSION = 0xFFFFFFFF;
    internal const uint WTS_CHANNEL_OPTION_DYNAMIC = 0x1;

    [LibraryImport("wtsapi32.dll", StringMarshalling = StringMarshalling.Utf8, SetLastError = true)]
    internal static partial nint WTSVirtualChannelOpenEx(uint sessionId, string virtualName, uint flags);

    [LibraryImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool WTSVirtualChannelRead(nint channelHandle, uint timeout, byte[] buffer, uint bufferSize, out uint bytesRead);

    [LibraryImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool WTSVirtualChannelWrite(nint channelHandle, byte[] buffer, uint length, out uint bytesWritten);

    [LibraryImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool WTSVirtualChannelClose(nint channelHandle);
}
