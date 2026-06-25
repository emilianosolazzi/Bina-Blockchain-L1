# TGBT Mining Stack - Stop All Services
# Run:  .\stop-all.ps1

$ErrorActionPreference = "SilentlyContinue"

Write-Host ""
Write-Host "=== TGBT Mining Stack - Stopping All Services ===" -ForegroundColor Red
Write-Host ""

# Miner
$miner = Get-Process temporal-gradient-miner -ErrorAction SilentlyContinue
if ($miner) {
    Stop-Process -Id $miner.Id -Force
    Write-Host "  [STOP] Miner stopped (PID $($miner.Id))" -ForegroundColor Yellow
}
else {
    Write-Host "  [SKIP] Miner not running" -ForegroundColor DarkGray
}

# Dashboard (port 4273; legacy 4173 kept for cleanup)
$dashStopped = $false
foreach ($dashPort in @(4273, 4173)) {
    $dash = Get-NetTCPConnection -LocalPort $dashPort -State Listen -ErrorAction SilentlyContinue
    if ($dash) {
        $dpid = ($dash | Select-Object -First 1).OwningProcess
        Stop-Process -Id $dpid -Force -ErrorAction SilentlyContinue
        Write-Host "  [STOP] Dashboard stopped on port $dashPort (PID $dpid)" -ForegroundColor Yellow
        $dashStopped = $true
    }
}
if (-not $dashStopped) {
    Write-Host "  [SKIP] Dashboard not running" -ForegroundColor DarkGray
}

# Heartbeat sidecar (port 4380)
$hb = Get-NetTCPConnection -LocalPort 4380 -State Listen -ErrorAction SilentlyContinue
if ($hb) {
    $hpid = ($hb | Select-Object -First 1).OwningProcess
    Stop-Process -Id $hpid -Force -ErrorAction SilentlyContinue
    Write-Host "  [STOP] Heartbeat sidecar stopped (PID $hpid)" -ForegroundColor Yellow
}
else {
    Write-Host "  [SKIP] Heartbeat sidecar not running" -ForegroundColor DarkGray
}

# Randomness API (port 4271)
$rand = Get-NetTCPConnection -LocalPort 4271 -State Listen -ErrorAction SilentlyContinue
if ($rand) {
    $rpid = ($rand | Select-Object -First 1).OwningProcess
    Stop-Process -Id $rpid -Force -ErrorAction SilentlyContinue
    Write-Host "  [STOP] Randomness API stopped (PID $rpid)" -ForegroundColor Yellow
}
else {
    Write-Host "  [SKIP] Randomness API not running" -ForegroundColor DarkGray
}

# Beacon API (port 3100)
$api = Get-NetTCPConnection -LocalPort 3100 -State Listen -ErrorAction SilentlyContinue
if ($api) {
    $apid = ($api | Select-Object -First 1).OwningProcess
    Stop-Process -Id $apid -Force -ErrorAction SilentlyContinue
    Write-Host "  [STOP] Beacon API stopped (PID $apid)" -ForegroundColor Yellow
}
else {
    Write-Host "  [SKIP] Beacon API not running" -ForegroundColor DarkGray
}

# Epoch Builder
$epochBuilderPidFile = Join-Path $PSScriptRoot ".runtime-logs\stack\epoch-builder.pid"
if (Test-Path $epochBuilderPidFile) {
    $ebPid = [int](Get-Content $epochBuilderPidFile -Raw).Trim()
    $ebProc = Get-Process -Id $ebPid -ErrorAction SilentlyContinue
    if ($ebProc) {
        Stop-Process -Id $ebPid -Force
        Write-Host "  [STOP] Epoch Builder stopped (PID $ebPid)" -ForegroundColor Yellow
    }
    else {
        Write-Host "  [SKIP] Epoch Builder not running" -ForegroundColor DarkGray
    }
    Remove-Item $epochBuilderPidFile -Force -ErrorAction SilentlyContinue
}
else {
    Write-Host "  [SKIP] Epoch Builder not running" -ForegroundColor DarkGray
}

# Redis
$redis = Get-Process redis-server -ErrorAction SilentlyContinue
if ($redis) {
    Stop-Process -Id $redis.Id -Force
    Write-Host "  [STOP] Redis stopped (PID $($redis.Id))" -ForegroundColor Yellow
}
else {
    Write-Host "  [SKIP] Redis not running" -ForegroundColor DarkGray
}

Write-Host "  [INFO] PostgreSQL left running (system service)" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  All TGBT services stopped." -ForegroundColor Yellow
Write-Host ""
