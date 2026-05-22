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

        Write-Host "[npm] Packing activesync-bridge ..."
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

        Write-Host "[npm] Packing activesync-sdk-js ..."
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
    }

    if (-not $SkipNuGet) {
        Write-Host "[nuget] Packing managed/native local packages ..."
        $packScript = Join-Path $repoRoot "dotnet-host\pack-local-nuget.ps1"
        $dotnetHostDir = Join-Path $repoRoot "dotnet-host"
        $nugetOutputRelative = [System.IO.Path]::GetRelativePath($dotnetHostDir, $nugetOutput)
        & $packScript -Version $Version -OutputDir $nugetOutputRelative
        if ($LASTEXITCODE -ne 0) {
            throw "pack-local-nuget.ps1 failed with exit code $LASTEXITCODE"
        }
    }

    if (-not $SkipCrates) {
        Write-Host "[crates] Packaging crate artifacts (.crate) ..."
        $crateIds = @(
            "activesync-core",
            "activesync-host-core",
            "activesync-host-ffi",
            "activesync-host-axum",
            "activesync-bridge"
        )

        $crateFailures = New-Object System.Collections.Generic.List[string]

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
                    continue
                }

                throw $msg
            }
        }

        $cargoPackageDir = Join-Path $repoRoot "target\package"
        if (Test-Path $cargoPackageDir) {
            Get-ChildItem -Path $cargoPackageDir -Filter "*.crate" | ForEach-Object {
                Copy-Item -Path $_.FullName -Destination (Join-Path $crateOutput $_.Name) -Force
            }
        }

        if ($crateFailures.Count -gt 0) {
            Write-Warning "Some crate packages were not produced. See warnings above for details."
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
