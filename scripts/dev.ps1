<#
.SYNOPSIS
    One-command native development workflow on Windows (developer convenience).

.DESCRIPTION
    Builds the four product binaries plus the native dev runner in one
    `cargo build --locked --jobs=1` invocation, then runs the same
    Cargo native development runner, pointed at the repo .dist/dev installation.
    A checkout on a network share (such as \\wsl.localhost) builds and stages
    under %LOCALAPPDATA%\Artisan Street Dev\build\<checkout-id> instead.

.EXAMPLE
    scripts/dev.ps1
    scripts/dev.ps1 -Performance
    scripts/dev.ps1 -Release -StageOnly
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$Performance,
    [switch]$StageOnly,
    [string]$DevDir = ""
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
if ($Release -and $Performance) {
    throw "Choose either -Release or -Performance"
}
$config = if ($Release) { "release" } elseif ($Performance) { "performance" } else { "debug" }

function Write-DevStep($message) {
    Write-Host "dev.ps1: $message"
}

# A checkout reached over a network share (for example \\wsl.localhost\...)
# cannot host Cargo's target directory (build-script hardlinks fail with
# "Access is denied") or the dev staging lock (byte-range locks fail). Build
# and stage on local disk instead, keyed per checkout so worktrees never share.
function Get-LocalBuildRoot {
    $key = $repo.ToLowerInvariant()
    $sha = [Security.Cryptography.SHA256]::Create()
    $digest = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($key))
    $id = (-join ($digest | ForEach-Object { $_.ToString("x2") })).Substring(0, 12)
    return Join-Path $env:LOCALAPPDATA "Artisan Street Dev\build\$id"
}

$onShare = $repo.StartsWith("\\")
if ($onShare) {
    $localRoot = Get-LocalBuildRoot
    if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        $env:CARGO_TARGET_DIR = Join-Path $localRoot "target"
        Write-DevStep "checkout is on a network share; building in $env:CARGO_TARGET_DIR"
    }
    if ([string]::IsNullOrWhiteSpace($DevDir)) {
        $DevDir = Join-Path $localRoot "dev"
    }
}
if ([string]::IsNullOrWhiteSpace($DevDir)) {
    $DevDir = Join-Path $repo ".dist/dev"
}

# Release installer builds refuse to compile without a trust anchor
# (modules/installer/RELEASE_TRUST.md). Local release builds use the
# checked-in pre-release anchor unless the caller exported a real one.
function Import-ReleaseTrustAnchor {
    $anchor = Join-Path $repo "modules/installer/release/trust_anchor.env"
    foreach ($line in Get-Content $anchor) {
        if ($line -match '^\s*(ARTISAN_RELEASE_(KEY_ID|PUBLIC_KEY_HEX))\s*=\s*(.+?)\s*$') {
            if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($Matches[1]))) {
                [Environment]::SetEnvironmentVariable($Matches[1], $Matches[3].Trim('"'), "Process")
            }
        }
    }
    Write-DevStep "release trust anchor: $env:ARTISAN_RELEASE_KEY_ID"
}

function Ensure-VsEnvironment {
    if (Get-Command cl.exe -ErrorAction SilentlyContinue) {
        Write-DevStep "using current Visual Studio environment"
        return
    }
    $vswhere = Join-Path ${Env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        throw "cl.exe is not on PATH and vswhere was not found; run from a Visual Studio developer console"
    }
    $install = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ([string]::IsNullOrWhiteSpace($install)) {
        throw "no Visual Studio installation with C++ tools found via vswhere"
    }
    $vcvars = Join-Path $install "VC/Auxiliary/Build/vcvars64.bat"
    if (-not (Test-Path $vcvars)) {
        throw "vcvars64.bat not found at $vcvars"
    }
    Write-DevStep "importing environment from $vcvars"
    # stderr (e.g. cmd's UNC working-directory warning) arrives as ErrorRecords;
    # keep only the `set` output lines.
    $envDump = cmd /c "`"$vcvars`" >nul && set" 2>&1 | Where-Object { $_ -is [string] }
    if ($LASTEXITCODE -ne 0) {
        throw "vcvars64.bat failed with exit $LASTEXITCODE"
    }
    foreach ($line in $envDump) {
        $split = $line.IndexOf("=")
        if ($split -gt 0) {
            $name = $line.Substring(0, $split)
            $value = $line.Substring($split + 1)
            [Environment]::SetEnvironmentVariable($name, $value, "Process")
        }
    }
    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        throw "cl.exe is still unavailable after importing the Visual Studio environment"
    }
}

function Get-TargetBinDir {
    # Resolve the real Cargo target directory (CARGO_TARGET_DIR, config
    # target-dir, or default) instead of assuming repo/target, so the
    # shared vendor cache and paths with spaces both work.
    Push-Location $repo
    try {
        $metadataJson = & cargo metadata --locked --no-deps --format-version 1 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "cargo metadata failed with exit $LASTEXITCODE (resolve Cargo.lock first): $metadataJson"
        }
        $metadata = $metadataJson | ConvertFrom-Json
        $targetDir = $metadata.target_directory
        if ([string]::IsNullOrWhiteSpace($targetDir)) {
            throw "cargo metadata reported no target directory"
        }
        if (-not [IO.Path]::IsPathRooted($targetDir)) {
            $targetDir = Join-Path $metadata.workspace_root $targetDir
        }
        return Join-Path $targetDir $config
    }
    finally {
        Pop-Location
    }
}

function Invoke-CargoBuild {
    $packages = @(
        "artisan-editor-cli",
        "artisan-backend",
        "artisan-frontend",
        "ae-installer",
        "artisan-native-dev"
    )
    $arguments = @("build", "--locked", "--jobs=1") +
        ($packages | ForEach-Object { @("--package", $_) }) +
        @("--bin", "ae", "--bin", "forge", "--bin", "editor", "--bin", "installer", "--bin", "dev")
    if ($Release) {
        $arguments += "--release"
    } elseif ($Performance) {
        $arguments += @("--profile", "performance")
    }
    Write-DevStep "cargo $($arguments -join ' ')"
    Push-Location $repo
    try {
        & cargo @arguments
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

Ensure-VsEnvironment
if ($Release) {
    Import-ReleaseTrustAnchor
}
Invoke-CargoBuild

$targetBin = Get-TargetBinDir
Write-DevStep "target binaries in $targetBin"
$runner = Join-Path $targetBin "dev.exe"
if (-not (Test-Path $runner)) {
    throw "dev runner not found at $runner after a successful build"
}
$runnerArgs = @("--bin-dir", $targetBin, "--dev-dir", $DevDir)
if ($StageOnly) {
    $runnerArgs += "--stage-only"
}
Write-DevStep "& $runner $($runnerArgs -join ' ')"
& $runner @runnerArgs
exit $LASTEXITCODE
