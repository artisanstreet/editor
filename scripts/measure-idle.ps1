# Read-only process sampler. CPU 100% means one fully occupied logical core.
# Example: .\scripts\measure-idle.ps1 -ProcessId 1234 -Seconds 60 -Scenario home
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateRange(1, 2147483647)][int]$ProcessId,
    [ValidateRange(1, 86400)][int]$Seconds = 60,
    [ValidateRange(100, 60000)][int]$IntervalMilliseconds = 1000,
    [ValidateRange(0, 3600)][int]$WarmupSeconds = 0,
    [string]$Scenario = "unspecified",
    [string]$Build = "unspecified",
    [string]$OutputPath
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not ("IdleWindowInfo" -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class IdleWindowInfo {
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct Point { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct MonitorInfo { public uint Size; public Rect Monitor, Work; public uint Flags; }
    [StructLayout(LayoutKind.Sequential)] public struct MemoryStatus {
        public uint Length, MemoryLoad;
        public ulong TotalPhysical, AvailablePhysical, TotalPageFile, AvailablePageFile;
        public ulong TotalVirtual, AvailableVirtual, AvailableExtendedVirtual;
    }
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool GlobalMemoryStatusEx(ref MemoryStatus status);
    public static MemoryStatus GetHostMemory() {
        MemoryStatus status = new MemoryStatus();
        status.Length = (uint)Marshal.SizeOf(typeof(MemoryStatus));
        if (!GlobalMemoryStatusEx(ref status)) throw new System.ComponentModel.Win32Exception();
        return status;
    }
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")] private static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);
    [DllImport("user32.dll")] private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo info);
    [DllImport("user32.dll")] private static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    // PowerShell may be DPI-unaware. Avoid reporting virtualized dimensions
    // as physical pixels, without changing the measured process's DPI mode.
    public static bool GetPhysicalClientRect(IntPtr window, out Rect rect) {
        IntPtr previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        if (previous == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
        try { return GetClientRect(window, out rect); }
        finally { SetThreadDpiAwarenessContext(previous); }
    }
    public static int[] GetPlacement(IntPtr window) {
        IntPtr previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        if (previous == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
        try {
            Point origin = new Point();
            MonitorInfo monitor = new MonitorInfo();
            monitor.Size = (uint)Marshal.SizeOf(typeof(MonitorInfo));
            if (!ClientToScreen(window, ref origin) ||
                !GetMonitorInfo(MonitorFromWindow(window, 2), ref monitor))
                throw new System.ComponentModel.Win32Exception();
            return new int[] { origin.X, origin.Y, monitor.Monitor.Left, monitor.Monitor.Top,
                monitor.Monitor.Right, monitor.Monitor.Bottom };
        } finally { SetThreadDpiAwarenessContext(previous); }
    }
}
'@
}

$targetProcess = Get-Process -Id $ProcessId
$processStarted = $targetProcess.StartTime.ToUniversalTime().ToString("o")
$executable = $targetProcess.Path
if ($WarmupSeconds -gt 0) { Start-Sleep -Seconds $WarmupSeconds }
$watch = [System.Diagnostics.Stopwatch]::StartNew()
$samples = [System.Collections.Generic.List[object]]::new()

function Read-Sample {
    $targetProcess.Refresh()
    if ($targetProcess.HasExited) { throw "Process $ProcessId exited during measurement." }
    if ($targetProcess.StartTime.ToUniversalTime().ToString("o") -ne $processStarted) {
        throw "Process $ProcessId was replaced during measurement."
    }
    $window = $targetProcess.MainWindowHandle
    $geometry = $null
    if ($window -ne [IntPtr]::Zero) {
        $rect = New-Object IdleWindowInfo+Rect
        if ([IdleWindowInfo]::GetPhysicalClientRect($window, [ref]$rect)) {
            $placement = [IdleWindowInfo]::GetPlacement($window)
            $width = $rect.Right - $rect.Left
            $height = $rect.Bottom - $rect.Top
            $geometry = [ordered]@{
                width_physical_px = $width
                height_physical_px = $height
                client_origin_physical_px = @($placement[0], $placement[1])
                monitor_bounds_physical_px = @($placement[2], $placement[3], $placement[4], $placement[5])
                client_inside_nearest_monitor = $placement[0] -ge $placement[2] -and $placement[1] -ge $placement[3] -and ($placement[0] + $width) -le $placement[4] -and ($placement[1] + $height) -le $placement[5]
                dpi = [IdleWindowInfo]::GetDpiForWindow($window)
                visible_style = [IdleWindowInfo]::IsWindowVisible($window)
                minimized = [IdleWindowInfo]::IsIconic($window)
                foreground = [IdleWindowInfo]::GetForegroundWindow() -eq $window
            }
        }
    }
    $hostMemory = [IdleWindowInfo]::GetHostMemory()
    [pscustomobject][ordered]@{
        elapsed_seconds = $watch.Elapsed.TotalSeconds
        cpu_seconds = $targetProcess.TotalProcessorTime.TotalSeconds
        working_set_bytes = $targetProcess.WorkingSet64
        private_committed_bytes = $targetProcess.PrivateMemorySize64
        threads = $targetProcess.Threads.Count
        handles = $targetProcess.HandleCount
        window = $geometry
        host_memory = [ordered]@{
            total_physical_bytes = $hostMemory.TotalPhysical
            available_physical_bytes = $hostMemory.AvailablePhysical
            physical_load_percent = $hostMemory.MemoryLoad
            reported_total_page_file_bytes = $hostMemory.TotalPageFile
            reported_available_page_file_bytes = $hostMemory.AvailablePageFile
        }
    }
}

$measuredAt = [DateTime]::UtcNow.ToString("o")
$samples.Add((Read-Sample))
while ($watch.Elapsed.TotalSeconds -lt $Seconds) {
    $remaining = ($Seconds - $watch.Elapsed.TotalSeconds) * 1000
    Start-Sleep -Milliseconds ([int][Math]::Max(1, [Math]::Min($IntervalMilliseconds, $remaining)))
    $samples.Add((Read-Sample))
}
$elapsed = $samples[-1].elapsed_seconds - $samples[0].elapsed_seconds
$cpu = $samples[-1].cpu_seconds - $samples[0].cpu_seconds
$workingSet = $samples | Measure-Object -Property working_set_bytes -Minimum -Maximum -Average
$private = $samples | Measure-Object -Property private_committed_bytes -Minimum -Maximum -Average
$hostAvailable = $samples | ForEach-Object { $_.host_memory.available_physical_bytes } | Measure-Object -Minimum -Maximum
$hostLoad = $samples | ForEach-Object { $_.host_memory.physical_load_percent } | Measure-Object -Maximum
$windowStates = @($samples | ForEach-Object { $_.window | ConvertTo-Json -Compress } | Select-Object -Unique)
$disqualifiers = [System.Collections.Generic.List[string]]::new()
if (@($samples | Where-Object { $null -eq $_.window }).Count -gt 0) {
    $disqualifiers.Add("Window geometry was unavailable for at least one sample.")
}
if (@($samples | Where-Object { $null -ne $_.window -and $_.window.minimized }).Count -gt 0) {
    $disqualifiers.Add("Window was minimized during measurement.")
}
if (@($samples | Where-Object { $null -ne $_.window -and -not $_.window.visible_style }).Count -gt 0) {
    $disqualifiers.Add("Window was hidden during measurement.")
}
if (@($samples | Where-Object { $null -ne $_.window -and -not $_.window.client_inside_nearest_monitor }).Count -gt 0) {
    $disqualifiers.Add("Client was not fully contained within its nearest monitor. Check for offscreen content or intentional monitor spanning.")
}
if ($windowStates.Count -gt 1) {
    $disqualifiers.Add("Window position, dimensions, monitor, DPI, focus or visibility changed during measurement.")
}
$report = [ordered]@{
    schema_version = 1
    platform = "windows"
    measured_at_utc = $measuredAt
    scenario = $Scenario
    build = $Build
    pid = $ProcessId
    process_started_utc = $processStarted
    executable = $executable
    logical_processors = [Environment]::ProcessorCount
    warmup_seconds = $WarmupSeconds
    qualification = [ordered]@{
        window_state_stable = $windowStates.Count -eq 1
        comparison_disqualifiers = $disqualifiers.ToArray()
        acceptance = "not_established"
        remaining_checks = "Verify the intended workload, actual visibility, lack of user input and build identity separately. Constant focus and dimensions alone do not establish idle acceptance."
    }
    summary = [ordered]@{
        elapsed_seconds = $elapsed
        cpu_seconds = $cpu
        cpu_core_percent = 100 * $cpu / $elapsed
        cpu_machine_percent = 100 * $cpu / $elapsed / [Environment]::ProcessorCount
        working_set_min_bytes = $workingSet.Minimum
        working_set_max_bytes = $workingSet.Maximum
        working_set_mean_bytes = $workingSet.Average
        private_committed_min_bytes = $private.Minimum
        private_committed_max_bytes = $private.Maximum
        private_committed_mean_bytes = $private.Average
        host_available_physical_min_bytes = $hostAvailable.Minimum
        host_available_physical_max_bytes = $hostAvailable.Maximum
        host_physical_load_max_percent = $hostLoad.Maximum
    }
    samples = $samples.ToArray()
    scope = "One process; includes its CPU-resident mappings, but does not measure dedicated GPU memory or other processes. Private committed bytes are not resident RAM. Scenario is supplied by the caller, not verified by this sampler."
}
$json = $report | ConvertTo-Json -Depth 6
if ($OutputPath) {
    [System.IO.File]::WriteAllText($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath), $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
} else {
    $json
}
