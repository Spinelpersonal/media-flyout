[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(1, 2147483647)]
    [int]$ProcessId,
    [ValidateRange(2, 3600)]
    [int]$Seconds = 30,
    [string]$Csv
)

$ErrorActionPreference = 'Stop'
$taskProcess = Get-Process -Id $ProcessId
$taskProcess.Refresh()
$taskInitialCpu = $taskProcess.TotalProcessorTime.TotalSeconds
$taskClock = [System.Diagnostics.Stopwatch]::StartNew()
$taskSamples = [System.Collections.Generic.List[object]]::new()

for ($taskIndex = 0; $taskIndex -lt $Seconds; $taskIndex++) {
    Start-Sleep -Seconds 1
    $taskProcess.Refresh()
    if ($taskProcess.HasExited) { throw 'The measured process exited.' }
    $taskSamples.Add([pscustomobject]@{
        ElapsedSeconds = [Math]::Round($taskClock.Elapsed.TotalSeconds, 3)
        WorkingSetMiB = $taskProcess.WorkingSet64 / 1MB
        PrivateMiB = $taskProcess.PrivateMemorySize64 / 1MB
        CpuSeconds = $taskProcess.TotalProcessorTime.TotalSeconds
        Threads = $taskProcess.Threads.Count
        Handles = $taskProcess.HandleCount
    })
}

$taskClock.Stop()
$taskCpuDelta = $taskProcess.TotalProcessorTime.TotalSeconds - $taskInitialCpu
$taskWorkingSet = $taskSamples | Measure-Object -Property WorkingSetMiB -Average -Maximum
$taskPrivate = $taskSamples | Measure-Object -Property PrivateMiB -Average -Maximum

if ($Csv) { $taskSamples | Export-Csv -LiteralPath $Csv -NoTypeInformation }

[pscustomobject]@{
    ProcessId = $ProcessId
    ElapsedSeconds = [Math]::Round($taskClock.Elapsed.TotalSeconds, 2)
    WorkingSetMeanMiB = [Math]::Round($taskWorkingSet.Average, 2)
    WorkingSetPeakMiB = [Math]::Round($taskWorkingSet.Maximum, 2)
    PrivateMeanMiB = [Math]::Round($taskPrivate.Average, 2)
    PrivatePeakMiB = [Math]::Round($taskPrivate.Maximum, 2)
    CpuSeconds = [Math]::Round($taskCpuDelta, 4)
    CpuPercentOfMachine = [Math]::Round(100 * $taskCpuDelta / $taskClock.Elapsed.TotalSeconds / [Environment]::ProcessorCount, 4)
    CpuPercentOfOneCore = [Math]::Round(100 * $taskCpuDelta / $taskClock.Elapsed.TotalSeconds, 4)
    ThreadsAtEnd = $taskProcess.Threads.Count
    HandlesAtEnd = $taskProcess.HandleCount
}
