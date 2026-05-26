param(
    [string]$Project = "tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj",
    [string]$Configuration = "Debug",
    [switch]$NoBuild,
    [string]$AdditionalFilter = ""
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptRoot
try {
    # Keep quick loop deterministic and short:
    # - include core runtime adapter unit tests
    # - exclude websocket FFI loop suites that can run long in quick loops
    # - exclude known long stress/churn/soak cases
    # - exclude native host-ffi binding tests (require local native lib path)
    $includeFilter = @(
        "FullyQualifiedName~RuntimeProtocolTests"
        "FullyQualifiedName~RuntimeMessageProcessorTests"
        "FullyQualifiedName~RuntimeTokenValidationServiceTests"
        "FullyQualifiedName~RuntimeWebSocketEndpointTests"
        "FullyQualifiedName~RuntimeFrameProcessorTests"
    ) -join "|"

    $excludeFilter = @(
        "FullyQualifiedName!~FfiBindingTests"
        "FullyQualifiedName!~parallel"
        "FullyQualifiedName!~churn"
        "FullyQualifiedName!~soak"
        "FullyQualifiedName!~partial_fragment_then_abort"
        "FullyQualifiedName!~stop_tick_with_tick_admin_capability"
    ) -join "&"

    $baseFilter = "($includeFilter)&$excludeFilter"

    $effectiveFilter = $baseFilter
    if (-not [string]::IsNullOrWhiteSpace($AdditionalFilter)) {
        $effectiveFilter = "$baseFilter&$AdditionalFilter"
    }

    $args = @(
        "test"
        $Project
        "--configuration", $Configuration
        "--filter", $effectiveFilter
        "-v", "minimal"
    )

    if ($NoBuild) {
        $args += "--no-build"
    }

    Write-Host "Running quick authz loop filter:" -ForegroundColor Cyan
    Write-Host $effectiveFilter
    Write-Host ""

    & dotnet @args
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
