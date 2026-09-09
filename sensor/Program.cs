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
using System.Diagnostics;
using System.Text.Json;
using System.Collections.Concurrent;

namespace RdpilotSensor;

internal static class Program
{
    /// Must match crates/rdpilot/src/connect.rs's RDPILOT_SENSOR const
    /// byte-for-byte (D-5.7's key_links contract).
    private const string ChannelName = "RDPILOT_SENSOR";

    private const int OpenRetries = 20;
    private const int OpenRetryDelayMs = 500;
    private const uint ReadTimeoutMs = 50;
    private const int ReadBufferSize = 8192;

    /// Argument that selects the AOT source-gen risk-gate smoke test
    /// (RESEARCH Pitfall 1 / Pattern 6, 06-03-PLAN Task 1) instead of the
    /// normal WTS channel loop — see <see cref="RunAotSmokeTest"/>.
    private const string SmokeTestArg = "--smoke-test";

    /// Argument that selects the UIA COM-interop AOT risk-gate smoke test
    /// (D-7.5, 07-02-PLAN.md Task 2) — sibling to <see cref="SmokeTestArg"/>,
    /// exercises every risky UiaInterop.cs marshalling path (CoCreateInstance,
    /// SAFEARRAY, BSTR, BOOL, RECT, FindAll) on the desktop root element
    /// before any real UiaTree handler (07-04) depends on any of it — see
    /// <see cref="RunUiaAotSmokeTest"/>.
    private const string UiaSmokeTestArg = "--smoke-test-uia";

    /// Argument that selects the FILE-03 BLOCKING adversarial
    /// path-traversal selftest (10-03-PLAN.md Task 3) — sibling to
    /// <see cref="SmokeTestArg"/>/<see cref="UiaSmokeTestArg"/>. Drives
    /// <see cref="FileTransfer.ValidateRemotePath"/> against the FILE-03
    /// matrix offline via `dotnet run` (linux-x64 JIT — needs no Windows and
    /// no NativeAOT publish), see <see cref="RunFileTraversalSelfTest"/>.
    private const string FileTraversalSelfTestArg = "--file-traversal-selftest";

    private const string DispatchSelfTestArg = "--dispatch-selftest";
    private const string DispatchSelfTestUiaArg = "--dispatch-selftest-uia";
    /// Default-off, fixed-root diagnostic used only by the ignored real-DVC
    /// adversarial proof. It accepts a capability token, never a path.
    private const string LiveDvcBlockedHandlerArg = "--live-dvc-blocked-handler";

    private static int Main(string[] args)
    {
        LiveDvcBlockedHandler? liveBlockedHandler = null;
        if (args.Length > 0 && args[0] == LiveDvcBlockedHandlerArg)
        {
            if (args.Length != 2 || !LiveDvcBlockedHandler.TryCreate(args[1], out liveBlockedHandler))
            {
                Console.Error.WriteLine("[live-dvc-blocked-handler] FAIL: expected one 32-character lowercase hexadecimal token");
                return 2;
            }
        }

        if (args.Length > 0 && args[0] == SmokeTestArg)
        {
            return RunAotSmokeTest();
        }

        if (args.Length > 0 && args[0] == UiaSmokeTestArg)
        {
            return RunUiaAotSmokeTest();
        }

        if (args.Length > 0 && args[0] == FileTraversalSelfTestArg)
        {
            return RunFileTraversalSelfTest();
        }

        if (args.Length > 0 && args[0] == DispatchSelfTestArg)
        {
            return RunDispatchSelfTest();
        }

        if (args.Length > 0 && args[0] == DispatchSelfTestUiaArg)
        {
            return RunDispatchSelfTestUia();
        }

        nint handle = OpenChannelWithRetry();
        if (handle == 0)
        {
            // Diagnostic already printed by OpenChannelWithRetry.
            return 1;
        }

        int exitCode = 1;
        try
        {
            exitCode = RunSensorLoop(new WtsEnvelopeReader(handle), new WtsEnvelopeWriter(handle), liveBlockedHandler) ? 0 : 1;
        }
        finally
        {
            Wts.WTSVirtualChannelClose(handle);
        }

        return exitCode;
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

    /// The UIA COM-interop AOT risk-gate smoke test (D-7.5, 07-02-PLAN.md
    /// Task 2), sibling to <see cref="RunAotSmokeTest"/>. Exercises EVERY
    /// risky UiaInterop.cs marshalling path in isolation, on the desktop
    /// root element (always available, no target-app dependency), in the
    /// order RESEARCH recommends so a failure pinpoints the culprit:
    /// `ReadRuntimeId` FIRST (SAFEARRAY, the single riskiest line in the
    /// phase — Pitfall 2/Assumption A1), then `GetCurrentName` (BSTR,
    /// A2), `GetCurrentIsEnabled` (BOOL, A3), `GetCurrentBoundingRectangle`
    /// (RECT-by-out-pointer, Open Question #2), `CreateTrueCondition`,
    /// `FindAll`, `GetLength`.
    ///
    /// This is a throwaway diagnostic — it does NOT go through the
    /// Envelope/DVC dispatch path and defines no `MsgType.Uia` (that
    /// arrives in 07-04). This plan (07-02) only proves the offline
    /// linux-x64 AOT-trim/source-gen surrogate publish is clean; the real
    /// behavioral RUN of this smoke test happens on a genuine Windows host
    /// at the 07-03 risk gate (RESEARCH Pitfall 7 — `CoCreateInstance` and
    /// `ole32.dll`/UIA do not exist on Linux, so running this binary here
    /// would be meaningless).
    ///
    /// Exit 0 + "PASS" (with the exercised values) on success, exit 1 +
    /// "FAIL: ..." to stderr on any exception — never throws out of Main
    /// (mirrors the T-05-01 drop-never-crash discipline used elsewhere in
    /// this file).
    private static int RunUiaAotSmokeTest()
    {
        try
        {
            Step("about to GetRootAutomation()");
            IUIAutomation automation = UiaInterop.GetRootAutomation();
            Step("GetRootAutomation() OK");

            nint desktop = User32Interop.GetDesktopWindow();
            Step($"GetDesktopWindow() OK hwnd=0x{desktop:X}");

            IUIAutomationElement root = automation.ElementFromHandle(desktop);
            Step("ElementFromHandle() OK");

            // The single riskiest call in the whole phase — isolate it
            // first (Pitfall 2 / Assumption A1).
            Step("about to ReadRuntimeId()");
            int[] runtimeId = UiaInterop.ReadRuntimeId(root);
            Step($"ReadRuntimeId() OK len={runtimeId.Length}");

            Step("about to ReadName()");
            string name = UiaInterop.ReadName(root);                     // BSTR marshalling proof (A2) -- live-diagnosed fix, see UiaInterop.cs
            Step($"ReadName() OK name='{name}'");

            Step("about to GetCurrentIsEnabled()");
            bool enabled = root.GetCurrentIsEnabled();                    // BOOL marshalling proof (A3)
            Step($"GetCurrentIsEnabled() OK enabled={enabled}");

            Step("about to GetCurrentBoundingRectangle()");
            Rect32 rect = root.GetCurrentBoundingRectangle();             // struct-by-out-pointer proof
            Step($"GetCurrentBoundingRectangle() OK rect={rect.Left},{rect.Top},{rect.Right},{rect.Bottom}");

            Step("about to CreateTrueCondition()");
            IUIAutomationCondition trueCondition = automation.CreateTrueCondition();
            Step("CreateTrueCondition() OK");

            Step("about to FindAll()");
            IUIAutomationElementArray children = root.FindAll(TreeScope.Children, trueCondition);
            Step("FindAll() OK");

            Step("about to GetLength()");
            int childCount = children.GetLength();
            Step($"GetLength() OK count={childCount}");

            Console.WriteLine(
                $"[smoke-test-uia] PASS: root name='{name}' enabled={enabled} " +
                $"rect={rect.Left},{rect.Top},{rect.Right},{rect.Bottom} " +
                $"runtimeId.len={runtimeId.Length} children={childCount}");
            Console.Out.Flush();
            return 0;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[smoke-test-uia] FAIL: {ex}");
            Console.Error.Flush();
            return 1;
        }
    }

    /// Diagnostic step tracer for <see cref="RunUiaAotSmokeTest"/> (D-7.5
    /// live-gate diagnosis): writes AND immediately flushes, so that if a
    /// later call crashes the process outright (e.g. a native access
    /// violation/heap corruption that bypasses the managed try/catch
    /// entirely), every step completed so far is still visible in the
    /// redirected output file instead of being lost in an unflushed
    /// buffered StreamWriter (.NET switches Console.Out to a buffered
    /// StreamWriter, AutoFlush=false, whenever stdout is redirected to a
    /// file rather than a real console).
    private static void Step(string message)
    {
        Console.WriteLine($"[smoke-test-uia] step: {message}");
        Console.Out.Flush();
    }

    /// The FILE-03 BLOCKING C#-side adversarial path-traversal selftest
    /// (10-03-PLAN.md Task 3, D-10.2's C# validator half). Drives
    /// <see cref="FileTransfer.ValidateRemotePath"/> against a fresh
    /// temp-directory test root (never the real <see cref="FileTransfer.ShareRoot"/>)
    /// over the FILE-03 required matrix: trailing `..` with no separator,
    /// mixed `/`+`\` separators, sibling-directory prefix (a directory whose
    /// name is a strict superstring of the root's own name — proves
    /// `ValidateRemotePath` is NOT doing a naive `string.StartsWith`,
    /// Pitfall 3), POSIX-absolute-as-relative, and a Windows-drive-absolute
    /// case. The Windows-drive-absolute case is annotated below: this
    /// validator's explicit <c>LooksRooted</c> pre-check (mirroring the
    /// Rust-side `looks_rooted()` fix, 10-01-SUMMARY.md Decision #1) makes
    /// it deterministic on both this offline linux-x64 host and the real
    /// win-x64 target, but per the plan's own discipline it is still
    /// RE-CONFIRMED at the 10-05 live gate rather than silently trusted
    /// offline (mirrors the FILE-04 chunk-size "confirm live" precedent). A
    /// final positive-control case (a legitimate nested path) guards against
    /// over-rejection.
    ///
    /// Exit 0 + "PASS" per case on success, exit 1 if ANY case's actual
    /// accept/reject outcome does not match its expectation — never throws
    /// out of Main (mirrors the RunAotSmokeTest/RunUiaAotSmokeTest
    /// never-crash discipline).
    private static int RunFileTraversalSelfTest()
    {
        try
        {
            string testRootParent = Path.Combine(Path.GetTempPath(), "rdpilot-selftest-" + Guid.NewGuid().ToString("N"));
            string testRoot = Path.Combine(testRootParent, "share");
            Directory.CreateDirectory(testRoot);

            // A sibling directory whose name is a strict superstring of the
            // root's own directory name ("share-evil" vs "share") — the
            // classic .NET StartsWith prefix-bypass footgun (Pitfall 3):
            // "/tmp/.../share-evil".StartsWith("/tmp/.../share") is `true`
            // even though it is a SIBLING, not a descendant.
            string siblingDir = Path.Combine(testRootParent, "share-evil");
            Directory.CreateDirectory(siblingDir);

            (string Name, string Candidate, bool ExpectAccept, string? LiveReconfirmNote)[] cases =
            [
                ("trailing-dotdot-no-separator", "..", false, null),
                ("mixed-separator-escape", "subdir\\../../evil.txt", false, null),
                ("sibling-directory-prefix", "../share-evil/secret.txt", false, null),
                ("posix-absolute-as-relative", "/etc/passwd", false, null),
                ("windows-drive-absolute", "C:\\Windows\\System32",
                    false, "faithfulness re-confirmed at the 10-05 live gate — Path.IsPathRooted alone is platform-dependent for drive letters; this offline PASS is backed by the explicit host-independent LooksRooted() check, not by Path.IsPathRooted"),
                ("legitimate-nested-path (positive control)", "nested/legit.txt", true, null),
            ];

            bool allPassed = true;
            foreach ((string name, string candidate, bool expectAccept, string? liveReconfirmNote) in cases)
            {
                bool accepted = FileTransfer.ValidateRemotePath(testRoot, candidate, out _);
                bool casePassed = accepted == expectAccept;
                allPassed &= casePassed;

                string status = casePassed ? "PASS" : "FAIL";
                string noteSuffix = liveReconfirmNote is null ? string.Empty : $" [LIVE-RECONFIRM: {liveReconfirmNote}]";
                Console.WriteLine(
                    $"[file-traversal-selftest] {status}: {name} candidate='{candidate}' " +
                    $"expectAccept={expectAccept} actualAccept={accepted}{noteSuffix}");
            }

            if (allPassed)
            {
                Console.WriteLine("[file-traversal-selftest] PASS: all FILE-03 adversarial cases matched expectations");
                return 0;
            }

            Console.Error.WriteLine("[file-traversal-selftest] FAIL: one or more cases did not match expectations");
            return 1;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[file-traversal-selftest] FAIL: unexpected exception: {ex}");
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

    private static bool RunSensorLoop(IEnvelopeReader reader, IEnvelopeWriter writer, LiveDvcBlockedHandler? liveBlockedHandler = null)
    {
        RequestDispatcher dispatcher = new(message => BuildReplyEnvelope(message, liveBlockedHandler), BuildFailureReplyEnvelope);
        return new SensorOwnerLoop(reader, writer, dispatcher, IsSupportedRequest).Run();
    }

    private static bool IsSupportedRequest(Envelope message) => message.Type is
        MsgType.WindowList or MsgType.ProcessTree or MsgType.SetForegroundWindow or
        MsgType.LaunchProcess or MsgType.Uia or MsgType.FileTransfer;

    private static Envelope BuildReplyEnvelope(Envelope message, LiveDvcBlockedHandler? liveBlockedHandler = null) => message.Type switch
    {
        MsgType.WindowList => BuildWindowListReplyEnvelope(message.ReqId, liveBlockedHandler),
        MsgType.ProcessTree => BuildProcessTreeReplyEnvelope(message.ReqId),
        MsgType.SetForegroundWindow => BuildSetForegroundWindowReplyEnvelope(message.ReqId, message.Payload),
        MsgType.LaunchProcess => BuildLaunchProcessReplyEnvelope(message.ReqId, message.Payload),
        MsgType.Uia => BuildUiaTreeReplyEnvelope(message.ReqId, message.Payload),
        MsgType.FileTransfer => BuildFileTransferReplyEnvelope(message.ReqId, message.Payload),
        _ => throw new InvalidOperationException($"unsupported request type {message.Type}"),
    };

    /// Builds the normal, already source-generated response shape for every
    /// request type. This lets timeout/overload retain wire compatibility.
    private static Envelope BuildFailureReplyEnvelope(Envelope message, string error) => message.Type switch
    {
        MsgType.WindowList => EnvelopeWith(message, new WindowListResponse { Success = false, Data = null, Error = error }, EnvelopeJsonContext.Default.WindowListResponse),
        MsgType.ProcessTree => EnvelopeWith(message, new ProcessTreeResponse { Success = false, Data = null, Error = error }, EnvelopeJsonContext.Default.ProcessTreeResponse),
        MsgType.SetForegroundWindow => EnvelopeWith(message, new SetForegroundWindowResponse { Success = false, Data = null, Error = error }, EnvelopeJsonContext.Default.SetForegroundWindowResponse),
        MsgType.LaunchProcess => EnvelopeWith(message, new LaunchProcessResponse { Success = false, Data = null, Error = error }, EnvelopeJsonContext.Default.LaunchProcessResponse),
        MsgType.Uia => EnvelopeWith(message, new UiaTreeResponse { Success = false, Data = null, Error = error }, EnvelopeJsonContext.Default.UiaTreeResponse),
        MsgType.FileTransfer => EnvelopeWith(message, new FileTransferResponse { Success = false, Data = null, Error = error, ErrorKind = "io" }, EnvelopeJsonContext.Default.FileTransferResponse),
        _ => throw new InvalidOperationException($"unsupported request type {message.Type}"),
    };

    private static Envelope EnvelopeWith<T>(Envelope request, T response, System.Text.Json.Serialization.Metadata.JsonTypeInfo<T> typeInfo)
        => new() { Version = ProtocolVersion.Value, ReqId = request.ReqId, Type = request.Type, Payload = JsonSerializer.SerializeToElement(response, typeInfo) };

    /// Build the WindowList reply envelope (T-06-01 / D-6.4): enumerates
    /// top-level windows via <see cref="WindowEnumeration.BuildWindowListResponse"/>
    /// and wraps it `success:true`; on ANY exception from the handler,
    /// degrades to a `success:false` reply carrying the exception message
    /// instead of throwing out of the dispatch loop (mirrors the
    /// ReadEnvelope/T-05-01 drop-never-crash discipline for the response
    /// side too).
    private static Envelope BuildWindowListReplyEnvelope(ulong reqId, LiveDvcBlockedHandler? liveBlockedHandler = null)
    {
        WindowListResponse response;
        try
        {
            liveBlockedHandler?.WaitForRelease();
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

    /// Build the ProcessTree reply envelope (T-06-01 / D-6.4): enumerates the
    /// whole-system process snapshot via
    /// <see cref="ProcessEnumeration.BuildProcessTreeResponse"/> (Toolhelp32,
    /// NEVER the managed WMI query API — Pattern 5/Pitfall 2) and wraps it
    /// `success:true`; on ANY exception from the handler, degrades to a
    /// `success:false` reply carrying the exception message instead of
    /// throwing out of the dispatch loop (mirrors
    /// BuildWindowListReplyEnvelope's discipline).
    private static Envelope BuildProcessTreeReplyEnvelope(ulong reqId)
    {
        ProcessTreeResponse response;
        try
        {
            response = ProcessEnumeration.BuildProcessTreeResponse();
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] ProcessTree handler failed: {ex}");
            response = new ProcessTreeResponse { Success = false, Data = null, Error = ex.Message };
        }

        JsonElement payload = JsonSerializer.SerializeToElement(response, EnvelopeJsonContext.Default.ProcessTreeResponse);
        return new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.ProcessTree,
            Payload = payload,
        };
    }

    /// Build the SetForegroundWindow reply envelope (T-06-01 / D-6.4):
    /// deserializes the request payload and calls
    /// <see cref="WindowControl.Focus"/>, which reports success only on the
    /// Win32 call's own nonzero return (Pitfall 5 — call-success, not
    /// visual outcome). A missing/malformed request payload, or any
    /// exception from the handler, degrades to a `success:false` reply
    /// instead of throwing out of the dispatch loop (D-6.4, T-06-01).
    private static Envelope BuildSetForegroundWindowReplyEnvelope(ulong reqId, JsonElement? payload)
    {
        SetForegroundWindowResponse response;
        try
        {
            SetForegroundWindowRequest? request = payload?.Deserialize(EnvelopeJsonContext.Default.SetForegroundWindowRequest);
            response = request is null
                ? new SetForegroundWindowResponse { Success = false, Data = null, Error = "missing/malformed SetForegroundWindow request payload" }
                : WindowControl.Focus(request);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] SetForegroundWindow handler failed: {ex}");
            response = new SetForegroundWindowResponse { Success = false, Data = null, Error = ex.Message };
        }

        JsonElement responsePayload = JsonSerializer.SerializeToElement(response, EnvelopeJsonContext.Default.SetForegroundWindowResponse);
        return new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.SetForegroundWindow,
            Payload = responsePayload,
        };
    }

    /// Build the LaunchProcess reply envelope (T-06-01 / D-6.4):
    /// deserializes the request payload and calls
    /// <see cref="ProcessLaunch.Launch"/>, which replies with the new PID
    /// the instant `CreateProcessW` returns (D-6.2 fire-and-forget,
    /// Pitfall 6). A missing/malformed request payload, or any exception
    /// from the handler, degrades to a `success:false` reply instead of
    /// throwing out of the dispatch loop (D-6.4, T-06-01).
    private static Envelope BuildLaunchProcessReplyEnvelope(ulong reqId, JsonElement? payload)
    {
        LaunchProcessResponse response;
        try
        {
            LaunchProcessRequest? request = payload?.Deserialize(EnvelopeJsonContext.Default.LaunchProcessRequest);
            response = request is null
                ? new LaunchProcessResponse { Success = false, Data = null, Error = "missing/malformed LaunchProcess request payload" }
                : ProcessLaunch.Launch(request);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] LaunchProcess handler failed: {ex}");
            response = new LaunchProcessResponse { Success = false, Data = null, Error = ex.Message };
        }

        JsonElement responsePayload = JsonSerializer.SerializeToElement(response, EnvelopeJsonContext.Default.LaunchProcessResponse);
        return new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.LaunchProcess,
            Payload = responsePayload,
        };
    }

    /// Build the Uia reply envelope (T-07-01 / D-6.4 / D-7.6): deserializes
    /// the request payload and calls
    /// <see cref="UiaTree.BuildUiaTreeResponse"/>, which itself degrades to
    /// `success:false` on total handler failure (D-7.6's per-element skip
    /// happens inside that call). A missing/malformed request payload, or
    /// any exception escaping the handler, degrades to a `success:false`
    /// reply instead of throwing out of the dispatch loop (D-6.4, T-07-01),
    /// mirroring BuildSetForegroundWindowReplyEnvelope/
    /// BuildLaunchProcessReplyEnvelope's discipline exactly.
    private static Envelope BuildUiaTreeReplyEnvelope(ulong reqId, JsonElement? payload)
    {
        UiaTreeResponse response;
        try
        {
            UiaTreeRequest? request = payload?.Deserialize(EnvelopeJsonContext.Default.UiaTreeRequest);
            response = request is null
                ? new UiaTreeResponse { Success = false, Data = null, Error = "missing/malformed Uia request payload" }
                : UiaTree.BuildUiaTreeResponse(request.Hwnd, request.MaxDepth);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] Uia handler failed: {ex}");
            response = new UiaTreeResponse { Success = false, Data = null, Error = ex.Message };
        }

        JsonElement responsePayload = JsonSerializer.SerializeToElement(response, EnvelopeJsonContext.Default.UiaTreeResponse);
        return new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.Uia,
            Payload = responsePayload,
        };
    }

    /// Build the FileTransfer reply envelope (T-10-01 / D-6.4 / D-10.4):
    /// deserializes the request payload and calls
    /// <see cref="FileTransfer.Transfer"/>, which validates the destination
    /// path (setting `error_kind = "path_traversal"` on rejection) and
    /// copies bytes with an inline SHA-256 pass. A missing/malformed request
    /// payload, or any exception escaping the handler, degrades to a
    /// `success:false` reply carrying `error_kind = "io"` instead of
    /// throwing out of the dispatch loop (D-6.4, T-10-01), mirroring
    /// BuildLaunchProcessReplyEnvelope/BuildUiaTreeReplyEnvelope's
    /// discipline exactly — this outer try/catch is defense-in-depth on top
    /// of `FileTransfer.Transfer`'s own internal try/catch (deserialization
    /// itself can throw before the handler is even reached).
    private static Envelope BuildFileTransferReplyEnvelope(ulong reqId, JsonElement? payload)
    {
        FileTransferResponse response;
        try
        {
            FileTransferRequest? request = payload?.Deserialize(EnvelopeJsonContext.Default.FileTransferRequest);
            response = request is null
                ? new FileTransferResponse { Success = false, Data = null, Error = "missing/malformed FileTransfer request payload", ErrorKind = null }
                : FileTransfer.Transfer(request);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[rdpilot-sensor] FileTransfer handler failed: {ex}");
            response = new FileTransferResponse { Success = false, Data = null, Error = ex.Message, ErrorKind = "io" };
        }

        JsonElement fileTransferResponsePayload = JsonSerializer.SerializeToElement(response, EnvelopeJsonContext.Default.FileTransferResponse);
        return new Envelope
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.FileTransfer,
            Payload = fileTransferResponsePayload,
        };
    }

    private sealed class WtsEnvelopeReader(nint handle) : IEnvelopeReader
    {
        public EnvelopeReadResult Read(uint timeoutMs)
        {
            byte[] buf = new byte[ReadBufferSize];
            if (!Wts.WTSVirtualChannelRead(handle, timeoutMs, buf, (uint)buf.Length, out uint bytesRead))
            {
                int error = Marshal.GetLastPInvokeError();
                return error == 0 || error == 1460
                    ? new(EnvelopeReadKind.Timeout)
                    : new(EnvelopeReadKind.Terminal, Error: error);
            }
            if (bytesRead == 0) return new(EnvelopeReadKind.Timeout);
            int jsonStart = Array.IndexOf(buf, (byte)'{', 0, (int)bytesRead);
            if (jsonStart < 0)
            {
                Console.Error.WriteLine($"[rdpilot-sensor] dropped malformed envelope: no '{{' found in {bytesRead} byte(s)");
                return new(EnvelopeReadKind.Drop);
            }
            try
            {
                Envelope? envelope = JsonSerializer.Deserialize(buf.AsSpan(jsonStart, (int)bytesRead - jsonStart), EnvelopeJsonContext.Default.Envelope);
                return envelope is null ? new(EnvelopeReadKind.Drop) : new(EnvelopeReadKind.Envelope, envelope);
            }
            catch (JsonException ex)
            {
                Console.Error.WriteLine($"[rdpilot-sensor] dropped malformed envelope: {ex.Message}");
                return new(EnvelopeReadKind.Drop);
            }
        }
    }

    private sealed class WtsEnvelopeWriter(nint handle) : IEnvelopeWriter
    {
        public bool Write(Envelope envelope, out int error)
        {
            byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(envelope, EnvelopeJsonContext.Default.Envelope);
            bool ok = Wts.WTSVirtualChannelWrite(handle, bytes, (uint)bytes.Length, out _);
            error = ok ? 0 : Marshal.GetLastPInvokeError();
            return ok;
        }
    }

    private static int RunDispatchSelfTest()
    {
        using ManualResetEventSlim releaseBlocked = new(false);
        List<Envelope> written = [];
        int ownerThread = 0;
        Queue<EnvelopeReadResult> input = new([
            new(EnvelopeReadKind.Envelope, new Envelope { Version = ProtocolVersion.Value, ReqId = 0, Type = MsgType.Version }),
            new(EnvelopeReadKind.Envelope, new Envelope { Version = ProtocolVersion.Value, ReqId = 41, Type = MsgType.WindowList }),
            new(EnvelopeReadKind.Envelope, new Envelope { Version = ProtocolVersion.Value, ReqId = 42, Type = MsgType.Ping }),
            new(EnvelopeReadKind.Envelope, new Envelope { Version = ProtocolVersion.Value, ReqId = 43, Type = MsgType.ProcessTree }),
        ]);
        int timeoutCount = 0;
        IEnvelopeReader reader = new SelfTestReader(input, () => ++timeoutCount > 180 ? new(EnvelopeReadKind.Terminal, Error: 995) : new(EnvelopeReadKind.Timeout));
        Stopwatch pingTimer = Stopwatch.StartNew();
        IEnvelopeWriter writer = new SelfTestWriter(written, () => ownerThread, pingTimer);
        RequestDispatcher dispatcher = new(message =>
        {
            if (message.ReqId == 41) releaseBlocked.Wait();
            return BuildFailureReplyEnvelope(message, "selftest handler completed");
        }, BuildFailureReplyEnvelope, workerCount: 2, queueCapacity: 2);
        SensorOwnerLoop loop = new(reader, writer, dispatcher, IsSupportedRequest);
        Thread owner = new(() => { ownerThread = Environment.CurrentManagedThreadId; loop.Run(); }) { IsBackground = true };
        owner.Start();
        owner.Join(3000);
        bool pingFast = ((SelfTestWriter)writer).PongLatencyMs is long latency && latency < 500;
        bool deadlineOnce = written.Count(e => e.ReqId == 41) == 1 && written.Any(e => e.ReqId == 41 && e.Type == MsgType.WindowList);
        bool unrelated = written.Count(e => e.ReqId == 43 && e.Type == MsgType.ProcessTree) == 1;
        releaseBlocked.Set();
        Thread.Sleep(50);
        bool noLateDuplicate = written.Count(e => e.ReqId == 41) == 1;
        bool ownerOnly = ((SelfTestWriter)writer).OwnerOnly;
        Queue<EnvelopeReadResult> terminalInput = new([new(EnvelopeReadKind.Envelope, new Envelope { Version = ProtocolVersion.Value, ReqId = 0, Type = MsgType.Version })]);
        RequestDispatcher terminalDispatcher = new(message => BuildReplyEnvelope(message), BuildFailureReplyEnvelope);
        bool terminalWriterStops = !new SensorOwnerLoop(
            new SelfTestReader(terminalInput, () => new(EnvelopeReadKind.Timeout)),
            new SelfTestWriter([], () => Environment.CurrentManagedThreadId, pingTimer, failWrites: true),
            terminalDispatcher, IsSupportedRequest).Run();
        bool passed = !owner.IsAlive && pingFast && deadlineOnce && unrelated && noLateDuplicate && ownerOnly && terminalWriterStops;
        Console.WriteLine($"[dispatch-selftest] {(passed ? "PASS" : "FAIL")}: deadlineOnce={deadlineOnce} pingFast={pingFast} unrelated={unrelated} noLateDuplicate={noLateDuplicate} ownerOnly={ownerOnly} terminalWriterStops={terminalWriterStops}");
        return passed ? 0 : 1;
    }

    /// Windows-only integration gate for the actual UIA request handler. It
    /// intentionally uses the normal dispatcher and owner-loop seams, rather
    /// than the direct UIA smoke-test path, so the captured reply proves the
    /// worker/owner handoff and correlation contract.
    private static int RunDispatchSelfTestUia()
    {
        if (!OperatingSystem.IsWindows())
        {
            Console.Error.WriteLine("[dispatch-selftest-uia] FAIL: Windows is required for dispatcher-worker UIA evidence");
            return 1;
        }

        ulong reqId = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
        JsonElement requestPayload = JsonSerializer.SerializeToElement(
            new UiaTreeRequest { Hwnd = (ulong)User32Interop.GetDesktopWindow(), MaxDepth = 0 },
            EnvelopeJsonContext.Default.UiaTreeRequest);
        Envelope request = new()
        {
            Version = ProtocolVersion.Value,
            ReqId = reqId,
            Type = MsgType.Uia,
            Payload = requestPayload,
        };

        ConcurrentQueue<WorkerStartupEvidence> workerStarts = new();
        UiaSelfTestTransport transport = new(request);
        RequestDispatcher dispatcher = new(
            message => BuildReplyEnvelope(message),
            BuildFailureReplyEnvelope,
            workerCount: 1,
            queueCapacity: 1,
            workerStartupObserver: workerStarts.Enqueue);
        SensorOwnerLoop loop = new(transport, transport, dispatcher, IsSupportedRequest);
        Thread owner = new(() => { loop.Run(); }) { IsBackground = true, Name = "rdpilot-sensor-uia-selftest-owner" };
        owner.Start();

        bool replyWritten = transport.WaitForUiaReply(TimeSpan.FromSeconds(5));
        owner.Join(1000);
        dispatcher.StopAccepting();

        WorkerStartupEvidence? startup = workerStarts.TryDequeue(out WorkerStartupEvidence? captured) ? captured : null;
        Envelope[] replies = transport.Written;
        Envelope[] uiaReplies = replies.Where(reply => reply.Type == MsgType.Uia).ToArray();
        UiaTreeResponse? response = uiaReplies.Length == 1
            ? uiaReplies[0].Payload?.Deserialize(EnvelopeJsonContext.Default.UiaTreeResponse)
            : null;
        bool correlated = uiaReplies.Length == 1 && uiaReplies[0].ReqId == reqId;
        bool validResponse = response is not null;
        bool workerMtaCom = startup is { ApartmentState: ApartmentState.MTA, ComInitializationSucceeded: true };
        bool passed = replyWritten && !owner.IsAlive && correlated && validResponse && workerMtaCom;

        string success = response is null ? "<malformed>" : response.Success.ToString().ToLowerInvariant();
        Console.WriteLine(
            $"[dispatch-selftest-uia] {(passed ? "PASS" : "FAIL")}: requestId={reqId} " +
            $"replyId={(uiaReplies.Length == 1 ? uiaReplies[0].ReqId : 0)} responseSuccess={success} " +
            $"workerName={startup?.ThreadName ?? "<missing>"} workerManagedId={startup?.ManagedThreadId} " +
            $"apartment={startup?.ApartmentState} coInitializeEx=0x{startup?.CoInitializeExHResult:X8} " +
            $"comInitialized={startup?.ComInitializationSucceeded} ownerWrites={replies.Length}");
        return passed ? 0 : 1;
    }

    private sealed class SelfTestReader(Queue<EnvelopeReadResult> input, Func<EnvelopeReadResult> after) : IEnvelopeReader
    {
        public EnvelopeReadResult Read(uint _) { if (input.TryDequeue(out EnvelopeReadResult result)) return result; Thread.Sleep(10); return after(); }
    }

    private sealed class UiaSelfTestTransport(Envelope request) : IEnvelopeReader, IEnvelopeWriter
    {
        private readonly Queue<EnvelopeReadResult> _input = new([
            new(EnvelopeReadKind.Envelope, new Envelope { Version = ProtocolVersion.Value, ReqId = 0, Type = MsgType.Version }),
            new(EnvelopeReadKind.Envelope, request),
        ]);
        private readonly List<Envelope> _written = [];
        private readonly ManualResetEventSlim _uiaReplyWritten = new(false);

        internal Envelope[] Written { get { lock (_written) return _written.ToArray(); } }

        public EnvelopeReadResult Read(uint _)
        {
            lock (_input)
            {
                if (_input.TryDequeue(out EnvelopeReadResult message)) return message;
            }
            if (_uiaReplyWritten.Wait(25)) return new(EnvelopeReadKind.Terminal, Error: 995);
            return new(EnvelopeReadKind.Timeout);
        }

        public bool Write(Envelope envelope, out int error)
        {
            lock (_written) _written.Add(envelope);
            if (envelope.Type == MsgType.Uia && envelope.ReqId == request.ReqId) _uiaReplyWritten.Set();
            error = 0;
            return true;
        }

        internal bool WaitForUiaReply(TimeSpan timeout) => _uiaReplyWritten.Wait(timeout);
    }

    private sealed class SelfTestWriter(List<Envelope> written, Func<int> ownerThread, Stopwatch pingTimer, bool failWrites = false) : IEnvelopeWriter
    {
        internal bool OwnerOnly { get; private set; } = true;
        internal long? PongLatencyMs { get; private set; }
        public bool Write(Envelope envelope, out int error)
        {
            OwnerOnly &= ownerThread() == Environment.CurrentManagedThreadId;
            if (failWrites) { error = 123; return false; }
            if (envelope is { Type: MsgType.Pong, ReqId: 42 }) PongLatencyMs = pingTimer.ElapsedMilliseconds;
            lock (written) written.Add(envelope);
            error = 0;
            return true;
        }
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

/// The single user32.dll P/Invoke <see cref="RunUiaAotSmokeTest"/> needs:
/// the desktop root window handle, an always-available target for the UIA
/// smoke test that has no dependency on Notepad or any other specific app
/// (07-02-PLAN.md Task 2).
internal static partial class User32Interop
{
    [LibraryImport("user32.dll")]
    internal static partial nint GetDesktopWindow();
}
