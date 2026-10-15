# Run capture-feature synthetic input inside one isolated Editor child.
# Script paths may contain {{CAPTURE_DIRECTORY}}, replaced with this run's output.
# This does not send global input or manage any Forge service.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][string]$Script,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$FixtureEditorData,
    [string]$Link,
    [ValidateSet('default', 'dx12', 'vulkan')][string]$Backend = 'default',
    [ValidateRange(1, 3600)][int]$TimeoutSeconds = 120
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
function Get-CaptureSha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $hasher = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($hasher.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $hasher.Dispose() }
}
$artifactDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if ((Test-Path -LiteralPath $artifactDirectory) -and
    @(Get-ChildItem -LiteralPath $artifactDirectory -Force).Count -gt 0) {
    throw 'OutputDirectory must be empty.'
}
New-Item -ItemType Directory -Path $artifactDirectory -Force | Out-Null
$runDirectory = Join-Path $env:TEMP ('artisan-capture-' + [Guid]::NewGuid().ToString('N'))
$dataDirectory = Join-Path $runDirectory 'editor'
$imageDirectory = Join-Path $runDirectory 'images'
New-Item -ItemType Directory -Path $dataDirectory, $imageDirectory | Out-Null
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
$hash = Get-CaptureSha256 $binary
$scriptText = [IO.File]::ReadAllText($Script).Replace('{{CAPTURE_DIRECTORY}}', $imageDirectory)
$runScript = Join-Path $runDirectory 'capture.script'
[IO.File]::WriteAllText($runScript, $scriptText, [Text.UTF8Encoding]::new($false))
Copy-Item -LiteralPath $runScript -Destination (Join-Path $artifactDirectory 'capture.script')
$environmentNames = @('ARTISAN_EDITOR_DATA', 'ARTISAN_EDITOR_SCRIPT', 'ARTISAN_EDITOR_CAPTURE',
    'ARTISAN_EDITOR_WINDOW_ORIGIN', 'RUST_LOG', 'RUST_BACKTRACE', 'GPUI_RENDERER_DIAGNOSTICS', 'WGPU_BACKEND')
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$child = $null
try {
    $env:ARTISAN_EDITOR_DATA = $dataDirectory
    $env:ARTISAN_EDITOR_SCRIPT = $runScript
    $env:ARTISAN_EDITOR_CAPTURE = $null
    $env:ARTISAN_EDITOR_WINDOW_ORIGIN = $null
    $env:RUST_LOG = 'info,gpui=warn,gpui::renderer_memory=info'
    $env:RUST_BACKTRACE = '1'
    $env:GPUI_RENDERER_DIAGNOSTICS = '1'
    if ($Backend -eq 'default') { $env:WGPU_BACKEND = $null }
    else { $env:WGPU_BACKEND = $Backend }
    $stderrPath = Join-Path $runDirectory 'stderr.txt'
    if ($Link) { $child = Start-Process -FilePath $binary -ArgumentList $Link -RedirectStandardError $stderrPath -PassThru }
    else { $child = Start-Process -FilePath $binary -RedirectStandardError $stderrPath -PassThru }
    # Keep the native handle open so ExitCode remains available after a short-lived child exits.
    $null = $child.Handle
    [ordered]@{
        launched_at_utc = [DateTime]::UtcNow.ToString('o')
        pid = $child.Id
        binary_sha256 = $hash
        source_executable = $Executable
        executable_copy = $binary
        editor_data = $dataDirectory
        fixture_editor_data = $FixtureEditorData
        source_script = $Script
        script_sha256 = Get-CaptureSha256 $runScript
        requested_wgpu_backend = $Backend
        input_scope = 'Synthetic dispatch inside this child only; no global mouse or keyboard input.'
        measurement_scope = 'Visual validation; capture readback changes memory and is not an idle benchmark.'
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $artifactDirectory 'context.json')
    Write-Host "Capturing isolated Editor PID $($child.Id), artifacts $artifactDirectory"
    if (-not $child.WaitForExit($TimeoutSeconds * 1000)) {
        throw "Capture child exceeded $TimeoutSeconds seconds."
    }
    $child.WaitForExit()
    if ($child.ExitCode -ne 0) { throw "Capture child exited with code $($child.ExitCode)." }
    $log = [IO.File]::ReadAllText((Join-Path $dataDirectory 'editor.log'))
    if ($log -match 'script step failed|couldn.t read the script|couldn.t open the window') {
        throw 'Capture script reported a failure; inspect editor.log.'
    }
    if ($log -notmatch 'script finished') { throw 'Capture script did not finish.' }
    $expectedCaptures = @($scriptText -split '\r?\n' | Where-Object { $_ -match '^\s*capture\s+' }).Count
    $actualCaptures = @(Get-ChildItem -LiteralPath $imageDirectory -File -Filter '*.png').Count
    if ($actualCaptures -ne $expectedCaptures) {
        throw "Expected $expectedCaptures PNG captures; found $actualCaptures."
    }
    Write-Host "Captured $actualCaptures images."
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
    if ($null -ne $child) {
        $child.Refresh()
        if (-not $child.HasExited) {
            [void]$child.CloseMainWindow()
            if (-not $child.WaitForExit(5000)) { $child.Kill(); $child.WaitForExit() }
        }
        Write-Host "Closed isolated Editor PID $($child.Id)"
        [ordered]@{
            pid = $child.Id
            exit_code = $child.ExitCode
            finished_at_utc = [DateTime]::UtcNow.ToString('o')
        } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $artifactDirectory 'process-result.json')
    }
    $logPath = Join-Path $dataDirectory 'editor.log'
    if (Test-Path -LiteralPath $logPath) {
        Copy-Item -LiteralPath $logPath -Destination (Join-Path $artifactDirectory 'editor.log')
    }
    $stderrPath = Join-Path $runDirectory 'stderr.txt'
    if (Test-Path -LiteralPath $stderrPath) {
        Copy-Item -LiteralPath $stderrPath -Destination (Join-Path $artifactDirectory 'stderr.txt')
    }
    Get-ChildItem -LiteralPath $imageDirectory -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $artifactDirectory
    }
}
