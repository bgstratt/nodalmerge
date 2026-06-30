[CmdletBinding()]
param(
    [string]$Version = "0.1.0-local",
    [string]$OutputDir = "../artifacts/nuget-local"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock]$Command,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

function Clear-GlobalNuGetPackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PackageId,
        [Parameter(Mandatory = $true)]
        [string]$PackageVersion
    )

    $globalPackagesRoot = Join-Path $HOME ".nuget\packages"
    $versionPath = Join-Path (Join-Path $globalPackagesRoot $PackageId.ToLowerInvariant()) $PackageVersion.ToLowerInvariant()
    if (Test-Path -LiteralPath $versionPath) {
        Write-Host "Clearing cached package: $PackageId $PackageVersion"
        Remove-Item -LiteralPath $versionPath -Recurse -Force
    }
}

function Resolve-CommandPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [string[]]$Fallbacks = @()
    )

    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($cmd) {
        return $cmd.Source
    }

    foreach ($fallback in $Fallbacks) {
        if (-not [string]::IsNullOrWhiteSpace($fallback) -and (Test-Path $fallback)) {
            return [System.IO.Path]::GetFullPath($fallback)
        }
    }

    return $null
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptDir
try {
    $cargoFallback = Join-Path $HOME ".cargo\bin\cargo.exe"
    $cargoPath = Resolve-CommandPath -Name "cargo" -Fallbacks @($cargoFallback)
    if (-not $cargoPath) {
        throw "cargo executable not found. Install Rust or add cargo to PATH (`$HOME\.cargo\bin`)."
    }

    $resolvedOutput = [System.IO.Path]::GetFullPath((Join-Path $scriptDir $OutputDir))
    New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null

    Write-Host "Building native runtime (release)..."
    Invoke-Checked -Name "cargo build host ffi runtime (nodalmerge-host-ffi)" -Command {
        & $cargoPath build -p nodalmerge-host-ffi --release
    }
    Invoke-Checked -Name "cargo build peer-local ffi runtime (nodalmerge-runtime-local-ffi)" -Command {
        & $cargoPath build -p nodalmerge-runtime-local-ffi --release
    }

    Write-Host "Packing managed packages to $resolvedOutput ..."
    Invoke-Checked -Name "dotnet pack NodalMerge.Host.Abstractions" -Command {
        dotnet pack ./src/NodalMerge.Host.Abstractions/NodalMerge.Host.Abstractions.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack NodalMerge.Host.Composition" -Command {
        dotnet pack ./src/NodalMerge.Host.Composition/NodalMerge.Host.Composition.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost" -Command {
        dotnet pack ./src/NodalMerge.DotNetHost/NodalMerge.DotNetHost.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    Write-Host "Packing native runtime packages to $resolvedOutput ..."
    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost.Native.win-x64" -Command {
        dotnet pack ./src/NodalMerge.DotNetHost.Native.win-x64/NodalMerge.DotNetHost.Native.win-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    $linuxLocalNative = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "../target/release/libnodalmerge_runtime_local_ffi.so"))
    if (-not (Test-Path -LiteralPath $linuxLocalNative)) {
        $isWindowsRuntime = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)
        if ($isWindowsRuntime) {
            Write-Warning "linux-x64 peer-local FFI artifact not found at $linuxLocalNative; creating placeholder for package restore on Windows."
            $linuxLocalDir = Split-Path -Parent $linuxLocalNative
            New-Item -ItemType Directory -Force -Path $linuxLocalDir | Out-Null
            New-Item -ItemType File -Force -Path $linuxLocalNative | Out-Null
        }
        else {
            throw "Missing linux peer-local native artifact at $linuxLocalNative"
        }
    }

    $linuxNative = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "../target/release/libnodalmerge_host_ffi.so"))
    if (-not (Test-Path -LiteralPath $linuxNative)) {
        $isWindowsRuntime = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)
        if ($isWindowsRuntime) {
            Write-Warning "linux-x64 native artifact not found at $linuxNative; creating local placeholder for package-mode restore on Windows."
            $linuxDir = Split-Path -Parent $linuxNative
            New-Item -ItemType Directory -Force -Path $linuxDir | Out-Null
            New-Item -ItemType File -Force -Path $linuxNative | Out-Null
        }
        else {
            throw "Missing linux native artifact at $linuxNative"
        }
    }

    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost.Native.linux-x64" -Command {
        dotnet pack ./src/NodalMerge.DotNetHost.Native.linux-x64/NodalMerge.DotNetHost.Native.linux-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    # Ensure subsequent restore picks up freshly packed local artifacts even when version is reused.
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.Host.Abstractions" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.Host.Composition" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.DotNetHost" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.DotNetHost.Native.win-x64" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.DotNetHost.Native.linux-x64" -PackageVersion $Version

    Write-Host "Done. Local packages available in: $resolvedOutput"
}
finally {
    Pop-Location
}

