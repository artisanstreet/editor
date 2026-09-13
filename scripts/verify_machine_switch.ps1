# Interaction check invoked by verify_visual.ps1 against only the process it launched.
# Requires a registered Ubuntu host. All mouse messages target that editor HWND;
# no global cursor movement or input is sent to the user's other windows.
param(
  [Parameter(Mandatory=$true)][System.Diagnostics.Process]$Process,
  [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class MachineInput {
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint message, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint message, IntPtr w, IntPtr l);
  public struct POINT { public int X, Y; }
}
'@
$editorHandle = $Process.MainWindowHandle
$machineScale = [MachineInput]::GetDpiForWindow($editorHandle) / 96.0
function Invoke-MachineClick([double]$X, [double]$Y) {
  $inputWatch = [System.Diagnostics.Stopwatch]::StartNew()
  $clientX = [int]($X * $machineScale)
  $clientY = [int]($Y * $machineScale)
  $screen = New-Object MachineInput+POINT
  $screen.X = $clientX; $screen.Y = $clientY
  [void][MachineInput]::ClientToScreen($editorHandle, [ref]$screen)
  $screenPoint = [IntPtr](($screen.Y -shl 16) -bor ($screen.X -band 65535))
  $hit = [MachineInput]::SendMessage($editorHandle, 0x0084, [IntPtr]::Zero, $screenPoint)
  if ($hit.ToInt32() -ne 1) { throw "Machine control is not a Windows client hit area: $hit at $X,$Y" }
  $point = [IntPtr](($clientY -shl 16) -bor ($clientX -band 65535))
  [void][MachineInput]::SendMessage($editorHandle, 0x0200, [IntPtr]::Zero, $point)
  [void][MachineInput]::SendMessage($editorHandle, 0x0201, [IntPtr]::new(1), $point)
  [void][MachineInput]::SendMessage($editorHandle, 0x0202, [IntPtr]::Zero, $point)
  $inputWatch.Stop()
  Write-Output "INPUT: click handled in $($inputWatch.ElapsedMilliseconds) ms"
  if ($inputWatch.ElapsedMilliseconds -gt 1500) { throw "Editor blocked while processing menu input" }
  Start-Sleep -Milliseconds 500
}
function Invoke-MachineKey([int]$Key) {
  # Keyboard accelerators run in the native message pump, so enqueue these messages.
  [void][MachineInput]::PostMessage($editorHandle, 0x0100, [IntPtr]::new($Key), [IntPtr]::Zero)
  [void][MachineInput]::PostMessage($editorHandle, 0x0101, [IntPtr]::new($Key), [IntPtr]::Zero)
  Start-Sleep -Milliseconds 350
}
function Open-HostSelect {
  # Profile header's ghost select is the first tab stop from the profile trigger.
  Invoke-MachineKey 9
  Invoke-MachineKey 13
}
function Wait-MachineTitle([string]$Name) {
  $titleDeadline = (Get-Date).AddSeconds(10)
  do {
    $Process.Refresh()
    if ($Process.MainWindowHandle -ne $editorHandle) { throw 'Machine selection changed the window handle' }
    if ($Process.MainWindowTitle.EndsWith($Name)) { Write-Output "SELECTED: $Name"; return }
    Start-Sleep -Milliseconds 100
  } while ((Get-Date) -lt $titleDeadline)
  throw "Machine selection did not select $Name in the same window"
}
$readyDeadline = (Get-Date).AddSeconds(45)
while (-not (Test-Path -LiteralPath $Receipt) -and (Get-Date) -lt $readyDeadline) {
  Start-Sleep -Milliseconds 100
}
if (-not (Test-Path -LiteralPath $Receipt)) { throw 'Initial local service did not settle' }
Remove-Item -LiteralPath $Receipt -Force
$clientRect = New-Object MachineInput+RECT
[void][MachineInput]::GetClientRect($editorHandle, [ref]$clientRect)
Invoke-MachineClick 50 (($clientRect.Bottom / $machineScale) - 24)
Open-HostSelect
Invoke-MachineKey 36
Invoke-MachineKey 40
Invoke-MachineKey 13
Wait-MachineTitle 'Ubuntu'
$readyDeadline = (Get-Date).AddSeconds(45)
do {
  if (Test-Path -LiteralPath $Receipt) {
    $observed = Get-Content -LiteralPath $Receipt -Raw | ConvertFrom-Json
    if ($observed.status -eq 'ready') { break }
  }
  Start-Sleep -Milliseconds 100
} while ((Get-Date) -lt $readyDeadline)
if (-not $observed -or $observed.status -ne 'ready') { throw 'Selected Ubuntu did not complete authenticated initial queries' }
# Exercise the return path through the real dropdown after selecting the local host.
Remove-Item -LiteralPath $Receipt -Force
Open-HostSelect
Invoke-MachineKey 36
Invoke-MachineKey 13
Wait-MachineTitle 'This computer'
Open-HostSelect
Invoke-MachineKey 36
Invoke-MachineKey 40
Invoke-MachineKey 13
Wait-MachineTitle 'Ubuntu'
Open-HostSelect
Write-Output "INTERACTION: Native profile click, ghost selector keyboard navigation, Ubuntu authentication, and same-HWND switching passed (pid=$($Process.Id))."
