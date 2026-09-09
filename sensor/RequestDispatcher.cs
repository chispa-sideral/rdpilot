using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;

namespace RdpilotSensor;

/// Results from a channel read. The owner loop treats malformed input as a
/// drop, unlike a terminal WTS failure which ends the transport.
internal enum EnvelopeReadKind { Timeout, Envelope, Drop, Terminal }
internal readonly record struct EnvelopeReadResult(EnvelopeReadKind Kind, Envelope? Envelope = null, int Error = 0);
internal interface IEnvelopeReader { EnvelopeReadResult Read(uint timeoutMs); }
internal interface IEnvelopeWriter { bool Write(Envelope envelope, out int error); }

internal sealed record DispatchRequest(Envelope Envelope, long DeadlineTicks);
internal sealed record DispatchCompletion(ulong ReqId, long CompletedAtTicks, Envelope Reply);
/// Observational evidence captured once when a dispatcher worker has attempted
/// its COM initialization. The observer is deliberately isolated from WTS I/O,
/// completion arbitration, and the owner writer.
internal sealed record WorkerStartupEvidence(
    string ThreadName,
    int ManagedThreadId,
    ApartmentState ApartmentState,
    int CoInitializeExHResult,
    bool ComInitializationSucceeded);

/// A bounded, process-lifetime worker pool. WTS I/O and response arbitration
/// deliberately do not live here: only the channel owner may perform either.
internal sealed class RequestDispatcher
{
    private const int DefaultWorkers = 4;
    private const int DefaultCapacity = 16;
    private readonly BlockingCollection<DispatchRequest> _work;
    private readonly ConcurrentQueue<DispatchCompletion> _completed = new();
    private readonly ConcurrentDictionary<ulong, byte> _cancelled = new();
    private readonly Dictionary<ulong, DispatchRequest> _outstanding = [];
    private readonly Queue<Envelope> _outbound = new();
    private readonly Func<Envelope, Envelope> _handle;
    private readonly Func<Envelope, string, Envelope> _failure;
    private readonly Action<WorkerStartupEvidence>? _workerStartupObserver;
    private readonly Thread[] _workers;
    private bool _accepting = true;

    internal RequestDispatcher(Func<Envelope, Envelope> handle, Func<Envelope, string, Envelope> failure,
        int workerCount = DefaultWorkers, int queueCapacity = DefaultCapacity,
        Action<WorkerStartupEvidence>? workerStartupObserver = null)
    {
        _handle = handle;
        _failure = failure;
        _workerStartupObserver = workerStartupObserver;
        _work = new BlockingCollection<DispatchRequest>(new ConcurrentQueue<DispatchRequest>(), queueCapacity);
        _workers = Enumerable.Range(0, workerCount).Select(CreateWorker).ToArray();
        foreach (Thread worker in _workers) worker.Start();
    }

    private Thread CreateWorker(int index)
    {
        Thread worker = new(WorkerMain)
        {
            IsBackground = true,
            Name = $"rdpilot-sensor-worker-{index + 1}",
        };
        if (OperatingSystem.IsWindows()) worker.SetApartmentState(ApartmentState.MTA);
        return worker;
    }

    internal bool Admit(Envelope envelope)
    {
        if (!_accepting || _outstanding.ContainsKey(envelope.ReqId)) return false;
        DispatchRequest request = new(envelope, Stopwatch.GetTimestamp() + DeadlineTicks(envelope.Type));
        _outstanding.Add(envelope.ReqId, request);
        if (_work.TryAdd(request)) return true;
        _outstanding.Remove(envelope.ReqId);
        _outbound.Enqueue(_failure(envelope, "sensor request queue is full"));
        return false;
    }

    /// Called only by the channel owner. Completion time, not observation
    /// time, decides a race with the monotonic request deadline.
    internal void Arbitrate()
    {
        List<DispatchCompletion> completed = [];
        while (_completed.TryDequeue(out DispatchCompletion? item)) completed.Add(item);
        foreach (DispatchCompletion completion in completed.OrderBy(c => c.CompletedAtTicks).ThenBy(c => c.ReqId))
        {
            if (!_outstanding.Remove(completion.ReqId, out DispatchRequest? request)) continue;
            if (completion.CompletedAtTicks <= request.DeadlineTicks)
                _outbound.Enqueue(completion.Reply);
            else
            {
                _cancelled.TryAdd(request.Envelope.ReqId, 0);
                _outbound.Enqueue(_failure(request.Envelope, "sensor request deadline exceeded"));
                _cancelled.TryRemove(request.Envelope.ReqId, out _);
            }
        }
        long now = Stopwatch.GetTimestamp();
        foreach (DispatchRequest due in _outstanding.Values.Where(r => r.DeadlineTicks <= now)
                     .OrderBy(r => r.DeadlineTicks).ThenBy(r => r.Envelope.ReqId).ToArray())
        {
            _outstanding.Remove(due.Envelope.ReqId);
            _cancelled.TryAdd(due.Envelope.ReqId, 0);
            _outbound.Enqueue(_failure(due.Envelope, "sensor request deadline exceeded"));
        }
    }

    internal void EnqueueControl(Envelope envelope) => _outbound.Enqueue(envelope);
    internal bool TryDequeueOutbound(out Envelope? envelope) => _outbound.TryDequeue(out envelope);

    /// Never joins, disposes, or closes worker-owned state: a native/UIA call
    /// may remain blocked. Its eventual completion is harmlessly ignored.
    internal void StopAccepting()
    {
        if (!_accepting) return;
        _accepting = false;
        foreach (ulong reqId in _outstanding.Keys) _cancelled.TryAdd(reqId, 0);
        _outstanding.Clear();
        _work.CompleteAdding();
    }

    private void WorkerMain()
    {
        bool uninitialize = false;
        string? initError = null;
        if (OperatingSystem.IsWindows())
        {
            int result = Ole32.CoInitializeEx(0, Ole32.COINIT_MULTITHREADED);
            uninitialize = result >= 0;
            if (result < 0) initError = $"worker COM initialization failed: 0x{result:X8}";
            try
            {
                _workerStartupObserver?.Invoke(new WorkerStartupEvidence(
                    Thread.CurrentThread.Name ?? "unnamed",
                    Environment.CurrentManagedThreadId,
                    Thread.CurrentThread.GetApartmentState(),
                    result,
                    uninitialize));
            }
            catch
            {
                // Observers are test/diagnostic-only and must never impair
                // a production worker after COM initialization.
            }
        }
        try
        {
            foreach (DispatchRequest request in _work.GetConsumingEnumerable())
            {
                if (_cancelled.TryRemove(request.Envelope.ReqId, out _)) continue;
                Envelope reply;
                try { reply = initError is null ? _handle(request.Envelope) : _failure(request.Envelope, initError); }
                catch (Exception ex) { reply = _failure(request.Envelope, ex.Message); }
                _completed.Enqueue(new DispatchCompletion(request.Envelope.ReqId, Stopwatch.GetTimestamp(), reply));
                _cancelled.TryRemove(request.Envelope.ReqId, out _);
            }
        }
        finally
        {
            if (uninitialize) Ole32.CoUninitialize();
        }
    }

    private static long DeadlineTicks(MsgType type)
    {
        int ms = type switch
        {
            MsgType.WindowList or MsgType.ProcessTree or MsgType.Uia => 1500,
            MsgType.SetForegroundWindow or MsgType.LaunchProcess => 350,
            MsgType.FileTransfer => 25_000,
            _ => 350,
        };
        return (long)(Stopwatch.Frequency * (ms / 1000.0));
    }
}

/// The sole owner of a WTS handle drives this loop. It can also be used with
/// fakes, proving that workers never directly write a response.
internal sealed class SensorOwnerLoop
{
    private readonly IEnvelopeReader _reader;
    private readonly IEnvelopeWriter _writer;
    private readonly RequestDispatcher _dispatcher;
    private readonly Func<Envelope, bool> _supported;

    internal SensorOwnerLoop(IEnvelopeReader reader, IEnvelopeWriter writer, RequestDispatcher dispatcher, Func<Envelope, bool> supported)
        => (_reader, _writer, _dispatcher, _supported) = (reader, writer, dispatcher, supported);

    internal bool Run()
    {
        bool handshaken = false;
        while (true)
        {
            _dispatcher.Arbitrate();
            while (_dispatcher.TryDequeueOutbound(out Envelope? reply))
            {
                if (!_writer.Write(reply!, out int error))
                {
                    Console.Error.WriteLine($"[rdpilot-sensor] terminal channel write failure: {error}");
                    _dispatcher.StopAccepting();
                    return false;
                }
            }

            EnvelopeReadResult read = _reader.Read(50);
            if (read.Kind == EnvelopeReadKind.Terminal)
            {
                Console.Error.WriteLine($"[rdpilot-sensor] terminal channel read failure: {read.Error}");
                _dispatcher.StopAccepting();
                return false;
            }
            if (read.Kind != EnvelopeReadKind.Envelope || read.Envelope is not Envelope message) continue;
            if (!handshaken)
            {
                if (message.Type != MsgType.Version) continue;
                handshaken = true;
                _dispatcher.EnqueueControl(new Envelope { Version = ProtocolVersion.Value, ReqId = 0, Type = MsgType.Version, Payload = null });
                continue;
            }
            if (message.Type == MsgType.Ping)
                _dispatcher.EnqueueControl(new Envelope { Version = ProtocolVersion.Value, ReqId = message.ReqId, Type = MsgType.Pong, Payload = null });
            else if (_supported(message))
                _dispatcher.Admit(message);
        }
    }
}

internal static partial class Ole32
{
    internal const uint COINIT_MULTITHREADED = 0x0;
    [LibraryImport("ole32.dll")] internal static partial int CoInitializeEx(nint reserved, uint coInit);
    [LibraryImport("ole32.dll")] internal static partial void CoUninitialize();
}
