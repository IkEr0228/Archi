$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent | Split-Path -Parent
$revision = '8799378'
$sourceRoot = Join-Path $root 'desktop-javafx/build/benchmark/sources'
$classRoot = Join-Path $root 'desktop-javafx/build/benchmark/classes'
$baselineSource = Join-Path $sourceRoot 'baseline/ArchiveModel.java'
$currentSource = Join-Path $root 'desktop-javafx/src/archi/ArchiveModel.java'
$gson = Join-Path $root '.cache/javafx/gson-2.13.2.jar'
$resultsRoot = Join-Path $root 'benchmark-results'
$out = Join-Path $resultsRoot 'java-archive-model-raw.jsonl'
$rawJson = Join-Path $resultsRoot 'java-archive-model-raw.json'
$summary = Join-Path $resultsRoot 'java-archive-model-summary.json'
$benchSource = Join-Path $PSScriptRoot 'src/archi/Bench.java'

if (!(Test-Path $gson)) { throw "Missing existing Gson dependency: $gson" }
$expectedGsonSha256 = 'DD0CE1B55A3ED2080CB70F9C655850CDA86C206862310009DCB5E5C95265A5E0'
$actualGsonSha256 = (Get-FileHash $gson -Algorithm SHA256).Hash
if ($actualGsonSha256 -ne $expectedGsonSha256) { throw "Gson checksum mismatch: $actualGsonSha256" }
if (!(Test-Path $currentSource)) { throw "Missing current ArchiveModel: $currentSource" }
& git --no-pager -C $root cat-file -e "$revision^{commit}"
if ($LASTEXITCODE) { throw "Git revision not found: $revision" }

New-Item -ItemType Directory -Force (Split-Path $baselineSource), (Join-Path $classRoot 'baseline'), (Join-Path $classRoot 'current'), $resultsRoot | Out-Null
$baselineText = (& git --no-pager -C $root show "$revision`:desktop-javafx/src/archi/ArchiveModel.java") -join "`n"
if ($LASTEXITCODE) { throw "Could not read ArchiveModel from $revision" }
[IO.File]::WriteAllText($baselineSource, $baselineText, [Text.UTF8Encoding]::new($false))

$javac = (Get-Command javac).Source
& $javac --release 21 -cp $gson -d (Join-Path $classRoot 'baseline') $benchSource $baselineSource
if ($LASTEXITCODE) { throw 'Baseline Java compilation failed' }
& $javac --release 21 -cp $gson -d (Join-Path $classRoot 'current') $benchSource $currentSource
if ($LASTEXITCODE) { throw 'Current Java compilation failed' }

Remove-Item $out -ErrorAction SilentlyContinue
$java = (Get-Command java).Source
$records = @()
for ($i = 0; $i -lt 12; $i++) {
    $variant = if ($i % 2 -eq 0) { 'baseline' } else { 'current' }
    $classpath = "$gson;$classRoot/$variant"
    $line = & $java -ea -cp $classpath archi.Bench $variant
    if ($LASTEXITCODE) { throw "Benchmark failed: $variant iteration $i" }
    Add-Content -LiteralPath $out -Value $line
    $records += $line | ConvertFrom-Json
}
function Median($values) {
    $sorted = @($values | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) { return [double]$sorted[$middle] }
    return ([double]$sorted[$middle - 1] + [double]$sorted[$middle]) / 2
}
$groups = foreach ($variant in @('baseline', 'current')) {
    $r = @($records | Where-Object variant -eq $variant)
    $extensionP50 = @($r | ForEach-Object { Median $_.extension_ms })
    $folderP50 = @($r | ForEach-Object { Median $_.folder_ms })
    [ordered]@{
        variant = $variant; processes = $r.Count; entries = $r[0].entries
        extension_count = $r[0].extension_count; folder_count = $r[0].folder_count
        extension_process_p50_ms = [ordered]@{ min = ($extensionP50 | Measure-Object -Minimum).Minimum; max = ($extensionP50 | Measure-Object -Maximum).Maximum }
        folder_process_p50_ms = [ordered]@{ min = ($folderP50 | Measure-Object -Minimum).Minimum; max = ($folderP50 | Measure-Object -Maximum).Maximum }
    }
}
$records | ConvertTo-Json -Depth 5 | Set-Content $rawJson
[ordered]@{
    schema = 1; baseline_revision = $revision
    workload = '50k entries; case-insensitive .JaVa extension search/name sort + folder query/size sort'
    warmups = 10; samples_per_process = 20; processes_per_variant = 6
    median = 'conventional median: average of the two middle sorted samples when sample count is even'
    results = $groups
} | ConvertTo-Json -Depth 5 | Set-Content $summary
Write-Host "Raw: $out"
Write-Host "Raw JSON: $rawJson"
Write-Host "Summary: $summary"
Get-Content $summary
