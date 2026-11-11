param([Parameter(Mandatory = $true)][string]$Package, [switch]$Registry)
$ErrorActionPreference = 'Stop'
$packagePath = (Resolve-Path $Package).Path
if (!(Test-Path (Join-Path $packagePath 'Archi.exe'))) { throw 'Missing packaged launcher.' }
if (!(Test-Path (Join-Path $packagePath 'app/archi-worker.exe'))) { throw 'Missing packaged worker.' }
$script = Join-Path (Split-Path $PSScriptRoot -Parent) 'install.ps1'
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script -Package $packagePath -RegisterOnly -WhatIf
if ($LASTEXITCODE) { throw 'Installer registration validation failed.' }
if ($Registry) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script -Package $packagePath -RegisterOnly
    if ($LASTEXITCODE) { throw 'Installer registration failed.' }
    $classes = 'HKCU:\Software\Classes'
    foreach ($extension in @('.zip', '.7z', '.rar', '.tar', '.gz', '.bz2', '.xz')) {
        $open = "$classes\SystemFileAssociations\$extension\shell\ArchiOpen\command"
        $extract = "$classes\SystemFileAssociations\$extension\shell\ArchiExtractHere\command"
        if (!(Test-Path $open) -or !(Test-Path $extract)) { throw "Missing registry verbs for $extension." }
        if ((Get-ItemPropertyValue $open -Name '(default)') -notlike '*--extract-here*' -and
            (Get-ItemPropertyValue $extract -Name '(default)') -notlike '*--extract-here*') {
            throw "Extract command missing for $extension."
        }
    }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script -Package $packagePath -Uninstall -KeepFiles
    if ($LASTEXITCODE) { throw 'Installer uninstall failed.' }
    if (Test-Path "$classes\SystemFileAssociations\.zip\shell\ArchiExtractHere") {
        throw 'Registry uninstall left archive verb.'
    }
}
Write-Host 'Installer package and per-user registration commands passed.'
