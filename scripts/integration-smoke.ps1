# Integration smoke — requires built Rust artifacts and a running nodalmerge-server.

# Usage:

#   .\scripts\integration-smoke.ps1 [-ServerUrl ws://127.0.0.1:7878] [-Room smoke-room]

#   .\scripts\integration-smoke.ps1 -TokenJsonPath .\token.json   # optional locked-room control plane



param(

    [string]$ServerUrl = "ws://127.0.0.1:7878",

    [string]$Room = "integration-smoke",

    [int]$RunSecs = 8,

    [string]$TokenJsonPath = ""

)



$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"



if ($TokenJsonPath -and (Test-Path $TokenJsonPath)) {

    $env:NODALMERGE_TOKEN_JSON = Get-Content -Raw $TokenJsonPath

    $env:NODALMERGE_SERVER_URL = "$ServerUrl/ws/$Room"

    $env:NODALMERGE_ROOM = $Room

}



Push-Location $repo

try {

    Write-Host "== Build =="

    cargo build -p nodalmerge-cli -p nodalmerge-headless -p nodalmerge-runtime-local-ffi 2>&1 | Out-Host

    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }



    Write-Host "== Vector smoke =="

    cargo test -p nodalmerge-runtime-local-ffi --test abi 2>&1 | Out-Host

    cargo test -p nodalmerge-cli --test topology_cli_vectors 2>&1 | Out-Host

    cargo test -p nodalmerge-cli --test archive_cli_vectors 2>&1 | Out-Host

    cargo test -p nodalmerge-headless --test headless_run_vectors 2>&1 | Out-Host
    cargo test -p nodalmerge-server auth_room_006 -- --nocapture 2>&1 | Out-Host



    Write-Host "== Headless health =="

    cargo run -p nodalmerge-headless -- --health 2>&1 | Out-Host



    $peerDir = Join-Path $env:TEMP "nodalmerge-smoke-peer"

    New-Item -ItemType Directory -Force -Path $peerDir | Out-Null



    Write-Host "== nodalmerge run (needs server at $ServerUrl) =="

    $env:NODALMERGE_HEADLESS_SERVER_URL = $ServerUrl

    $env:NODALMERGE_HEADLESS_ROOM = $Room

    $env:NODALMERGE_HEADLESS_BACKEND = "embedded"

    $env:NODALMERGE_HEADLESS_DATA_DIR = $peerDir

    $reportPath = Join-Path $peerDir "session.json"

    cargo run -p nodalmerge-cli -- run --run-secs $RunSecs --report-json $reportPath 2>&1 | Out-Host

    if ($LASTEXITCODE -ne 0) {

        Write-Warning "nodalmerge run failed — is nodalmerge-server running on $ServerUrl ?"

        exit $LASTEXITCODE

    }



    Write-Host "== Session report =="

    if (-not (Test-Path $reportPath)) {

        throw "missing session report at $reportPath"

    }

    $report = Get-Content -Raw $reportPath | ConvertFrom-Json

    if (-not $report.canonical_hash_hex) {

        throw "session report missing canonical_hash_hex"

    }

    Write-Host "canonical_hash_hex=$($report.canonical_hash_hex)"



    if ($env:NODALMERGE_TOKEN_JSON) {

        Write-Host "== CLI archive describe (room://, token present) =="

        cargo run -p nodalmerge-cli -- archive describe --archive-ref "room://$Room" --room $Room 2>&1 | Out-Host

        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }



        Write-Host "== CLI query list-projections =="

        cargo run -p nodalmerge-cli -- query list-projections --room $Room 2>&1 | Out-Host

        if ($LASTEXITCODE -ne 0) {

            Write-Warning "query list-projections failed — query control plane may require NodalMerge.DotNetHost /ws/runtime"

        }

        $tmpQueryDir = Join-Path $env:TEMP "nodalmerge-smoke-query"
        New-Item -ItemType Directory -Force -Path $tmpQueryDir | Out-Null
        $descriptorPath = Join-Path $tmpQueryDir "descriptor.json"
        @'
{ "prefix": "world/" }
'@ | Set-Content -Path $descriptorPath -Encoding UTF8

        Write-Host "== CLI query queue contention smoke (register/build/read) =="
        cargo run -p nodalmerge-cli -- query register-spec --room $Room --query-spec-id q.smoke --version v1 --descriptor-file $descriptorPath 2>&1 | Out-Host
        cargo run -p nodalmerge-cli -- query build-projection --room $Room --projection-id p.smoke --query-spec-id q.smoke --selector latest 2>&1 | Out-Host
        cargo run -p nodalmerge-cli -- query read-projection --room $Room --projection-id p.smoke --limit 5 2>&1 | Out-Host

    }

    else {

        Write-Host "== Skipping archive/query control-plane (pass -TokenJsonPath for locked-room smoke) =="

    }



    Write-Host "== Done =="

    Write-Host "Session report: $reportPath"

}

finally {

    Pop-Location

}


