<#
.SYNOPSIS
    Verifies that the pinned gpui-ce revision resolves on the fork remote.

.DESCRIPTION
    Reads the `rev` of the gpui git dependency from Cargo.toml and checks,
    through the GitHub API, that:
      1. the commit exists on artisanstreet/gpui-ce; and
      2. it is reachable from the `artisan/editor` integration branch.
    A pin that only exists on one workstation is the failure this guards
    against. Requires an authenticated `gh` CLI.
#>
[CmdletBinding()]
param(
    [string]$Root
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
}

$manifest = Join-Path $Root "Cargo.toml"
if (-not (Test-Path -LiteralPath $manifest)) {
    throw "Cargo.toml not found at $manifest"
}

$cargo = Get-Content -LiteralPath $manifest -Raw
$match = [regex]::Match(
    $cargo,
    'gpui\s*=\s*\{[^}]*git\s*=\s*"https://github\.com/artisanstreet/gpui-ce"[^}]*rev\s*=\s*"(?<rev>[0-9a-f]{40})"'
)
if (-not $match.Success) {
    throw "no pinned gpui-ce git revision found in $manifest"
}
$rev = $match.Groups["rev"].Value
Write-Host "gpui-ce pin: $rev"

$sha = gh api "repos/artisanstreet/gpui-ce/commits/$rev" --jq ".sha" 2>$null
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($sha)) {
    throw "pinned commit $rev does not exist on artisanstreet/gpui-ce"
}

$status = gh api "repos/artisanstreet/gpui-ce/compare/$rev...artisan/editor" --jq ".status" 2>$null
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($status)) {
    throw "cannot compare $rev with artisan/editor on artisanstreet/gpui-ce"
}
if ($status -notin @("ahead", "identical")) {
    throw "pin $rev is not reachable from artisan/editor (compare status: $status)"
}

Write-Host "ok: $rev exists and is reachable from artisan/editor"
