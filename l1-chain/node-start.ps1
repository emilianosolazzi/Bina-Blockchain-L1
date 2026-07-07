# ── Bina Chain node launcher ($BINA) ────────────────────────────────────────
# Usage:  .\node-start.ps1           — start (if not already running)
#         .\node-start.ps1 -Stop     — stop the node
#         .\node-start.ps1 -Status   — print status from API + recent log
#         .\node-start.ps1 -Log      — tail the last 30 lines of the output log
#
# The node runs detached (no terminal dependency).
# API is always at http://127.0.0.1:8181
# Logs:  l1-chain/logs/node.out.log  (mining output)
#        l1-chain/logs/node.err.log  (errors / warnings)

param(
    [switch]$Stop,
    [switch]$Status,
    [switch]$Log
)

$root   = $PSScriptRoot
$logDir = Join-Path $root "logs"
$pidFile = Join-Path $logDir "node.pid"
$outLog  = Join-Path $logDir "node.out.log"
$errLog  = Join-Path $logDir "node.err.log"
$binPath = Join-Path $root "target\debug\l1-node.exe"

New-Item -ItemType Directory -Force -Path $logDir | Out-Null

function Get-NodePid {
    if (Test-Path $pidFile) {
        $id = [int](Get-Content $pidFile -Raw).Trim()
        try { Get-Process -Id $id -ErrorAction Stop | Out-Null; return $id } catch { }
    }
    return $null
}

# ── STOP ─────────────────────────────────────────────────────────────────────
if ($Stop) {
    $nodeId = Get-NodePid
    if ($null -eq $nodeId) { Write-Host "Node is not running."; exit 0 }
    Stop-Process -Id $nodeId -Force
    Remove-Item $pidFile -ErrorAction SilentlyContinue
    Write-Host "Node PID=$nodeId stopped."
    exit 0
}

# ── STATUS ───────────────────────────────────────────────────────────────────
if ($Status) {
    $nodeId = Get-NodePid
    if ($null -eq $nodeId) {
        Write-Host "Node is NOT running."
    } else {
        Write-Host "Node PID=$nodeId is running."
        try {
            $s = Invoke-RestMethod http://127.0.0.1:8181/ -TimeoutSec 3
            Write-Host "  height          : $($s.height)"
            Write-Host "  avg_hashrate_mhs: $($s.avg_hashrate_mhs)"
            Write-Host "  uptime_secs     : $($s.uptime_secs)"
            Write-Host "  btc_height      : $($s.btc_height)"
            Write-Host "  nullifiers_spent: $($s.nullifiers_spent)"
            Write-Host "  total_hashes    : $($s.total_hashes)"
        } catch {
            Write-Host "  (API not responding yet — node may be starting up)"
        }
    }
    exit 0
}

# ── LOG ──────────────────────────────────────────────────────────────────────
if ($Log) {
    if (Test-Path $outLog) { Get-Content $outLog -Tail 30 }
    else { Write-Host "No log file yet: $outLog" }
    exit 0
}

# ── START ────────────────────────────────────────────────────────────────────
$existing = Get-NodePid
if ($null -ne $existing) {
    Write-Host "Node is already running (PID=$existing)."
    Write-Host "  API  : http://127.0.0.1:8181"
    Write-Host "  Log  : $outLog"
    exit 0
}

if (-not (Test-Path $binPath)) {
    Write-Host "Binary not found at $binPath — building..."
    Push-Location $root
    cargo build -p l1-node
    Pop-Location
}

$proc = Start-Process `
    -FilePath $binPath `
    -WorkingDirectory $root `
    -RedirectStandardOutput $outLog `
    -RedirectStandardError  $errLog `
    -WindowStyle Hidden `
    -PassThru

$proc.Id | Set-Content $pidFile
Write-Host "Bina Chain node started  (PID=$($proc.Id))"
Write-Host "  API  : http://127.0.0.1:8181"
Write-Host "  Log  : $outLog"
Write-Host ""
Write-Host "  Endpoints:"
Write-Host "    GET /                  — node status (JSON)"
Write-Host "    GET /randomness/latest — latest random output + nullifier"
Write-Host "    GET /chain/latest      — latest mined block"
Write-Host "    GET /chain/blocks      — last 20 blocks"
Write-Host "    POST /chain/submit     — submit a signed mined block claim"
Write-Host "    POST /p2p/message      — receive signed gossip messages"
Write-Host "    POST /p2p/hello        — peer introduction"
Write-Host "    GET /p2p/peers         — known peer list"
Write-Host "    GET /block/:height     — block at height N"
Write-Host ""
Write-Host "  Seeds : set BINA_SEEDS='host:port,host2:port' before start"
Write-Host "  Listen: set BINA_P2P_LISTEN_ADDR='host:port' for peer hello"
Write-Host "  Stop : .\node-start.ps1 -Stop"
Write-Host "  Status: .\node-start.ps1 -Status"
Write-Host "  Log  : .\node-start.ps1 -Log"
