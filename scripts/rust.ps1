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
$cargo = Join-Path $bin 'cargo.exe'
$env:PATH = "$bin;$env:PATH"
$env:CARGO_HOME = Join-Path $project '.tools/cargo-cache'
$env:CARGO_TARGET_DIR = Join-Path $project 'artifacts/rust'
Push-Location (Join-Path $project 'native')
try {
    & $cargo @args
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
