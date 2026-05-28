param(
    [string]$ServerUrl = "ws://127.0.0.1:7878",
    [string]$ParentRoom = "drill-parent",
    [string]$ChildRoom = "drill-child",
    [string]$TokenJsonPath = ""
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Push-Location $repo
try {
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

    if ($TokenJsonPath -and (Test-Path $TokenJsonPath)) {
        $env:NODALMERGE_TOKEN_JSON = Get-Content -Raw $TokenJsonPath
    }

    $env:NODALMERGE_SERVER_URL = "$ServerUrl/ws/$ParentRoom"
    $env:NODALMERGE_ROOM = $ParentRoom

    Write-Host "== topology list-children (pre) =="
    cargo run -p nodalmerge-cli -- topology list-children --parent-room $ParentRoom 2>&1 | Out-Host

    $checkpoint = Join-Path $env:TEMP "nm-topology-checkpoint.json"
    Write-Host "== topology snapshot-checkpoint =="
    cargo run -p nodalmerge-cli -- topology snapshot-checkpoint --room $ParentRoom --out-file $checkpoint 2>&1 | Out-Host

    Write-Host "== topology create-child =="
    cargo run -p nodalmerge-cli -- topology create-child --parent-room $ParentRoom --child-room $ChildRoom --purpose operator-drill --policy promotion-based --parent-checkpoint-file $checkpoint 2>&1 | Out-Host

    Write-Host "== topology show-lineage =="
    cargo run -p nodalmerge-cli -- topology show-lineage --room $ChildRoom 2>&1 | Out-Host

    Write-Host "== topology list-children (post) =="
    cargo run -p nodalmerge-cli -- topology list-children --parent-room $ParentRoom 2>&1 | Out-Host
}
finally {
    Pop-Location
}
