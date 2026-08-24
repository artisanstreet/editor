#requires -Version 7
<#
.SYNOPSIS
Samples private memory, commit, CPU, thread, and handle counts for a whole
process tree, one JSON line per interval.

.DESCRIPTION
NATIVE-0001 baseline primitive. The sampler never touches the target beyond
reading: it enumerates descendants of the root process through
Win32_Process.ParentProcessId, then measures each live process twice per
interval to derive CPU percent from TotalProcessorTime deltas.

Output is JSON Lines: one compact object per completed interval. Feed the
file to Record-Baseline.ps1's summary stage or read it directly.

.NOTES
Processes whose parent exits keep a stale ParentProcessId on Windows; such
grandchildren drop out of the tree. This is acceptable for supervised
launches where the root outlives its children, and is recorded here so the
limitation is explicit rather than discovered.
#>
param(
	[Parameter(Mandatory)] [int]$RootProcessId,
	[double]$IntervalSeconds = 1,
	[Parameter(Mandatory)][ValidateRange(1, 3600)] [int]$SampleCount,
	[Parameter(Mandatory)] [string]$OutputPath,
	[switch]$PassThru
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$coreCount = [Environment]::ProcessorCount
$startedAt = [DateTime]::UtcNow

function Get-DescendantIds {
	param([int]$Root, [hashtable]$ChildrenByParent)
	$queue = [System.Collections.Generic.Queue[int]]::new()
	foreach ($child in $ChildrenByParent[$Root]) { $queue.Enqueue($child) }
	$found = [System.Collections.Generic.List[int]]::new()
	while ($queue.Count -gt 0) {
		$current = $queue.Dequeue()
		$found.Add($current)
		if ($ChildrenByParent.ContainsKey($current)) {
			foreach ($child in $ChildrenByParent[$current]) { $queue.Enqueue($child) }
		}
	}
	return , $found
}

function Get-CpuSeconds {
	param([System.Diagnostics.Process]$Process)
	try {
		return $Process.TotalProcessorTime.TotalSeconds
	} catch {
		return $null
	}
}

$previousCpu = @{}
$writer = [System.IO.StreamWriter]::new($OutputPath, $false, [Text.Encoding]::UTF8)

try {
	for ($sampleIndex = 0; $sampleIndex -lt $SampleCount; $sampleIndex++) {
		$intervalStart = [DateTime]::UtcNow

		$processRows = @(Get-CimInstance -ClassName Win32_Process -Property ProcessId, ParentProcessId, Name)
		$childrenByParent = @{}
		$parentByPid = @{}
		foreach ($row in $processRows) {
			$parent = [int]$row.ParentProcessId
			$parentByPid[[int]$row.ProcessId] = $parent
			if (-not $childrenByParent.ContainsKey($parent)) {
				$childrenByParent[$parent] = [System.Collections.Generic.List[int]]::new()
			}
			$childrenByParent[$parent].Add([int]$row.ProcessId)
		}

		$rootAlive = $null -ne (Get-Process -Id $RootProcessId -ErrorAction SilentlyContinue)
		if (-not $rootAlive -and $sampleIndex -gt 0) {
			break
		}

		$treeIds = [System.Collections.Generic.List[int]]::new()
		$treeIds.Add($RootProcessId)
		$treeIds.AddRange((Get-DescendantIds -Root $RootProcessId -ChildrenByParent $childrenByParent))

		$snapshots = @{}
		foreach ($id in $treeIds) {
			$process = Get-Process -Id $id -ErrorAction SilentlyContinue
			if ($null -ne $process) { $snapshots[$id] = $process }
		}

		Start-Sleep -Milliseconds ([int]($IntervalSeconds * 1000))

		$processes = [System.Collections.Generic.List[object]]::new()
		$totalWorkingSet = [uint64]0
		$totalCommit = [uint64]0
		foreach ($id in $treeIds) {
			if (-not $snapshots.ContainsKey($id)) { continue }
			$endProcess = Get-Process -Id $id -ErrorAction SilentlyContinue
			if ($null -eq $endProcess) { continue }

			$cpuBefore = $previousCpu[$id]
			if ($null -eq $cpuBefore) { $cpuBefore = Get-CpuSeconds -Process $snapshots[$id] }
			$cpuAfter = Get-CpuSeconds -Process $endProcess
			$previousCpu[$id] = $cpuAfter

			$cpuPercent = $null
			if ($null -ne $cpuBefore -and $null -ne $cpuAfter) {
				$cpuPercent = [math]::Round((($cpuAfter - $cpuBefore) / ($IntervalSeconds * $coreCount)) * 100, 2)
				if ($cpuPercent -lt 0) { $cpuPercent = 0 }
			}

			$totalWorkingSet += [uint64]$endProcess.WorkingSet64
			$totalCommit += [uint64]$endProcess.PrivateMemorySize64

			$processes.Add(@{
				pid                = $id
				parentPid          = $parentByPid[$id]
				name               = $endProcess.Name
				workingSetBytes    = [uint64]$endProcess.WorkingSet64
				privateCommitBytes = [uint64]$endProcess.PrivateMemorySize64
				cpuPercent         = $cpuPercent
				threadCount        = $endProcess.Threads.Count
				handleCount        = $endProcess.HandleCount
			})
		}

		$elapsedMs = [int]([DateTime]::UtcNow - $startedAt).TotalMilliseconds
		$record = [ordered]@{
			t                       = $intervalStart.ToString('o')
			elapsedMs               = $elapsedMs
			sample                  = $sampleIndex
			rootPid                 = $RootProcessId
			coreCount               = $coreCount
			processCount            = $processes.Count
			totalWorkingSetBytes    = $totalWorkingSet
			totalPrivateCommitBytes = $totalCommit
			processes               = $processes
		}
		$line = $record | ConvertTo-Json -Compress -Depth 6 -WarningAction SilentlyContinue
		$writer.WriteLine($line)
		$writer.Flush()
		if ($PassThru) { $line }
	}
} finally {
	$writer.Dispose()
}
