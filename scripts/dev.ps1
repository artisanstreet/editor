<#
.SYNOPSIS
    `cargo dev` on Windows: build, install, and launch the dev Editor.

.DESCRIPTION
    Imports the Visual Studio build environment when needed, then runs the
    Rust dev runner (`cargo dev`), which builds the product binaries, installs
    them as a signed dev-channel release into
    %LOCALAPPDATA%\Artisan Street Dev through the shipping installer code,
    and launches the installed Editor (closing any previous dev Editor).

    A checkout on a network share (such as \\wsl.localhost) cannot host
    Cargo's target directory, so it builds under
    %LOCALAPPDATA%\Artisan Street Dev\build\<checkout-id> instead.

    Any other arguments pass through to `cargo dev`; see `cargo dev --help`.

.EXAMPLE
    scripts/dev.ps1
    scripts/dev.ps1 -Performance
    scripts/dev.ps1 -Release -StageOnly
    scripts/dev.ps1 where
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$Performance,
    [switch]$StageOnly,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Passthrough = @()
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
if ($Release -and $Performance) {
    throw "Choose either -Release or -Performance"
}

function Write-DevStep($message) {
    Write-Host "dev.ps1: $message"
}

# Cargo's build-script hardlinks fail with "Access is denied" in a target
# directory on a network share, so such checkouts build on local disk, keyed
# per checkout so worktrees never share a target directory.
function Get-LocalBuildRoot {
    $key = $repo.ToLowerInvariant()
    $sha = [Security.Cryptography.SHA256]::Create()
    $digest = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($key))
    $id = (-join ($digest | ForEach-Object { $_.ToString("x2") })).Substring(0, 12)
    return Join-Path $env:LOCALAPPDATA "Artisan Street Dev\build\$id"
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

if ($repo.StartsWith("\\") -and [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    $env:CARGO_TARGET_DIR = Join-Path (Get-LocalBuildRoot) "target"
    Write-DevStep "checkout is on a network share; building in $env:CARGO_TARGET_DIR"
}
Ensure-VsEnvironment

$arguments = @()
if ($StageOnly) {
    $arguments += "stage"
}
if ($Release) {
    $arguments += "--release"
} elseif ($Performance) {
    $arguments += @("--profile", "performance")
}
$arguments += $Passthrough

Write-DevStep "cargo dev $($arguments -join ' ')"
Push-Location $repo
try {
    & cargo dev @arguments
    $code = $LASTEXITCODE
}
finally {
    Pop-Location
}
exit $code
