// Default-off capability for the live DVC dispatcher proof.  It deliberately
// derives both coordination paths from FileTransfer's fixed sensor-owned root:
// callers can exchange the two marker files only through FileTransfer.
namespace RdpilotSensor;

internal sealed class LiveDvcBlockedHandler
{
    private const int TokenLength = 32;
    private readonly string _enteredPath;
    private readonly string _releasePath;

    private LiveDvcBlockedHandler(string token)
    {
        _enteredPath = Path.Combine(FileTransfer.ShareRoot, $"live-dvc-{token}.entered");
        _releasePath = Path.Combine(FileTransfer.ShareRoot, $"live-dvc-{token}.release");
    }

    internal static bool TryCreate(string token, out LiveDvcBlockedHandler? handler)
    {
        handler = null;
        if (token.Length != TokenLength || token.Any(c => !(c is >= '0' and <= '9' or >= 'a' and <= 'f')))
        {
            return false;
        }
        handler = new LiveDvcBlockedHandler(token);
        return true;
    }

    /// A WindowList worker writes an entered marker then intentionally waits
    /// for the matching release marker. It neither accepts a user path nor
    /// affects ordinary sensor mode. The bounded dispatcher remains free to
    /// deliver its deadline failure and serve unrelated requests.
    internal void WaitForRelease()
    {
        FileTransfer.EnsureShareRootExists();
        if (File.Exists(_enteredPath) || File.Exists(_releasePath))
        {
            throw new IOException("live DVC marker already exists");
        }
        using (FileStream entered = new(_enteredPath, FileMode.CreateNew, FileAccess.Write, FileShare.Read))
        {
            entered.Flush(flushToDisk: true);
        }
        while (!File.Exists(_releasePath))
        {
            Thread.Sleep(25);
        }
    }
}
