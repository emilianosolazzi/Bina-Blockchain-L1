$raw = Get-Content "$PSScriptRoot\sourcify_batch.json" -Raw | ConvertFrom-Json
$meta = ($raw.files | Where-Object name -eq "metadata.json").content | ConvertFrom-Json
Write-Host "Compiler: $($meta.compiler.version)"

$sources = @{}
foreach ($f in $raw.files) {
    if ($f.name -match "\.sol$") {
        $relPath = $f.path -replace ".*?sources/", ""
        $sources[$relPath] = @{ content = $f.content }
    }
}
Write-Host "Source paths:"
$sources.Keys | Sort-Object

$stdJson = [ordered]@{
    language = "Solidity"
    sources  = $sources
    settings = [ordered]@{
        optimizer  = [ordered]@{ enabled = $true; runs = 200 }
        evmVersion = "prague"
        outputSelection = @{ "*" = [ordered]@{ "*" = @("abi", "evm.bytecode", "evm.deployedBytecode", "metadata"); "" = @("ast") } }
    }
}
$out = $stdJson | ConvertTo-Json -Depth 12 -Compress
Write-Host "JSON length: $($out.Length)"
$out | Out-File "$PSScriptRoot\batch_std_input.json" -Encoding utf8NoBOM
Write-Host "Saved."
