#requires -Version 7
<#
.SYNOPSIS
Records one NATIVE-0001 baseline artifact for a scenario: machine header,
JSONL process-tree samples, and a summary with peaks and percentiles.

.EXAMPLE
# Launch a command and measure its whole tree:
./Record-Baseline.ps1 -Name harness-selftest -Command pwsh, -NoProfile, -Command,
  '$blob=[byte[]]::new(120MB); $end=60; Start-Sleep -Seconds 25' `
  -DurationSeconds 12 -IntervalSeconds 2

# Attach to an already-running root process (for example the Forge daemon):
./Record-Baseline.ps1 -Name forge-idle -AttachPid 42424 -DurationSeconds 60
#>
param(
	[Parameter(Mandatory)] [string]$Name,
	[string[]]$Command,
	[int]$AttachPid,
	[int]$SettleSeconds = 3,
	[ValidateRange(2, 3600)] [int]$DurationSeconds = 30,
	[ValidateRange(1, 60)] [double]$IntervalSeconds = 1,
	[string]$OutputDirectory = '.dist/native-baseline',
	[string[]]$Notes,
	[switch]$NoKill
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $Command -and -not $AttachPid) {
	throw 'Provide either -Command (launch and measure) or -AttachPid (measure an existing tree).'
}
if ($Command -and $AttachPid) {
	throw 'Provide only one of -Command and -AttachPid.'
}

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repositoryRoot = (Resolve-Path (Join-Path $scriptDirectory '..\..\..')).Path
$samplerPath = Join-Path $scriptDirectory 'Measure-ProcessTree.ps1'

function Get-MachineHeader {
	$os = Get-CimInstance -ClassName Win32_OperatingSystem -Property Caption, Version, BuildNumber, TotalVisibleMemorySize
	$cpu = Get-CimInstance -ClassName Win32_Processor -Property Name, NumberOfCores, NumberOfLogicalProcessors |
		Select-Object -First 1
	$computer = Get-CimInstance -ClassName Win32_ComputerSystem -Property Manufacturer, Model, TotalPhysicalMemory
	return @{
		hostName            = [Environment]::MachineName
		model               = "$($computer.Manufacturer) $($computer.Model)".Trim()
		osCaption           = $os.Caption
		osVersion           = "$($os.Version) build $($os.BuildNumber)"
		cpuName             = ($cpu.Name -replace '\s+', ' ').Trim()
		logicalCoreCount    = [Environment]::ProcessorCount
		totalPhysicalMemory = [uint64]$computer.TotalPhysicalMemory
	}
}

function Get-GitRevision {
	try {
		return (git -C $repositoryRoot rev-parse HEAD 2>$null)
	} catch {
		return $null
	}
}

function Get-Percentile {
	param([double[]]$SortedValues, [double]$Quantile)
	if ($SortedValues.Count -eq 0) { return $null }
	$index = [math]::Ceiling($Quantile * $SortedValues.Count) - 1
	if ($index -lt 0) { $index = 0 }
	return [math]::Round($SortedValues[$index], 3)
}

if ($Command) {
	$launched = Start-Process -FilePath $Command[0] -ArgumentList (@($Command | Select-Object -Skip 1)) -PassThru -NoNewWindow
	$rootPid = $launched.Id
	Write-Host "launched '$($Command -join ' ')' as pid $rootPid"
	Start-Sleep -Seconds $SettleSeconds
	if ($launched.HasExited) {
		throw "Launched process exited during settle with code $($launched.ExitCode); nothing to measure."
	}
} else {
	$rootPid = $AttachPid
	if (-not (Get-Process -Id $rootPid -ErrorAction SilentlyContinue)) {
		throw "No live process with pid $rootPid to attach to."
	}
	Write-Host "attaching to pid $rootPid"
}

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
$artifactPath = Join-Path (Join-Path $repositoryRoot $OutputDirectory) "$stamp-$Name.json"
$samplesPath = [IO.Path]::ChangeExtension($artifactPath, '.samples.jsonl')
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $artifactPath) | Out-Null

try {
	& $samplerPath -RootProcessId $rootPid -SampleCount ([int][math]::Ceiling($DurationSeconds / $IntervalSeconds)) `
		-IntervalSeconds $IntervalSeconds -OutputPath $samplesPath

	$samples = @(Get-Content $samplesPath | ForEach-Object { $_ | ConvertFrom-Json })
	if ($samples.Count -eq 0) {
		throw 'Sampler produced no samples; refusing to write an empty artifact.'
	}

	$peakWorkingSet = ($samples | Measure-Object -Property totalWorkingSetBytes -Maximum).Maximum
	$peakCommit = ($samples | Measure-Object -Property totalPrivateCommitBytes -Maximum).Maximum

	$byName = @{}
	foreach ($sample in $samples) {
		foreach ($process in $sample.processes) {
			if (-not $byName.ContainsKey($process.name)) {
				$byName[$process.name] = @{
					workingSetBytes   = [System.Collections.Generic.List[double]]::new()
					privateCommit     = [System.Collections.Generic.List[double]]::new()
					cpuPercent        = [System.Collections.Generic.List[double]]::new()
				}
			}
			$bucket = $byName[$process.name]
			$bucket.workingSetBytes.Add([double]$process.workingSetBytes)
			$bucket.privateCommit.Add([double]$process.privateCommitBytes)
			if ($null -ne $process.cpuPercent) { $bucket.cpuPercent.Add([double]$process.cpuPercent) }
		}
	}

	$perProcess = foreach ($processName in ($byName.Keys | Sort-Object)) {
		$bucket = $byName[$processName]
		$sortedWorkingSet = $bucket.workingSetBytes.ToArray(); [array]::Sort($sortedWorkingSet)
		$sortedCommit = $bucket.privateCommit.ToArray(); [array]::Sort($sortedCommit)
		$sortedCpu = $bucket.cpuPercent.ToArray(); [array]::Sort($sortedCpu)
		@{
			name                  = $processName
			peakWorkingSetBytes   = [uint64]$sortedWorkingSet[-1]
			p95WorkingSetBytes    = [uint64](Get-Percentile -SortedValues $sortedWorkingSet -Quantile 0.95)
			peakPrivateCommit     = [uint64]$sortedCommit[-1]
			p95CpuPercent         = Get-Percentile -SortedValues $sortedCpu -Quantile 0.95
			maxCpuPercent         = if ($sortedCpu.Count) { $sortedCpu[-1] } else { $null }
		}
	}

	$artifact = [ordered]@{
		schema       = 'artisan.native.baseline/1'
		scenario     = $Name
		recordedAt   = (Get-Date).ToUniversalTime().ToString('o')
		gitRevision  = Get-GitRevision
		machine      = Get-MachineHeader
		measurement  = @{
			mode                 = if ($Command) { 'launch' } else { 'attach' }
			rootPid              = $rootPid
			settleSeconds        = $SettleSeconds
			durationSeconds      = $DurationSeconds
			intervalSeconds      = $IntervalSeconds
			sampleCount          = $samples.Count
		}
		summary      = @{
			peakTotalWorkingSetBytes    = [uint64]$peakWorkingSet
			peakTotalPrivateCommitBytes = [uint64]$peakCommit
			perProcess                  = $perProcess
		}
		notes        = $Notes
		samplesFile  = [IO.Path]::GetFileName($samplesPath)
	}

	$artifact | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 -Path $artifactPath
	Write-Host "`nbaseline artifact: $artifactPath"
	Write-Host ("peak working set: {0:N1} MB, peak commit: {1:N1} MB" -f ($peakWorkingSet / 1MB), ($peakCommit / 1MB))
	$artifactPath
} finally {
	if ($Command -and -not $NoKill) {
		$stillRunning = Get-Process -Id $rootPid -ErrorAction SilentlyContinue
		if ($stillRunning) {
			$null = $stillRunning.CloseMainWindow()
			if (-not $stillRunning.WaitForExit(5000)) {
				try { $stillRunning.Kill($true) } catch { Write-Warning "failed to stop pid ${rootPid}: $_" }
			}
		}
	}
}
