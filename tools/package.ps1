$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$package = Join-Path $root 'dist/RustLink-windows-x64'
New-Item -ItemType Directory -Force -Path $package | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'target/release/rustlink.exe') -Destination $package
Copy-Item -LiteralPath (Join-Path $root 'target/release/examples/simulator.exe') -Destination $package
Copy-Item -LiteralPath (Join-Path $root 'examples/config.simulated.json') -Destination $package
foreach ($file in @('LICENSE', 'NOTICE.md', 'README.md')) {
    Copy-Item -LiteralPath (Join-Path $root $file) -Destination $package
}
Copy-Item -LiteralPath (Join-Path $root 'docs') -Destination $package -Recurse -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'demo.ps1') -Destination $package
$evidence = Join-Path $package 'evidence'
New-Item -ItemType Directory -Force -Path $evidence | Out-Null
foreach ($file in @('benchmark.json', 'differential.json', 'windows-tests.log', 'linux-tests.log', 'windows-clippy.log', 'linux-clippy.log', 'wasm-clippy.log', 'ui.log', 'ui-home.png', 'ui-settings.png')) {
    Copy-Item -LiteralPath (Join-Path $root "artifacts/$file") -Destination $evidence
}
if (Test-Path -LiteralPath (Join-Path $root 'artifacts/package-verification.log')) {
    Copy-Item -LiteralPath (Join-Path $root 'artifacts/package-verification.log') -Destination $evidence
}
# Bundle corresponding source, fixed test inputs and generated browser assets.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$sourceZip = Join-Path $package 'source.zip'
if (Test-Path -LiteralPath $sourceZip) { Remove-Item -LiteralPath $sourceZip }
$archive = [System.IO.Compression.ZipFile]::Open($sourceZip, 'Create')
try {
    $files = @()
    foreach ($dir in @('src', 'web', 'tests', 'tools', 'examples', 'assets', 'docs', '.github')) {
        $files += Get-ChildItem -LiteralPath (Join-Path $root $dir) -File -Recurse | Where-Object { $_.FullName -notmatch '[\\/]__pycache__[\\/]' }
    }
    $files += Get-ChildItem -LiteralPath $root -File
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $relative, 'Optimal') | Out-Null
    }
} finally { $archive.Dispose() }
$zip = Join-Path $root 'dist/RustLink-windows-x64.zip'
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
[System.IO.Compression.ZipFile]::CreateFromDirectory($package, $zip)
$hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText("$zip.sha256", "$hash  RustLink-windows-x64.zip`n")
Write-Host "Packaged: $zip"
Write-Host "SHA256: $hash"
