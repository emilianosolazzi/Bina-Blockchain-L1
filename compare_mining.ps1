param()
# Compare local 8bbdd19 runs=1 bytecode vs on-chain MiningModule
$rpc = "https://api.nativebtc.org/v1/arb?key=fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9"
$addr = "0xb2b3d9bC63993b725Aea36aC90601c22292F3171"

# Get on-chain bytecode
$body = '{"jsonrpc":"2.0","method":"eth_getCode","params":["' + $addr + '","latest"],"id":1}'
$resp = Invoke-RestMethod -Uri $rpc -Method Post -Body $body -ContentType "application/json"
$onChain = $resp.result.Substring(2)  # strip 0x
Write-Host "On-chain len: $($onChain.Length) hex chars"

# Get local bytecode from _verify_scan/MiningModule_8bbdd19 (runs=1, via_ir=true)
$artPath = "C:\Users\comar\OneDrive\Documents\Entropy_Randomness\_verify_scan\MiningModule_8bbdd19\l2-mining\out\MiningModule.sol\MiningModule.json"
$art = Get-Content $artPath -Raw | ConvertFrom-Json
$local = $art.deployedBytecode.object.Substring(2)  # strip 0x
Write-Host "Local len: $($local.Length) hex chars"
Write-Host "Diff in bytes: $(($onChain.Length - $local.Length) / 2)"

# Find first mismatch position
$minLen = [Math]::Min($onChain.Length, $local.Length)
$firstDiff = -1
for ($i = 0; $i -lt $minLen; $i += 2) {
    if ($onChain[$i] -ne $local[$i] -or $onChain[$i+1] -ne $local[$i+1]) {
        $firstDiff = $i
        break
    }
}
if ($firstDiff -eq -1) {
    Write-Host "Bytecodes are IDENTICAL up to min length!"
} else {
    $bytePos = $firstDiff / 2
    Write-Host "First diff at byte $bytePos (hex char $firstDiff)"
    Write-Host "On-chain ctx: $($onChain.Substring([Math]::Max(0,$firstDiff-20), [Math]::Min(60, $onChain.Length - [Math]::Max(0,$firstDiff-20))))"
    Write-Host "Local ctx:    $($local.Substring([Math]::Max(0,$firstDiff-20), [Math]::Min(60, $local.Length - [Math]::Max(0,$firstDiff-20))))"
}

# Compare last 120 chars (CBOR metadata)
Write-Host ""
Write-Host "=== On-chain tail 120 ==="
Write-Host $onChain.Substring($onChain.Length - 120)
Write-Host "=== Local tail 120 ==="
Write-Host $local.Substring($local.Length - 120)
