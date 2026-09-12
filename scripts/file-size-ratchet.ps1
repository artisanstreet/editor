<#
.SYNOPSIS
    Ratchets hand-written Rust file sizes toward the 800-line maintainability bar.

.DESCRIPTION
    Scans `modules/**/*.rs` (generated Cap'n Proto mirrors excluded) and fails
    when a file above the limit is not frozen in `scripts/file-size-allowlist.txt`
    or grows past its frozen size. Extract a module, then run `-Update` to shrink
    the allowance; the allowlist never grows automatically.
#>
[CmdletBinding()]
param(
    [string]$Root,
    [int]$Limit = 800,
    [switch]$Update
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
}
$allowlistPath = Join-Path $Root "scripts/file-size-allowlist.txt"
$generated = @("artisan_capnp.rs", "composer_state_capnp.rs")

$sized = foreach ($file in Get-ChildItem (Join-Path $Root "modules") -Recurse -Filter "*.rs") {
    if ($file.FullName -match "\\target\\") { continue }
    if ($generated -contains $file.Name) { continue }
    $lines = @(Get-Content -LiteralPath $file.FullName)
    if ($lines.Count -gt $Limit) {
        [PSCustomObject]@{
            Path  = $file.FullName.Substring($Root.Length + 1).Replace("\", "/")
            Lines = $lines.Count
        }
    }
}
$entries = @($sized | Sort-Object Path)

if ($Update) {
    $entries | ForEach-Object { "{0} {1}" -f $_.Lines, $_.Path } | Set-Content -LiteralPath $allowlistPath
    Write-Host "file-size ratchet allowlist updated: $($entries.Count) file(s) above $Limit"
    exit 0
}

if (-not (Test-Path -LiteralPath $allowlistPath)) {
    throw "allowlist missing at $allowlistPath; run with -Update once"
}
$allowed = @{}
foreach ($line in Get-Content -LiteralPath $allowlistPath) {
    $line = $line.Trim()
    if ($line.Length -eq 0 -or $line.StartsWith("#")) { continue }
    $parts = $line -split "\s+", 2
    $allowed[$parts[1]] = [int]$parts[0]
}

$findings = @()
foreach ($entry in $entries) {
    if (-not $allowed.ContainsKey($entry.Path)) {
        $findings += "new file above ${Limit} lines: $($entry.Path) ($($entry.Lines))"
    } elseif ($entry.Lines -gt $allowed[$entry.Path]) {
        $findings += "grew past frozen size $($allowed[$entry.Path]): $($entry.Path) ($($entry.Lines))"
    }
}
if ($findings.Count -gt 0) {
    $findings | ForEach-Object { Write-Error $_ }
    exit 1
}

$shrunk = @($allowed.Keys | Where-Object { $entries.Path -notcontains $_ })
Write-Host "file-size ratchet clean: $($entries.Count) frozen file(s), none grew"
if ($shrunk.Count -gt 0) {
    Write-Host "note: $($shrunk.Count) allowlisted file(s) are now at or below $Limit; run -Update to shrink the allowance"
}
