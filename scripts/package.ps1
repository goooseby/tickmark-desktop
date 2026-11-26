$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$hint = Join-Path $project '.tools/rust-bin.txt'
if ($env:TICKMARK_RUST_BIN) {
    $bin = $env:TICKMARK_RUST_BIN
} elseif (Test-Path -LiteralPath $hint) {
    $bin = (Get-Content -Raw -LiteralPath $hint).Trim()
} else {
    $command = Get-Command cargo.exe -ErrorAction Stop
    $bin = Split-Path -Parent $command.Source
}
$env:CARGO_HOME = Join-Path $project '.tools/cargo-cache'
$env:CARGO_TARGET_DIR = Join-Path $project 'artifacts/rust'
$env:PATH = "$bin;$env:PATH"

$cli = Join-Path $project 'artifacts/rust/release/tickmark.exe'
if (-not (Test-Path -LiteralPath $cli)) {
    throw 'Missing release tickmark.exe. Run: powershell scripts/rust.ps1 build --workspace --release --locked'
}

$sidecar = Join-Path $project 'native/ui/binaries'
New-Item -ItemType Directory -Force -Path $sidecar | Out-Null
Copy-Item -LiteralPath $cli -Destination (Join-Path $sidecar 'tickmark-x86_64-pc-windows-msvc.exe') -Force

$tauri = Join-Path $bin 'cargo-tauri.exe'
if (-not (Test-Path -LiteralPath $tauri)) {
    $tauri = Join-Path $env:CARGO_HOME 'bin/cargo-tauri.exe'
}
if (-not (Test-Path -LiteralPath $tauri)) {
    throw 'Missing cargo-tauri. Run: powershell scripts/rust.ps1 install tauri-cli --version "^2" --locked'
}

Push-Location (Join-Path $project 'native/ui')
try {
    & $tauri build --bundles nsis --config tauri.bundle.conf.json
    if ($LASTEXITCODE -ne 0) { throw "cargo tauri build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$setup = Get-ChildItem (Join-Path $project 'artifacts/rust/release/bundle/nsis') -Filter '*-setup.exe' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($setup) {
    Write-Host "Installer: $($setup.FullName)"
}
