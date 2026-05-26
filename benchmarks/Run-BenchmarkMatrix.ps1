[CmdletBinding()]
param(
    [string]$MatrixPath = "./benchmarks/benchmark-matrix.v1.json",
    [string]$OutputPath = "./benchmarks/results/benchmark-matrix-latest.json",
    [string[]]$RowIds = @(),
    [string]$RustBind = "127.0.0.1:7878",
    [string]$DotnetBaseUrl = "http://127.0.0.1:8787",
    [string]$FfiDllPath = "",
    [switch]$SkipGates
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-CargoPath {
    $cargoCandidates = @(
        (Get-Command cargo -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
        (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe")
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($candidate in $cargoCandidates) {
        if (Test-Path $candidate) {
            return [System.IO.Path]::GetFullPath($candidate)
        }
    }

    throw "cargo executable not found. Install Rust or add cargo to PATH."
}

function Resolve-DotnetPath {
    $dotnet = Get-Command dotnet -ErrorAction SilentlyContinue
    if ($null -eq $dotnet) {
        throw "dotnet executable not found in PATH."
    }
    return $dotnet.Source
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

function Resolve-FfiDll {
    param([string]$Explicit)

    if (-not [string]::IsNullOrWhiteSpace($Explicit) -and (Test-Path $Explicit)) {
        return [System.IO.Path]::GetFullPath($Explicit)
    }

    $candidates = @(
        ".\target\debug\nodalmerge_host_ffi.dll",
        ".\target\release\nodalmerge_host_ffi.dll",
        ".\target\debug\activesync_host_ffi.dll",
        ".\target\release\activesync_host_ffi.dll"
    )

    foreach ($candidate in $candidates) {
        $full = [System.IO.Path]::GetFullPath($candidate)
        if (Test-Path $full) {
            return $full
        }
    }

    throw "NODALMERGE_HOST_FFI_DLL could not be resolved. Build host-ffi first (cargo build -p activesync-host-ffi, legacy crate id) or pass -FfiDllPath."
}

function Merge-Env {
    param(
        [hashtable]$Base,
        [hashtable]$Overlay
    )

    $merged = @{}
    foreach ($k in $Base.Keys) { $merged[$k] = [string]$Base[$k] }
    foreach ($k in $Overlay.Keys) { $merged[$k] = [string]$Overlay[$k] }
    return $merged
}

function Get-OptionalPropertyValue {
    param(
        [object]$Object,
        [string]$PropertyName,
        $DefaultValue
    )

    if ($null -eq $Object) {
        return $DefaultValue
    }

    $prop = $Object.PSObject.Properties[$PropertyName]
    if ($null -eq $prop -or $null -eq $prop.Value) {
        return $DefaultValue
    }

    return $prop.Value
}

function Start-RowTargets {
    param(
        [string]$CargoPath,
        [string]$DotnetPath,
        [string]$FfiDll,
        [string]$RustBindAddress,
        [string]$DotnetUrl,
        [hashtable]$RustEnv,
        [hashtable]$DotnetEnv
    )

    $rustEnvBase = @{ AS_BIND_ADDR = $RustBindAddress }
    $dotnetEnvBase = @{
        ASPNETCORE_URLS = $DotnetUrl
        NODALMERGE_HOST_FFI_DLL = $FfiDll
    }

    $rustEnv = Merge-Env -Base $rustEnvBase -Overlay $RustEnv
    $dotnetEnv = Merge-Env -Base $dotnetEnvBase -Overlay $DotnetEnv
    $dotnetHostProject = Resolve-DotnetHostProjectPath

    $rustProc = Start-Process -FilePath $CargoPath -ArgumentList @("run", "-p", "activesync-server", "--bin", "nodalmerge-server") -PassThru -NoNewWindow -Env $rustEnv
    $dotnetProc = Start-Process -FilePath $DotnetPath -ArgumentList @("run", "--project", $dotnetHostProject, "--no-launch-profile") -PassThru -NoNewWindow -Env $dotnetEnv

    # Give endpoints time to bind before scenario runner starts probing.
    Start-Sleep -Seconds 4

    return @{
        rust = $rustProc
        dotnet = $dotnetProc
    }
}

function Stop-Proc {
    param([System.Diagnostics.Process]$Proc)

    if ($null -eq $Proc) { return }
    try {
        if (-not $Proc.HasExited) {
            $Proc.Kill($true)
        }
    }
    catch {
        # best-effort cleanup
    }
}

function Get-ScenarioAggregates {
    param([object]$ScenarioJson)

    $aggregates = @{}

    foreach ($entry in $ScenarioJson.results) {
        if ($entry.status -ne "ok") {
            continue
        }

        $target = [string]$entry.target
        if (-not $aggregates.ContainsKey($target)) {
            $aggregates[$target] = [ordered]@{
                map_avg_ms = @()
                map_p95_ms = @()
                list_avg_ms = @()
                list_p95_ms = @()
                blob_avg_ms = @()
                blob_p95_ms = @()
            }
        }

        foreach ($metric in @("map_avg_ms","map_p95_ms","list_avg_ms","list_p95_ms","blob_avg_ms","blob_p95_ms")) {
            $val = $entry.$metric
            if ($null -ne $val -and $val -is [double] -or $val -is [float] -or $val -is [decimal] -or $val -is [int] -or $val -is [long]) {
                $aggregates[$target][$metric] += [double]$val
            }
        }
    }

    $final = @{}
    foreach ($target in $aggregates.Keys) {
        $raw = $aggregates[$target]
        $final[$target] = [ordered]@{}
        foreach ($metric in $raw.Keys) {
            $values = @($raw[$metric])
            $final[$target][$metric] = if ($values.Count -gt 0) {
                [Math]::Round((($values | Measure-Object -Average).Average), 3)
            } else {
                $null
            }
        }
    }

    return $final
}

function Get-RegressionPct {
    param(
        [double]$Baseline,
        [double]$Candidate
    )

    if ($Baseline -eq 0.0) {
        if ($Candidate -eq 0.0) { return 0.0 }
        return 100.0
    }

    return [Math]::Round((($Candidate - $Baseline) / $Baseline) * 100.0, 4)
}

function Invoke-Gates {
    param(
        $Rows,
        [hashtable]$Thresholds
    )

    $rowsArray = @($Rows)
    if ($rowsArray.Count -eq 0) { return }

    $baseline = $rowsArray[0]
    foreach ($row in $rowsArray) {
        if ($row.id -eq $baseline.id) {
            $row.gate = [ordered]@{ status = "baseline" }
            continue
        }

        $violations = New-Object System.Collections.Generic.List[string]
        $comparisons = @{}

        foreach ($target in $row.aggregates.PSObject.Properties.Name) {
            if (-not $baseline.aggregates.PSObject.Properties.Name.Contains($target)) {
                continue
            }

            $comparisons[$target] = [ordered]@{}
            foreach ($metric in @("map_avg_ms","list_avg_ms","blob_avg_ms","map_p95_ms","list_p95_ms","blob_p95_ms")) {
                $baseMetric = $baseline.aggregates.$target.$metric
                $candMetric = $row.aggregates.$target.$metric
                if ($null -eq $baseMetric -or $null -eq $candMetric) {
                    continue
                }

                $reg = Get-RegressionPct -Baseline ([double]$baseMetric) -Candidate ([double]$candMetric)
                $comparisons[$target][$metric] = $reg

                $thresholdKey = switch ($metric) {
                    "map_avg_ms" { "map_avg_regression_pct_max" }
                    "list_avg_ms" { "list_avg_regression_pct_max" }
                    "blob_avg_ms" { "blob_avg_regression_pct_max" }
                    "map_p95_ms" { "map_p95_regression_pct_max" }
                    "list_p95_ms" { "list_p95_regression_pct_max" }
                    "blob_p95_ms" { "blob_p95_regression_pct_max" }
                }

                $maxAllowed = [double]$Thresholds[$thresholdKey]
                if ($reg -gt $maxAllowed) {
                    $violations.Add("$target.$metric regression $reg exceeds $maxAllowed")
                }
            }
        }

        $row.gate = [ordered]@{
            status = if ($violations.Count -eq 0) { "pass" } else { "fail" }
            comparisons = $comparisons
            violations = @($violations)
        }
    }
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
Push-Location $repoRoot
try {
    $matrixFullPath = [System.IO.Path]::GetFullPath($MatrixPath)
    if (-not (Test-Path $matrixFullPath)) {
        throw "Matrix file not found: $matrixFullPath"
    }

    $cargoPath = Resolve-CargoPath
    $dotnetPath = Resolve-DotnetPath
    $ffiDll = Resolve-FfiDll -Explicit $FfiDllPath

    $matrix = Get-Content -Raw -Path $matrixFullPath | ConvertFrom-Json -Depth 20
    $rows = @($matrix.rows)
    if ($RowIds.Count -gt 0) {
        $rows = @($rows | Where-Object { $RowIds -contains $_.id })
    }

    if ($rows.Count -eq 0) {
        throw "No matrix rows selected."
    }

    $runId = "bench-matrix-" + [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
    $resultRows = New-Object System.Collections.Generic.List[object]

    foreach ($row in $rows) {
        Write-Host "==> row $($row.id): $($row.label)"
        $startedAt = [DateTime]::UtcNow
        $procs = $null
        $scenarioOut = "./benchmarks/results/$($runId)-$($row.id)-sdk-scenarios.json"

        try {
            $rustEnv = @{}
            if ($null -ne $row.rust -and $null -ne $row.rust.env) {
                foreach ($p in $row.rust.env.PSObject.Properties) {
                    $rustEnv[$p.Name] = [string]$p.Value
                }
            }

            $dotnetEnv = @{}
            if ($null -ne $row.dotnet -and $null -ne $row.dotnet.env) {
                foreach ($p in $row.dotnet.env.PSObject.Properties) {
                    $dotnetEnv[$p.Name] = [string]$p.Value
                }
            }

            $procs = Start-RowTargets -CargoPath $cargoPath -DotnetPath $dotnetPath -FfiDll $ffiDll -RustBindAddress $RustBind -DotnetUrl $DotnetBaseUrl -RustEnv $rustEnv -DotnetEnv $dotnetEnv

            $rowPeerCounts = Get-OptionalPropertyValue -Object $row -PropertyName "peerCounts" -DefaultValue $matrix.defaults.peerCounts
            $peerCsv = (@($rowPeerCounts) -join ",")
            $commandMix = Get-OptionalPropertyValue -Object $row -PropertyName "commandMix" -DefaultValue $matrix.defaults.commandMix
            $iterations = [int](Get-OptionalPropertyValue -Object $row -PropertyName "iterations" -DefaultValue $matrix.defaults.iterations)
            $rowTargets = [string](Get-OptionalPropertyValue -Object $row -PropertyName "targets" -DefaultValue $matrix.defaults.targets)
            $scenario = Get-OptionalPropertyValue -Object $row -PropertyName "scenario" -DefaultValue $null

            $nodeArgs = @(
                "./benchmarks/Run-SdkScenarioBenchmarks.mjs",
                "--iterations", [string]$iterations,
                "--peers", $peerCsv,
                "--targets", $rowTargets,
                "--mapOps", [string]$commandMix.mapOps,
                "--listOps", [string]$commandMix.listOps,
                "--blobOps", [string]$commandMix.blobOps,
                "--blobSizeBytes", [string]$commandMix.blobSizeBytes,
                "--warmupOps", [string]$matrix.defaults.warmupOps,
                "--timeoutMs", [string]$matrix.defaults.timeoutMs,
                "--transport", [string]$matrix.defaults.transport,
                "--outputJsonPath", $scenarioOut
            )

            if ($null -ne $scenario) {
                $authMode = Get-OptionalPropertyValue -Object $scenario -PropertyName "authMode" -DefaultValue $null
                if (-not [string]::IsNullOrWhiteSpace([string]$authMode)) {
                    $nodeArgs += @("--authMode", [string]$authMode)
                }

                $tokenExpiry = Get-OptionalPropertyValue -Object $scenario -PropertyName "tokenExpirySecs" -DefaultValue $null
                if ($null -ne $tokenExpiry) {
                    $nodeArgs += @("--tokenExpirySecs", [string]$tokenExpiry)
                }

                $authTokenCaps = Get-OptionalPropertyValue -Object $scenario -PropertyName "authTokenCaps" -DefaultValue $null
                if ($null -ne $authTokenCaps) {
                    $nodeArgs += @("--authTokenCaps", ((@($authTokenCaps)) -join ","))
                }

                $authAdminCaps = Get-OptionalPropertyValue -Object $scenario -PropertyName "authAdminCaps" -DefaultValue $null
                if ($null -ne $authAdminCaps) {
                    $nodeArgs += @("--authAdminCaps", ((@($authAdminCaps)) -join ","))
                }
            }

            & node @nodeArgs
            if ($LASTEXITCODE -ne 0) {
                throw "Run-SdkScenarioBenchmarks.mjs failed for row '$($row.id)' with exit code $LASTEXITCODE"
            }

            $scenario = Get-Content -Raw -Path $scenarioOut | ConvertFrom-Json -Depth 20
            $aggregates = Get-ScenarioAggregates -ScenarioJson $scenario

            $resultRows.Add([ordered]@{
                id = [string]$row.id
                label = [string]$row.label
                status = "ok"
                dimensions = $row.dimensions
                scenario_result_path = $scenarioOut
                started_utc = $startedAt.ToString("o")
                completed_utc = [DateTime]::UtcNow.ToString("o")
                aggregates = $aggregates
            })
        }
        catch {
            $resultRows.Add([ordered]@{
                id = [string]$row.id
                label = [string]$row.label
                status = "error"
                dimensions = $row.dimensions
                scenario_result_path = $scenarioOut
                started_utc = $startedAt.ToString("o")
                completed_utc = [DateTime]::UtcNow.ToString("o")
                error = $_.Exception.Message
            })
        }
        finally {
            if ($null -ne $procs) {
                Stop-Proc -Proc $procs.rust
                Stop-Proc -Proc $procs.dotnet
            }
        }
    }

    if (-not $SkipGates) {
        $rowsForGates = $resultRows.ToArray()
        Invoke-Gates -Rows $rowsForGates -Thresholds @{
            map_avg_regression_pct_max = [double]$matrix.gates.map_avg_regression_pct_max
            list_avg_regression_pct_max = [double]$matrix.gates.list_avg_regression_pct_max
            blob_avg_regression_pct_max = [double]$matrix.gates.blob_avg_regression_pct_max
            map_p95_regression_pct_max = [double]$matrix.gates.map_p95_regression_pct_max
            list_p95_regression_pct_max = [double]$matrix.gates.list_p95_regression_pct_max
            blob_p95_regression_pct_max = [double]$matrix.gates.blob_p95_regression_pct_max
        }
    }

    $result = [ordered]@{
        schema_version = "benchmark-matrix-result.v1"
        run_id = $runId
        generated_utc = [DateTime]::UtcNow.ToString("o")
        matrix_path = $MatrixPath
        rows = $resultRows.ToArray()
    }

    $outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
    $outputDir = Split-Path -Parent $outputFullPath
    if (-not (Test-Path $outputDir)) {
        New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
    }
    $result | ConvertTo-Json -Depth 20 | Set-Content -Path $outputFullPath -Encoding UTF8

    Write-Host "Benchmark matrix result written: $outputFullPath"
}
finally {
    Pop-Location
}