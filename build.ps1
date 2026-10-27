param([switch]$SkipTests)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
function Run-Checked { param([scriptblock]$Command) & $Command; if ($LASTEXITCODE -ne 0) { throw "Build step failed: $Command" } }
Run-Checked { rustup target add wasm32-unknown-unknown }
if (-not (Get-Command wasm-bindgen -ErrorAction SilentlyContinue)) { Run-Checked { cargo install wasm-bindgen-cli --version 0.2.129 --locked } }
Run-Checked { cargo build --locked -p rustlink-web --target wasm32-unknown-unknown --release }
Run-Checked { wasm-bindgen target/wasm32-unknown-unknown/release/rustlink_web.wasm --target web --out-dir web/dist/pkg --no-typescript }
if (-not $SkipTests) {
    Run-Checked { cargo test --locked --workspace }
    Run-Checked { cargo clippy --locked --workspace --all-targets -- -D warnings }
}
Run-Checked { cargo build --locked --release -p rustlink }
Write-Host "Built standalone gateway: $PSScriptRoot\target\release\rustlink.exe"
