param([switch]$Run, [switch]$Test, [switch]$Smoke, [switch]$Release, [string]$Archive = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$cache = Join-Path $root '.cache/javafx'
$classes = Join-Path $PSScriptRoot 'build/classes'
New-Item -ItemType Directory -Force $cache, $classes | Out-Null

$artifacts = @(
    @('org/openjfx/javafx-base/21.0.10/javafx-base-21.0.10-win.jar', '527C8088E1BF106A73BA3005A512B656CC9F49E12E0186D16268DFB181141332'),
    @('org/openjfx/javafx-graphics/21.0.10/javafx-graphics-21.0.10-win.jar', '9A2F35F836453F710EBC7F5E2A8D0D3BC08CEDEA9FA4B2FE8D0A178B51080A4E'),
    @('org/openjfx/javafx-controls/21.0.10/javafx-controls-21.0.10-win.jar', '3ED969AEF4232E57AA12E574227C93287A7D75954EC5A53FE08ED747242659D2'),
    @('com/google/code/gson/gson/2.13.2/gson-2.13.2.jar', 'DD0CE1B55A3ED2080CB70F9C655850CDA86C206862310009DCB5E5C95265A5E0')
)
foreach ($artifact in $artifacts) {
    $file = Join-Path $cache ($artifact[0].Split('/')[-1])
    if (!(Test-Path $file)) {
        Invoke-WebRequest -UseBasicParsing ('https://repo.maven.apache.org/maven2/' + $artifact[0]) -OutFile $file
    }
    if ((Get-FileHash $file -Algorithm SHA256).Hash -ne $artifact[1]) { throw "Dependency checksum mismatch: $file" }
}
$java = if ($env:JAVA_HOME -and (Test-Path "$env:JAVA_HOME/bin/javac.exe")) { "$env:JAVA_HOME/bin" } else { Split-Path (Get-Command javac).Source }
$modulePath = ((Get-ChildItem "$cache/javafx-*.jar").FullName -join ';')
$gson = Join-Path $cache 'gson-2.13.2.jar'
$sources = @((Get-ChildItem "$PSScriptRoot/src" -Filter '*.java' -Recurse).FullName)
& "$java/javac.exe" --release 21 --module-path $modulePath --add-modules javafx.controls -cp $gson -d $classes @sources
if ($LASTEXITCODE) { throw 'Java compilation failed' }
Copy-Item "$PSScriptRoot/resources/*" $classes -Recurse -Force
if ($Test -or $Smoke) {
    $tests = @((Get-ChildItem "$PSScriptRoot/test" -Filter '*.java' -Recurse).FullName)
    & "$java/javac.exe" --release 21 --module-path $modulePath --add-modules javafx.controls -cp "$gson;$classes" -d $classes @tests
    if ($LASTEXITCODE) { throw 'Test compilation failed' }
    & "$java/java.exe" -ea --module-path $modulePath --add-modules javafx.controls -cp "$gson;$classes" archi.ModelTest
    if ($LASTEXITCODE) { throw 'Java tests failed' }
}
if ($Run -or $Test -or $Smoke) {
    # Rust/MSVC incremental state can produce stale anonymous-symbol LNK2019 errors
    # after interrupted/restarted builds; Java smoke must use a clean worker link.
    $env:CARGO_INCREMENTAL = '0'
    $cargoArgs = @('build', '--locked', '--manifest-path', "$root/Cargo.toml", '-p', 'archi-worker')
    if ($Release) { $cargoArgs += '--release' }
    & cargo @cargoArgs
    if ($LASTEXITCODE) { throw 'Worker build failed' }
    $profile = if ($Release) { 'release' } else { 'debug' }
}
if ($Test) {
    & "$java/java.exe" -ea "-Darchi.worker=$root/target/$profile/archi-worker.exe" --module-path $modulePath --add-modules javafx.controls -cp "$gson;$classes" archi.WorkerTest
    if ($LASTEXITCODE) { throw 'Java worker integration tests failed' }
}
if ($Smoke) {
    & "$java/java.exe" -ea "-Darchi.worker=$root/target/$profile/archi-worker.exe" "-Darchi.smoke.output=$PSScriptRoot/build" --module-path $modulePath --add-modules javafx.controls -cp "$gson;$classes" archi.FxSmokeTest
    if ($LASTEXITCODE) { throw 'JavaFX smoke test failed' }
    & "$java/java.exe" -ea "-Darchi.worker=$root/target/$profile/archi-worker.exe" "-Darchi.smoke.output=$PSScriptRoot/build" --module-path $modulePath --add-modules javafx.controls -cp "$gson;$classes" archi.FxSmokeTest "$root/crates/archi-core/tests/fixtures/comment-hpw-password.rar"
    if ($LASTEXITCODE) { throw 'JavaFX password smoke test failed' }
}
if ($Run) {
    & "$java/java.exe" "-Darchi.worker=$root/target/$profile/archi-worker.exe" --module-path $modulePath --add-modules javafx.controls -cp "$gson;$classes" archi.Launcher $Archive
    if ($LASTEXITCODE) { throw 'JavaFX application failed' }
}
