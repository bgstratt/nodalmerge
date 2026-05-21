[CmdletBinding()]
param(
    [string]$Version = "0.1.0-local",
    [string]$OutputDir = "../artifacts/nuget-local"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptDir
try {
    $resolvedOutput = [System.IO.Path]::GetFullPath((Join-Path $scriptDir $OutputDir))
    New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null

    Write-Host "Building native runtime (release)..."
    cargo build -p activesync-host-ffi --release

    Write-Host "Packing managed packages to $resolvedOutput ..."
    dotnet pack ./src/ActiveSync.Host.Abstractions/ActiveSync.Host.Abstractions.csproj -c Release -o $resolvedOutput /p:Version=$Version
    dotnet pack ./src/ActiveSync.Host.Composition/ActiveSync.Host.Composition.csproj -c Release -o $resolvedOutput /p:Version=$Version

    Write-Host "Packing native runtime packages to $resolvedOutput ..."
    dotnet pack ./src/ActiveSync.DotNetHost.Native.win-x64/ActiveSync.DotNetHost.Native.win-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    dotnet pack ./src/ActiveSync.DotNetHost.Native.linux-x64/ActiveSync.DotNetHost.Native.linux-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version

    Write-Host "Done. Local packages available in: $resolvedOutput"
}
finally {
    Pop-Location
}
