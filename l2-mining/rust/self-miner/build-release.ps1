<#
.SYNOPSIS
    Build the self-miner release binary and copy it to a distribution folder.
.DESCRIPTION
    Compiles tg-self-miner in release mode and places the distributable EXE
    in dist/. This is the ONLY file a user needs — no source code ships.
#>

$ErrorActionPreference = 'Stop'

$scriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$rustRoot   = Split-Path -Parent $scriptDir            # l2-mining/rust
$distDir    = Join-Path $scriptDir 'dist'
$cargoToml  = Join-Path $scriptDir 'Cargo.toml'

# Read version from Cargo.toml
$version = (Select-String -Path $cargoToml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
Write-Host "`n  Building tg-self-miner v$version (release)...`n" -ForegroundColor Cyan

# Build
Push-Location $scriptDir
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

# Locate binary
$binary = Join-Path $scriptDir 'target\release\tg-self-miner.exe'
if (-not (Test-Path $binary)) {
    throw "Binary not found at $binary"
}

# Copy to dist/
if (-not (Test-Path $distDir)) { New-Item -ItemType Directory -Path $distDir | Out-Null }

$distName = "tg-self-miner-v$version-windows-x64.exe"
$distPath = Join-Path $distDir $distName
Copy-Item $binary $distPath -Force

# Build a clean ZIP that you hand to users (exe + readme only)
$zipName = "TG-SelfMiner-v$version-windows-x64.zip"
$zipPath = Join-Path $distDir $zipName
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }

$stagingDir = Join-Path $env:TEMP "tg-selfminer-staging"
if (Test-Path $stagingDir) { Remove-Item $stagingDir -Recurse -Force }
New-Item -ItemType Directory -Path $stagingDir | Out-Null

Copy-Item $distPath (Join-Path $stagingDir "tg-self-miner.exe") -Force
$readmeSrc = Join-Path $distDir "README.txt"
if (Test-Path $readmeSrc) {
    Copy-Item $readmeSrc (Join-Path $stagingDir "README.txt") -Force
}

Compress-Archive -Path "$stagingDir\*" -DestinationPath $zipPath -Force
Remove-Item $stagingDir -Recurse -Force

$zipSize = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)
Write-Host "  ZIP: $zipName ($zipSize MB)" -ForegroundColor Green

$size = [math]::Round((Get-Item $distPath).Length / 1MB, 1)
Write-Host "`n  Distributable ready:" -ForegroundColor Green
Write-Host "  $distPath ($size MB)`n"
Write-Host "  Ship this file only. Users run it — no source code needed." -ForegroundColor DarkGray
Write-Host ""
