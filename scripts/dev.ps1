<#
.SYNOPSIS
    One-command native development workflow on Windows (developer convenience).

.DESCRIPTION
    Builds the four product binaries plus the native dev runner in one
    `cargo build --locked --jobs=1` invocation, then runs the same
    `scripts/native_dev` runner the Bazel `//:dev` target uses, pointed at
    the repo `.dist/dev` installation. This script is an honest build
    driver for developers without Bazel on PATH; it is never wired into
    Bazel, which remains the authoritative build.

.EXAMPLE
    scripts/dev.ps1
    scripts/dev.ps1 -Release -StageOnly
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$StageOnly,
    [string]$DevDir = ""
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($DevDir)) {
    $DevDir = Join-Path $repo ".dist/dev"
}
$config = if ($Release) { "release" } else { "debug" }
$targetBin = Join-Path (Join-Path $repo "target") $config

function Write-DevStep($message) {
    Write-Host "dev.ps1: $message"
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
    $envDump = cmd /c "`"$vcvars`" >nul && set" 2>&1
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
Invoke-CargoBuild

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
