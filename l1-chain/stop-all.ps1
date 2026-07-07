# Stop all local BINA L1 development services.
# Stops:
#   - l1-node via node-start.ps1 -Stop / logs\node.pid
#   - any process listening on the dashboard static port 8182

param(
    [switch]$Status
)

$root = $PSScriptRoot
$nodeStart = Join-Path $root "node-start.ps1"
$logDir = Join-Path $root "logs"
$dashboardPort = 8182
$dashboardPidFile = Join-Path $logDir "dashboard.pid"

function Get-ListeningPids {
    param([int]$Port)

    $listeners = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    if (-not $listeners) { return @() }
    return @($listeners | Select-Object -ExpandProperty OwningProcess -Unique)
}

function Stop-Pids {
    param(
        [int[]]$Pids,
        [string]$Label
    )

    foreach ($pidValue in $Pids) {
        try {
            $proc = Get-Process -Id $pidValue -ErrorAction Stop
            Stop-Process -Id $pidValue -Force -ErrorAction Stop
            Write-Host "$Label stopped PID=$pidValue ($($proc.ProcessName))."
        } catch {
            Write-Host "$Label PID=$pidValue was already stopped or could not be stopped: $($_.Exception.Message)"
        }
    }
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
    }
    exit 0
}

Write-Host "Stopping BINA L1 services..."

if (Test-Path $nodeStart) {
    & $nodeStart -Stop
} else {
    Write-Host "Node launcher missing: $nodeStart"
}

$dashboardPids = Get-ListeningPids -Port $dashboardPort
if ($dashboardPids.Count -eq 0) {
    Write-Host "Dashboard static server is not running on port $dashboardPort."
} else {
    Stop-Pids -Pids $dashboardPids -Label "Dashboard static server"
}

Remove-Item $dashboardPidFile -ErrorAction SilentlyContinue

Write-Host "All BINA L1 local services stopped."
