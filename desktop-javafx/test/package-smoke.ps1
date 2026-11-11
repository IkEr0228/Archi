param([Parameter(Mandatory)][string]$Application)
$ErrorActionPreference = 'Stop'
$Application = (Resolve-Path $Application).Path
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$fixture = Join-Path $root 'src-tauri/tests/fixtures/version.rar'
$launcher = Start-Process -FilePath $Application -ArgumentList ('"' + $fixture + '"') -WorkingDirectory $env:TEMP -PassThru
$window = $null
$workers = @()
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(25)
    do {
        # Windows jpackage may relaunch itself: inspect only this launcher's child.
        $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($launcher.Id)" | Where-Object { $_.ExecutablePath -eq $Application })
        foreach ($id in @($launcher.Id) + @($children.ProcessId)) {
            $candidate = Get-Process -Id $id -ErrorAction SilentlyContinue
            if ($candidate -and $candidate.MainWindowTitle -like 'version.rar*') { $window = $candidate; break }
        }
        if (!$window) { Start-Sleep -Milliseconds 100 }
    } while (!$window -and [DateTime]::UtcNow -lt $deadline)
    if (!$window) { throw 'Packaged archive did not open from an unrelated working directory.' }
    $workers = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($window.Id)" | Where-Object { $_.Name -eq 'archi-worker.exe' })
    if ($workers.Count -ne 1 -or $workers[0].ExecutablePath -ne (Join-Path (Split-Path $Application -Parent) 'app/archi-worker.exe')) { throw 'Bundled worker discovery failed.' }
    if (!$window.CloseMainWindow() -or !$window.WaitForExit(10000) -or !$launcher.WaitForExit(10000)) { throw 'Packaged application did not close.' }
    foreach ($worker in $workers) {
        if (Get-Process -Id $worker.ProcessId -ErrorAction SilentlyContinue) { throw 'Worker survived packaged application shutdown.' }
    }
    Write-Host 'Packaged runtime, archive opening, worker discovery and shutdown passed.'
} finally {
    foreach ($owned in @($window, $launcher)) {
        if ($owned -and !$owned.HasExited) { Stop-Process -Id $owned.Id -Force -ErrorAction SilentlyContinue }
    }
    foreach ($worker in $workers) { Stop-Process -Id $worker.ProcessId -Force -ErrorAction SilentlyContinue }
}
