# Launch an isolated Editor copy, measure it, and close only that child process.
# Does not invoke the dev launcher or start, stop or restart any Forge.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$FixtureEditorData,
    [string]$Link,
    [string]$Scenario = 'unpaired-home',
    [string]$Build = 'unspecified',
    [ValidateSet('default', 'dx12', 'vulkan')][string]$Backend = 'default',
    [ValidateRange(1, 86400)][int]$Seconds = 60,
    [ValidateRange(0, 3600)][int]$WarmupSeconds = 20,
    [ValidateRange(480, 8192)][int]$ClientWidth = 1280,
    [ValidateRange(420, 8192)][int]$ClientHeight = 900,
    [switch]$InspectMemory,
    [switch]$RendererDiagnostics,
    [switch]$DisableDirectComposition,
    [switch]$CaptureWindow,
    [switch]$RequireCompletedConversation,
    [switch]$DiagnoseDriverSpin,
    [switch]$SampleDriverInstruction,
    [ValidateRange(1, 100)][int]$InstructionSamples = 12
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not ('IdleWindowSizer' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class IdleWindowSizer {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint processId);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr h, int index);
    public static long[] Styles(IntPtr h, uint expectedProcessId) {
        uint owner;
        if (GetWindowThreadProcessId(h, out owner) == 0 || owner != expectedProcessId)
            throw new InvalidOperationException("Window does not belong to the owned child.");
        return new long[] { GetWindowLongPtr(h, -16).ToInt64() & 0xffffffffL,
                            GetWindowLongPtr(h, -20).ToInt64() & 0xffffffffL };
    }
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int height, uint flags);
    [DllImport("user32.dll")] static extern bool InvalidateRect(IntPtr h, IntPtr rect, bool erase);
    [DllImport("user32.dll")] static extern bool UpdateWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int command);
    public static void Repaint(IntPtr h) {
        if (!InvalidateRect(h, IntPtr.Zero, false) || !UpdateWindow(h))
            throw new System.ComponentModel.Win32Exception();
    }
    public static void MinimizeWithoutActivation(IntPtr h) { ShowWindow(h, 7); }
    public static void RestoreWithoutActivation(IntPtr h) { ShowWindow(h, 4); }
    public static bool Resize(IntPtr h, int logicalWidth, int logicalHeight) {
        IntPtr previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        if (previous == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
        try {
            Rect client, window;
            if (!GetClientRect(h, out client) || !GetWindowRect(h, out window))
                throw new System.ComponentModel.Win32Exception();
            double scale = GetDpiForWindow(h) / 96.0;
            int clientWidth = (int)Math.Round(logicalWidth * scale);
            int clientHeight = (int)Math.Round(logicalHeight * scale);
            if (client.R - client.L == clientWidth && client.B - client.T == clientHeight)
                return false;
            int width = clientWidth + window.R - window.L - client.R + client.L;
            int height = clientHeight + window.B - window.T - client.B + client.T;
            if (!SetWindowPos(h, IntPtr.Zero, 0, 0, width, height, 0x16))
                throw new System.ComponentModel.Win32Exception();
            return true;
        } finally { SetThreadDpiAwarenessContext(previous); }
    }
}
'@
}
$artifactDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
if ((Test-Path -LiteralPath $artifactDirectory) -and
    @(Get-ChildItem -LiteralPath $artifactDirectory -Force).Count -gt 0) {
    throw 'OutputDirectory must be empty so stale results cannot survive a failed run.'
}
New-Item -ItemType Directory -Path $artifactDirectory -Force | Out-Null
$runDirectory = Join-Path $env:TEMP ('artisan-idle-' + [Guid]::NewGuid().ToString('N'))
$dataDirectory = Join-Path $runDirectory 'editor'
New-Item -ItemType Directory -Path $dataDirectory | Out-Null
if ($FixtureEditorData) {
    foreach ($name in @('identity.json', 'hosts.json', 'settings.json')) {
        $source = Join-Path $FixtureEditorData $name
        if (Test-Path -LiteralPath $source) {
            Copy-Item -LiteralPath $source -Destination (Join-Path $dataDirectory $name)
        }
    }
}
$binary = Join-Path $runDirectory 'artisan-editor.exe'
Copy-Item -LiteralPath $Executable -Destination $binary
$stream = [System.IO.File]::OpenRead($binary)
$hasher = [System.Security.Cryptography.SHA256]::Create()
try { $hash = [BitConverter]::ToString($hasher.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
finally { $stream.Dispose(); $hasher.Dispose() }
$oldData = $env:ARTISAN_EDITOR_DATA
$oldLog = $env:RUST_LOG
$oldDiagnostics = $env:GPUI_RENDERER_DIAGNOSTICS
$oldBackend = $env:WGPU_BACKEND
$oldDirectComposition = $env:GPUI_DISABLE_DIRECT_COMPOSITION
$oldScript = $env:ARTISAN_EDITOR_SCRIPT
$oldCapture = $env:ARTISAN_EDITOR_CAPTURE
$oldOrigin = $env:ARTISAN_EDITOR_WINDOW_ORIGIN
$child = $null
try {
    $env:ARTISAN_EDITOR_DATA = $dataDirectory
    $env:ARTISAN_EDITOR_SCRIPT = $null
    $env:ARTISAN_EDITOR_CAPTURE = $null
    $env:ARTISAN_EDITOR_WINDOW_ORIGIN = $null
    $env:RUST_LOG = 'info,gpui=warn,gpui::renderer_memory=info'
    if ($RendererDiagnostics) { $env:GPUI_RENDERER_DIAGNOSTICS = '1' }
    else { $env:GPUI_RENDERER_DIAGNOSTICS = $null }
    if ($Backend -eq 'default') { $env:WGPU_BACKEND = $null }
    else { $env:WGPU_BACKEND = $Backend }
    if ($DisableDirectComposition) { $env:GPUI_DISABLE_DIRECT_COMPOSITION = '1' }
    else { $env:GPUI_DISABLE_DIRECT_COMPOSITION = $null }
    if ($Link) { $child = Start-Process -FilePath $binary -ArgumentList $Link -PassThru }
    else { $child = Start-Process -FilePath $binary -PassThru }
    [ordered]@{
        launched_at_utc = [DateTime]::UtcNow.ToString('o')
        pid = $child.Id
        binary_sha256 = $hash
        binary_bytes = (Get-Item -LiteralPath $binary).Length
        source_executable = $Executable
        executable_copy = $binary
        editor_data = $dataDirectory
        fixture_editor_data = $FixtureEditorData
        scenario = $Scenario
        build = $Build
        requested_wgpu_backend = $Backend
        direct_composition_disabled = [bool]$DisableDirectComposition
        requested_client_logical_px = @($ClientWidth, $ClientHeight)
        renderer_diagnostics = [bool]$RendererDiagnostics
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $artifactDirectory 'launch-context.json')
    Write-Host "Measuring isolated Editor PID $($child.Id), artifacts $artifactDirectory"
    $startup = [Diagnostics.Stopwatch]::StartNew()
    do {
        Start-Sleep -Milliseconds 100
        $child.Refresh()
        if ($child.HasExited) { throw 'Isolated Editor exited during startup.' }
        if ($startup.Elapsed.TotalSeconds -gt 30) { throw 'Isolated Editor window did not appear.' }
    } while ($child.MainWindowHandle -eq [IntPtr]::Zero)
    $resizedAtStartup = [IdleWindowSizer]::Resize($child.MainWindowHandle, $ClientWidth, $ClientHeight)
    $initialStyles = [IdleWindowSizer]::Styles($child.MainWindowHandle, [uint32]$child.Id)
    Start-Sleep -Seconds $WarmupSeconds
    if ($RequireCompletedConversation) {
        $log = Get-Content -Raw -LiteralPath (Join-Path $dataDirectory 'editor.log')
        if ($log -notmatch 'thread opened thread=idle-conversation .*items=40 rows=40 settled=true') {
            throw 'The completed 40-item conversation did not settle before measurement.'
        }
    }
    & (Join-Path $PSScriptRoot 'measure-idle.ps1') -ProcessId $child.Id -Seconds $Seconds -Scenario $Scenario -Build $Build -OutputPath (Join-Path $artifactDirectory 'measurement.json')
    $finalStyles = [IdleWindowSizer]::Styles($child.MainWindowHandle, [uint32]$child.Id)
    [ordered]@{
        captured_at_utc = [DateTime]::UtcNow.ToString('o')
        pid = $child.Id
        window_handle = ('0x{0:x}' -f $child.MainWindowHandle.ToInt64())
        direct_composition_disabled = [bool]$DisableDirectComposition
        initial = @{ style = ('0x{0:x8}' -f $initialStyles[0]); extended_style = ('0x{0:x8}' -f $initialStyles[1]) }
        after_measurement = @{ style = ('0x{0:x8}' -f $finalStyles[0]); extended_style = ('0x{0:x8}' -f $finalStyles[1]) }
        scope = 'Read-only style inspection of the exact runner-owned HWND, before warmup and after measurement. Geometry, focus and visibility remain in every measurement sample.'
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $artifactDirectory 'native-window-metadata.json')
    if ($InspectMemory -or $DiagnoseDriverSpin -or $SampleDriverInstruction) {
        & (Join-Path $PSScriptRoot 'inspect-memory.ps1') -ProcessId $child.Id -Scenario $Scenario -OutputPath (Join-Path $artifactDirectory 'memory.json')
    }
    if ($CaptureWindow) {
        & (Join-Path $PSScriptRoot 'capture-window.ps1') -ProcessId $child.Id -OutputPath (Join-Path $artifactDirectory 'window-after-measurement.png')
    }
    $result = Get-Content -Raw -LiteralPath (Join-Path $artifactDirectory 'measurement.json') | ConvertFrom-Json
    if ($DiagnoseDriverSpin -or $SampleDriverInstruction) {
        $initialMemory = Get-Content -Raw -LiteralPath (Join-Path $artifactDirectory 'memory.json') | ConvertFrom-Json
        $driverThreads = @($initialMemory.threads | Where-Object { $_.start_module -in @('amdxc64.dll', 'amdvlk64.dll') } | Sort-Object cpu_seconds -Descending)
        $phases = [System.Collections.Generic.List[object]]::new()
        if ($result.summary.cpu_core_percent -ge 50 -and $driverThreads.Count -gt 0) {
            $driverThreadId = $driverThreads[0].id
            if ($SampleDriverInstruction) {
                if (-not ('ArtisanThreadContext' -as [type])) {
                    Add-Type -Path (Join-Path $PSScriptRoot 'windows-thread-context.cs')
                }
                $instructions = [System.Collections.Generic.List[object]]::new()
                for ($index = 0; $index -lt $InstructionSamples; $index++) {
                    $child.Refresh()
                    if ($child.HasExited) { throw 'Owned child exited before instruction sampling.' }
                    $sample = [ArtisanThreadContext]::Read($child.Id, $driverThreadId)
                    $address = [Convert]::ToInt64($sample.instruction_address.Substring(2), 16)
                    $moduleName = $null
                    $moduleOffset = $null
                    foreach ($module in $initialMemory.modules) {
                        $base = [Convert]::ToInt64($module.base_address.Substring(2), 16)
                        if ($address -ge $base -and $address -lt $base + $module.virtual_bytes) {
                            $moduleName = $module.name
                            $moduleOffset = '0x' + ($address - $base).ToString('x')
                            break
                        }
                    }
                    $instructions.Add([ordered]@{
                        sample = $sample
                        module = $moduleName
                        module_offset = $moduleOffset
                    })
                    Start-Sleep -Milliseconds 25
                }
                [ordered]@{
                    scope = 'Post-measurement diagnosis only. Each sample briefly suspends this owned child thread, reads control/integer registers plus one word each at RSP and R8 if readable, and resumes it in finally. No registers or process memory are written.'
                    process_id = $child.Id
                    thread_id = $driverThreadId
                    samples = $instructions.ToArray()
                } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $artifactDirectory 'driver-instruction-samples.json')
            }
            $interventions = @()
            if ($DiagnoseDriverSpin) { $interventions = @('repaint', 'minimize-restore') }
            foreach ($phase in $interventions) {
                $child.Refresh()
                if ($child.HasExited) { throw 'Owned child exited before its diagnostic intervention.' }
                if ($phase -eq 'repaint') {
                    [IdleWindowSizer]::Repaint($child.MainWindowHandle)
                } else {
                    [IdleWindowSizer]::MinimizeWithoutActivation($child.MainWindowHandle)
                    Start-Sleep -Seconds 1
                    [IdleWindowSizer]::RestoreWithoutActivation($child.MainWindowHandle)
                }
                Start-Sleep -Seconds 3
                $child.Refresh()
                $before = @($child.Threads | Where-Object { $_.Id -eq $driverThreadId } | ForEach-Object { $_.TotalProcessorTime.TotalSeconds })
                $watch = [Diagnostics.Stopwatch]::StartNew()
                $phaseFile = Join-Path $artifactDirectory ($phase + '-measurement.json')
                & (Join-Path $PSScriptRoot 'measure-idle.ps1') -ProcessId $child.Id -Seconds 20 -Scenario ($Scenario + '-after-' + $phase) -Build $Build -OutputPath $phaseFile
                $child.Refresh()
                $after = @($child.Threads | Where-Object { $_.Id -eq $driverThreadId } | ForEach-Object { $_.TotalProcessorTime.TotalSeconds })
                $elapsed = $watch.Elapsed.TotalSeconds
                $phaseResult = Get-Content -Raw -LiteralPath $phaseFile | ConvertFrom-Json
                & (Join-Path $PSScriptRoot 'inspect-memory.ps1') -ProcessId $child.Id -Scenario ($Scenario + '-after-' + $phase) -OutputPath (Join-Path $artifactDirectory ($phase + '-memory.json'))
                $phases.Add([ordered]@{
                    intervention = $phase
                    thread_id = $driverThreadId
                    thread_cpu_before_seconds = $before
                    thread_cpu_after_seconds = $after
                    thread_interval_seconds = $elapsed
                    summary = $phaseResult.summary
                    qualification = $phaseResult.qualification
                })
                if ($phaseResult.summary.cpu_core_percent -lt 50) { break }
            }
        }
        [ordered]@{
            scope = 'Diagnostic interventions on this runner-owned child only, after the initial unchanged sample. These phases are not acceptance measurements.'
            initial_cpu_core_percent = $result.summary.cpu_core_percent
            initial_amd_threads = $driverThreads
            phases = $phases.ToArray()
        } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $artifactDirectory 'driver-spin-diagnostics.json')
    }
    $context = [ordered]@{
        binary_sha256 = $hash
        binary_bytes = (Get-Item -LiteralPath $binary).Length
        source_executable = $Executable
        executable_copy = $binary
        editor_data = $dataDirectory
        scenario = $Scenario
        build = $Build
        requested_wgpu_backend = $Backend
        direct_composition_disabled = [bool]$DisableDirectComposition
        warmup_seconds = $WarmupSeconds
        requested_client_logical_px = @($ClientWidth, $ClientHeight)
        window_resized_at_startup = $resizedAtStartup
        completed_conversation_model_verified = [bool]$RequireCompletedConversation
        visible_conversation_verified = $false
        visual_qualification = 'Model log checks do not prove visible content. Inspect post-measurement pixels separately.'
        renderer_diagnostics = [bool]$RendererDiagnostics
        external_window_capture_requested = [bool]$CaptureWindow
        diagnostic_interventions_requested = [bool]$DiagnoseDriverSpin
        driver_instruction_sampling_requested = [bool]$SampleDriverInstruction
        fixture_editor_data = $FixtureEditorData
    }
    $context | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $artifactDirectory 'context.json')
    $result.summary | ConvertTo-Json
    $result.qualification | ConvertTo-Json -Depth 3
} finally {
    $env:ARTISAN_EDITOR_DATA = $oldData
    $env:RUST_LOG = $oldLog
    $env:GPUI_RENDERER_DIAGNOSTICS = $oldDiagnostics
    $env:WGPU_BACKEND = $oldBackend
    $env:GPUI_DISABLE_DIRECT_COMPOSITION = $oldDirectComposition
    $env:ARTISAN_EDITOR_SCRIPT = $oldScript
    $env:ARTISAN_EDITOR_CAPTURE = $oldCapture
    $env:ARTISAN_EDITOR_WINDOW_ORIGIN = $oldOrigin
    if ($null -ne $child) {
        $child.Refresh()
        if (-not $child.HasExited) {
            [void]$child.CloseMainWindow()
            if (-not $child.WaitForExit(5000)) { $child.Kill(); $child.WaitForExit() }
        }
        Write-Host "Closed isolated Editor PID $($child.Id)"
    }
    $logPath = Join-Path $dataDirectory 'editor.log'
    if (Test-Path -LiteralPath $logPath) {
        Copy-Item -LiteralPath $logPath -Destination (Join-Path $artifactDirectory 'editor.log')
    }
}
