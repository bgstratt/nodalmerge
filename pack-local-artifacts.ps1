[CmdletBinding()]
param(
    [string]$Version = "0.1.0-local",
    [string]$OutputRoot = "./artifacts/package-local",
    [switch]$SkipNuGet,
    [switch]$SkipNpm,
    [switch]$SkipCrates,
    [switch]$AllowDirtyCrates,
    [switch]$AllowCrateDependencyFailures
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-CommandPath {
    param(
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

function Get-CrateVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CargoTomlPath
    )

    if (-not (Test-Path $CargoTomlPath)) {
        throw "Cargo.toml not found at $CargoTomlPath"
    }

    $content = Get-Content -Path $CargoTomlPath -Raw
    $match = [regex]::Match($content, '(?m)^version\s*=\s*"([^"]+)"\s*$')
    if (-not $match.Success) {
        throw "Could not parse crate version from $CargoTomlPath"
    }

    return $match.Groups[1].Value
}

function New-LocalCrateArchive {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CrateId,
        [Parameter(Mandatory = $true)]
        [string]$CrateDir,
        [Parameter(Mandatory = $true)]
        [string]$CrateOutputDir,
        [Parameter(Mandatory = $true)]
        [string]$TarPath
    )

    $cargoToml = Join-Path $CrateDir "Cargo.toml"
    $version = Get-CrateVersion -CargoTomlPath $cargoToml
    $archiveName = "$CrateId-$version-local.crate"
    $archivePath = Join-Path $CrateOutputDir $archiveName

    if (Test-Path $archivePath) {
        Remove-Item -Path $archivePath -Force
    }

    & $TarPath -czf $archivePath -C $CrateDir .
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to create fallback local crate archive for $CrateId"
    }

    return $archivePath
}

$repoRoot = [System.IO.Path]::GetFullPath($PSScriptRoot)
$outputRootFull = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputRoot))

$npmOutput = Join-Path $outputRootFull "npm"
$nugetOutput = Join-Path $outputRootFull "nuget"
$crateOutput = Join-Path $outputRootFull "crates"

New-Item -ItemType Directory -Path $outputRootFull -Force | Out-Null
if (-not $SkipNpm) { New-Item -ItemType Directory -Path $npmOutput -Force | Out-Null }
if (-not $SkipNuGet) { New-Item -ItemType Directory -Path $nugetOutput -Force | Out-Null }
if (-not $SkipCrates) { New-Item -ItemType Directory -Path $crateOutput -Force | Out-Null }

$cargoFallback = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
$cargoPath = if ($SkipCrates) { $null } else { Resolve-CommandPath -Name "cargo" -Fallbacks @($cargoFallback) }
$dotnetPath = if ($SkipNuGet) { $null } else { Resolve-CommandPath -Name "dotnet" }
$npmPath = if ($SkipNpm) { $null } else { Resolve-CommandPath -Name "npm" }
$wasmPackPath = if ($SkipNpm) { $null } else { Resolve-CommandPath -Name "wasm-pack" }

if (-not $SkipCrates -and -not $cargoPath) {
    throw "cargo executable not found. Install Rust or add cargo to PATH."
}
if (-not $SkipNuGet -and -not $dotnetPath) {
    throw "dotnet executable not found in PATH."
}
if (-not $SkipNpm -and -not $npmPath) {
    throw "npm executable not found in PATH."
}
if (-not $SkipNpm -and -not $wasmPackPath) {
    throw "wasm-pack executable not found. Install wasm-pack to build bridge/pkg assets."
}

$tarPath = if ($SkipCrates) { $null } else { Resolve-CommandPath -Name "tar" }
if (-not $SkipCrates -and -not $tarPath) {
    throw "tar executable not found. Install tar to enable fallback local crate archiving."
}

Write-Host "Packaging output root: $outputRootFull"

Push-Location $repoRoot
try {
    if (-not $SkipNpm) {
        Write-Host "[npm] Building wasm bridge package assets ..."
        Push-Location (Join-Path $repoRoot "bridge")
        try {
            wasm-pack build --target web
            if ($LASTEXITCODE -ne 0) {
                throw "wasm-pack build failed with exit code $LASTEXITCODE"
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing activesync-bridge (legacy compatibility) ..."
        Push-Location (Join-Path $repoRoot "bridge\pkg")
        try {
            npm pack
            if ($LASTEXITCODE -ne 0) {
                throw "npm pack (bridge/pkg) failed with exit code $LASTEXITCODE"
            }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing activesync-sdk-js (legacy compatibility) ..."
        Push-Location (Join-Path $repoRoot "sdk-js")
        try {
            npm pack
            if ($LASTEXITCODE -ne 0) {
                throw "npm pack (sdk-js) failed with exit code $LASTEXITCODE"
            }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing nodalmerge-bridge (primary wrapper) ..."
        Push-Location (Join-Path $repoRoot "wrappers\npm\nodalmerge-bridge")
        try {
            npm pack
            if ($LASTEXITCODE -ne 0) {
                throw "npm pack (wrappers/npm/nodalmerge-bridge) failed with exit code $LASTEXITCODE"
            }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing nodalmerge-sdk-js (primary wrapper) ..."
        Push-Location (Join-Path $repoRoot "wrappers\npm\nodalmerge-sdk-js")
        try {
            npm pack
            if ($LASTEXITCODE -ne 0) {
                throw "npm pack (wrappers/npm/nodalmerge-sdk-js) failed with exit code $LASTEXITCODE"
            }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }
    }

    if (-not $SkipNuGet) {
        Write-Host "[nuget] Packing managed/native local packages ..."
        $packScript = Join-Path $repoRoot "nodalmerge-host\pack-local-nuget.ps1"
        $dotnetHostDir = Join-Path $repoRoot "nodalmerge-host"
        $nugetOutputRelative = [System.IO.Path]::GetRelativePath($dotnetHostDir, $nugetOutput)
        & $packScript -Version $Version -OutputDir $nugetOutputRelative
        if ($LASTEXITCODE -ne 0) {
            throw "pack-local-nuget.ps1 failed with exit code $LASTEXITCODE"
        }
    }

    if (-not $SkipCrates) {
        Write-Host "[crates] Packaging crate artifacts (.crate) ..."
        $crateDirs = @{
            "nodalmerge-core" = "core"
            "nodalmerge-host-core" = "host-core"
            "nodalmerge-host-ffi" = "host-ffi"
            "nodalmerge-host-axum" = "host-axum"
            "activesync-bridge" = "bridge"
            "nodalmerge-core" = "wrappers/nodalmerge-core"
            "nodalmerge-gc" = "wrappers/nodalmerge-gc"
            "nodalmerge-host-core" = "wrappers/nodalmerge-host-core"
            "nodalmerge-host-axum" = "wrappers/nodalmerge-host-axum"
            "nodalmerge-host-ffi" = "wrappers/nodalmerge-host-ffi"
            "nodalmerge-server" = "wrappers/nodalmerge-server"
            "nodalmerge-jwt-bridge" = "wrappers/nodalmerge-jwt-bridge"
            "nodalmerge-s3-blobs" = "wrappers/nodalmerge-s3-blobs"
            "nodalmerge-mongo-store" = "wrappers/nodalmerge-mongo-store"
            "nodalmerge-postgres-store" = "wrappers/nodalmerge-postgres-store"
            "nodalmerge-nodestore-conformance" = "wrappers/nodalmerge-nodestore-conformance"
        }
        $crateIds = @(
            "nodalmerge-core",
            "nodalmerge-host-core",
            "nodalmerge-host-ffi",
            "nodalmerge-host-axum",
            "activesync-bridge",
            "nodalmerge-core",
            "nodalmerge-gc",
            "nodalmerge-host-core",
            "nodalmerge-host-axum",
            "nodalmerge-host-ffi",
            "nodalmerge-server",
            "nodalmerge-jwt-bridge",
            "nodalmerge-s3-blobs",
            "nodalmerge-mongo-store",
            "nodalmerge-postgres-store",
            "nodalmerge-nodestore-conformance"
        )

        # Ensure crate output from this run is fresh and not contaminated by stale files.
        if (Test-Path $crateOutput) {
            Get-ChildItem -Path $crateOutput -Filter "*.crate" -File -ErrorAction SilentlyContinue | Remove-Item -Force
        }

        $cargoPackageDir = Join-Path $repoRoot "target\package"
        if (Test-Path $cargoPackageDir) {
            Get-ChildItem -Path $cargoPackageDir -Filter "*.crate" -File -ErrorAction SilentlyContinue | Remove-Item -Force
        }

        $crateFailures = New-Object System.Collections.Generic.List[string]
        $crateFallbacks = New-Object System.Collections.Generic.List[string]

        foreach ($crateId in $crateIds) {
            $cargoArgList = @("package", "--package", $crateId, "--no-verify")
            if ($AllowDirtyCrates) {
                $cargoArgList += "--allow-dirty"
            }

            & $cargoPath @cargoArgList
            if ($LASTEXITCODE -ne 0) {
                $msg = "cargo package ($crateId) failed with exit code $LASTEXITCODE"
                if ($AllowCrateDependencyFailures) {
                    Write-Warning $msg
                    $crateFailures.Add($msg)

                    $crateRelDir = $crateDirs[$crateId]
                    if (-not $crateRelDir) {
                        throw "No crate directory mapping configured for $crateId"
                    }

                    $crateDir = Join-Path $repoRoot $crateRelDir
                    $fallbackArchive = New-LocalCrateArchive -CrateId $crateId -CrateDir $crateDir -CrateOutputDir $crateOutput -TarPath $tarPath
                    Write-Warning "Created fallback local crate archive: $fallbackArchive"
                    $crateFallbacks.Add($fallbackArchive)
                    continue
                }

                throw $msg
            }
        }

        if (Test-Path $cargoPackageDir) {
            Get-ChildItem -Path $cargoPackageDir -Filter "*.crate" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $crateOutput $_.Name) -Force
            }
        }

        if ($crateFailures.Count -gt 0) {
            Write-Warning "Some crate packages were not produced. See warnings above for details."
        }
        if ($crateFallbacks.Count -gt 0) {
            Write-Warning "Fallback local crate archives were created for failed cargo-package crates."
        }
    }
}
finally {
    Pop-Location
}

Write-Host "Done. Staged artifacts:"
if (-not $SkipNpm) { Write-Host "  npm:    $npmOutput" }
if (-not $SkipNuGet) { Write-Host "  nuget:  $nugetOutput" }
if (-not $SkipCrates) { Write-Host "  crates: $crateOutput" }
