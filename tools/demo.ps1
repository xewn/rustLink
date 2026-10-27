$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$data = Join-Path $PSScriptRoot 'demo-data'
New-Item -ItemType Directory -Force -Path $data | Out-Null
if (-not (Test-Path -LiteralPath (Join-Path $data 'config.json'))) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'config.simulated.json') -Destination (Join-Path $data 'config.json')
}
$simulator = Start-Process -FilePath (Join-Path $PSScriptRoot 'simulator.exe') -WindowStyle Hidden -PassThru
try {
    & (Join-Path $PSScriptRoot 'rustlink.exe') --data-dir $data --auto-start
} finally {
    if (-not $simulator.HasExited) { Stop-Process -Id $simulator.Id }
}
