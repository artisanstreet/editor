param(
    [Parameter(Mandatory)]
    [string] $Worktree,

    [Parameter(Mandatory)]
    [string] $ContractPath,

    [Parameter(Mandatory)]
    [string] $LogDirectory,

    [Parameter(Mandatory)]
    [string] $Authorization
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version Latest

$expectedAuthorization = 'VP_GO_EXTERNAL_CODEX_LUNA_MAX_STANDARD_NO_SUBAGENTS'
if ($Authorization -cne $expectedAuthorization) {
    throw 'HOLD: exact VP Luna authorization is required.'
}

$scriptRoot = Split-Path -Parent $PSCommandPath
$providerStatus = Join-Path $scriptRoot 'PROVIDER_STATUS.md'
if (-not (Test-Path -LiteralPath $providerStatus)) {
    throw 'HOLD: canonical provider status is missing.'
}
$statusText = Get-Content -Raw -LiteralPath $providerStatus
if ($statusText -notmatch 'Current new-worker tier:\s*\*\*GPT-5\.6 Luna') {
    throw 'HOLD: provider status does not authorize the Luna tier.'
}

$resolvedWorktree = (Resolve-Path -LiteralPath $Worktree).Path
$resolvedContract = (Resolve-Path -LiteralPath $ContractPath).Path
if (-not (Test-Path -LiteralPath (Join-Path $resolvedWorktree '.git'))) {
    throw 'HOLD: worker path is not a Git worktree.'
}
if (@(& git -C $resolvedWorktree status --porcelain).Count -ne 0) {
    throw 'HOLD: initial Luna worker requires a clean worktree.'
}

$liveExec = @(Get-CimInstance Win32_Process | Where-Object {
    $_.Name -eq 'codex.exe' -and $_.CommandLine -match '\bexec\b'
})
if ($liveExec.Count -ne 0) {
    throw "HOLD: another external codex exec worker is live: $($liveExec.ProcessId -join ',')"
}

$codex = Get-Command codex -ErrorAction Stop
$pwsh = Get-Command pwsh -ErrorAction Stop
$codexVersion = (& $codex.Source --version | Out-String).Trim()
$codexHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $codex.Source).Hash
$features = (& $codex.Source features list | Out-String)
if ($features -notmatch '(?m)^fast_mode\s+stable' -or
    $features -notmatch '(?m)^multi_agent\s+stable') {
    throw 'HOLD: installed Codex CLI does not expose required safety features.'
}

if (-not (Test-Path -LiteralPath $LogDirectory)) {
    New-Item -ItemType Directory -Path $LogDirectory | Out-Null
}
$resolvedLogs = (Resolve-Path -LiteralPath $LogDirectory).Path
$stdoutPath = Join-Path $resolvedLogs 'worker.stdout.jsonl'
$stderrPath = Join-Path $resolvedLogs 'worker.stderr.log'
$finalPath = Join-Path $resolvedLogs 'worker.final.md'
$telemetryPath = Join-Path $resolvedLogs 'launch-telemetry.json'
foreach ($path in @($stdoutPath, $stderrPath, $finalPath, $telemetryPath)) {
    if (Test-Path -LiteralPath $path) {
        throw "HOLD: single-use evidence path already exists: $path"
    }
}

$arguments = @(
    '-NoLogo',
    '-NoProfile',
    '-File', $codex.Source,
    'exec',
    '--strict-config',
    '--ignore-user-config',
    '--disable', 'fast_mode',
    '--disable', 'multi_agent',
    '--disable', 'multi_agent_v2',
    '--model', 'gpt-5.6-luna',
    '--config', 'model_reasoning_effort=max',
    '--config', 'approval_policy=never',
    '--sandbox', 'workspace-write',
    '--cd', $resolvedWorktree,
    '--output-last-message', $finalPath,
    '--json',
    '-'
)

$startedAt = Get-Date
$process = Start-Process `
    -FilePath $pwsh.Source `
    -ArgumentList $arguments `
    -WorkingDirectory $resolvedWorktree `
    -RedirectStandardInput $resolvedContract `
    -RedirectStandardOutput $stdoutPath `
    -RedirectStandardError $stderrPath `
    -WindowStyle Hidden `
    -PassThru

[ordered]@{
    outcome = 'LAUNCHED'
    provider = 'openai-codex-cli'
    model = 'gpt-5.6-luna'
    reasoning_effort = 'max'
    fast_mode = 'disabled'
    multi_agent = 'disabled'
    multi_agent_v2 = 'disabled'
    codex_version = $codexVersion
    codex_wrapper = $codex.Source
    codex_wrapper_sha256 = $codexHash
    wrapper_pid = $process.Id
    started_at = $startedAt.ToString('o')
    worktree = $resolvedWorktree
    contract = $resolvedContract
} | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -LiteralPath $telemetryPath

$process.WaitForExit()
$exitCode = $process.ExitCode

$telemetry = Get-Content -Raw -LiteralPath $telemetryPath | ConvertFrom-Json
$telemetry | Add-Member -NotePropertyName outcome -NotePropertyValue 'COMPLETED' -Force
$telemetry | Add-Member -NotePropertyName exit_code -NotePropertyValue $exitCode
$telemetry | Add-Member -NotePropertyName ended_at -NotePropertyValue (Get-Date).ToString('o')
$telemetry | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -LiteralPath $telemetryPath

if ($exitCode -ne 0) {
    throw "Luna worker failed with exit $exitCode; inspect $stderrPath"
}
