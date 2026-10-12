# Installs Artisan on Windows:
#
#   irm https://github.com/sandersonstabo/artisan-editor/releases/latest/download/install.ps1 | iex
#   & ([scriptblock]::Create((irm …/install.ps1))) forge --wsl Ubuntu     (arguments go to `ae install`)
#
# Downloads release.json and the Windows archive, checks the archive's
# SHA-256 against release.json, and runs the `ae.exe` inside with `install`.
# `ae install` downloads the release again and verifies release.json's
# Ed25519 signature before installing anything. $env:ARTISAN_RELEASES
# overrides the feed (a release.json URL or a directory holding a release).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$feed = 'https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release.json'
if ($env:ARTISAN_RELEASES) { $feed = $env:ARTISAN_RELEASES }
$source = $feed
$remote = $feed -match '^https?://'
if ($remote) { $base = $feed.Substring(0, $feed.LastIndexOf('/')) } else { $base = $feed; $feed = Join-Path $feed 'release.json' }

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'x86_64' }
    'ARM64' { 'aarch64' }
    default { throw "Unsupported CPU $($env:PROCESSOR_ARCHITECTURE)." }
}

function Get-ReleaseFile([string] $Source, [string] $Destination) {
    if ($remote) { Invoke-WebRequest -UseBasicParsing -Uri $Source -OutFile $Destination } else { Copy-Item -LiteralPath $Source -Destination $Destination }
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('artisan-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Get-ReleaseFile $feed (Join-Path $tmp 'release.json')
    $release = Get-Content -Raw -LiteralPath (Join-Path $tmp 'release.json') | ConvertFrom-Json
    $artifact = $release.artifacts | Where-Object { $_.os -eq 'windows' -and $_.arch -eq $arch } | Select-Object -First 1
    if (-not $artifact) { throw "Release $($release.version) has no build for Windows $arch." }
    if ($artifact.file -notmatch '^[A-Za-z0-9_+-][A-Za-z0-9._+-]*$') { throw "Unusable archive name '$($artifact.file)'." }

    Write-Host "Downloading $($artifact.file)..."
    $archive = Join-Path $tmp $artifact.file
    if ($remote) { Get-ReleaseFile "$base/$($artifact.file)" $archive } else { Get-ReleaseFile (Join-Path $base $artifact.file) $archive }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash
    if ($actual -ne $artifact.sha256) { throw "$($artifact.file) is corrupt or was tampered with (SHA-256 $actual, expected $($artifact.sha256))." }

    $unpacked = Join-Path $tmp 'unpacked'
    Expand-Archive -LiteralPath $archive -DestinationPath $unpacked
    & (Join-Path $unpacked 'ae.exe') install --from $source @args
    if ($LASTEXITCODE -ne 0) { throw "ae install failed (exit code $LASTEXITCODE)." }
} finally {
    Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
}
