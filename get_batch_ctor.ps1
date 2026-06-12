param()
$raw = Get-Content "$PSScriptRoot\sourcify_batch.json" -Raw | ConvertFrom-Json
$txHash = ($raw.files | Where-Object name -eq "creator-tx-hash.txt").content.Trim()
Write-Host "Creator tx: $txHash"

$rpc = "https://api.nativebtc.org/v1/arb?key=fp_2d93df5e6cebe485b69c363a62e237fc9d0f88b9"
$body = '{"jsonrpc":"2.0","method":"eth_getTransactionByHash","params":["' + $txHash + '"],"id":1}'
$resp = Invoke-RestMethod -Uri $rpc -Method Post -Body $body -ContentType "application/json"
$inputData = $resp.result.input
Write-Host "Input total len: $($inputData.Length)"

# Read on-chain bytecode length to find where constructor args start
$codeResp = Invoke-RestMethod -Uri $rpc -Method Post -Body '{"jsonrpc":"2.0","method":"eth_getCode","params":["0xAf07E37D104E9be17639FE7a51B36972D4738651","latest"],"id":1}' -ContentType "application/json"
$onChainCode = $codeResp.result
Write-Host "On-chain bytecode len (with 0x): $($onChainCode.Length)"

# Constructor args are after the init bytecode
# The init bytecode length can be inferred, but easier: check the Sourcify metadata for constructor
$meta = ($raw.files | Where-Object name -eq "metadata.json").content | ConvertFrom-Json
$ctor = $meta.output.abi | Where-Object { $_.type -eq "constructor" }
Write-Host "Constructor inputs: $(($ctor.inputs | ConvertTo-Json -Compress))"
