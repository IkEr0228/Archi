param([string]$Destination = '', [switch]$SkipBuild, [switch]$Verify, [switch]$Installer)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (!$Destination) { $Destination = Join-Path $root 'dist-release/javafx' }
$Destination = [IO.Path]::GetFullPath($Destination)
if (Test-Path (Join-Path $Destination 'Archi')) { throw 'Archi output already exists. Choose a new -Destination; existing builds are not deleted.' }
$java = if ($env:JAVA_HOME -and (Test-Path "$env:JAVA_HOME/bin/jpackage.exe")) { "$env:JAVA_HOME/bin" } else { Split-Path (Get-Command jpackage).Source }
if (!$SkipBuild) {
    & "$PSScriptRoot/build.ps1"
    & cargo build --offline --locked --release --manifest-path "$root/Cargo.toml" -p archi-worker
    if ($LASTEXITCODE) { throw 'Release worker build failed' }
}
$classes = Join-Path $PSScriptRoot 'build/classes'
if (!(Test-Path "$classes/archi/Launcher.class") -or !(Test-Path "$root/target/release/archi-worker.exe")) { throw 'Compile Java classes and release worker before packaging.' }
$cache = Join-Path $root '.cache/javafx'
$scratch = Join-Path $PSScriptRoot ('build/package-' + [guid]::NewGuid().ToString('N'))
$input = Join-Path $scratch 'input'
$runtime = Join-Path $scratch 'runtime'
New-Item -ItemType Directory -Force $input, $Destination | Out-Null
$manifest = Join-Path $scratch 'MANIFEST.MF'
"Manifest-Version: 1.0`r`nMain-Class: archi.Launcher`r`nClass-Path: gson-2.13.2.jar`r`n`r`n" | Set-Content -Encoding Ascii $manifest
# Package source-backed production classes only; build/classes also holds tests.
$staging = Join-Path $scratch 'classes'
New-Item -ItemType Directory -Force "$staging/archi" | Out-Null
$names = @((Get-ChildItem "$PSScriptRoot/src/archi" -Filter '*.java').BaseName)
Get-ChildItem "$classes/archi" -Filter '*.class' | Where-Object { ($_.BaseName -split '\$')[0] -in $names } | Copy-Item -Destination "$staging/archi"
Copy-Item "$classes/archi.css" $staging
& "$java/jar.exe" --create --file "$input/archi.jar" --manifest $manifest -C $staging .
if ($LASTEXITCODE) { throw 'Application JAR creation failed' }
Copy-Item "$cache/gson-2.13.2.jar", "$root/target/release/archi-worker.exe" $input
$modules = ((Get-ChildItem "$cache/javafx-*-win.jar").FullName -join ';') + ';' + (Join-Path (Split-Path $java -Parent) 'jmods')
& "$java/jlink.exe" --module-path $modules --add-modules javafx.controls,java.desktop,java.logging,jdk.unsupported --strip-debug --no-header-files --no-man-pages --output $runtime
if ($LASTEXITCODE) { throw 'Java runtime creation failed' }
& "$java/jpackage.exe" --type app-image --name Archi --app-version 0.5.0 --vendor Archi --input $input --main-jar archi.jar --main-class archi.Launcher --runtime-image $runtime --java-options '--add-modules=javafx.controls' --dest $Destination
if ($LASTEXITCODE) { throw 'Application image creation failed' }
$licenses = Join-Path $Destination 'Archi/app/licenses'
New-Item -ItemType Directory -Force $licenses | Out-Null
Copy-Item "$root/LICENSE" "$licenses/Archi-MIT.txt"
Copy-Item "$root/vendor/sevenz-rust2/LICENSE" "$licenses/sevenz-rust2-Apache-2.0.txt"
Remove-Item $scratch -Recurse -Force
Write-Host "Portable application: $Destination/Archi/Archi.exe"
Write-Host 'Java runtime included.'
if ($Installer) {
    $installerDir = Join-Path $Destination 'installer'
    New-Item -ItemType Directory -Force $installerDir | Out-Null
    Copy-Item "$PSScriptRoot/install.ps1" $installerDir -Force
    Write-Host "Installer script: $installerDir/install.ps1"
    Write-Host 'Run install.ps1 -Package <Archi directory> to install and register current-user associations.'
}
if ($Verify) { & "$PSScriptRoot/test/package-smoke.ps1" -Application (Join-Path $Destination 'Archi/Archi.exe') }
