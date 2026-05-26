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

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $scriptDir
try {
    $resolvedOutput = [System.IO.Path]::GetFullPath((Join-Path $scriptDir $OutputDir))
    New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null

    Write-Host "Building native runtime (release)..."
    Invoke-Checked -Name "cargo build host ffi runtime (activesync-host-ffi legacy crate id)" -Command {
        cargo build -p activesync-host-ffi --release
    }

    Write-Host "Packing managed packages to $resolvedOutput ..."
    Invoke-Checked -Name "dotnet pack ActiveSync.Host.Abstractions (legacy compat)" -Command {
        dotnet pack ./src/ActiveSync.Host.Abstractions/ActiveSync.Host.Abstractions.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack ActiveSync.Host.Composition (legacy compat)" -Command {
        dotnet pack ./src/ActiveSync.Host.Composition/ActiveSync.Host.Composition.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack NodalMerge.Host.Abstractions" -Command {
        dotnet pack ./src/NodalMerge.Host.Abstractions/NodalMerge.Host.Abstractions.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack NodalMerge.Host.Composition" -Command {
        dotnet pack ./src/NodalMerge.Host.Composition/NodalMerge.Host.Composition.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    Write-Host "Packing native runtime packages to $resolvedOutput ..."
    Invoke-Checked -Name "dotnet pack ActiveSync.DotNetHost.Native.win-x64 (legacy compat)" -Command {
        dotnet pack ./src/ActiveSync.DotNetHost.Native.win-x64/ActiveSync.DotNetHost.Native.win-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost.Native.win-x64" -Command {
        dotnet pack ./src/NodalMerge.DotNetHost.Native.win-x64/NodalMerge.DotNetHost.Native.win-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    $linuxNative = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "../target/release/libactivesync_host_ffi.so"))
    if (-not (Test-Path -LiteralPath $linuxNative)) {
        $isWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)
        if ($isWindows) {
            Write-Warning "linux-x64 native artifact not found at $linuxNative; creating local placeholder for package-mode restore on Windows."
            $linuxDir = Split-Path -Parent $linuxNative
            New-Item -ItemType Directory -Force -Path $linuxDir | Out-Null
            New-Item -ItemType File -Force -Path $linuxNative | Out-Null
        }
        else {
            throw "Missing linux native artifact at $linuxNative"
        }
    }

    Invoke-Checked -Name "dotnet pack ActiveSync.DotNetHost.Native.linux-x64 (legacy compat)" -Command {
        dotnet pack ./src/ActiveSync.DotNetHost.Native.linux-x64/ActiveSync.DotNetHost.Native.linux-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }
    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost.Native.linux-x64" -Command {
        dotnet pack ./src/NodalMerge.DotNetHost.Native.linux-x64/NodalMerge.DotNetHost.Native.linux-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    # Ensure subsequent restore picks up freshly packed local artifacts even when version is reused.
    Clear-GlobalNuGetPackageVersion -PackageId "ActiveSync.Host.Abstractions" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "ActiveSync.Host.Composition" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "ActiveSync.DotNetHost.Native.win-x64" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "ActiveSync.DotNetHost.Native.linux-x64" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.Host.Abstractions" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.Host.Composition" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.DotNetHost.Native.win-x64" -PackageVersion $Version
    Clear-GlobalNuGetPackageVersion -PackageId "NodalMerge.DotNetHost.Native.linux-x64" -PackageVersion $Version

    Write-Host "Done. Local packages available in: $resolvedOutput"
}
finally {
    Pop-Location
}
