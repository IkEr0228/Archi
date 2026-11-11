param(
    [Parameter(Mandatory = $true)][string]$Package,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'Programs/Archi'),
    [switch]$RegisterOnly,
    [switch]$Uninstall,
    [switch]$KeepFiles,
    [switch]$WhatIf
)
$ErrorActionPreference = 'Stop'
$WhatIfPreference = $WhatIf.IsPresent

$progId = 'Archi.Archive'
$extensions = @('.zip', '.7z', '.rar', '.tar', '.gz', '.bz2', '.xz')
$classes = 'HKCU:\Software\Classes'
$commandKeys = @(
    "$classes\$progId\shell\open\command"
)
foreach ($extension in $extensions) {
    $commandKeys += "$classes\SystemFileAssociations\$extension\shell\ArchiOpen\command"
    $commandKeys += "$classes\SystemFileAssociations\$extension\shell\ArchiExtractHere\command"
}

function Invoke-Registry([scriptblock]$Action) {
    & $Action
}

if ($Uninstall) {
    foreach ($key in $commandKeys) {
        if (Test-Path $key) { Remove-Item (Split-Path $key -Parent) -Recurse -Force -ErrorAction SilentlyContinue }
    }
    $progPath = "$classes\$progId"
    if (Test-Path $progPath) { Remove-Item $progPath -Recurse -Force -ErrorAction SilentlyContinue }
    foreach ($extension in $extensions) {
        $key = "$classes\$extension"
        if ((Test-Path $key) -and (Get-ItemPropertyValue $key -Name '(default)' -ErrorAction SilentlyContinue) -eq $progId) {
            Remove-ItemProperty $key -Name '(default)' -ErrorAction SilentlyContinue
        }
    }
    if (!$KeepFiles -and (Test-Path $InstallRoot)) { Remove-Item $InstallRoot -Recurse -Force }
    Write-Host "Archi registration removed."
    exit 0
}

$packagePath = (Resolve-Path $Package).Path
$exe = Join-Path $packagePath 'Archi.exe'
$worker = Join-Path $packagePath 'app/archi-worker.exe'
if (!(Test-Path $exe) -or !(Test-Path $worker)) { throw 'Package must contain Archi.exe and app/archi-worker.exe.' }

if ($WhatIf) {
    Write-Host "WhatIf: install package from $packagePath"
    Write-Host "WhatIf: register $($extensions.Count) archive associations under $classes"
    Write-Host "WhatIf: register Open and Extract here commands for current user"
    exit 0
}

if (!$RegisterOnly) {
    New-Item -ItemType Directory -Force $InstallRoot | Out-Null
    Copy-Item (Join-Path $packagePath '*') $InstallRoot -Recurse -Force
    $exe = Join-Path $InstallRoot 'Archi.exe'
}

$openCommand = '"' + $exe + '" "%1"'
$extractCommand = '"' + $exe + '" --extract-here "%1"'
Invoke-Registry { New-Item "$classes\$progId\shell\open\command" -Force }
Invoke-Registry { Set-ItemProperty "$classes\$progId" -Name '(default)' -Value 'Archi archive' }
Invoke-Registry { Set-ItemProperty "$classes\$progId\shell\open\command" -Name '(default)' -Value $openCommand }
foreach ($extension in $extensions) {
    $openKey = "$classes\SystemFileAssociations\$extension\shell\ArchiOpen\command"
    $extractKey = "$classes\SystemFileAssociations\$extension\shell\ArchiExtractHere\command"
    Invoke-Registry { New-Item $openKey -Force }
    Invoke-Registry { Set-ItemProperty (Split-Path $openKey -Parent) -Name '(default)' -Value 'Open with Archi' }
    Invoke-Registry { Set-ItemProperty $openKey -Name '(default)' -Value $openCommand }
    Invoke-Registry { New-Item $extractKey -Force }
    Invoke-Registry { Set-ItemProperty (Split-Path $extractKey -Parent) -Name '(default)' -Value 'Extract here with Archi' }
    Invoke-Registry { Set-ItemProperty $extractKey -Name '(default)' -Value $extractCommand }
    Invoke-Registry { New-Item "$classes\$extension" -Force }
    Invoke-Registry { Set-ItemProperty "$classes\$extension" -Name '(default)' -Value $progId }
}
Write-Host "Archi installed: $exe"
Write-Host 'File associations and Explorer extraction commands registered for current user.'
