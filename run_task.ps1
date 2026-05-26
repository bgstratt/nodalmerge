try {
    function Start-MongoContainer {
        foreach ($name in @("nodalmerge-mongo", "activesync-mongo")) {
            docker start $name *> $null
            if ($LASTEXITCODE -eq 0) {
                if ($name -eq "activesync-mongo") {
                    Write-Host "Using Mongo container: $name (legacy compatibility alias)"
                }
                else {
                    Write-Host "Using Mongo container: $name"
                }
                return
            }
        }
        throw "Could not start Mongo container (tried nodalmerge-mongo first, then activesync-mongo legacy alias)."
    }

    function Resolve-DotnetHostProjectPath {
        $candidates = @(
            "nodalmerge-host/src/NodalMerge.DotNetHost/NodalMerge.DotNetHost.csproj",
            "nodalmerge-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj"
        )

        foreach ($candidate in $candidates) {
            if (Test-Path $candidate) {
                return $candidate
            }
        }

        throw "Unable to locate DotNet host project. Checked NodalMerge path first, then legacy ActiveSync compatibility path."
    }

    Write-Host "Starting Docker container..."
    Start-MongoContainer

    $rustEnv = @{
        "AS_BIND_ADDR" = "127.0.0.1:7979"
        "MONGO_URI" = "mongodb://127.0.0.1:27017"
        "MONGO_DATABASE" = "nodalmerge_bench"
    }
    Write-Host "Starting Rust server..."
    $rustJob = Start-Process cargo -ArgumentList "run -p activesync-dev-server --bin nodalmerge-dev-server" -Environment $rustEnv -PassThru -NoNewWindow

    $dotnetEnv = @{
        "ASPNETCORE_URLS" = "http://127.0.0.1:8787"
        "ASPNETCORE_ENVIRONMENT" = "Development"
        "NodalMerge__Providers__NodeStorage" = "Mongo"
        "NodalMerge__Providers__BlobStorage" = "WsOnly"
        "NodalMerge__Storage__Mongo__ConnectionString" = "mongodb://127.0.0.1:27017"
        "NodalMerge__Storage__Mongo__DatabaseName" = "nodalmerge_bench"
    }
    $dotnetHostProject = Resolve-DotnetHostProjectPath
    Write-Host "Starting DotNet host..."
    $dotnetJob = Start-Process dotnet -ArgumentList "run --project $dotnetHostProject --no-launch-profile" -Environment $dotnetEnv -PassThru -NoNewWindow

    Write-Host "Waiting 10 seconds for servers to start..."
    Start-Sleep -Seconds 10

    $ops_list = @(2, 6, 12, 30)
    $results_dir = ".\benchmarks\results"
    if (!(Test-Path $results_dir)) { New-Item -ItemType Directory -Path $results_dir }

    foreach ($op in $ops_list) {
        $output_file = Join-Path $results_dir "sdk-scenarios-peers6-ops$op-integrated-vs-dotnet-mongo.json"
        Write-Host "Running benchmark for ops $op..."
        # Note: Assuming 'node .\benchmarks\sdk-scenarios.js' or similar exists and takes parameters
        # Adjusting the command based on the prompt's implied benchmark script
        node .\benchmarks\run-bench.js --peers 6 --iterations 3 --ops $op --ws-only --targets rust-integrated-hosted-server,dotnet-host-runtime-alias --output $output_file
        Write-Host "Finished ops $op: $output_file"
    }

    # Build Markdown
    $mdPath = ".\benchmarks\results\ops-sweep-peers6-integrated-vs-dotnet-mongo.md"
    $mdContent = "# Benchmark results

"

    foreach ($type in @("Map", "List", "Blob")) {
        $mdContent += "## $type
"
        $mdContent += "| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
"
        $mdContent += "| --- | --- | --- | --- | --- |
"
        
        foreach ($op in $ops_list) {
            $json_file = Join-Path $results_dir "sdk-scenarios-peers6-ops$op-integrated-vs-dotnet-mongo.json"
            if (Test-Path $json_file) {
                $data = Get-Content $json_file | ConvertFrom-Json
                # This part depends on the JSON structure. Assuming $data.results[target][type].avg_ms
                # Rust: rust-integrated-hosted-server
                # Dotnet: dotnet-host-runtime-alias
                $rust_avg = $data.results."rust-integrated-hosted-server"."$type".avg_ms
                $dotnet_avg = $data.results."dotnet-host-runtime-alias"."$type".avg_ms
                
                $rust_ops_s = [math]::Round(($op * 6) / ($rust_avg / 1000), 2)
                $dotnet_ops_s = [math]::Round(($op * 6) / ($dotnet_avg / 1000), 2)
                
                $mdContent += "| $op | $rust_avg | $rust_ops_s | $dotnet_avg | $dotnet_ops_s |
"
            }
        }
        $mdContent += "
"
    }

    Set-Content -Path $mdPath -Value $mdContent
    Write-Host "Markdown generated."
    Get-Content $mdPath

} finally {
    Write-Host "Stopping processes..."
    if ($rustJob) { Stop-Process -Id $rustJob.Id -Force -ErrorAction SilentlyContinue }
    if ($dotnetJob) { Stop-Process -Id $dotnetJob.Id -Force -ErrorAction SilentlyContinue }
}
