# Operates only on the window created by verify_visual.ps1 after Ubuntu connects.
param([Parameter(Mandatory=$true)][System.Diagnostics.Process]$Process)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
# A second editor must report contention without disturbing the connected window.
$hostDir = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'artisan\hosts') -Directory | Sort-Object LastWriteTime -Descending | Select-Object -First 1
$probePreference = $ErrorActionPreference
try {
  $ErrorActionPreference = 'Continue'
  $probe = (& $Process.Path --host-home $hostDir.FullName --probe-host 2>&1 | Out-String)
} finally { $ErrorActionPreference = $probePreference }
if ($probe -notmatch 'ConnectionBusy') { throw 'Second connection did not report the exclusive host lease clearly' }
Write-Output 'CONNECTION: second client reports ConnectionBusy without taking over the host.'
$editorHandle = $Process.MainWindowHandle
# Close the nested host menu and its parent, then use New thread with no project selected.
foreach ($key in @(27, 27)) {
  [void][MachineInput]::PostMessage($editorHandle, 0x0100, [IntPtr]::new($key), [IntPtr]::Zero)
  [void][MachineInput]::PostMessage($editorHandle, 0x0101, [IntPtr]::new($key), [IntPtr]::Zero)
  Start-Sleep -Milliseconds 250
}
$scale = [MachineInput]::GetDpiForWindow($editorHandle) / 96.0
$point = [IntPtr](([int](76 * $scale) -shl 16) -bor [int](65 * $scale))
[void][MachineInput]::PostMessage($editorHandle, 0x0200, [IntPtr]::Zero, $point)
[void][MachineInput]::PostMessage($editorHandle, 0x0201, [IntPtr]::new(1), $point)
[void][MachineInput]::PostMessage($editorHandle, 0x0202, [IntPtr]::Zero, $point)
$condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ProcessIdProperty, $Process.Id)
$deadline = (Get-Date).AddSeconds(20)
$dialog = $null
while (-not $dialog -and (Get-Date) -lt $deadline) {
  $windows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
  foreach ($window in $windows) {
    if ($window.Current.Name -eq 'Choose a project folder in WSL') { $dialog = $window; break }
  }
  if (-not $dialog) { Start-Sleep -Milliseconds 250 }
}
if (-not $dialog) { throw 'Native Windows WSL project picker did not open' }
try {
  $items = $dialog.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  $names = @($items | ForEach-Object { $_.Current.Name })
  if (-not ($names | Where-Object { $_ -match 'Ubuntu' })) { throw 'Picker did not start in the Ubuntu share' }
  $linuxHome = (& "$env:SystemRoot\System32\wsl.exe" --distribution Ubuntu --exec printenv HOME | Out-String).Trim()
  if ($LASTEXITCODE -ne 0 -or -not $linuxHome.StartsWith('/')) { throw 'Could not resolve Ubuntu home for verification' }
  $homeLeaf = ($linuxHome.TrimEnd('/') -split '/')[-1]
  if (-not ($names | Where-Object { $_ -eq $homeLeaf -or $_ -like "*$linuxHome*" })) { throw "Picker did not start in the Ubuntu user home: $linuxHome" }
  Write-Output "PICKER: native Windows dialog opened at Ubuntu home $linuxHome."
  $dialogHandle = [IntPtr]::new($dialog.Current.NativeWindowHandle)
  $rect = New-Object CapNative+RECT
  [void][CapNative]::GetWindowRect($dialogHandle, [ref]$rect)
  $bitmap = New-Object Drawing.Bitmap(($rect.Right-$rect.Left), ($rect.Bottom-$rect.Top))
  $graphics = [Drawing.Graphics]::FromImage($bitmap)
  $dc = $graphics.GetHdc()
  $captured = [CapNative]::PrintWindow($dialogHandle, $dc, 2)
  $graphics.ReleaseHdc($dc)
  if ($captured) {
    $capture = Join-Path (Split-Path $PSScriptRoot) ('evidence\wsl-project-picker-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.png')
    $bitmap.Save($capture, [Drawing.Imaging.ImageFormat]::Png)
    Write-Output "PICKER_CAPTURE: $capture"
  }
  $graphics.Dispose(); $bitmap.Dispose()

} finally {
  # Closing the owned common dialog maps to cancellation without selecting a folder.
  [void][MachineInput]::PostMessage([IntPtr]::new($dialog.Current.NativeWindowHandle), 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
  $closedDeadline = (Get-Date).AddSeconds(5)
  do {
    Start-Sleep -Milliseconds 100
    $remaining = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
    $open = @($remaining | Where-Object { $_.Current.Name -eq 'Choose a project folder in WSL' }).Count -gt 0
  } while ($open -and (Get-Date) -lt $closedDeadline)
  if ($open) { throw 'WSL folder picker did not cancel cleanly' }
  Write-Output 'PICKER: cancelled without selecting or attaching a directory.'
}
