# SPDX-License-Identifier: MPL-2.0
#Requires -Version 5.1
[CmdletBinding()]
param(
    [ValidateRange(5, 3600)]
    [int]$Seconds = 30,
    [ValidateRange(250, 10000)]
    [int]$IntervalMilliseconds = 1000,
    [ValidateRange(0, 60)]
    [int]$SettleSeconds = 5,
    [string]$OutputDirectory,
    [switch]$IncludeTraffic
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:OS -ne 'Windows_NT') {
    throw 'Run this script on Windows with Windows PowerShell 5.1 or newer.'
}
if ($IntervalMilliseconds -gt ($Seconds * 1000)) {
    throw 'IntervalMilliseconds must not exceed the duration of a stage.'
}

# This script reads local process counters only. It never opens Discord IPC,
# reads application settings or credentials, or generates network/chat traffic.
$measurementClock = [System.Diagnostics.Stopwatch]::StartNew()
$processGroups = @('Discord', 'Sideglow')

function Get-ProcessSnapshot {
    $entries = @{}
    $observed = @{ Discord = 0; Sideglow = 0 }
    $unreadable = @{ Discord = 0; Sideglow = 0 }
    $unclassified = 0
    foreach ($process in @(Get-Process -ErrorAction Stop)) {
        try {
            try { $processName = $process.ProcessName }
            catch {
                # A process can disappear before even its name can be read.
                # Its group is unknown, so mark both groups' interval partial.
                $unclassified++
                continue
            }
            if ($processName -like 'Discord*') {
                $group = 'Discord'
            }
            elseif ($processName -eq 'sideglow') {
                $group = 'Sideglow'
            }
            else {
                continue
            }
            $observed[$group]++
            try {
                # PID reuse must not mix two different processes' CPU counters.
                $startedTicks = $process.StartTime.ToUniversalTime().Ticks
                $identity = '{0}:{1}' -f $process.Id, $startedTicks
                $cpuSeconds = $process.TotalProcessorTime.TotalSeconds
                $privateBytes = $process.PrivateMemorySize64
                $entries[$identity] = [pscustomobject]@{
                    Group = $group
                    CpuSeconds = [double]$cpuSeconds
                    PrivateBytes = [long]$privateBytes
                }
            }
            catch {
                # Access denial or exit during collection is not a zero reading.
                $unreadable[$group]++
            }
        }
        finally {
            $process.Dispose()
        }
    }
    [pscustomobject]@{
        Entries = $entries
        Observed = $observed
        Unreadable = $unreadable
        Unclassified = $unclassified
        ElapsedSeconds = $measurementClock.Elapsed.TotalSeconds
        Utc = [DateTime]::UtcNow.ToString('o')
    }
}

function Test-StagePresence {
    param($Snapshot, [string]$Stage)
    if ($Snapshot.Observed.Discord -eq 0) { return $false }
    if ($Stage -eq 'baseline') { return ($Snapshot.Observed.Sideglow -eq 0) }
    return ($Snapshot.Observed.Sideglow -gt 0)
}

function Get-IntervalRow {
    param($Previous, $Current, [string]$Stage, [string]$Group, [int]$Index)
    $interval = $Current.ElapsedSeconds - $Previous.ElapsedSeconds
    $previousIds = @($Previous.Entries.Keys | Where-Object { $Previous.Entries[$_].Group -eq $Group })
    $currentIds = @($Current.Entries.Keys | Where-Object { $Current.Entries[$_].Group -eq $Group })
    $added = 0
    $removed = 0
    $matched = 0
    $counterReset = $false
    $cpuDelta = 0.0
    $privateBytes = 0.0
    foreach ($identity in $currentIds) {
        $entry = $Current.Entries[$identity]
        $privateBytes += $entry.PrivateBytes
        if ($Previous.Entries.ContainsKey($identity)) {
            $delta = $entry.CpuSeconds - $Previous.Entries[$identity].CpuSeconds
            if ($delta -lt 0) { $counterReset = $true }
            else { $cpuDelta += $delta; $matched++ }
        }
        else { $added++ }
    }
    foreach ($identity in $previousIds) {
        if (-not $Current.Entries.ContainsKey($identity)) { $removed++ }
    }

    $required = ($Group -eq 'Discord' -or $Stage -ne 'baseline')
    $missing = $required -and ($Previous.Observed[$Group] -eq 0 -or $Current.Observed[$Group] -eq 0)
    $reasons = New-Object 'System.Collections.Generic.List[string]'
    if ($added -gt 0 -or $removed -gt 0) { $reasons.Add('process_set_changed') }
    if ($Previous.Unreadable[$Group] -gt 0 -or $Current.Unreadable[$Group] -gt 0) { $reasons.Add('unreadable_process') }
    if ($Previous.Unclassified -gt 0 -or $Current.Unclassified -gt 0) { $reasons.Add('unclassified_process') }
    if ($counterReset) { $reasons.Add('counter_reset') }
    if ($missing) { $reasons.Add('required_process_missing') }
    if ($interval -le 0) { $reasons.Add('invalid_interval') }

    $knownCpuPercent = $null
    if ($interval -gt 0) {
        $knownCpuPercent = 100.0 * $cpuDelta / $interval / $logicalProcessors
    }
    $cpuPercent = $null
    if ($reasons.Count -eq 0) { $cpuPercent = $knownCpuPercent }
    $memoryMiB = $null
    if ($Current.Unreadable[$Group] -eq 0 -and $Current.Unclassified -eq 0 -and -not ($required -and $Current.Observed[$Group] -eq 0)) {
        $memoryMiB = $privateBytes / 1MB
    }
    [pscustomobject][ordered]@{
        stage = $Stage
        sample = $Index
        timestamp_utc = $Current.Utc
        process_group = $Group
        interval_seconds = $interval
        process_count = $Current.Observed[$Group]
        readable_process_count = $currentIds.Count
        unreadable_process_count = $Current.Unreadable[$Group]
        unclassified_process_count = $Current.Unclassified
        matched_cpu_process_count = $matched
        added_process_identities = $added
        removed_process_identities = $removed
        cpu_total_capacity_percent = $cpuPercent
        cpu_known_processes_percent = $knownCpuPercent
        cpu_sample_status = $(if ($reasons.Count -eq 0) { 'complete' } else { $reasons -join ';' })
        private_memory_mib = $memoryMiB
        private_memory_readable_processes_mib = $privateBytes / 1MB
        stage_process_presence_ok = ((Test-StagePresence $Previous $Stage) -and (Test-StagePresence $Current $Stage))
    }
}

function Get-MetricSummary {
    param([object[]]$Rows, [string]$Field, [switch]$WeightByInterval)
    $valid = @($Rows | Where-Object { $null -ne $_.$Field })
    $mean = $null
    $maximum = $null
    $p95 = $null
    $validSeconds = 0.0
    if ($valid.Count -gt 0) {
        $weightedTotal = 0.0
        $weightTotal = 0.0
        foreach ($row in $valid) {
            $weight = 1.0
            if ($WeightByInterval) { $weight = [double]$row.interval_seconds }
            $weightedTotal += [double]$row.$Field * $weight
            $weightTotal += $weight
            $validSeconds += [double]$row.interval_seconds
        }
        $orderedValues = @($valid | ForEach-Object { [double]$_.$Field } | Sort-Object)
        $mean = [Math]::Round($weightedTotal / $weightTotal, 6)
        $maximum = [Math]::Round($orderedValues[-1], 6)
        $rank = [int][Math]::Ceiling(0.95 * $orderedValues.Count) - 1
        $p95 = [Math]::Round($orderedValues[$rank], 6)
    }
    [pscustomobject][ordered]@{
        mean = $mean
        max = $maximum
        p95 = $p95
        valid_samples = $valid.Count
        excluded_samples = $Rows.Count - $valid.Count
        valid_seconds = [Math]::Round($validSeconds, 6)
        mean_method = $(if ($WeightByInterval) { 'duration_weighted' } else { 'arithmetic' })
        p95_method = 'nearest_rank'
    }
}

function Get-Difference {
    param($After, $Before)
    if ($null -eq $After -or $null -eq $Before) { return $null }
    return [Math]::Round(([double]$After - [double]$Before), 6)
}

function Save-MeasurementSummary {
    $stageSummaries = New-Object 'System.Collections.Generic.List[object]'
    foreach ($record in $stageRecords) {
        $stageRows = @($allRows.ToArray() | Where-Object { $_.stage -eq $record.id })
        $presenceOk = $stageRows.Count -gt 0 -and @($stageRows | Where-Object { -not $_.stage_process_presence_ok }).Count -eq 0
        $groups = New-Object 'System.Collections.Generic.List[object]'
        foreach ($group in $processGroups) {
            $rows = @($stageRows | Where-Object { $_.process_group -eq $group })
            $processMaximum = 0
            if ($rows.Count -gt 0) { $processMaximum = ($rows | Measure-Object process_count -Maximum).Maximum }
            $groups.Add([pscustomobject][ordered]@{
                process_group = $group
                samples = $rows.Count
                max_process_count = $processMaximum
                churn_intervals = @($rows | Where-Object { $_.added_process_identities -gt 0 -or $_.removed_process_identities -gt 0 }).Count
                unreadable_intervals = @($rows | Where-Object { $_.cpu_sample_status -match 'unreadable_process|unclassified_process' }).Count
                cpu_total_capacity_percent = Get-MetricSummary $rows 'cpu_total_capacity_percent' -WeightByInterval
                private_memory_mib = Get-MetricSummary $rows 'private_memory_mib'
            })
        }
        $stageSummaries.Add([pscustomobject][ordered]@{
            id = $record.id
            status = $record.status
            started_utc = $record.started_utc
            finished_utc = $record.finished_utc
            actual_seconds = $record.actual_seconds
            process_presence_ok = $presenceOk
            connection_and_traffic_confirmed_by_user = ($record.id -ne 'baseline')
            groups = $groups.ToArray()
        })
    }
    $comparisons = New-Object 'System.Collections.Generic.List[object]'
    $baseline = @($stageSummaries.ToArray() | Where-Object { $_.id -eq 'baseline' })
    if ($baseline.Count -eq 1) {
        foreach ($stage in @($stageSummaries.ToArray() | Where-Object { $_.id -ne 'baseline' })) {
            foreach ($group in $processGroups) {
                $before = @($baseline[0].groups | Where-Object { $_.process_group -eq $group })[0]
                $after = @($stage.groups | Where-Object { $_.process_group -eq $group })[0]
                $conditionsOk = $baseline[0].status -eq 'complete' -and $stage.status -eq 'complete' -and $baseline[0].process_presence_ok -and $stage.process_presence_ok
                $comparisons.Add([pscustomobject][ordered]@{
                    stage = $stage.id
                    compared_with = 'baseline'
                    process_group = $group
                    stage_conditions_ok = $conditionsOk
                    comparison_scope = $(if ($stage.id -eq 'b_traffic') { 'Includes traffic change as well as B; not an isolated B overhead estimate.' } else { 'Idle B versus idle Discord baseline; background variation remains possible.' })
                    cpu_mean_delta_percentage_points = Get-Difference $after.cpu_total_capacity_percent.mean $before.cpu_total_capacity_percent.mean
                    private_memory_mean_delta_mib = Get-Difference $after.private_memory_mib.mean $before.private_memory_mib.mean
                    baseline_valid_cpu_samples = $before.cpu_total_capacity_percent.valid_samples
                    stage_valid_cpu_samples = $after.cpu_total_capacity_percent.valid_samples
                    baseline_excluded_cpu_samples = $before.cpu_total_capacity_percent.excluded_samples
                    stage_excluded_cpu_samples = $after.cpu_total_capacity_percent.excluded_samples
                })
            }
        }
    }
    $summary = [pscustomobject][ordered]@{
        schema_version = 1
        started_utc = $runStartedUtc
        updated_utc = [DateTime]::UtcNow.ToString('o')
        run_complete = $runComplete
        requested_seconds_per_stage = $Seconds
        requested_interval_milliseconds = $IntervalMilliseconds
        settle_seconds_per_stage = $SettleSeconds
        logical_processors = $logicalProcessors
        logical_processor_count_source = $logicalProcessorSource
        powershell_version = $PSVersionTable.PSVersion.ToString()
        process_filters = [pscustomobject]@{ Discord = 'Discord*'; Sideglow = 'sideglow' }
        notes = @(
            'Local process counters only; no chat messages, credentials, application settings, or network activity are collected or generated.',
            'CPU is normalized to total logical-processor capacity. 100 means the full machine capacity; deltas are percentage points.',
            'CPU means are duration-weighted. Memory means are arithmetic sample means. Max and p95 are interval/sample statistics, not instantaneous peaks.',
            'CPU intervals with changed/unreadable identities are excluded, not counted as zero. PID plus process start time identifies a process.',
            'Processes that start and exit entirely between samples cannot be observed. Process churn or missing samples can bias comparisons.',
            'Private memory is private allocated bytes divided by 1048576, not physical working set, VRAM, or total system memory impact.',
            'Readiness and real B connection are confirmed manually. This tool does not authenticate to Discord or prove that IPC permissions work.',
            'Results are sequential observations, not a controlled proof of causation or an automatic performance pass/fail.'
        )
        stages = $stageSummaries.ToArray()
        comparisons = $comparisons.ToArray()
    }
    $json = $summary | ConvertTo-Json -Depth 10
    [System.IO.File]::WriteAllText($summaryPath, $json, (New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false))
}

$logicalProcessorSource = 'Win32_ComputerSystem.NumberOfLogicalProcessors'
try {
    $logicalProcessors = [int](Get-CimInstance -ClassName Win32_ComputerSystem -Property NumberOfLogicalProcessors -ErrorAction Stop).NumberOfLogicalProcessors
    if ($logicalProcessors -lt 1) { throw 'Logical processor count unavailable.' }
}
catch {
    $logicalProcessors = [Environment]::ProcessorCount
    $logicalProcessorSource = 'Environment.ProcessorCount (fallback)'
    Write-Warning 'CPU count used the .NET fallback. On large or CPU-restricted systems, verify logical_processors in summary.json.'
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Read-Host 'Local output folder (Enter: .\b-measurements)'
    if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { $OutputDirectory = '.\b-measurements' }
}
if (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    if ((Get-Location).Provider.Name -ne 'FileSystem') { throw 'Choose a filesystem output folder.' }
    $OutputDirectory = Join-Path (Get-Location).Path $OutputDirectory
}
$fullOutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
if ($fullOutputDirectory.StartsWith('\\')) { throw 'Choose a local output folder, not a UNC network path.' }
$outputDrive = New-Object -TypeName System.IO.DriveInfo -ArgumentList ([System.IO.Path]::GetPathRoot($fullOutputDirectory))
if ($outputDrive.DriveType -eq [System.IO.DriveType]::Network) { throw 'Choose a local output folder, not a mapped network drive.' }
$runFolder = 'sideglow-b-{0}-{1}' -f [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'), [Guid]::NewGuid().ToString('N').Substring(0, 8)
$resultDirectory = Join-Path $fullOutputDirectory $runFolder
[System.IO.Directory]::CreateDirectory($resultDirectory) | Out-Null
$samplesPath = Join-Path $resultDirectory 'samples.csv'
$summaryPath = Join-Path $resultDirectory 'summary.json'
$runStartedUtc = [DateTime]::UtcNow.ToString('o')
$runComplete = $false
$allRows = New-Object 'System.Collections.Generic.List[object]'
$stageRecords = New-Object 'System.Collections.Generic.List[object]'
$stages = @(
    [pscustomobject]@{ id = 'baseline'; instruction = 'Keep Discord running without new chat traffic. Completely exit Sideglow. Keep the same game, call, channel, and foreground state for all stages.' },
    [pscustomobject]@{ id = 'b_idle'; instruction = 'Start Sideglow and complete the real B connection to one channel. Wait for initial history to load. Keep it connected with no new chat traffic.' }
)
if ($IncludeTraffic) {
    $stages += [pscustomobject]@{ id = 'b_traffic'; instruction = 'Keep B connected and observe ordinary, manually arranged chat traffic. This script will not send any messages.' }
}

Write-Host ('Results: {0}' -f $resultDirectory)
Write-Host ('CPU normalization: {0} logical processors; stage duration: {1} seconds.' -f $logicalProcessors, $Seconds)
Write-Host 'ON/OFF may only hide the overlay; it is not the disconnected baseline. Exit Sideglow for baseline.'
Write-Host 'Keep Discord/Sideglow executable names unchanged. The script does not launch, close, or configure them.'

try {
    foreach ($stage in $stages) {
        Write-Host ''
        Write-Host ('[{0}] {1}' -f $stage.id, $stage.instruction)
        do {
            Read-Host ('Press Enter when ready for {0}; Ctrl+C cancels' -f $stage.id) | Out-Null
            $readySnapshot = Get-ProcessSnapshot
            $ready = Test-StagePresence $readySnapshot $stage.id
            if (-not $ready) {
                Write-Warning ('Unexpected processes: Discord={0}, Sideglow={1}. Baseline needs Discord only; B stages need both.' -f $readySnapshot.Observed.Discord, $readySnapshot.Observed.Sideglow)
            }
        } while (-not $ready)

        $record = [pscustomobject]@{
            id = $stage.id
            status = 'interrupted'
            started_utc = [DateTime]::UtcNow.ToString('o')
            finished_utc = $null
            actual_seconds = 0.0
        }
        $stageRecords.Add($record)
        Write-Host ('Settling for {0} seconds, then measuring for {1} seconds. Return to the same game/foreground state now.' -f $SettleSeconds, $Seconds)
        if ($SettleSeconds -gt 0) { Start-Sleep -Seconds $SettleSeconds }
        $previous = Get-ProcessSnapshot
        $record.started_utc = $previous.Utc
        $stageStarted = $previous.ElapsedSeconds
        $deadline = $stageStarted + $Seconds
        $sampleIndex = 0
        try {
            while ($previous.ElapsedSeconds -lt $deadline) {
                $nextSample = [Math]::Min($deadline, $previous.ElapsedSeconds + $IntervalMilliseconds / 1000.0)
                $waitMilliseconds = [int][Math]::Ceiling(($nextSample - $measurementClock.Elapsed.TotalSeconds) * 1000.0)
                if ($waitMilliseconds -gt 0) { Start-Sleep -Milliseconds $waitMilliseconds }
                $current = Get-ProcessSnapshot
                $sampleIndex++
                $intervalRows = @(
                    foreach ($group in $processGroups) {
                        $row = Get-IntervalRow $previous $current $stage.id $group $sampleIndex
                        $allRows.Add($row)
                        $row
                    }
                )
                if (Test-Path -LiteralPath $samplesPath) {
                    $intervalRows | Export-Csv -LiteralPath $samplesPath -NoTypeInformation -Encoding UTF8 -Append
                }
                else {
                    $intervalRows | Export-Csv -LiteralPath $samplesPath -NoTypeInformation -Encoding UTF8
                }
                $previous = $current
                $record.actual_seconds = [Math]::Round($current.ElapsedSeconds - $stageStarted, 6)
            }
            $record.status = 'complete'
        }
        finally {
            $record.finished_utc = [DateTime]::UtcNow.ToString('o')
        }
        Save-MeasurementSummary
        Write-Host ('Completed {0}: {1} intervals.' -f $stage.id, $sampleIndex)
    }
    $runComplete = $true
}
finally {
    Save-MeasurementSummary
    Write-Host ''
    if (Test-Path -LiteralPath $samplesPath) { Write-Host ('Samples: {0}' -f $samplesPath) }
    else { Write-Host 'No interval samples were recorded.' }
    Write-Host ('Summary: {0}' -f $summaryPath)
    Write-Host 'Read stage_conditions_ok, excluded sample counts, and both process groups before interpreting deltas.'
}
