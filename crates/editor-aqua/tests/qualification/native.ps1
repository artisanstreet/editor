param(
  [Parameter(Mandatory=$true)][string]$Exe,
  [Parameter(Mandatory=$true)][string]$Request,
  [Parameter(Mandatory=$true)][string]$Out,
  [string]$Steps = "",
  [int]$BootSeconds = 8
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class QualificationWindow {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public delegate bool EnumProc(IntPtr h, IntPtr data);
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr data);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out Rect rect);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect rect);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref Point point);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int height, uint flags);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint message, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  public static IntPtr Find(uint wanted) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, data) => { uint pid; GetWindowThreadProcessId(h, out pid); if(pid != wanted) return true; found = h; return false; }, IntPtr.Zero);
    return found;
  }
  public static void Capture(IntPtr h, string path) {
    Rect client, outer; GetClientRect(h, out client); GetWindowRect(h, out outer);
    var origin = new Point(); ClientToScreen(h, ref origin);
    using(var image = new Bitmap(outer.Right - outer.Left, outer.Bottom - outer.Top, PixelFormat.Format32bppArgb)) {
      using(var graphics = Graphics.FromImage(image)) {
        IntPtr dc = graphics.GetHdc();
        bool captured = PrintWindow(h, dc, 2);
        graphics.ReleaseHdc(dc);
        if(!captured) throw new InvalidOperationException("PrintWindow failed");
      }
      using(var area = image.Clone(new Rectangle(origin.X - outer.Left, origin.Y - outer.Top, client.Right, client.Bottom), PixelFormat.Format32bppArgb)) area.Save(path, ImageFormat.Png);
    }
  }

}
"@
[void][QualificationWindow]::SetProcessDPIAware()
$request_path = (Resolve-Path -LiteralPath $Request).ProviderPath
$request_value = Get-Content -Raw -LiteralPath $request_path | ConvertFrom-Json
$editor_dir = $request_value.editor_dir
if (-not $editor_dir -or -not (Test-Path -LiteralPath (Join-Path $editor_dir ".artisan-qualification"))) {
  throw "A marked isolated qualification profile is required"
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$output_dir = (Resolve-Path -LiteralPath $Out).ProviderPath
$run_dir = Join-Path $env:TEMP ("artisan-native-qualification-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $run_dir | Out-Null
$binary = Join-Path $run_dir "artisan-editor.exe"
Copy-Item -LiteralPath $Exe -Destination $binary
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $binary
$start.Arguments = "--qualify-editor"
$start.WorkingDirectory = $run_dir
$start.UseShellExecute = $false
$start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.EnvironmentVariables["ARTISAN_EDITOR_QUALIFICATION"] = $request_path
$start.EnvironmentVariables["ARTISAN_EDITOR_DATA"] = $editor_dir
$start.EnvironmentVariables["AQUA_RENDER_WHEN_COVERED"] = "1"
$state_path = Join-Path $output_dir "native-state.json"
$start.EnvironmentVariables["ARTISAN_AQUA_QUALIFICATION_STATE"] = $state_path
foreach ($name in @("ARTISAN_EDITOR_SCRIPT", "ARTISAN_EDITOR_CAPTURE", "ARTISAN_AQUA_SERVE_EDITOR_DIR")) {
  $start.EnvironmentVariables.Remove($name)
}
$foreground = [QualificationWindow]::GetForegroundWindow()
$process = [Diagnostics.Process]::Start($start)
$stdout = $process.StandardOutput.ReadToEndAsync()
$stderr = $process.StandardError.ReadToEndAsync()
try {
  $window = [IntPtr]::Zero
  $deadline = (Get-Date).AddSeconds(45)
  while ($window -eq [IntPtr]::Zero -and (Get-Date) -lt $deadline -and -not $process.HasExited) {
    $window = [QualificationWindow]::Find([uint32]$process.Id)
    Start-Sleep -Milliseconds 50
  }
  if ($window -eq [IntPtr]::Zero) { throw "No qualification window appeared" }
  function AssertBackground {
    $active = [QualificationWindow]::GetForegroundWindow()
    $active_pid = [uint32]0
    [void][QualificationWindow]::GetWindowThreadProcessId($active, [ref]$active_pid)
    if ($active_pid -eq $process.Id) { throw "Qualification took foreground focus" }
  }
  AssertBackground
  [void][QualificationWindow]::SetWindowPos($window, [IntPtr]1, 40, 40, 0, 0, 0x0011)
  [void][QualificationWindow]::ShowWindow($window, 4)
  Start-Sleep -Seconds $BootSeconds
  AssertBackground
  $restored_minimized = [QualificationWindow]::IsIconic($window)
  if ($restored_minimized) {
    [void][QualificationWindow]::ShowWindow($window, 4)
    Start-Sleep -Milliseconds 250
    AssertBackground
    if ([QualificationWindow]::IsIconic($window)) { throw "Owned qualification window remained minimized after its single nonactivating restore" }
  }
  $scale = [QualificationWindow]::GetDpiForWindow($window) / 96.0
  $client = New-Object QualificationWindow+Rect
  $outer = New-Object QualificationWindow+Rect
  [void][QualificationWindow]::GetClientRect($window, [ref]$client)
  [void][QualificationWindow]::GetWindowRect($window, [ref]$outer)
  $width = $outer.Right - $outer.Left + [int][Math]::Round($request_value.width * $scale) - $client.Right
  $height = $outer.Bottom - $outer.Top + [int][Math]::Round($request_value.height * $scale) - $client.Bottom
  [void][QualificationWindow]::SetWindowPos($window, [IntPtr]1, 40, 40, $width, $height, 0x0010)
  Start-Sleep -Seconds 2
  [void][QualificationWindow]::GetClientRect($window, [ref]$client)
  function AssertGeometry {
    if ([QualificationWindow]::IsIconic($window)) { throw "Owned qualification window was minimized; native validation stopped" }
    $actual = New-Object QualificationWindow+Rect
    if (-not [QualificationWindow]::GetClientRect($window, [ref]$actual)) { throw "Owned qualification client geometry is unavailable" }
    $expected_width = [int][Math]::Round($request_value.width * $scale)
    $expected_height = [int][Math]::Round($request_value.height * $scale)
    if ($actual.Right -ne $expected_width -or $actual.Bottom -ne $expected_height) {
      throw "Owned client geometry is $($actual.Right)x$($actual.Bottom), expected $($expected_width)x$($expected_height)"
    }
  }
  AssertGeometry
  $closed = $false
  $verified_text = @()
  function PointParameter($x, $y) {
    $px = [int][Math]::Round([double]$x * $scale)
    $py = [int][Math]::Round([double]$y * $scale)
    [IntPtr](($py -shl 16) -bor ($px -band 0xffff))
  }
  function Post($message, $value, $point) {
    if (-not [QualificationWindow]::PostMessage($window, $message, [IntPtr]$value, $point)) { throw "Input delivery failed" }
  }
  function ReadShared($path) {
    $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    $reader = New-Object IO.StreamReader($stream)
    $text = $reader.ReadToEnd()
    $reader.Dispose()
    return $text
  }
  function FindVisibleElement($name) {
    $until = (Get-Date).AddSeconds(30)
    $last = $null
    $stable_since = Get-Date
    while ((Get-Date) -lt $until) {
      AssertBackground
      AssertGeometry
      $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
      $elements = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
      $matches = @($elements | Where-Object {
        -not $_.Current.IsOffscreen -and $_.Current.IsEnabled -and $_.Current.Name -like $name -and
        $_.Current.BoundingRectangle.Width -gt 0 -and $_.Current.BoundingRectangle.Height -gt 0
      })
      $candidate = $null
      if ($matches.Count -gt 1) {
        $interactive = @($matches | Where-Object {
          $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -or
          $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::MenuItem
        })
        if ($interactive.Count -ne 1) { throw "Ambiguous native selector: $name" }
        $candidate = $interactive[0]
      } elseif ($matches.Count -eq 1) {
        $candidate = $matches[0]
      }
      if ($candidate) {
        $signature = ($candidate.GetRuntimeId() -join ":") + ":" + $candidate.Current.BoundingRectangle.ToString()
        if ($signature -eq $last) {
          if (((Get-Date) - $stable_since).TotalMilliseconds -ge 400) { return $candidate }
        } else {
          $last = $signature
          $stable_since = Get-Date
        }
      } else {
        $last = $null
      }
      Start-Sleep -Milliseconds 100
    }
    @($elements | ForEach-Object { $_.Current.Name }) | ConvertTo-Json |
      Set-Content -LiteralPath (Join-Path $output_dir "native-visible-elements.json")
    throw "Visible native selector was not ready: $name"
  }
  foreach ($line in $Steps.Split(';')) {
    $parts = $line.Trim().Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
    if ($parts.Count -eq 0) { continue }
    AssertBackground
    AssertGeometry
    Add-Content -LiteralPath (Join-Path $output_dir "native-steps.log") -Value $line.Trim()
    switch ($parts[0]) {
      "wait-name" {
        $name = $line.Trim().Substring(10)
        $element = FindVisibleElement $name
        $verified_text += $element.Current.Name
      }
      "click-name" {
        $name = $line.Trim().Substring(11)
        $element = FindVisibleElement $name
        $bounds = $element.Current.BoundingRectangle
        $origin = New-Object QualificationWindow+Point
        [void][QualificationWindow]::ClientToScreen($window, [ref]$origin)
        $x = [int][Math]::Round($bounds.X + $bounds.Width / 2 - $origin.X)
        $y = [int][Math]::Round($bounds.Y + $bounds.Height / 2 - $origin.Y)
        if ($x -lt 0 -or $y -lt 0 -or $x -ge $client.Right -or $y -ge $client.Bottom) { throw "Native selector is outside the owned client: $name" }
        [ordered]@{ selector = $name; bounds = @($bounds.X, $bounds.Y, $bounds.Width, $bounds.Height); client_origin = @($origin.X, $origin.Y); physical_point = @($x, $y); scale = $scale } |
          ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $output_dir "native-inputs.jsonl")
        $point = [IntPtr](($y -shl 16) -bor ($x -band 0xffff))
        Post 0x200 0 $point
        Post 0x201 1 $point
        Post 0x202 0 $point
      }
      "wait-state" {
        $ready = $false
        $until = (Get-Date).AddSeconds(30)
        while (-not $ready -and (Get-Date) -lt $until) {
          AssertBackground
          AssertGeometry
          if (Test-Path -LiteralPath $state_path) {
            $text = ReadShared $state_path
            if ($text.EndsWith('}')) {
              $state = $text | ConvertFrom-Json
              $ready = $state.page_loaded -and -not $state.page_in_flight -and -not $state.load_error -and $state.row_count -gt 0 -and $state.measured_row_count -gt 0 -and -not $state.landing
            }
          }
          if (-not $ready) { Start-Sleep -Milliseconds 100 }
        }
        if (-not $ready) {
          [QualificationWindow]::Capture($window, (Join-Path $output_dir "not-ready.png"))
          throw "Fixture transcript did not reach a loaded, measured frame"
        }
      }
      "wait-text" {
        $expected = $line.Trim().Substring(10)
        $ready = $false
        $until = (Get-Date).AddSeconds(30)
        while (-not $ready -and (Get-Date) -lt $until) {
          AssertBackground
          AssertGeometry
          $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
          $elements = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
          foreach ($element in $elements) {
            $element_name = [string]$element.Current.Name
            if (-not $element.Current.IsOffscreen -and $element_name.Contains($expected)) { $ready = $true; break }
          }
          if (-not $ready) { Start-Sleep -Milliseconds 100 }
        }
        if (-not $ready) {
          @($elements | ForEach-Object { $_.Current.Name }) | ConvertTo-Json |
            Set-Content -LiteralPath (Join-Path $output_dir "native-visible-elements.json")
          [QualificationWindow]::Capture($window, (Join-Path $output_dir "not-ready.png"))
          throw "Visible fixture text was not ready: $expected"
        }
        $verified_text += $expected
      }
      "wait" { Start-Sleep -Milliseconds ([int]$parts[1]) }
      "click" {
        $point = PointParameter $parts[1] $parts[2]
        Post 0x200 0 $point
        Post 0x201 1 $point
        Post 0x202 0 $point
      }
      "type" { foreach ($character in $line.Trim().Substring(5).ToCharArray()) { Post 0x102 ([int]$character) ([IntPtr]0) } }
      "escape" { Post 0x100 27 ([IntPtr]0); Post 0x101 27 ([IntPtr]0) }
      "close" {
        Post 0x10 0 ([IntPtr]0)
        if (-not $process.WaitForExit(15000)) { throw "Orderly close timed out" }
        if ($process.ExitCode -ne 0) { throw "Orderly close failed: $($process.ExitCode)" }
        $closed = $true
      }
      "shot" {
        if ($parts[1] -notmatch '^[a-z0-9-]+$') { throw "Invalid screenshot name" }
        [QualificationWindow]::Capture($window, (Join-Path $output_dir ($parts[1] + ".png")))
        foreach ($record in @("native-state.json")) {
          $record_path = Join-Path $output_dir $record
          if (Test-Path -LiteralPath $record_path) { [IO.File]::WriteAllText((Join-Path $output_dir ($parts[1] + "-" + $record)), (ReadShared $record_path)) }
        }
      }
      default { throw "Unknown native step: $line" }
    }
  }
  if ($process.HasExited -and -not $closed) { throw "Qualification exited unexpectedly: $($process.ExitCode)" }
  $after = [QualificationWindow]::GetForegroundWindow()
  $foreground_pid = [uint32]0
  [void][QualificationWindow]::GetWindowThreadProcessId($after, [ref]$foreground_pid)
  AssertBackground
  [ordered]@{ pid = $process.Id; orderly_close = $closed; foreground_unchanged = ($foreground -eq $after); foreground_stolen = $false; restored_minimized = $restored_minimized; verified_visible_text = $verified_text; logical_width = $request_value.width; logical_height = $request_value.height; dpi = ($scale * 96); client_width = $client.Right; client_height = $client.Bottom; executable_sha256 = (Get-FileHash $binary -Algorithm SHA256).Hash; request = $request_path } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output_dir "native-result.json")
} finally {
  if (-not $process.HasExited) {
    $process.Kill(); $process.WaitForExit()
  }
  [IO.File]::WriteAllText((Join-Path $output_dir "native-stdout.log"), $stdout.Result)
  [IO.File]::WriteAllText((Join-Path $output_dir "native-stderr.log"), $stderr.Result)
  $process.Dispose()
  Remove-Item -Recurse -Force -LiteralPath $run_dir
}
