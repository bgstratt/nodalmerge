param(
    [Parameter(Mandatory = $true)]
    [string]$ManifestPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $ManifestPath)) {
    throw "Manifest path not found: $ManifestPath"
}

$raw = Get-Content -LiteralPath $ManifestPath -Raw
$manifest = $raw | ConvertFrom-Json

function Assert-Field([object]$Value, [string]$Name) {
    if ($null -eq $Value -or [string]::IsNullOrWhiteSpace([string]$Value)) {
        throw "Missing required field: $Name"
    }
}

Assert-Field $manifest.manifest_type "manifest_type"
Assert-Field $manifest.schema_version "schema_version"
Assert-Field $manifest.generated_at_utc "generated_at_utc"
Assert-Field $manifest.core_release.version "core_release.version"
Assert-Field $manifest.core_release.git_tag "core_release.git_tag"
Assert-Field $manifest.core_release.git_sha "core_release.git_sha"

if ($manifest.manifest_type -ne "core_release_handoff") {
    throw "manifest_type must be 'core_release_handoff'"
}

if ($manifest.schema_version -ne "1") {
    throw "schema_version must be '1'"
}

$sha = [string]$manifest.core_release.git_sha
if ($sha -match '<40-lowercase-hex-core-commit-sha>') {
    throw "core_release.git_sha placeholder is not allowed in concrete manifests"
}
if ($sha -notmatch '^[0-9a-f]{40}$') {
    throw "core_release.git_sha must be 40 lowercase hex characters"
}

foreach ($group in @("crates", "nuget", "npm", "native_runtime")) {
    $entries = $manifest.artifacts.$group
    if ($null -eq $entries -or $entries.Count -eq 0) {
        throw "artifacts.$group must contain at least one entry"
    }
    foreach ($entry in $entries) {
        Assert-Field $entry.version "artifacts.$group[].version"
    }
}

Write-Host "Core release manifest validation passed: $ManifestPath"
