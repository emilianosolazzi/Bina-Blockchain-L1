# Start all local BINA L1 development services.
# Starts:
#   - l1-node via node-start.ps1
#   - dashboard static server at http://127.0.0.1:8182/l1-chain/index-Bina.html?v=2

param(
    [switch]$Status
)

$root = $PSScriptRoot
$workspaceRoot = Split-Path $root -Parent
$nodeStart = Join-Path $root "node-start.ps1"
$logDir = Join-Path $root "logs"
$dashboardPort = 8182
$dashboardPidFile = Join-Path $logDir "dashboard.pid"
$dashboardOutLog = Join-Path $logDir "dashboard.out.log"
$dashboardErrLog = Join-Path $logDir "dashboard.err.log"

New-Item -ItemType Directory -Force -Path $logDir | Out-Null

function Get-ListeningPids {
    param([int]$Port)

    $listeners = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    if (-not $listeners) { return @() }
    return @($listeners | Select-Object -ExpandProperty OwningProcess -Unique)
}

function Get-PythonPath {
    $python = Get-Command python -ErrorAction SilentlyContinue
    if ($python) { return $python.Source }

    $py = Get-Command py -ErrorAction SilentlyContinue
    if ($py) { return $py.Source }

    return $null
}

function Start-DashboardServer {
    $dashboardPids = Get-ListeningPids -Port $dashboardPort
    if ($dashboardPids.Count -gt 0) {
        Write-Host "Dashboard static server already listening on port ${dashboardPort}: PID(s) $($dashboardPids -join ', ')"
        return
    }

    $pythonPath = Get-PythonPath
    if (-not $pythonPath) {
        Write-Error "Python was not found. Install Python or start any static server on port $dashboardPort rooted at $workspaceRoot."
        exit 1
    }

    $args = @("-m", "http.server", $dashboardPort.ToString(), "--bind", "127.0.0.1", "--directory", $workspaceRoot)
    if ((Split-Path $pythonPath -Leaf) -ieq "py.exe") {
        $args = @("-3") + $args
    }

    $proc = Start-Process `
        -FilePath $pythonPath `
        -ArgumentList $args `
        -WorkingDirectory $workspaceRoot `
        -RedirectStandardOutput $dashboardOutLog `
        -RedirectStandardError $dashboardErrLog `
        -WindowStyle Hidden `
        -PassThru

    $proc.Id | Set-Content $dashboardPidFile
    Write-Host "Dashboard static server started (PID=$($proc.Id))"
    Write-Host "  URL : http://127.0.0.1:$dashboardPort/l1-chain/index-Bina.html?v=2"
    Write-Host "  Log : $dashboardOutLog"
}

if ($Status) {
    if (Test-Path $nodeStart) {
        & $nodeStart -Status
    } else {
        Write-Host "Node launcher missing: $nodeStart"
    }

    $dashboardPids = Get-ListeningPids -Port $dashboardPort
    if ($dashboardPids.Count -eq 0) {
        Write-Host "Dashboard static server is NOT listening on port $dashboardPort."
    } else {
        Write-Host "Dashboard static server listening on port ${dashboardPort}: PID(s) $($dashboardPids -join ', ')"
        Write-Host "  URL : http://127.0.0.1:$dashboardPort/l1-chain/index-Bina.html?v=2"
    }
    exit 0
}

Write-Host "Starting BINA L1 services..."

if (Test-Path $nodeStart) {
    & $nodeStart
} else {
    Write-Error "Node launcher missing: $nodeStart"
    exit 1
}

Start-DashboardServer

Write-Host "BINA L1 services are ready."
