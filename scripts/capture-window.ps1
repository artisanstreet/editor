# Post-measurement image of an explicit process's own native client window.
# PrintWindow may return success with unusable pixels; inspection is mandatory.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [Parameter(Mandatory = $true)][string]$OutputPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.Drawing
if (-not ('OwnedWindowImage' -as [type])) {
    Add-Type -ReferencedAssemblies System.Drawing @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class OwnedWindowImage {
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint process);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    public static int[] Save(IntPtr window, int expectedProcess, string output) {
        uint process;
        if (window == IntPtr.Zero || GetWindowThreadProcessId(window, out process) == 0 || process != expectedProcess)
            throw new InvalidOperationException("Window does not belong to the explicit process.");
        if (IsIconic(window)) throw new InvalidOperationException("Cannot qualify a minimized window.");
        IntPtr previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        if (previous == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
        try {
            Rect bounds;
            if (!GetClientRect(window, out bounds)) throw new System.ComponentModel.Win32Exception();
            int width = bounds.R - bounds.L, height = bounds.B - bounds.T;
            if (width <= 0 || height <= 0) throw new InvalidOperationException("Empty client area.");
            using (Bitmap bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb)) {
                using (Graphics graphics = Graphics.FromImage(bitmap)) {
                    graphics.Clear(Color.Magenta);
                    IntPtr dc = graphics.GetHdc();
                    bool success;
                    try { success = PrintWindow(window, dc, 3); } // CLIENTONLY | RENDERFULLCONTENT
                    finally { graphics.ReleaseHdc(dc); }
                    if (!success) throw new InvalidOperationException("PrintWindow returned false.");
                }
                if (GetWindowThreadProcessId(window, out process) == 0 || process != expectedProcess)
                    throw new InvalidOperationException("Window changed owner during capture.");
                bitmap.Save(output, ImageFormat.Png);
            }
            return new int[] { width, height, (int)GetDpiForWindow(window) };
        } finally { SetThreadDpiAwarenessContext(previous); }
    }
}
'@
}
$targetProcess = Get-Process -Id $ProcessId -ErrorAction Stop
$window = $targetProcess.MainWindowHandle
$absolutePath = [IO.Path]::GetFullPath($OutputPath)
$dimensions = [OwnedWindowImage]::Save($window, $ProcessId, $absolutePath)
$metadata = [ordered]@{
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    pid = $ProcessId
    process_started_utc = $targetProcess.StartTime.ToUniversalTime().ToString('o')
    window_handle = ('0x{0:x}' -f $window.ToInt64())
    method = 'PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT)'
    width_physical_px = $dimensions[0]
    height_physical_px = $dimensions[1]
    dpi = $dimensions[2]
    image = $absolutePath
    visible_conversation_verified = $false
    qualification = 'Requires pixel inspection. API success alone does not prove content, visibility or correctness. Capture occurred after measurement and may allocate memory or request painting.'
}
$metadata | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath ([IO.Path]::ChangeExtension($absolutePath, '.json'))
$metadata | ConvertTo-Json -Depth 3
