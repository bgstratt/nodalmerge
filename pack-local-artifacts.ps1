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

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [scriptblock]$Command
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

function Get-RelativePathCompat {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BasePath,
        [Parameter(Mandatory = $true)]
        [string]$TargetPath
    )

    if ($null -ne [System.IO.Path].GetMethod("GetRelativePath", [Type[]]@([string], [string]))) {
        return [System.IO.Path]::GetRelativePath($BasePath, $TargetPath)
    }

    $baseFull = [System.IO.Path]::GetFullPath($BasePath)
    $targetFull = [System.IO.Path]::GetFullPath($TargetPath)

    $baseUriString = $baseFull
    if (-not $baseUriString.EndsWith([System.IO.Path]::DirectorySeparatorChar)) {
        $baseUriString += [System.IO.Path]::DirectorySeparatorChar
    }

    $baseUri = [System.Uri]::new($baseUriString)
    $targetUri = [System.Uri]::new($targetFull)
    $relativeUri = $baseUri.MakeRelativeUri($targetUri)

    return [System.Uri]::UnescapeDataString($relativeUri.ToString()).Replace('/', [System.IO.Path]::DirectorySeparatorChar)
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
$rustupFallback = Join-Path $env:USERPROFILE ".cargo\bin\rustup.exe"
$wasmPackFallback = Join-Path $env:USERPROFILE ".cargo\bin\wasm-pack.exe"
$requiresCargo = (-not $SkipCrates) -or (-not $SkipNuGet)
$requiresWasmBridge = -not $SkipNpm
$cargoPath = if ($requiresCargo -or $requiresWasmBridge) {
    Resolve-CommandPath -Name "cargo" -Fallbacks @($cargoFallback)
} else {
    $null
}
$dotnetPath = if ($SkipNuGet) { $null } else { Resolve-CommandPath -Name "dotnet" }
$npmPath = if ($SkipNpm) { $null } else { Resolve-CommandPath -Name "npm" }
$wasmPackPath = if ($SkipNpm) { $null } else { Resolve-CommandPath -Name "wasm-pack" -Fallbacks @($wasmPackFallback) }

if (($requiresCargo -or $requiresWasmBridge) -and -not $cargoPath) {
    throw "cargo executable not found. Install Rust or add cargo to PATH (`$HOME\.cargo\bin`)."
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

if ($requiresCargo -or $requiresWasmBridge) {
    $cargoBinDir = Split-Path -Parent $cargoPath
    if (-not [string]::IsNullOrWhiteSpace($cargoBinDir) -and ($env:Path -notlike "*$cargoBinDir*")) {
        $env:Path = "$cargoBinDir;$env:Path"
    }
}

if ($requiresWasmBridge) {
    $rustupPath = Resolve-CommandPath -Name "rustup" -Fallbacks @($rustupFallback)
    if (-not $rustupPath) {
        throw "rustup executable not found. wasm-pack needs rustup to install/check the wasm32-unknown-unknown target (`$HOME\.cargo\bin`)."
    }
    Write-Host "[npm] Ensuring wasm32-unknown-unknown target is installed ..."
    Invoke-Checked -Name "rustup target add wasm32-unknown-unknown" -Command {
        & $rustupPath target add wasm32-unknown-unknown
    }
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
        Push-Location (Join-Path $repoRoot "clients\bridge-wasm")
        try {
            Invoke-Checked -Name "wasm-pack build bridge" -Command {
                & $wasmPackPath build --target web --out-dir pkg --out-name nodalmerge_bridge
            }
            foreach ($requiredAsset in @("pkg\nodalmerge_bridge_bg.wasm", "pkg\nodalmerge_bridge.js", "pkg\package.json")) {
                $requiredPath = Join-Path (Get-Location) $requiredAsset
                if (-not (Test-Path $requiredPath)) {
                    throw "Missing expected wasm bridge build asset: $requiredPath"
                }
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing nodalmerge-bridge ..."
        Push-Location (Join-Path $repoRoot "clients\bridge-wasm\pkg")
        try {
            Invoke-Checked -Name "npm pack bridge/pkg" -Command { & $npmPath pack }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing nodalmerge-sdk-js ..."
        Push-Location (Join-Path $repoRoot "clients\sdk-js")
        try {
            Invoke-Checked -Name "npm pack sdk-js" -Command { & $npmPath pack }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing nodalmerge-bridge (primary wrapper) ..."
        Push-Location (Join-Path $repoRoot "compat\npm\nodalmerge-bridge")
        try {
            Invoke-Checked -Name "npm pack wrapper nodalmerge-bridge" -Command { & $npmPath pack }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        Write-Host "[npm] Packing nodalmerge-sdk-js (primary wrapper) ..."
        Push-Location (Join-Path $repoRoot "compat\npm\nodalmerge-sdk-js")
        try {
            Invoke-Checked -Name "npm pack wrapper nodalmerge-sdk-js" -Command { & $npmPath pack }
            Get-ChildItem -Path . -Filter "*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }

        # Wrapper packs are compatibility stubs. Re-stage the real WASM + SDK tarballs last.
        Write-Host "[npm] Staging canonical bridge/pkg and sdk-js tarballs ..."
        Push-Location (Join-Path $repoRoot "clients\bridge-wasm\pkg")
        try {
            Invoke-Checked -Name "npm pack bridge/pkg (canonical)" -Command { & $npmPath pack }
            Get-ChildItem -Path . -Filter "nodalmerge-bridge-*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }
        Push-Location (Join-Path $repoRoot "clients\sdk-js")
        try {
            Invoke-Checked -Name "npm pack sdk-js (canonical)" -Command { & $npmPath pack }
            Get-ChildItem -Path . -Filter "nodalmerge-sdk-js-*.tgz" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $npmOutput $_.Name) -Force
            }
        }
        finally {
            Pop-Location
        }
    }

    if (-not $SkipNuGet) {
        Write-Host "[nuget] Packing managed/native local packages ..."
        $packScript = Join-Path $repoRoot "hosts\dotnet\pack-local-nuget.ps1"
        $dotnetHostDir = Join-Path $repoRoot "nodalmerge-host"
        $nugetOutputRelative = Get-RelativePathCompat -BasePath $dotnetHostDir -TargetPath $nugetOutput
        Invoke-Checked -Name "pack-local-nuget.ps1" -Command {
            & $packScript -Version $Version -OutputDir $nugetOutputRelative
        }
    }

    if (-not $SkipCrates) {
        Write-Host "[crates] Packaging crate artifacts (.crate) ..."
        $crateDirs = @{
            "nodalmerge-core" = "core/crdt"
            "nodalmerge-gc" = "core/gc"
            "nodalmerge-host-core" = "engine/host-core"
            "nodalmerge-host-ffi" = "engine/host-ffi"
            "nodalmerge-host-axum" = "server/axum-embed"
            "nodalmerge-bridge" = "clients/bridge-wasm"
            "nodalmerge-server" = "server/server"
            "nodalmerge-jwt-bridge" = "server/jwt-bridge"
            "nodalmerge-s3-blobs" = "server/s3-blobs"
            "nodalmerge-runtime-local" = "peer/runtime-local"
            "nodalmerge-runtime-local-ffi" = "peer/runtime-local-ffi"
            "nodalmerge-headless" = "peer/headless"
            "nodalmerge-cli" = "peer/cli"
            "activesync-core" = "compat/rust/nodalmerge-core"
            "activesync-gc" = "compat/rust/nodalmerge-gc"
            "activesync-host-core" = "compat/rust/nodalmerge-host-core"
            "activesync-host-axum" = "compat/rust/nodalmerge-host-axum"
            "activesync-host-ffi" = "compat/rust/nodalmerge-host-ffi"
            "activesync-server" = "compat/rust/nodalmerge-server"
            "activesync-jwt-bridge" = "compat/rust/nodalmerge-jwt-bridge"
            "activesync-s3-blobs" = "compat/rust/nodalmerge-s3-blobs"
            "activesync-mongo-store" = "compat/rust/nodalmerge-mongo-store"
            "activesync-postgres-store" = "compat/rust/nodalmerge-postgres-store"
            "activesync-nodestore-conformance" = "compat/rust/nodalmerge-nodestore-conformance"
        }
        $crateIds = @(
            "nodalmerge-core",
            "nodalmerge-gc",
            "nodalmerge-host-core",
            "nodalmerge-host-ffi",
            "nodalmerge-host-axum",
            "nodalmerge-bridge",
            "nodalmerge-server",
            "nodalmerge-jwt-bridge",
            "nodalmerge-s3-blobs",
            "nodalmerge-runtime-local",
            "nodalmerge-runtime-local-ffi",
            "nodalmerge-headless",
            "nodalmerge-cli",
            "activesync-core",
            "activesync-gc",
            "activesync-host-core",
            "activesync-host-axum",
            "activesync-host-ffi",
            "activesync-server",
            "activesync-jwt-bridge",
            "activesync-s3-blobs",
            "activesync-mongo-store",
            "activesync-postgres-store",
            "activesync-nodestore-conformance"
        )

        if ($crateIds.Count -ne ($crateIds | Select-Object -Unique).Count) {
            throw "crateIds list contains duplicates; packaging order must be explicit and unique."
        }
        foreach ($crateId in $crateIds) {
            if (-not $crateDirs.ContainsKey($crateId)) {
                throw "Missing crate directory mapping for $crateId"
            }
        }

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
