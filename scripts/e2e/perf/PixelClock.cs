using System;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

// Test-only painter for the performance workload (not shipped).
//
// A borderless window covers the whole desktop. At the top it shows the current
// system time as a black and white bar code; below it a content area scrolls
// continuously, so that every tick changes a large part of the screen. A
// colour block toggles on every key press (the input-to-change cross-check).
// The painter also serves the same clock over loopback HTTP, so that a browser
// can compare its own clock with it.
//
// Bar code: Cells cells of Cell x RowHeight pixels, white = 1, black = 0.
//   cells 0..3    sync 1,0,1,0
//   cells 4..51   48-bit time in microseconds since the Unix epoch (mod 2^48), MSB first
//   cells 52..67  CRC-16/CCITT-FALSE of the six time bytes, MSB first
//   cells 68..71  sync 0,1,0,1
// Two identical rows (copies) are stacked; the layout must match run-perf-proof.py.
//
// The source stays in the C# 5 subset: Windows PowerShell 5.1 compiles it.
public static class PixelClock
{
    public const int Cell = 24;
    public const int Cells = 72;
    public const int RowHeight = 32;
    public const int Rows = 2;
    public const int StripHeight = 64;

    // The toggle block, in desktop pixels; it must match run-perf-proof.py.
    public const int BlockX = 1620;
    public const int BlockY = 100;
    public const int BlockW = 280;
    public const int BlockH = 200;

    [DllImport("kernel32.dll")]
    static extern void GetSystemTimePreciseAsFileTime(out long fileTime);

    [DllImport("user32.dll")]
    static extern bool SetProcessDPIAware();

    [DllImport("winmm.dll")]
    static extern uint timeBeginPeriod(uint milliseconds);

    public static long UnixMicros()
    {
        long fileTime;
        GetSystemTimePreciseAsFileTime(out fileTime);
        return (fileTime - 116444736000000000L) / 10L;
    }

    public static ushort Crc16(byte[] data)
    {
        int crc = 0xFFFF;
        for (int i = 0; i < data.Length; i++)
        {
            crc ^= data[i] << 8;
            for (int bit = 0; bit < 8; bit++)
            {
                if ((crc & 0x8000) != 0) { crc = ((crc << 1) ^ 0x1021) & 0xFFFF; }
                else { crc = (crc << 1) & 0xFFFF; }
            }
        }
        return (ushort)crc;
    }

    public static bool[] Encode(long micros)
    {
        ulong value = ((ulong)micros) & 0xFFFFFFFFFFFFUL;
        byte[] bytes = new byte[6];
        for (int i = 0; i < 6; i++) { bytes[i] = (byte)((value >> (8 * (5 - i))) & 0xFFUL); }
        ushort crc = Crc16(bytes);
        bool[] bits = new bool[Cells];
        bits[0] = true; bits[1] = false; bits[2] = true; bits[3] = false;
        for (int i = 0; i < 48; i++) { bits[4 + i] = ((value >> (47 - i)) & 1UL) != 0UL; }
        for (int i = 0; i < 16; i++) { bits[52 + i] = ((crc >> (15 - i)) & 1) != 0; }
        bits[68] = false; bits[69] = true; bits[70] = false; bits[71] = true;
        return bits;
    }

    public static long Draws;
    public static long MaxDrawMicros;
    public static long LateWakeups;
    public static long Toggles;
    public static int ScrollStep;
    public static int IntervalMs;
    public static int ScreenWidth;
    public static int ScreenHeight;

    public static void Run(int port, int intervalMs, int step)
    {
        SetProcessDPIAware();
        timeBeginPeriod(1);
        ScrollStep = step;
        IntervalMs = intervalMs;
        ClockServer.Start(port);
        Application.EnableVisualStyles();
        PixelClockForm form = new PixelClockForm(step);
        ScreenWidth = form.PixelWidth;
        ScreenHeight = form.PixelHeight;
        form.Show();
        form.Activate();
        Stopwatch clock = Stopwatch.StartNew();
        long next = 0;
        while (form.Visible)
        {
            long before = clock.ElapsedTicks;
            form.Draw(UnixMicros());
            long took = (clock.ElapsedTicks - before) * 1000000L / Stopwatch.Frequency;
            if (took > MaxDrawMicros) { MaxDrawMicros = took; }
            Draws++;
            Application.DoEvents();
            next += intervalMs;
            long wait = next - clock.ElapsedMilliseconds;
            if (wait > 0) { Thread.Sleep((int)wait); }
            else { next = clock.ElapsedMilliseconds; LateWakeups++; }
        }
    }
}

public class PixelClockForm : Form
{
    [DllImport("gdi32.dll")]
    static extern bool BitBlt(IntPtr destination, int x, int y, int width, int height,
        IntPtr source, int sourceX, int sourceY, int operation);

    const int SrcCopy = 0x00CC0020;

    readonly Bitmap buffer;
    readonly int width;
    readonly int height;
    readonly int step;
    readonly SolidBrush white = new SolidBrush(Color.White);
    readonly SolidBrush black = new SolidBrush(Color.Black);
    readonly SolidBrush blockOff = new SolidBrush(Color.FromArgb(0, 0, 160));
    readonly SolidBrush blockOn = new SolidBrush(Color.FromArgb(255, 220, 0));
    readonly SolidBrush bandA = new SolidBrush(Color.FromArgb(44, 52, 70));
    readonly SolidBrush bandB = new SolidBrush(Color.FromArgb(70, 60, 48));
    readonly SolidBrush ink = new SolidBrush(Color.FromArgb(230, 230, 230));
    readonly Font font = new Font("Consolas", 10f);
    readonly Random random = new Random(1);
    bool blockIsOn;
    long band;

    public int PixelWidth { get { return width; } }
    public int PixelHeight { get { return height; } }

    public PixelClockForm(int scrollStep)
    {
        Rectangle bounds = Screen.PrimaryScreen.Bounds;
        width = bounds.Width;
        height = bounds.Height;
        step = scrollStep;
        FormBorderStyle = FormBorderStyle.None;
        StartPosition = FormStartPosition.Manual;
        Location = new Point(0, 0);
        ClientSize = new Size(width, height);
        TopMost = true;
        ShowInTaskbar = false;
        BackColor = Color.Black;
        KeyPreview = true;
        buffer = new Bitmap(width, height, PixelFormat.Format32bppRgb);
        using (Graphics offscreen = Graphics.FromImage(buffer))
        {
            offscreen.Clear(Color.FromArgb(44, 52, 70));
        }
        SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.Opaque | ControlStyles.UserPaint, true);
        KeyDown += OnKeyDown;
    }

    void OnKeyDown(object sender, KeyEventArgs e)
    {
        blockIsOn = !blockIsOn;
        PixelClock.Toggles++;
        e.Handled = true;
    }

    protected override void OnPaintBackground(PaintEventArgs e) { }

    protected override void OnPaint(PaintEventArgs e)
    {
        e.Graphics.DrawImageUnscaled(buffer, 0, 0);
    }

    public void Draw(long micros)
    {
        bool[] bits = PixelClock.Encode(micros);
        int top = PixelClock.StripHeight;
        using (Graphics offscreen = Graphics.FromImage(buffer))
        {
            // One blit moves the content area up by one step.
            IntPtr dc = offscreen.GetHdc();
            try
            {
                BitBlt(dc, 0, top, width, height - top - step, dc, 0, top + step, SrcCopy);
            }
            finally
            {
                offscreen.ReleaseHdc(dc);
            }
            offscreen.TextRenderingHint = TextRenderingHint.SingleBitPerPixelGridFit;
            DrawBand(offscreen, height - step);
            for (int row = 0; row < PixelClock.Rows; row++)
            {
                for (int cell = 0; cell < PixelClock.Cells; cell++)
                {
                    offscreen.FillRectangle(bits[cell] ? white : black,
                        cell * PixelClock.Cell, row * PixelClock.RowHeight, PixelClock.Cell, PixelClock.RowHeight);
                }
            }
            offscreen.FillRectangle(blockIsOn ? blockOn : blockOff,
                PixelClock.BlockX, PixelClock.BlockY, PixelClock.BlockW, PixelClock.BlockH);
        }
        using (Graphics screen = CreateGraphics())
        {
            screen.CompositingMode = CompositingMode.SourceCopy;
            screen.DrawImageUnscaled(buffer, 0, 0);
        }
    }

    // One new band of stripes and text at the bottom of the content area.
    void DrawBand(Graphics g, int y)
    {
        band++;
        g.FillRectangle((band & 1L) == 0L ? bandA : bandB, 0, y, width, step);
        string text = string.Format(CultureInfo.InvariantCulture, "{0:X8} {1:X8} {2:X8} {3:X8} {4:X8} band {5}",
            random.Next(), random.Next(), random.Next(), random.Next(), random.Next(), band);
        g.DrawString(text, font, ink, 8f, (float)(y + 3));
    }
}

// Loopback HTTP: GET /t answers the precise clock in microseconds since the
// Unix epoch, read after the request arrived; GET /stats answers painter counters.
public static class ClockServer
{
    public static void Start(int port)
    {
        TcpListener listener = new TcpListener(IPAddress.Loopback, port);
        listener.Start();
        Thread thread = new Thread(delegate () { Loop(listener); });
        thread.IsBackground = true;
        thread.Start();
    }

    static void Loop(TcpListener listener)
    {
        while (true)
        {
            TcpClient client;
            try { client = listener.AcceptTcpClient(); }
            catch (Exception) { return; }
            ThreadPool.QueueUserWorkItem(Serve, client);
        }
    }

    static string Number(long value)
    {
        return value.ToString(CultureInfo.InvariantCulture);
    }

    static void Serve(object state)
    {
        TcpClient client = (TcpClient)state;
        try
        {
            client.NoDelay = true;
            client.ReceiveTimeout = 2000;
            NetworkStream stream = client.GetStream();
            byte[] buffer = new byte[2048];
            int used = 0;
            while (used < buffer.Length)
            {
                int n = stream.Read(buffer, used, buffer.Length - used);
                if (n <= 0) { break; }
                used += n;
                if (Encoding.ASCII.GetString(buffer, 0, used).Contains("\r\n\r\n")) { break; }
            }
            string request = Encoding.ASCII.GetString(buffer, 0, used);
            long micros = PixelClock.UnixMicros();
            string body;
            if (request.StartsWith("GET /stats"))
            {
                body = "{\"draws\":" + Number(PixelClock.Draws)
                    + ",\"max_draw_us\":" + Number(PixelClock.MaxDrawMicros)
                    + ",\"late_wakeups\":" + Number(PixelClock.LateWakeups)
                    + ",\"toggles\":" + Number(PixelClock.Toggles)
                    + ",\"scroll_step\":" + Number(PixelClock.ScrollStep)
                    + ",\"interval_ms\":" + Number(PixelClock.IntervalMs)
                    + ",\"width\":" + Number(PixelClock.ScreenWidth)
                    + ",\"height\":" + Number(PixelClock.ScreenHeight)
                    + ",\"now_us\":" + Number(micros) + "}";
            }
            else
            {
                body = Number(micros);
            }
            string head = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\n"
                + "Content-Type: text/plain\r\nContent-Length: " + Number(body.Length)
                + "\r\nConnection: close\r\n\r\n";
            byte[] reply = Encoding.ASCII.GetBytes(head + body);
            stream.Write(reply, 0, reply.Length);
        }
        catch (Exception) { }
        finally { client.Close(); }
    }
}
