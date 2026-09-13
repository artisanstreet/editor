# verify_visual.ps1 - the only acceptance gate that matters.
# Builds the editor, launches it, waits for the window, captures it to PNG.
# Output: evidence\<name>-<timestamp>.png  - attach this path to your report.
# A task without a capture from THIS script is rejected. Agent claims are not evidence.
param(
  [string]$Name = "task",
  [switch]$KeepAlive,
  [string]$ExePath,  # optional: launch a prebuilt editor
  [string]$HostHome,
  [switch]$OpenMachines,
  [switch]$VerifyWslPicker,
  [switch]$VerifyMachineSwitch  # click Ubuntu and back in the owned window
)
$ErrorActionPreference = 'Stop'
$verificationFailure = $null
if ($VerifyWslPicker) { $VerifyMachineSwitch = $true }
# UI acceptance uses the optimized dev profile; ordinary debug builds are for debugging.
if ($ExePath -match '[\\/]debug[\\/]') {
  Write-Warning 'Unoptimized debug executable: use target/performance/editor.exe for FPS and interaction evaluation.'
}
if (($HostHome -or $OpenMachines -or $VerifyMachineSwitch) -and -not $ExePath) { throw 'Host capture requires -ExePath' }
$repo = $PSScriptRoot | Split-Path   # script lives in <repo>\scripts\
New-Item -ItemType Directory -Force -Path (Join-Path $repo 'evidence') | Out-Null
Set-Location $repo

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -ErrorAction SilentlyContinue @'
using System;
using System.Runtime.InteropServices;
public class CapNative {
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
# Measure and capture physical pixels on scaled Windows displays.
[CapNative]::SetThreadDpiAwarenessContext([IntPtr]::new(-4)) | Out-Null

$proc = $null
$win = $null
$machineReceipt = $null
$previousReceipt = $env:ARTISAN_DEV_STARTUP_RECEIPT
try {
if ($VerifyMachineSwitch) {
  if ($HostHome -or $OpenMachines) { throw 'Machine interaction verification starts with the default host and a closed dropdown' }
  $machineReceipt = Join-Path ([System.IO.Path]::GetTempPath()) ('artisan-machine-' + [guid]::NewGuid() + '.json')
  $env:ARTISAN_DEV_STARTUP_RECEIPT = $machineReceipt
}
if ($ExePath) {
  $launch = @{ FilePath = $ExePath; PassThru = $true }
  $arguments = @()
  if ($HostHome) {
    if ($HostHome.Contains('"')) { throw 'Invalid host home path' }
    $arguments += '--host-home', ('"{0}"' -f $HostHome.TrimEnd('\'))
  }
  if ($OpenMachines) { $arguments += '--machines' }
  if ($arguments.Count -gt 0) { $launch.ArgumentList = $arguments }
  $proc = Start-Process @launch
} else {
  # build + run in one shot; cargo streams build output to stderr, window appears when ready
  $proc = Start-Process -FilePath 'cargo' -ArgumentList 'run','--locked','--profile','performance','-p','artisan-frontend','--bin','editor' -WorkingDirectory $repo -PassThru
}

# wait for the window (up to 8 min for cold builds)
$deadline = (Get-Date).AddMinutes(8)
$win = $null
while ((Get-Date) -lt $deadline) {
  Start-Sleep -Seconds 2
  $win = Get-Process | Where-Object {
    $_.MainWindowTitle -match '^Artisan' -and $_.MainWindowHandle -ne 0 -and
    (-not $ExePath -or $_.Id -eq $proc.Id)
  } | Select-Object -First 1
  if ($win) {
    # The window can start minimized (rect parked at -25600,-25600, 159px
    # wide); restore it before probing so the rect reflects real size.
    [CapNative]::ShowWindow($win.MainWindowHandle, 9) | Out-Null
    Start-Sleep -Milliseconds 250
    # Get-Process .MainWindowWidth reads 0/empty on this host; use the
    # Win32 rect the script already imports for the real dimensions.
    $probe = New-Object CapNative+RECT
    [CapNative]::GetWindowRect($win.MainWindowHandle, [ref]$probe) | Out-Null
    if (($probe.Right - $probe.Left) -gt 300) { break }
  }
  $win = $null
}
if (-not $win) { Write-Error "No Artisan window appeared within 8 minutes. Build failed or app crashed."; if (-not $KeepAlive) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }; exit 1 }

[CapNative]::ShowWindow($win.MainWindowHandle, 9) | Out-Null
[CapNative]::SetForegroundWindow($win.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 900   # let it settle/paint

if ($VerifyMachineSwitch) {
  try {
    & (Join-Path $PSScriptRoot 'verify_machine_switch.ps1') -Process $win -Receipt $machineReceipt
    if ($VerifyWslPicker) { & (Join-Path $PSScriptRoot 'verify_wsl_picker.ps1') -Process $win }
  } catch {
    $verificationFailure = $_
    Write-Warning "Interaction failed; capturing the owned window before cleanup."
  }
}

$r = New-Object CapNative+RECT
[CapNative]::GetWindowRect($win.MainWindowHandle, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
if ($w -lt 300 -or $h -lt 300) { Write-Error "Window degenerate (${w}x${h}) - not a real render."; exit 1 }

$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
# CopyFromScreen intermittently returns black for the GPU-composited D3D
# window; PrintWindow with PW_RENDERFULLCONTENT reads the surface directly.
$hdc = $g.GetHdc()
$printed = [CapNative]::PrintWindow($win.MainWindowHandle, $hdc, 2)
$g.ReleaseHdc($hdc)
if (-not $printed) {
  $g.Dispose(); $bmp.Dispose()
  throw "Could not capture the owned editor surface"
}
$out = Join-Path $repo "evidence\$Name-$(Get-Date -Format 'yyyyMMdd-HHmmss').png"
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()

Write-Output "CAPTURE: $out"
Write-Output "WINDOW:  $($win.MainWindowTitle) ${w}x${h} pid=$($win.Id)"
if ($verificationFailure) { throw $verificationFailure }

} finally {
  $env:ARTISAN_DEV_STARTUP_RECEIPT = $previousReceipt
  if (-not $KeepAlive) {
    if ($win -and -not $win.HasExited) {
      [void]$win.CloseMainWindow()
      if (-not $win.WaitForExit(15000)) { Stop-Process -Id $win.Id -Force -ErrorAction SilentlyContinue }
    } elseif ($proc -and -not $proc.HasExited) {
      Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
  }
  if ($machineReceipt) { Remove-Item -LiteralPath $machineReceipt -Force -ErrorAction SilentlyContinue }
}
