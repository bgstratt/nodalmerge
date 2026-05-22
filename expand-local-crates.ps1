[CmdletBinding()]
param(
    [string]$CrateDir = "./artifacts/package-local/crates",
    [string]$OutputDir = "./artifacts/package-local/crates/unpacked"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath($PSScriptRoot)
$crateDirFull = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $CrateDir))
$outputDirFull = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))

if (-not (Test-Path $crateDirFull)) {
    throw "Crate directory not found: $crateDirFull"
}

$tarPath = (Get-Command tar -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue)
if (-not $tarPath) {
    throw "tar command not found. Install BSD tar/GNU tar or run from an environment where tar is available."
}

New-Item -ItemType Directory -Path $outputDirFull -Force | Out-Null

$crateFiles = Get-ChildItem -Path $crateDirFull -Filter "*.crate"
if ($crateFiles.Count -eq 0) {
    throw "No .crate files found under $crateDirFull"
}

foreach ($crate in $crateFiles) {
    $name = [System.IO.Path]::GetFileNameWithoutExtension($crate.Name)
    $target = Join-Path $outputDirFull $name

    if (Test-Path $target) {
        Remove-Item -Path $target -Recurse -Force
    }

    New-Item -ItemType Directory -Path $target -Force | Out-Null

    Push-Location $target
    try {
        & $tarPath -xf $crate.FullName
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to extract $($crate.Name)"
        }
    }
    finally {
        Pop-Location
    }
}

Write-Host "Extracted crates to: $outputDirFull"
