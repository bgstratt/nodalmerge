param(
    [string]$Project = "",
    [string]$ResultsDirectory = "../docs/acceptance",
    [string]$LogFileName = "authz-dotnet-conformance-targeted.trx",
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

function Resolve-TestProjectPath {
    param([string]$Explicit)

    if (-not [string]::IsNullOrWhiteSpace($Explicit)) {
        return $Explicit
    }

    $candidates = @(
        "tests/ActiveSync.DotNetHost.Tests/NodalMerge.DotNetHost.Tests.csproj",
        "tests/ActiveSync.DotNetHost.Tests/NodalMerge.DotNetHost.Tests.csproj"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw "Unable to locate DotNet host test project. Checked NodalMerge path first, then legacy ActiveSync compatibility path."
}

Push-Location $scriptRoot
try {
    $resolvedProject = Resolve-TestProjectPath -Explicit $Project

    $filter = @(
        "FullyQualifiedName~RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope"
        "FullyQualifiedName~RuntimeMessageProcessorTests.Start_tick_without_capability_returns_control_plane_forbidden_error_envelope"
        "FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_denies_when_provider_rejects_token"
        "FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_allows_when_provider_accepts_token"
        "FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch"
        "FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed"
        "FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_without_tick_admin_capability_is_denied_before_bridge_dispatch"
        "FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_with_tick_admin_capability_is_allowed"
    ) -join "|"

    if (-not (Test-Path $ResultsDirectory)) {
        New-Item -ItemType Directory -Path $ResultsDirectory | Out-Null
    }

    $args = @(
        "test"
        $resolvedProject
        "--filter", $filter
        "--logger", "trx;LogFileName=$LogFileName"
        "--results-directory", $ResultsDirectory
        "-v", "minimal"
    )

    if ($NoBuild) {
        $args += "--no-build"
    }

    Write-Host "Running authz baseline slice (8 tests)..." -ForegroundColor Cyan
    & dotnet @args
    $testExit = $LASTEXITCODE

    $trxPath = Join-Path $ResultsDirectory $LogFileName
    if (-not (Test-Path $trxPath)) {
        Write-Host "TRX file missing: $trxPath" -ForegroundColor Red
        exit 1
    }

    [xml]$trx = Get-Content $trxPath -Raw
    $c = $trx.TestRun.ResultSummary.Counters
    Write-Host ""
    Write-Host "Baseline counters:" -ForegroundColor Green
    Write-Host ("total={0} executed={1} passed={2} failed={3} notExecuted={4}" -f $c.total, $c.executed, $c.passed, $c.failed, $c.notExecuted)
    Write-Host "artifact=$trxPath"

    exit $testExit
}
finally {
    Pop-Location
}

