param()
$rpc = "https://api.nativebtc.org/v1/arb?key=fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9"
$addr = "0xb2b3d9bC63993b725Aea36aC90601c22292F3171"

$body = '{"jsonrpc":"2.0","method":"eth_getCode","params":["' + $addr + '","latest"],"id":1}'
$resp = Invoke-RestMethod -Uri $rpc -Method Post -Body $body -ContentType "application/json"
$onChain = $resp.result.Substring(2)

$artPath = "C:\Users\comar\OneDrive\Documents\Entropy_Randomness\_verify_scan\MiningModule_8bbdd19\l2-mining\out\MiningModule.sol\MiningModule.json"
$art = Get-Content $artPath -Raw | ConvertFrom-Json
$local = $art.deployedBytecode.object.Substring(2)

Write-Host "On-chain: $($onChain.Length) hex chars ($($onChain.Length/2) bytes)"
Write-Host "Local:    $($local.Length) hex chars ($($local.Length/2) bytes)"
Write-Host "Diff:     $(($onChain.Length - $local.Length)/2) bytes"
Write-Host ""

# Align-and-compare: try to find where the local matches on-chain after the divergence
# Hypothesis: 19 bytes inserted at some point X; after X+19, bytecodes match again
$firstDiff = 1374  # hex chars
Write-Host "First diff at hex char $firstDiff (byte $($firstDiff/2))"

# Look for realignment: scan on-chain from firstDiff onward matching local from firstDiff
$shift = 38  # 19 bytes = 38 hex chars
$realignPos = -1
for ($i = $firstDiff; $i -lt [Math]::Min($onChain.Length, $local.Length + $shift) - 100; $i += 2) {
    $localStart = $i - $shift
    if ($localStart -lt 0) { continue }
    $localSlice = $local.Substring($localStart, 40)
    $onChainSlice = $onChain.Substring($i, 40)
    if ($onChainSlice -eq $localSlice) {
        $realignPos = $i
        Write-Host "Realignment found at on-chain byte $($i/2) / local byte $(($i-$shift)/2)"
        Write-Host "Matching 40-char block: $localSlice"
        break
    }
}

if ($realignPos -eq -1) {
    Write-Host "No simple 19-byte-shift realignment found. The insertion may be earlier."
    # Try alignment searching the region 0..firstDiff
    Write-Host "Searching for insertion point in first $($firstDiff/2) bytes..."
    for ($insertAt = 0; $insertAt -lt $firstDiff - 40; $insertAt += 2) {
        # If 19 bytes inserted at insertAt: on-chain[insertAt..insertAt+shift] = extra
        # and on-chain[insertAt+shift:] should match local[insertAt:]
        $matches = 0
        for ($j = 0; $j -lt 80; $j += 2) {
            $oc = $insertAt + $shift + $j
            $lo = $insertAt + $j
            if ($oc -ge $onChain.Length -or $lo -ge $local.Length) { break }
            if ($onChain[$oc] -eq $local[$lo] -and $onChain[$oc+1] -eq $local[$lo+1]) { $matches++ }
            else { break }
        }
        if ($matches -ge 39) {
            Write-Host "INSERT at byte $($insertAt/2): on-chain has 19 extra bytes here"
            Write-Host "Extra bytes: $($onChain.Substring($insertAt + 0, $shift))"
            break
        }
    }
}
