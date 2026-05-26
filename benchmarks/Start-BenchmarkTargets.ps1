[CmdletBinding()]
param(
    [string]$RustCombinedBind = "127.0.0.1:7878",
    [string]$RustIntegratedBind = "127.0.0.1:7979",
    [string]$DotnetBaseUrl = "http://127.0.0.1:8787",
    [switch]$StartRustCombined = $true,
    [switch]$StartRustIntegrated,
    [switch]$StartDotnet = $true,
    [string]$MongoUri = "",
    [string]$MongoDatabase = "activesync_bench",
    [string]$FfiDllPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-FfiDllPath {
    param([string]$Explicit)

    if (-not [string]::IsNullOrWhiteSpace($Explicit) -and (Test-Path $Explicit)) {
        return [System.IO.Path]::GetFullPath($Explicit)
    }

    $candidates = @(
        (Join-Path $PSScriptRoot "..\target\debug\activesync_host_ffi.dll"),
        (Join-Path $PSScriptRoot "..\target\release\activesync_host_ffi.dll")
    )

    foreach ($candidate in $candidates) {
        $full = [System.IO.Path]::GetFullPath($candidate)
        if (Test-Path $full) {
            return $full
        }
    }

    return $null
}

$started = New-Object System.Collections.Generic.List[object]

try {
    if ($StartRustCombined) {
        Write-Host "Starting rust-combined-server on $RustCombinedBind"
        $envMap = @{ AS_BIND_ADDR = $RustCombinedBind }
        $proc = Start-Process cargo -ArgumentList @("run", "-p", "activesync-server") -PassThru -NoNewWindow -Env $envMap
        $started.Add([pscustomobject]@{ Name = "rust-combined-server"; Process = $proc })
    }

    if ($StartRustIntegrated) {
        if ([string]::IsNullOrWhiteSpace($MongoUri)) {
            throw "StartRustIntegrated requires -MongoUri for activesync-dev-server"
        }

        Write-Host "Starting rust-integrated-hosted-server on $RustIntegratedBind"
        $envMap = @{
            AS_BIND_ADDR = $RustIntegratedBind
            MONGO_URI = $MongoUri
            MONGO_DATABASE = $MongoDatabase
        }
        $proc = Start-Process cargo -ArgumentList @("run", "-p", "activesync-dev-server") -PassThru -NoNewWindow -Env $envMap
        $started.Add([pscustomobject]@{ Name = "rust-integrated-hosted-server"; Process = $proc })
    }

    if ($StartDotnet) {
        $ffiPath = Resolve-FfiDllPath -Explicit $FfiDllPath
        if (-not $ffiPath) {
            Write-Warning "No NODALMERGE_HOST_FFI_DLL/ACTIVESYNC_HOST_FFI_DLL found. Build host-ffi first: cargo build -p activesync-host-ffi"
        }

        Write-Host "Starting dotnet-host-runtime at $DotnetBaseUrl"
        $envMap = @{ ASPNETCORE_URLS = $DotnetBaseUrl }
        if ($ffiPath) {
            $envMap["NODALMERGE_HOST_FFI_DLL"] = $ffiPath
            $envMap["ACTIVESYNC_HOST_FFI_DLL"] = $ffiPath
        }

        $proc = Start-Process dotnet -ArgumentList @("run", "--project", "nodalmerge-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj", "--no-launch-profile") -PassThru -NoNewWindow -Env $envMap
        $started.Add([pscustomobject]@{ Name = "dotnet-host-runtime"; Process = $proc })
    }

    Write-Host ""
    Write-Host "Started targets:"
    foreach ($entry in $started) {
        Write-Host ("  {0} pid={1}" -f $entry.Name, $entry.Process.Id)
    }

    Write-Host ""
    Write-Host "Run benchmarks:"
    Write-Host "  pwsh -File .\benchmarks\Run-HostedServiceBenchmarks.ps1 -MeasureIterations 100"
    Write-Host ""
    Write-Host "Press Enter to stop all started targets..."
    [void](Read-Host)
}
finally {
    foreach ($entry in $started) {
        if ($entry.Process -and -not $entry.Process.HasExited) {
            Write-Host ("Stopping {0} pid={1}" -f $entry.Name, $entry.Process.Id)
            try {
                $entry.Process.Kill($true)
            }
            catch {
                Write-Warning ("Failed to stop {0}: {1}" -f $entry.Name, $_.Exception.Message)
            }
        }
    }
}
