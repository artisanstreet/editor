# Attribute one Windows process's committed and resident pages without reading
# their contents. This samples a changing process, not an atomic snapshot.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateRange(1, 2147483647)][int]$ProcessId,
    [string]$Scenario = "unspecified",
    [string]$OutputPath
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
if (-not ("ArtisanMemoryInspector" -as [type])) {
    Add-Type -Path (Join-Path $PSScriptRoot 'windows-memory.cs')
}
$targetProcess = Get-Process -Id $ProcessId
$started = $targetProcess.StartTime.ToUniversalTime().ToString('o')
$watch = [System.Diagnostics.Stopwatch]::StartNew()
$measuredAt = [DateTime]::UtcNow.ToString('o')
$regions = [ArtisanMemoryInspector]::Inspect($ProcessId)
$modules = @($targetProcess.Modules | ForEach-Object {
    [ordered]@{
        name = $_.ModuleName
        path = $_.FileName
        base_address = '0x' + $_.BaseAddress.ToInt64().ToString('x')
        virtual_bytes = $_.ModuleMemorySize
    }
})
$threads = @($targetProcess.Threads | ForEach-Object {
    $startAddress = [ArtisanMemoryInspector]::ThreadStartAddress($_.Id)
    $startModule = $null
    $startModuleOffset = $null
    if ($startAddress) {
        $address = [Convert]::ToInt64($startAddress.Substring(2), 16)
        foreach ($module in $modules) {
            $base = [Convert]::ToInt64($module.base_address.Substring(2), 16)
            if ($address -ge $base -and $address -lt $base + $module.virtual_bytes) {
                $startModule = $module.name
                $startModuleOffset = '0x' + ($address - $base).ToString('x')
                break
            }
        }
    }
    [ordered]@{
        id = $_.Id
        description = [ArtisanMemoryInspector]::ThreadDescription($_.Id)
        start_address = $startAddress
        start_module = $startModule
        start_module_offset = $startModuleOffset
        started_at_utc = $_.StartTime.ToUniversalTime().ToString('o')
        cpu_seconds = $_.TotalProcessorTime.TotalSeconds
        user_cpu_seconds = $_.UserProcessorTime.TotalSeconds
        kernel_cpu_seconds = $_.PrivilegedProcessorTime.TotalSeconds
        state = $_.ThreadState.ToString()
    }
})
$targetProcess.Refresh()
if ($targetProcess.HasExited -or $targetProcess.StartTime.ToUniversalTime().ToString('o') -ne $started) {
    throw 'Process exited or was replaced during measurement.'
}
$categories = @($regions | Group-Object kind | ForEach-Object {
    $committed = @($_.Group | Where-Object { $_.state -eq 'committed' })
    [ordered]@{
        kind = $_.Name
        committed_bytes = ($committed | Measure-Object bytes -Sum).Sum
        resident_bytes = ($committed | Measure-Object resident_bytes -Sum).Sum
        private_resident_bytes = ($committed | Measure-Object private_resident_bytes -Sum).Sum
        shared_resident_bytes = ($committed | Measure-Object shared_resident_bytes -Sum).Sum
        working_set_bytes = ($committed | Measure-Object working_set_bytes -Sum).Sum
        working_set_private_bytes = ($committed | Measure-Object working_set_private_bytes -Sum).Sum
        working_set_shared_bytes = ($committed | Measure-Object working_set_shared_bytes -Sum).Sum
    }
})
$report = [ordered]@{
    schema_version = 1
    measured_at_utc = $measuredAt
    elapsed_seconds = $watch.Elapsed.TotalSeconds
    pid = $ProcessId
    process_started_utc = $started
    executable = $targetProcess.Path
    scenario = $Scenario
    process_working_set_bytes = $targetProcess.WorkingSet64
    process_private_committed_bytes = $targetProcess.PrivateMemorySize64
    page_size_bytes = [Environment]::SystemPageSize
    categories = $categories
    modules = $modules
    threads = $threads
    regions = $regions
    scope = 'Non-atomic metadata-only snapshot. working_set_* fields enumerate actual process working-set membership with QueryWorkingSet. resident_* fields use QueryWorkingSetEx and can include pages outside the process working set. Private pages include private copy-on-write image pages. Shared pages have a share count greater than one. Committed regions include file-backed mappings and are not the same as private process commit. GPU allocations outside the process address space are excluded. No working-set trimming or page-content reads.'
}
$json = $report | ConvertTo-Json -Depth 6
if ($OutputPath) {
    [System.IO.File]::WriteAllText($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath), $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
} else {
    $json
}
