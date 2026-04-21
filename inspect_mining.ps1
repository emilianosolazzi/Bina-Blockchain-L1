param()
$rpc = "https://api.nativebtc.org/v1/arb?key=fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9"
$addr = "0xb2b3d9bC63993b725Aea36aC90601c22292F3171"

$body = '{"jsonrpc":"2.0","method":"eth_getCode","params":["' + $addr + '","latest"],"id":1}'
$resp = Invoke-RestMethod -Uri $rpc -Method Post -Body $body -ContentType "application/json"
$onChain = $resp.result.Substring(2)

# Show context around byte 687 in on-chain bytecode
$diffPos = 1374  # hex char position
$start = [Math]::Max(0, $diffPos - 60)
$end   = [Math]::Min($onChain.Length, $diffPos + 100)
Write-Host "=== On-chain bytes 640-780 ==="
Write-Host $onChain.Substring($start, $end - $start)

# Also look at byte 0..100 to identify the contract structure
Write-Host "=== On-chain first 200 chars ==="
Write-Host $onChain.Substring(0, 200)

# Show bytes 600-700 in chunks
Write-Host "=== On-chain hex [byte 620-720] ==="
for ($i = 1240; $i -lt 1440; $i += 40) {
    $chunk = $onChain.Substring($i, [Math]::Min(40, $onChain.Length - $i))
    Write-Host "byte $($i/2): $chunk"
}
