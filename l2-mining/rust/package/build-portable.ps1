param(
    [string]$OutputDir = ".\dist"
)

$ErrorActionPreference = 'Stop'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptRoot

try {
    cargo build --release --bins

    $packageRoot = Join-Path $scriptRoot "portable\TemporalGradientMiner-win-x64"
    $binDir = Join-Path $packageRoot "bin"
    $configDir = Join-Path $packageRoot "config"

    Remove-Item $packageRoot -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    New-Item -ItemType Directory -Force -Path $configDir | Out-Null

    Copy-Item ".\target\release\tg-miner-installer.exe" (Join-Path $binDir "tg-miner-installer.exe") -Force
    Copy-Item ".\target\release\temporal-gradient-miner.exe" (Join-Path $binDir "temporal-gradient-miner.exe") -Force
    & (Join-Path $binDir "tg-miner-installer.exe") write-config --output (Join-Path $configDir "miner-config.json") | Out-Host
    Copy-Item ".\README.md" (Join-Path $packageRoot "README.md") -Force
    Copy-Item ".\GETTING_STARTED.md" (Join-Path $packageRoot "GETTING_STARTED.md") -Force

    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    $zipPath = Join-Path $OutputDir "TemporalGradientMiner-win-x64.zip"
    if (Test-Path $zipPath) {
        Remove-Item $zipPath -Force
    }

    Compress-Archive -Path (Join-Path $packageRoot "*") -DestinationPath $zipPath
    Write-Host "Portable package created at $zipPath"
}
finally {
    Pop-Location
}