[CmdletBinding()]
param(
    [string]$Version = "0.1.0-local",
    [string]$OutputDir = "../../artifacts/nuget-local"
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

    # Native runtime packages are packed here, *before* NodalMerge.DotNetHost below — that pack step
    # now needs to restore PackageReference entries pointing at these exact packages/version (see the
    # comment on that step), so they must already exist in $resolvedOutput by the time it runs.
    Write-Host "Packing native runtime packages to $resolvedOutput ..."
    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost.Native.win-x64" -Command {
        dotnet pack ./src/NodalMerge.DotNetHost.Native.win-x64/NodalMerge.DotNetHost.Native.win-x64.csproj -c Release -o $resolvedOutput /p:Version=$Version
    }

    $repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "../.."))
    $linuxLocalNative = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "../../target/release/libnodalmerge_runtime_local_ffi.so"))
    $linuxNative = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "../../target/release/libnodalmerge_host_ffi.so"))
    $isWindowsRuntime = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)

    if ($isWindowsRuntime) {
        $wslPath = Resolve-CommandPath -Name "wsl"
        if ($wslPath) {
            $repoRootWsl = (& wsl.exe -- wslpath -a ($repoRoot -replace '\\', '/')).Trim()
            Write-Host "Building linux-x64 native runtime via WSL (release)..."
            # WSL cargo must NOT share target/ with Windows cargo: the two rustc versions
            # can differ, and WSL-written dep artifacts make subsequent Windows builds fail
            # with E0514 (crate compiled by an incompatible version of rustc).
            Invoke-Checked -Name "wsl cargo build host ffi runtime (nodalmerge-host-ffi)" -Command {
                & wsl.exe -- bash -lc "cd '$repoRootWsl' && CARGO_TARGET_DIR=target-wsl cargo build -p nodalmerge-host-ffi --release"
            }
            Invoke-Checked -Name "wsl cargo build peer-local ffi runtime (nodalmerge-runtime-local-ffi)" -Command {
                & wsl.exe -- bash -lc "cd '$repoRootWsl' && CARGO_TARGET_DIR=target-wsl cargo build -p nodalmerge-runtime-local-ffi --release"
            }
            # The Native.linux-x64 csproj expects the .so files under target/release.
            $windowsReleaseDir = Join-Path $repoRoot "target\release"
            New-Item -ItemType Directory -Force -Path $windowsReleaseDir | Out-Null
            foreach ($soName in @("libnodalmerge_host_ffi.so", "libnodalmerge_runtime_local_ffi.so")) {
                $wslBuilt = Join-Path $repoRoot "target-wsl\release\$soName"
                if (Test-Path -LiteralPath $wslBuilt) {
                    Copy-Item -LiteralPath $wslBuilt -Destination (Join-Path $windowsReleaseDir $soName) -Force
                }
            }
        }
        else {
            Write-Warning "wsl executable not found; cannot build real linux-x64 native artifacts from Windows."
        }
    }

    if (-not (Test-Path -LiteralPath $linuxLocalNative)) {
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

    if (-not (Test-Path -LiteralPath $linuxNative)) {
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

    Invoke-Checked -Name "dotnet pack NodalMerge.DotNetHost" -Command {
        # NodalMergeUseNuGetPackages=true is required here: it's what makes NodalMerge.DotNetHost.csproj
        # reference NodalMerge.DotNetHost.Native.win-x64/linux-x64 as PackageReferences (its default,
        # project-reference-only branch never touches the native packages at all), so those flowed into
        # this package's own nuspec <dependencies> instead of being silently dropped — the bug this fixes.
        # NodalMergePackageVersion must match $Version so those PackageReferences resolve to the exact
        # native packages just packed above, not the csproj's unrelated "0.1.0-local" default.
        # RestoreAdditionalProjectSources appends $resolvedOutput to the default feeds (nuget.org)
        # so the freshly packed Abstractions/Composition/Native packages resolve from disk while
        # everything else restores normally. (Neither `dotnet pack --source` nor a URL inside
        # /p:RestoreSources works here: both get misparsed as relative local paths on current SDKs
        # whenever restore actually has to enumerate the sources, i.e. on a cold package cache.)
        dotnet pack ./src/NodalMerge.DotNetHost/NodalMerge.DotNetHost.csproj -c Release -o $resolvedOutput `
            /p:Version=$Version /p:NodalMergeUseNuGetPackages=true /p:NodalMergePackageVersion=$Version `
            "/p:RestoreAdditionalProjectSources=$resolvedOutput"
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

