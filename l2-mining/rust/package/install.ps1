param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptRoot

try {
    if (-not $SkipBuild) {
        cargo build --release --bins
    }

    $installerSource = Join-Path $scriptRoot "target\release\tg-miner-installer.exe"

    if (-not (Test-Path $installerSource)) {
        throw "Expected installer binary not found at $installerSource"
    }

    & $installerSource install | Out-Host

    Write-Host ""
    Write-Host "Installed Temporal Gradient miner binaries into the per-user application directories."
    Write-Host "Next step: edit the generated config file, then run tg-miner-installer launch --foreground"
}
finally {
    Pop-Location
}