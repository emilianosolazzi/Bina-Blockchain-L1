param()
$apiKey = "8HVN717M5MCN4X7EY5XU8YXD4GE8DCPBCA"
$address = "0xAf07E37D104E9be17639FE7a51B36972D4738651"
$compiler = "v0.8.30+commit.73712a01"
$stdJson = Get-Content "$PSScriptRoot\batch_std_input.json" -Raw

Write-Host "Submitting BatchMiningModule verification to Etherscan..."
Write-Host "Standard JSON size: $($stdJson.Length) chars"

$body = @{
    apikey               = $apiKey
    module               = "contract"
    action               = "verifysourcecode"
    contractaddress      = $address
    contractname         = "contracts/randomness/modules/BatchMiningModule.sol:BatchMiningModule"
    compilerversion      = $compiler
    codeformat           = "solidity-standard-json-input"
    sourceCode           = $stdJson
    constructorArguements = ""
    licenseType          = "1"
}

$resp = Invoke-RestMethod -Uri "https://api.etherscan.io/v2/api?chainid=42161" -Method Post -Body $body -ContentType "application/x-www-form-urlencoded"
Write-Host "Status: $($resp.status)"
Write-Host "Message: $($resp.message)"
Write-Host "Result (GUID): $($resp.result)"
