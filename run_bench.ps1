function Cleanup-Ports {
    foreach ($p in @(7979, 8787)) {
        $id = Get-NetTCPConnection -LocalPort $p -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -First 1
        if ($id) { Stop-Process -Id $id -Force }
    }
}

function Start-MongoContainer {
    $name = "nodalmerge-mongo"
    docker start $name *> $null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "Using Mongo container: $name"
        return
    }
    throw "Could not start Mongo container '$name'."
}

function Resolve-DotnetHostProjectPath {
    $candidates = @(
        "nodalmerge-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw "Unable to locate DotNet host project at nodalmerge-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj."
}

Cleanup-Ports
New-Item -ItemType Directory -Force -Path ".\benchmarks\results\logs"
Start-MongoContainer
$ffiPath = (Get-ChildItem -Recurse -Filter "*host_ffi.dll" | Where-Object { $_.FullName -like "*target\debug*" } | Select-Object -First 1).FullName
if (-not $ffiPath) { $ffiPath = (Get-ChildItem -Recurse -Filter "*host_ffi.dll" | Where-Object { $_.FullName -like "*target\release*" } | Select-Object -First 1).FullName }

$env:AS_BIND_ADDR='127.0.0.1:7979'; $env:MONGO_URI='mongodb://127.0.0.1:27017'; $env:MONGO_DATABASE='nodalmerge_bench'
$rP = Start-Process -FilePath "cargo" -ArgumentList "run -p nodalmerge-dev-server --bin nodalmerge-dev-server" -NoNewWindow -PassThru -RedirectStandardOutput ".\benchmarks\results\logs\rust-integrated-mongo-clean.log" -RedirectStandardError ".\benchmarks\results\logs\rust-integrated-mongo-clean.err"

$dotnetHostProject = Resolve-DotnetHostProjectPath
$env:ASPNETCORE_URLS='http://127.0.0.1:8787'; $env:ASPNETCORE_ENVIRONMENT='Development'; $env:NodalMerge__Providers__NodeStorage='Mongo'
$env:NodalMerge__Providers__BlobStorage='WsOnly'; $env:NodalMerge__Storage__Mongo__ConnectionString='mongodb://127.0.0.1:27017'; $env:NodalMerge__Storage__Mongo__DatabaseName='nodalmerge_bench'; $env:NODALMERGE_HOST_FFI_DLL=$ffiPath
$dP = Start-Process -FilePath "dotnet" -ArgumentList "run --project $dotnetHostProject --no-launch-profile" -NoNewWindow -PassThru -RedirectStandardOutput ".\benchmarks\results\logs\dotnet-mongo-clean.log" -RedirectStandardError ".\benchmarks\results\logs\dotnet-mongo-clean.err"

try {
    $r79=0; $r87=0; $s=Get-Date
    while(((Get-Date)-$s).TotalSeconds -lt 45){
        if(!$r79){$t=New-Object System.Net.Sockets.TcpClient;try{$t.Connect('127.0.0.1',7979);$r79=1;$t.Close()}catch{}}
        if(!$r87){$t=New-Object System.Net.Sockets.TcpClient;try{$t.Connect('127.0.0.1',8787);$r87=1;$t.Close()}catch{}}
        if($r79 -and $r87){break}; Start-Sleep -s 1
    }
    Write-Host "Readiness: 7979=$r79, 8787=$r87"
    if(!($r79 -and $r87)){ exit 1 }

    node .\benchmarks\Run-SdkScenarioBenchmarks.mjs --targets rust-integrated-hosted-server,dotnet-host-runtime-alias --iterations 2 --peers 6 --mapOps 2 --listOps 2 --blobOps 2 --blobSizeBytes 1024 --warmupOps 4 --timeoutMs 30000 --opDelayMs 2 --transport ws-only --outputJsonPath ".\benchmarks\results\sdk-scenarios-peers6-ops2-integrated-vs-dotnet-mongo-clean.json"
    $smoke = Get-Content ".\benchmarks\results\sdk-scenarios-peers6-ops2-integrated-vs-dotnet-mongo-clean.json" | ConvertFrom-Json
    $rustS = $smoke | Where-Object { $_.target -eq 'rust-integrated-hosted-server' }; $dotnetS = $smoke | Where-Object { $_.target -eq 'dotnet-host-runtime-alias' }
    Write-Host "Smoke Status: Rust=$($rustS.status), Dotnet=$($dotnetS.status)"

    foreach ($ops in @(6, 12, 30)) {
        node .\benchmarks\Run-SdkScenarioBenchmarks.mjs --targets rust-integrated-hosted-server,dotnet-host-runtime-alias --iterations 3 --peers 6 --mapOps $ops --listOps $ops --blobOps $ops --blobSizeBytes 1024 --warmupOps 4 --timeoutMs 45000 --opDelayMs 2 --transport ws-only --outputJsonPath ".\benchmarks\results\sdk-scenarios-peers6-ops$ops-integrated-vs-dotnet-mongo-clean.json"
    }

    $md = "## NodalMerge Performance Sweep (Peers 6, Mongo Backend)`n`n"
    foreach ($type in @('map', 'list', 'blob')) {
        $md += "### $($type.ToUpper()) Scenario`n| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |`n| --- | --- | --- | --- | --- |`n"
        foreach ($ops in @(6, 12, 30)) {
            $json = Get-Content ".\benchmarks\results\sdk-scenarios-peers6-ops$ops-integrated-vs-dotnet-mongo-clean.json" | ConvertFrom-Json
            $r = $json | Where-Object { $_.target -eq 'rust-integrated-hosted-server' }; $d = $json | Where-Object { $_.target -eq 'dotnet-host-runtime-alias' }
            $rA = [Math]::Round($r.scenarios.$type.avg, 2); $dA = [Math]::Round($d.scenarios.$type.avg, 2)
            $rO = [Math]::Round(($ops * 6) / ($rA / 1000), 2); $dO = [Math]::Round(($ops * 6) / ($dA / 1000), 2)
            $md += "| $ops | $rA | $rO | $dA | $dO |`n"
        }
    }; $md | Set-Content ".\benchmarks\results\ops-sweep-peers6-integrated-vs-dotnet-mongo-clean.md"; Write-Host $md
} finally {
    Stop-Process -Id $rP.Id -Force -ErrorAction SilentlyContinue; Stop-Process -Id $dP.Id -Force -ErrorAction SilentlyContinue
}
