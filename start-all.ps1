# TGBT Mining Stack - One-Click Start
# Run:  .\start-all.ps1
# Stop: .\stop-all.ps1

$ErrorActionPreference = "SilentlyContinue"
$ROOT = $PSScriptRoot
$JS   = Join-Path $ROOT "l2-mining\js"
$DASH = Join-Path $ROOT "l2-mining\miner-dashboard"
$RAND = Join-Path $ROOT "l2-mining\randomness-api"
$SEC  = Join-Path $ROOT "l2-mining\security"
$RUST = Join-Path $ROOT "l2-mining\rust"
$LOGS = Join-Path $ROOT ".runtime-logs\stack"
$MINER_CONFIG = Join-Path $env:APPDATA "entropy\TemporalGradientMiner\config\miner-config.json"
$MINER_EXE_BUILD = Join-Path $RUST "target\release\temporal-gradient-miner.exe"
$MINER_EXE_DEPLOY = Join-Path $env:LOCALAPPDATA "entropy\TemporalGradientMiner\data\bin\temporal-gradient-miner.exe"
# Prefer the deployed binary (matches running process path); fall back to build output
$MINER_EXE = if (Test-Path $MINER_EXE_DEPLOY) { $MINER_EXE_DEPLOY } else { $MINER_EXE_BUILD }
$TELEMETRY_FILE = Join-Path $env:LOCALAPPDATA "entropy\TemporalGradientMiner\data\logs\telemetry.jsonl"

New-Item -ItemType Directory -Force -Path $LOGS | Out-Null

function Wait-ForPort {
    param(
        [int]$Port,
        [int]$TimeoutSeconds = 15
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $listener = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($listener) { return $listener }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)

    return $null
}

function Wait-ForHttpJson {
    param(
        [string]$Uri,
        [scriptblock]$Success,
        [int]$TimeoutSeconds = 20
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        try {
            $response = Invoke-RestMethod -Uri $Uri -TimeoutSec 5
            if (& $Success $response) {
                return $response
            }
        }
        catch {
        }
        Start-Sleep -Milliseconds 750
    } while ((Get-Date) -lt $deadline)

    return $null
}

function Wait-ForTelemetryFresh {
    param(
        [string]$TelemetryFile,
        [int]$TimeoutSeconds = 25,
        [int]$FreshSeconds = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (Test-Path $TelemetryFile) {
            try {
                $lastLine = Get-Content $TelemetryFile -Tail 1 | ConvertFrom-Json -ErrorAction Stop
                if ($lastLine -and $lastLine.timestamp_unix_ms) {
                    $ageMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - [int64]$lastLine.timestamp_unix_ms
                    if ($ageMs -ge 0 -and $ageMs -lt ($FreshSeconds * 1000)) {
                        return $lastLine
                    }
                }
            }
            catch {
            }
        }
        Start-Sleep -Milliseconds 750
    } while ((Get-Date) -lt $deadline)

    return $null
}

function Start-BackgroundProcess {
    param(
        [string]$Name,
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$WorkingDirectory,
        [string]$LogPrefix
    )

    $stdout = Join-Path $LOGS "$LogPrefix.out.log"
    $stderr = Join-Path $LOGS "$LogPrefix.err.log"
    if (Test-Path $stdout) { Remove-Item $stdout -Force -ErrorAction SilentlyContinue }
    if (Test-Path $stderr) { Remove-Item $stderr -Force -ErrorAction SilentlyContinue }

    return Start-Process -FilePath $FilePath `
        -ArgumentList $Arguments `
        -WorkingDirectory $WorkingDirectory `
        -WindowStyle Hidden `
        -RedirectStandardOutput $stdout `
        -RedirectStandardError $stderr `
        -PassThru
}

function Get-LogHint {
    param([string]$LogPrefix)
    return "logs: $(Join-Path $LOGS "$LogPrefix.err.log")"
}

function Get-MinerConfigObject {
    param([string]$ConfigPath)

    if (-not (Test-Path $ConfigPath)) {
        return $null
    }

    try {
        return Get-Content $ConfigPath -Raw | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
        return $null
    }
}

function Get-LatestRustSourceWriteTime {
    param([string]$RustRoot)

    $latest = Get-ChildItem -Path $RustRoot -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Extension -in '.rs', '.toml' } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1

    if ($latest) {
        return $latest.LastWriteTimeUtc
    }

    return [datetime]::MinValue
}

function Ensure-MinerBinary {
    param(
        [string]$RustRoot,
        [string]$BinaryPath,
        [string]$ConfigPath
    )

    $config = Get-MinerConfigObject -ConfigPath $ConfigPath
    $staleEnabled = $false
    $staleApi = $null

    if ($config -and $config.stale_block -and $config.stale_block.enabled) {
        $staleEnabled = $true
        $staleApi = $config.stale_block.bitcoin_api_url
    }

    if ($staleEnabled) {
        Write-Host "  [INFO] Stale-block harvesting enabled ($staleApi)" -ForegroundColor DarkCyan
    }

    $binaryExists = Test-Path $BinaryPath
    $binaryItem = if ($binaryExists) { Get-Item $BinaryPath } else { $null }
    $featureStamp = Join-Path $RustRoot "target\release\temporal-gradient-miner.stale-mining.stamp"
    $latestSourceWrite = Get-LatestRustSourceWriteTime -RustRoot $RustRoot

    $needsBuild = (-not $binaryExists)
    if (-not $needsBuild -and $binaryItem.LastWriteTimeUtc -lt $latestSourceWrite) {
        $needsBuild = $true
    }
    if (-not $needsBuild -and $staleEnabled -and -not (Test-Path $featureStamp)) {
        $needsBuild = $true
    }

    if (-not $needsBuild) {
        return $true
    }

    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        Write-Host "  [FAIL] cargo is required to build the miner" -ForegroundColor Red
        return $binaryExists
    }

    $buildArgs = @("build", "--release", "-p", "temporal-gradient-miner-installer", "--bin", "temporal-gradient-miner")
    if ($staleEnabled) {
        $buildArgs += @("--features", "stale-mining")
    }

    Write-Host "  [WAIT] Building miner binary$(if ($staleEnabled) { ' with stale-mining support' } else { '' })..." -ForegroundColor DarkYellow
    Push-Location $RustRoot
    try {
        & $cargo.Source @buildArgs
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  [FAIL] Miner build failed" -ForegroundColor Red
            return $false
        }
    }
    finally {
        Pop-Location
    }

    if ($staleEnabled) {
        Set-Content -Path $featureStamp -Value ([DateTimeOffset]::UtcNow.ToString("o")) -Encoding ascii
    }

    # Also sync the build to the AppData deploy location used by the installer
    $deployDir = Join-Path $env:LOCALAPPDATA "entropy\TemporalGradientMiner\data\bin"
    $deployBin = Join-Path $deployDir "temporal-gradient-miner.exe"
    if ((Test-Path $BinaryPath) -and (Test-Path $deployDir)) {
        Copy-Item -Path $BinaryPath -Destination $deployBin -Force
        Write-Host "  [OK] Synced patched binary to $deployBin" -ForegroundColor Green
    }

    return (Test-Path $BinaryPath)
}

Write-Host ""
Write-Host "=== TGBT Mining Stack - Starting All Services ===" -ForegroundColor Cyan
Write-Host ""

# ---- 1. Redis ----
Write-Host "[1/5] Redis" -ForegroundColor Yellow
$redisProc = Get-Process redis-server -ErrorAction SilentlyContinue
if ($redisProc) {
    Write-Host "  [OK] Redis already running (PID $($redisProc.Id))" -ForegroundColor Green
}
else {
    $redisExe = "C:\Program Files\Redis\redis-server.exe"
    if (Test-Path $redisExe) {
        Start-Process -FilePath $redisExe -WindowStyle Hidden
        Start-Sleep -Seconds 2
        $redisProc = Get-Process redis-server -ErrorAction SilentlyContinue
        if ($redisProc) {
            Write-Host "  [OK] Redis started (PID $($redisProc.Id))" -ForegroundColor Green
        }
        else {
            Write-Host "  [FAIL] Redis failed to start" -ForegroundColor Red
        }
    }
    else {
        Write-Host "  [FAIL] Redis not installed. Run: winget install Redis.Redis" -ForegroundColor Red
    }
}

# ---- 2. PostgreSQL ----
Write-Host "[2/5] PostgreSQL" -ForegroundColor Yellow
$pgService = Get-Service postgresql-x64-17 -ErrorAction SilentlyContinue
if (-not $pgService) {
    $pgService = Get-Service postgresql* -ErrorAction SilentlyContinue | Select-Object -First 1
}
if ($pgService -and $pgService.Status -eq "Running") {
    Write-Host "  [OK] PostgreSQL running (service: $($pgService.Name))" -ForegroundColor Green
}
elseif ($pgService) {
    Start-Service $pgService.Name -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    $pgService = Get-Service $pgService.Name
    if ($pgService.Status -eq "Running") {
        Write-Host "  [OK] PostgreSQL started" -ForegroundColor Green
    }
    else {
        Write-Host "  [FAIL] PostgreSQL failed to start" -ForegroundColor Red
    }
}
else {
    Write-Host "  [FAIL] PostgreSQL service not found" -ForegroundColor Red
}

# ---- 3. Beacon API (port 3100) ----
Write-Host "[3/6] Beacon API" -ForegroundColor Yellow
$apiPort = 3100
$apiUp = Wait-ForPort -Port $apiPort -TimeoutSeconds 1
if ($apiUp) {
    $apiPid = ($apiUp | Select-Object -First 1).OwningProcess
    Write-Host "  [OK] Beacon API already on port $apiPort (PID $apiPid)" -ForegroundColor Green
}
else {
    if (-not (Test-Path (Join-Path $JS "node_modules"))) {
        Write-Host "  [WAIT] Installing npm dependencies..." -ForegroundColor DarkYellow
        Push-Location $JS
        npm install --silent 2>&1 | Out-Null
        Pop-Location
    }
    Push-Location $JS
    node db/migrate.js 2>&1 | Out-Null
    Pop-Location
    $apiProc = Start-BackgroundProcess -Name "Beacon API" -FilePath "node" -Arguments @("beacon-api-server.js") -WorkingDirectory $JS -LogPrefix "beacon-api"
    $apiUp = Wait-ForPort -Port $apiPort -TimeoutSeconds 20
    if ($apiUp) {
        $apiPid = ($apiUp | Select-Object -First 1).OwningProcess
        Write-Host "  [OK] Beacon API started on port $apiPort (PID $apiPid)" -ForegroundColor Green
    }
    else {
        $apiProc | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Host "  [FAIL] Beacon API failed to start ($(Get-LogHint -LogPrefix 'beacon-api'))" -ForegroundColor Red
    }
}

# ---- 4. Randomness API (port 4271) ----
Write-Host "[4/7] Randomness API" -ForegroundColor Yellow
$randPort = 4271
$randUp = Wait-ForPort -Port $randPort -TimeoutSeconds 1
if ($randUp) {
    $randPid = ($randUp | Select-Object -First 1).OwningProcess
    Write-Host "  [OK] Randomness API already on port $randPort (PID $randPid)" -ForegroundColor Green
}
else {
    $randProc = Start-BackgroundProcess -Name "Randomness API" -FilePath "node" -Arguments @("server.js") -WorkingDirectory $RAND -LogPrefix "randomness-api"
    $randUp = Wait-ForPort -Port $randPort -TimeoutSeconds 20
    if ($randUp) {
        $randPid = ($randUp | Select-Object -First 1).OwningProcess
        Write-Host "  [OK] Randomness API started on port $randPort (PID $randPid)" -ForegroundColor Green
    }
    else {
        $randProc | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Host "  [FAIL] Randomness API failed to start ($(Get-LogHint -LogPrefix 'randomness-api'))" -ForegroundColor Red
    }
}

# ---- 5. Heartbeat sidecar (port 4380) ----
Write-Host "[5/7] Heartbeat sidecar" -ForegroundColor Yellow
$hbPort = 4380
$hbUp = Wait-ForPort -Port $hbPort -TimeoutSeconds 1
if ($hbUp) {
    $hbPid = ($hbUp | Select-Object -First 1).OwningProcess
    Write-Host "  [OK] Heartbeat sidecar already on port $hbPort (PID $hbPid)" -ForegroundColor Green
}
else {
    $hbProc = Start-BackgroundProcess -Name "Heartbeat sidecar" -FilePath "node" -Arguments @("heartbeat-sidecar.js") -WorkingDirectory $SEC -LogPrefix "heartbeat-sidecar"
    $hbUp = Wait-ForPort -Port $hbPort -TimeoutSeconds 20
    if ($hbUp) {
        $hbPid = ($hbUp | Select-Object -First 1).OwningProcess
        Write-Host "  [OK] Heartbeat sidecar started on port $hbPort (PID $hbPid)" -ForegroundColor Green
    }
    else {
        $hbProc | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Host "  [FAIL] Heartbeat sidecar failed to start ($(Get-LogHint -LogPrefix 'heartbeat-sidecar'))" -ForegroundColor Red
    }
}

# ---- 6. Miner Dashboard (port 4173) ----
Write-Host "[6/7] Miner Dashboard" -ForegroundColor Yellow
$dashPort = 4173
$dashUp = Wait-ForPort -Port $dashPort -TimeoutSeconds 1
if ($dashUp) {
    $dashPid = ($dashUp | Select-Object -First 1).OwningProcess
    Write-Host "  [OK] Dashboard already on port $dashPort (PID $dashPid)" -ForegroundColor Green
}
else {
    if (-not (Test-Path (Join-Path $DASH "node_modules"))) {
        Write-Host "  [WAIT] Installing dashboard dependencies..." -ForegroundColor DarkYellow
        Push-Location $DASH
        npm install --silent 2>&1 | Out-Null
        Pop-Location
    }
    $dashProc = Start-BackgroundProcess -Name "Miner Dashboard" -FilePath "node" -Arguments @("server.js") -WorkingDirectory $DASH -LogPrefix "miner-dashboard"
    $dashUp = Wait-ForPort -Port $dashPort -TimeoutSeconds 20
    if ($dashUp) {
        $dashPid = ($dashUp | Select-Object -First 1).OwningProcess
        Write-Host "  [OK] Dashboard started on port $dashPort (PID $dashPid)" -ForegroundColor Green
    }
    else {
        $dashProc | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Host "  [FAIL] Dashboard failed to start ($(Get-LogHint -LogPrefix 'miner-dashboard'))" -ForegroundColor Red
    }
}

# ---- 7. Miner ----
Write-Host "[7/7] Miner" -ForegroundColor Yellow
$minerProc = Get-Process temporal-gradient-miner -ErrorAction SilentlyContinue
if ($minerProc) {
    Write-Host "  [OK] Miner already running (PID $($minerProc.Id))" -ForegroundColor Green
}
else {
    $minerReady = Ensure-MinerBinary -RustRoot $RUST -BinaryPath $MINER_EXE_BUILD -ConfigPath $MINER_CONFIG
    if ($minerReady -and (Test-Path $MINER_EXE)) {
        $minerProc = Start-BackgroundProcess -Name "Miner" -FilePath $MINER_EXE -Arguments @("--config", $MINER_CONFIG) -WorkingDirectory $ROOT -LogPrefix "miner"
        Start-Sleep -Seconds 2
        $minerProc = Get-Process temporal-gradient-miner -ErrorAction SilentlyContinue
        if ($minerProc) {
            Write-Host "  [OK] Miner started (PID $($minerProc.Id))" -ForegroundColor Green
        }
        else {
            Write-Host "  [FAIL] Miner exited immediately ($(Get-LogHint -LogPrefix 'miner'))" -ForegroundColor Red
        }
    }
    else {
        Write-Host "  [SKIP] Miner binary not found (need cargo build --release)" -ForegroundColor DarkGray
    }
}

# ---- Health Check ----
Write-Host ""
Write-Host "--- Health Check ---" -ForegroundColor Cyan
Start-Sleep -Seconds 1

try {
    $health = Wait-ForHttpJson -Uri "http://localhost:$apiPort/healthz" -Success { param($r) $r.status -eq 'ok' } -TimeoutSeconds 20
    if ($health) {
        Write-Host "  [OK] Beacon API: healthy (block $($health.blockNumber))" -ForegroundColor Green
    }
    else {
        Write-Host "  [FAIL] Beacon API: not healthy ($(Get-LogHint -LogPrefix 'beacon-api'))" -ForegroundColor Red
    }
}
catch {
    Write-Host "  [FAIL] Beacon API: not responding" -ForegroundColor Red
}

try {
    $rand = Wait-ForHttpJson -Uri "http://127.0.0.1:$randPort/api/health" -Success { param($r) $r.status -eq 'ok' } -TimeoutSeconds 20
    if ($rand) {
        Write-Host "  [OK] Randomness API: healthy (epochs $($rand.epochs))" -ForegroundColor Green
    }
    else {
        Write-Host "  [FAIL] Randomness API: not healthy ($(Get-LogHint -LogPrefix 'randomness-api'))" -ForegroundColor Red
    }
}
catch {
    Write-Host "  [FAIL] Randomness API: not responding" -ForegroundColor Red
}

try {
    $hb = Wait-ForHttpJson -Uri "http://127.0.0.1:$hbPort/api/health" -Success { param($r) $r.ok -eq $true } -TimeoutSeconds 20
    if ($hb) {
        $hbState = if ($hb.status) { $hb.status } else { 'ok' }
        Write-Host "  [OK] Heartbeat sidecar: $hbState" -ForegroundColor Green
    }
    else {
        Write-Host "  [FAIL] Heartbeat sidecar: not healthy ($(Get-LogHint -LogPrefix 'heartbeat-sidecar'))" -ForegroundColor Red
    }
}
catch {
    Write-Host "  [FAIL] Heartbeat sidecar: not responding" -ForegroundColor Red
}

try {
    $null = Invoke-WebRequest -Uri "http://127.0.0.1:$dashPort/" -TimeoutSec 5 -UseBasicParsing
    Write-Host "  [OK] Dashboard: serving at http://127.0.0.1:$dashPort" -ForegroundColor Green
}
catch {
    Write-Host "  [FAIL] Dashboard: not responding" -ForegroundColor Red
}

$lastLine = Wait-ForTelemetryFresh -TelemetryFile $TELEMETRY_FILE -TimeoutSeconds 25 -FreshSeconds 30
if ($lastLine) {
    $ageMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - [int64]$lastLine.timestamp_unix_ms
    $ageSec = [math]::Round($ageMs / 1000)
    $hr = [math]::Round(($lastLine.hashrate_hs | ForEach-Object { $_ }), 1)
    Write-Host "  [OK] Miner: $hr H/s, phase=$($lastLine.mining_phase), telemetry ${ageSec}s old" -ForegroundColor Green
}
elseif (Get-Process temporal-gradient-miner -ErrorAction SilentlyContinue) {
    Write-Host "  [WARN] Miner process is running but telemetry did not refresh in time ($(Get-LogHint -LogPrefix 'miner'))" -ForegroundColor Yellow
}
else {
    Write-Host "  [FAIL] Miner is not running ($(Get-LogHint -LogPrefix 'miner'))" -ForegroundColor Red
}

Write-Host ""
Write-Host "=== Done. Dashboard: http://127.0.0.1:$dashPort ===" -ForegroundColor Green
Write-Host "=== Service logs: $LOGS ===" -ForegroundColor DarkGray
Write-Host ""
