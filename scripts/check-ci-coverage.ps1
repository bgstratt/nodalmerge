# check-ci-coverage.ps1 — CI coverage meta-check (plan: nodalmerge-studio/plans/dotnet-host-readiness.md C1+C3,
# executing the deferred Phase 8.1/8.3 of nodalmerge-studio/plans/blob-cas-remediation.md).
#
# Asserts that every test suite in the tree is actually executed by some workflow under
# .github/workflows/, so that a test file existing but running in NO workflow (the audit's
# failure mode — steps that do not exist at all) becomes a hard CI failure instead of a
# silent gap.
#
# What counts as an inventory item:
#   - Rust: every `tests/*.rs` file (top level only — cargo integration-test targets) of every
#     crate in the repo (git-tracked; scope is ALL crates: server/**, peer/**, engine/**,
#     core/**, and anything added later — the scan is repo-wide on purpose) that contains at
#     least one `#[test]` / `#[tokio::test]` / `#[rstest]` / `#[test_case]` attribute.
#     Item id: rust:<repo-relative-path>, e.g. rust:server/server/tests/persistence.rs
#   - .NET: every class bearing [Fact]/[Theory] methods in any git-tracked *.csproj that
#     references xunit. Item id: dotnet:<Namespace>.<ClassName>,
#     e.g. dotnet:NodalMerge.DotNetHost.Tests.SpecAuthVectorsTests
#
# What counts as coverage (CONSERVATIVE — when unsure, a step covers NOTHING):
#   - `cargo test -p <pkg> --test <name>`      -> covers exactly that integration-test target
#                                                 (even when a positional test-name filter is
#                                                 also present: the target itself is named).
#   - `cargo test -p <pkg>` unfiltered         -> covers ALL of that package's tests/*.rs.
#     (no --test/--lib/--bin/..., no positional test-name filter, no unknown flags)
#   - `cargo test` / `cargo test --workspace` unfiltered -> covers all packages.
#   - `cargo test -p <pkg> --lib [module::]`   -> covers NO tests/*.rs file, ever. This exact
#     shape (filtered lib-test steps mistaken for package coverage) has hidden gaps repeatedly;
#     it is deliberately treated as zero integration-test coverage.
#   - `cargo test -p <pkg> some_test_name`     -> positional filter, NOT whole-package coverage;
#     covers nothing at the file level.
#   - `dotnet test <proj.csproj>` (no --filter) -> covers all test classes in that project.
#   - `dotnet test <proj.csproj> --filter "<expr>"` -> expr split on `|`; each
#     `FullyQualifiedName~X` clause covers classes whose namespace-qualified name contains X;
#     `FullyQualifiedName=X` covers the class it names (exactly or as method prefix). Any
#     expression containing `&` or `(` is too complex -> covers nothing (warning).
#
# Baseline (.github/ci-coverage-baseline.txt): the explicit, reviewed allow-list of suites that
# are known-uncovered (or deliberately excluded). Format: one item id per line, optional
# `# reason` comment after it. The check fails on:
#   - any discovered item that is neither covered nor baselined            (silent gap)
#   - any baseline entry that no longer exists in the tree                 (baseline rot)
#   - any baseline entry that is now covered                               (ratchet: burn it down)
#     ...EXCEPT entries tagged `PARITY:` that belong to a parity pair which is not yet fully
#     covered — those must leave the baseline together with their twin (see below).
#
# Parity pairs (C3 / Phase 8.3): cross-runtime contracts that must be pinned from BOTH runtimes.
# Declared in $ParityPairs below. A pair passes only if
#   (a) every member is covered by a workflow, OR
#   (b) every member (including any already-covered side!) is baselined with a `PARITY:` tag —
#       the "known one-sided gap, top C2 priority" state.
# Anything else — exactly one side covered, one side baselined without the other, a PARITY tag
# on an entry that is in no declared pair, a pair member that doesn't exist — is a hard failure.
# This makes "both sides leave the baseline together" mechanically enforced.
#
# Run locally:   pwsh -NoProfile -File scripts/check-ci-coverage.ps1
# Show inventory: add -ShowCovered
# Sanity/test overrides: -WorkflowsDir <dir>  -BaselineFile <file>

[CmdletBinding()]
param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$WorkflowsDir,
    [string]$BaselineFile,
    [switch]$ShowCovered
)

$ErrorActionPreference = 'Stop'
if (-not $WorkflowsDir) { $WorkflowsDir = Join-Path $RepoRoot '.github/workflows' }
if (-not $BaselineFile) { $BaselineFile = Join-Path $RepoRoot '.github/ci-coverage-baseline.txt' }

# ---------------------------------------------------------------------------
# Parity pairs (C3). Ids must match inventory ids exactly. Verified against the
# tree 2026-07-17:
#   spec_auth_vectors.rs / branch_fork_vectors.rs — the server-side twins named
#   by the audit (core/crdt has its own branch_fork_vectors.rs + spec_auth_replay_vectors.rs,
#   covered wholesale by `cargo test -p nodalmerge-core`; the pair pins the server ones).
#   blob-layout v1 AND v3 vectors are both consumed on the .NET side by
#   BlobLayoutParityTests (EncodingVectorsV3/NameVectorsV3 in that class), so it is the
#   .NET member of both blob-layout pairs.
# ---------------------------------------------------------------------------
$ParityPairs = @(
    @{ Name = 'spec-auth'
       Members = @('rust:server/server/tests/spec_auth_vectors.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.SpecAuthVectorsTests') }
    @{ Name = 'branch-fork'
       Members = @('rust:server/server/tests/branch_fork_vectors.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.BranchForkVectorsTests') }
    @{ Name = 'query-materialization'
       Members = @('rust:server/server/tests/query_materialization_vectors.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.QueryMaterializationVectorsTests') }
    @{ Name = 'zstd-interop'
       Members = @('rust:server/server/tests/zstd_interop.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.ZstdInteropTests') }
    @{ Name = 'blob-layout-v1'
       Members = @('rust:server/server/tests/blob_layout_vectors.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.BlobLayoutParityTests') }
    @{ Name = 'blob-layout-v3'
       Members = @('rust:server/server/tests/blob_layout_vectors_v3.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.BlobLayoutParityTests') }
    @{ Name = 'capcomp'
       Members = @('rust:server/capability-profile/tests/capcomp_vectors.rs',
                   'dotnet:NodalMerge.DotNetHost.Tests.CapcompParityTests') }
)

$failures = New-Object System.Collections.Generic.List[string]
$warnings = New-Object System.Collections.Generic.List[string]

# ---------------------------------------------------------------------------
# 1. Inventory — from git-tracked files (untracked files can't run in CI either,
#    and this automatically skips target/, node_modules/, bin/, obj/).
# ---------------------------------------------------------------------------
$trackedFiles = & git -C $RepoRoot ls-files
if ($LASTEXITCODE -ne 0) { throw "git ls-files failed in $RepoRoot" }

function Read-RepoFile([string]$rel) {
    Get-Content -LiteralPath (Join-Path $RepoRoot $rel) -Raw
}

# --- Rust integration-test targets ---
$cargoTomls = $trackedFiles | Where-Object { $_ -match '(^|/)Cargo\.toml$' }
$pkgByDir = @{}   # crate dir (repo-relative, '' for root) -> package name
foreach ($toml in $cargoTomls) {
    $content = Read-RepoFile $toml
    $pkgMatch = [regex]::Match($content, '(?ms)^\[package\].*?^\s*name\s*=\s*"([^"]+)"')
    if ($pkgMatch.Success) {
        $dir = if ($toml -eq 'Cargo.toml') { '' } else { $toml -replace '/Cargo\.toml$', '' }
        $pkgByDir[$dir] = $pkgMatch.Groups[1].Value
    }
}

$testAttrRe = '#\[\s*(?:tokio::test|test|rstest|test_case)'
$rustItems = @{}   # id -> @{ Package; Target; Path }
foreach ($dir in $pkgByDir.Keys) {
    $testsPrefix = if ($dir -eq '') { 'tests/' } else { "$dir/tests/" }
    $testFiles = $trackedFiles | Where-Object {
        $_.StartsWith($testsPrefix) -and $_ -match '\.rs$' -and ($_.Substring($testsPrefix.Length) -notmatch '/')
    }
    foreach ($tf in $testFiles) {
        $content = Read-RepoFile $tf
        if ($content -notmatch $testAttrRe) { continue }  # helper/module file, not a test target
        $target = [System.IO.Path]::GetFileNameWithoutExtension($tf)
        $rustItems["rust:$tf"] = @{ Package = $pkgByDir[$dir]; Target = $target; Path = $tf }
    }
}

# --- .NET test classes ---
$csprojs = $trackedFiles | Where-Object { $_ -match '\.csproj$' }
$testProjects = @()   # repo-relative csproj paths that reference xunit
foreach ($proj in $csprojs) {
    $content = Read-RepoFile $proj
    if ($content -match '(?i)xunit') { $testProjects += $proj }
}

$dotnetItems = @{}    # id -> @{ Project; Fqcn; ClassName; File }
foreach ($proj in $testProjects) {
    $projDir = $proj -replace '/[^/]+$', ''
    $csFiles = $trackedFiles | Where-Object {
        $_.StartsWith("$projDir/") -and $_ -match '\.cs$' -and $_ -notmatch '(^|/)(bin|obj)/'
    }
    foreach ($cs in $csFiles) {
        $content = Read-RepoFile $cs
        if ($content -notmatch '\[\s*(Fact|Theory)') { continue }
        $nsMatch = [regex]::Match($content, '(?m)^\s*namespace\s+([\w.]+)')
        $ns = if ($nsMatch.Success) { $nsMatch.Groups[1].Value } else { '' }
        # Attribute each [Fact]/[Theory] to the nearest preceding TOP-LEVEL class
        # declaration. Nested helper classes are indented deeper; xUnit reports
        # nested-class test methods under Outer+Inner anyway, so the outer class
        # name is the one workflow --filter clauses match. "Top-level" is detected
        # pragmatically as the minimum class-decl indentation in the file.
        $classDecls = [regex]::Matches($content,
            '(?m)^([ \t]*)(?:(?:public|internal|private|protected|sealed|static|abstract|partial|file)\s+)*class\s+([A-Za-z_]\w*)')
        if ($classDecls.Count -eq 0) { continue }
        $minIndent = ($classDecls | ForEach-Object { $_.Groups[1].Value.Length } | Measure-Object -Minimum).Minimum
        $topLevelDecls = @($classDecls | Where-Object { $_.Groups[1].Value.Length -eq $minIndent })
        $attrs = [regex]::Matches($content, '\[\s*(Fact|Theory)\b')
        $testClassNames = New-Object System.Collections.Generic.HashSet[string]
        foreach ($attr in $attrs) {
            $owner = $null
            foreach ($cd in $topLevelDecls) {
                if ($cd.Index -lt $attr.Index) { $owner = $cd } else { break }
            }
            if ($null -ne $owner) { [void]$testClassNames.Add($owner.Groups[2].Value) }
        }
        foreach ($cn in $testClassNames) {
            $fqcn = if ($ns) { "$ns.$cn" } else { $cn }
            $dotnetItems["dotnet:$fqcn"] = @{ Project = $proj; Fqcn = $fqcn; ClassName = $cn; File = $cs }
        }
    }
}

# ---------------------------------------------------------------------------
# 2. Workflow parsing — extract run: commands (pragmatic, line-based; comments
#    and name:/prose lines are never treated as commands).
# ---------------------------------------------------------------------------
function Get-RunCommands([string[]]$lines) {
    $cmds = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match '^\s*#') { continue }
        $m = [regex]::Match($line, '^(\s*)(?:-\s+)?run:\s*(.*)$')
        if (-not $m.Success) { continue }
        $indent = $m.Groups[1].Value.Length
        $rest = $m.Groups[2].Value.Trim()
        if ($rest -match '^[|>][+\-0-9]*\s*$') {
            $block = @()
            $j = $i + 1
            while ($j -lt $lines.Count) {
                $l = $lines[$j]
                if ($l -match '^\s*$') { $block += ''; $j++; continue }
                $li = ([regex]::Match($l, '^\s*')).Value.Length
                if ($li -le $indent) { break }
                $block += $l
                $j++
            }
            $cmds += , ($block -join "`n")
            $i = $j - 1
        }
        elseif ($rest.Length -gt 0) {
            if ($rest -match '^"(.*)"$') { $rest = $Matches[1] }
            elseif ($rest -match "^'(.*)'$") { $rest = $Matches[1] }
            $cmds += $rest
        }
    }
    return $cmds
}

# Coverage accumulators
$fullyCoveredRustPkgs   = New-Object System.Collections.Generic.HashSet[string]
$coverAllRustPkgs       = $false   # bare/workspace-wide unfiltered `cargo test`
$coveredRustTargets     = New-Object System.Collections.Generic.HashSet[string]  # "pkg|target"
$fullyCoveredProjects   = New-Object System.Collections.Generic.HashSet[string]  # normalized csproj rel path
$dotnetFilterClauses    = @()      # @{ Project; Op; Value; Source }

$cargoNoArgFlags = @(
    '--workspace', '--all', '--release', '--no-default-features', '--all-features',
    '--locked', '--offline', '--frozen', '--quiet', '-q', '--verbose', '-v', '-vv',
    '--no-fail-fast', '--all-targets', '--doc', '--no-run', '--ignore-rust-version'
)
$cargoArgFlags = @(
    '--features', '-F', '--manifest-path', '--profile', '--jobs', '-j', '--target',
    '--target-dir', '--exclude', '--color', '--config', '-Z'
)

function Resolve-ProjectRel([string]$token) {
    # Normalize a csproj path token from a workflow command to a repo-relative
    # forward-slash path (workflow cwds are the repo root in this repo's workflows).
    return ($token -replace '\\', '/').TrimStart('./')
}

$workflowFiles = Get-ChildItem -LiteralPath $WorkflowsDir -File |
    Where-Object { $_.Name -match '\.(yml|yaml)$' }

foreach ($wf in $workflowFiles) {
    $lines = Get-Content -LiteralPath $wf.FullName
    foreach ($cmd in (Get-RunCommands $lines)) {
        # Join backslash line-continuations, then split into shell fragments.
        $joined = $cmd -replace '\\\s*\r?\n\s*', ' '
        $fragments = $joined -split '(\r?\n|&&|;)' | Where-Object { $_ -notmatch '^(\r?\n|&&|;)$' }
        foreach ($frag in $fragments) {
            $frag = $frag.Trim()
            if ($frag -match '^\s*#') { continue }

            # ----- cargo test -----
            if ($frag -match '(?<![\w-])cargo\s+test\b') {
                $tail = ($frag -split 'cargo\s+test\s*', 2)[1]
                if ($null -eq $tail) { $tail = '' }
                $tokens = @($tail -split '\s+' | Where-Object { $_ -ne '' })
                $pkg = $null; $testTargets = @(); $libOrOtherTarget = $false
                $positionalFilter = $false; $unknownFlag = $false; $workspaceWide = $false
                for ($t = 0; $t -lt $tokens.Count; $t++) {
                    $tok = $tokens[$t]
                    if ($tok -eq '--') { break }
                    elseif ($tok -in @('-p', '--package')) { $t++; if ($t -lt $tokens.Count) { $pkg = $tokens[$t] } }
                    elseif ($tok -eq '--test') { $t++; if ($t -lt $tokens.Count) { $testTargets += $tokens[$t] } }
                    elseif ($tok -in @('--lib', '--bins', '--examples', '--benches')) { $libOrOtherTarget = $true }
                    elseif ($tok -in @('--bin', '--example', '--bench')) { $libOrOtherTarget = $true; $t++ }
                    elseif ($tok -in @('--workspace', '--all')) { $workspaceWide = $true }
                    elseif ($tok -in $cargoNoArgFlags) { }
                    elseif ($tok -in $cargoArgFlags) { $t++ }
                    elseif ($tok.StartsWith('-')) { $unknownFlag = $true }
                    else { $positionalFilter = $true }
                }
                if ($testTargets.Count -gt 0) {
                    foreach ($tt in $testTargets) {
                        if ($pkg) { [void]$coveredRustTargets.Add("$pkg|$tt") }
                        else {
                            # --test without -p: covers that target name in any package (rare; be
                            # generous only about the name itself, which is still explicit).
                            [void]$coveredRustTargets.Add("*|$tt")
                        }
                    }
                }
                elseif ($libOrOtherTarget) {
                    # --lib / --bin steps NEVER cover tests/*.rs. (The 13-times-bitten shape.)
                }
                elseif ($positionalFilter -or $unknownFlag) {
                    # Test-name-filtered or unparsable -> conservative: covers nothing file-level.
                    if ($unknownFlag) { $warnings.Add("unrecognized cargo test flag in '$frag' ($($wf.Name)) — treated as covering nothing") }
                }
                elseif ($pkg) { [void]$fullyCoveredRustPkgs.Add($pkg) }
                elseif ($workspaceWide -or $tokens.Count -eq 0 -or -not $positionalFilter) { $coverAllRustPkgs = $true }
            }

            # ----- dotnet test -----
            if ($frag -match '(?<![\w-])dotnet\s+test\b') {
                $tail = ($frag -split 'dotnet\s+test\s*', 2)[1]
                if ($null -eq $tail) { $tail = '' }
                $projTokenMatch = [regex]::Match($tail, '(\S+\.csproj)')
                $filterMatch = [regex]::Match($tail, '--filter\s+(?:"([^"]+)"|''([^'']+)''|(\S+))')
                if (-not $projTokenMatch.Success) {
                    if ($tail -match '\.sln') { $warnings.Add("dotnet test on a solution in '$frag' ($($wf.Name)) — not supported, treated as covering nothing") }
                    else { $warnings.Add("dotnet test without a csproj in '$frag' ($($wf.Name)) — treated as covering nothing") }
                    continue
                }
                $projRel = Resolve-ProjectRel $projTokenMatch.Groups[1].Value
                if (-not $filterMatch.Success) {
                    [void]$fullyCoveredProjects.Add($projRel)
                }
                else {
                    $expr = ($filterMatch.Groups[1].Value + $filterMatch.Groups[2].Value + $filterMatch.Groups[3].Value)
                    if ($expr -match '[&()]') {
                        $warnings.Add("complex --filter expression '$expr' ($($wf.Name)) — treated as covering nothing")
                        continue
                    }
                    foreach ($clause in ($expr -split '\|')) {
                        $cm = [regex]::Match($clause.Trim(), '^FullyQualifiedName\s*(~|=)\s*(.+)$')
                        if ($cm.Success) {
                            $dotnetFilterClauses += @{ Project = $projRel; Op = $cm.Groups[1].Value; Value = $cm.Groups[2].Value.Trim(); Source = $wf.Name }
                        }
                        else {
                            $warnings.Add("unrecognized filter clause '$clause' ($($wf.Name)) — treated as covering nothing")
                        }
                    }
                }
            }
        }
    }
}

# ---------------------------------------------------------------------------
# 3. Coverage evaluation
# ---------------------------------------------------------------------------
function Test-RustCovered($item) {
    if ($coverAllRustPkgs) { return $true }
    if ($fullyCoveredRustPkgs.Contains($item.Package)) { return $true }
    if ($coveredRustTargets.Contains("$($item.Package)|$($item.Target)")) { return $true }
    if ($coveredRustTargets.Contains("*|$($item.Target)")) { return $true }
    return $false
}

function Test-DotnetCovered($item) {
    if ($fullyCoveredProjects.Contains($item.Project)) { return $true }
    foreach ($clause in $dotnetFilterClauses) {
        if ($clause.Project -ne $item.Project) { continue }
        if ($clause.Op -eq '~') {
            if ($item.Fqcn.Contains($clause.Value)) { return $true }
        }
        else {
            if ($clause.Value -eq $item.Fqcn -or $clause.Value.StartsWith($item.Fqcn + '.')) { return $true }
        }
    }
    return $false
}

$covered = @{}   # id -> bool
foreach ($id in $rustItems.Keys)   { $covered[$id] = Test-RustCovered $rustItems[$id] }
foreach ($id in $dotnetItems.Keys) { $covered[$id] = Test-DotnetCovered $dotnetItems[$id] }
$allIds = @($rustItems.Keys) + @($dotnetItems.Keys)

# ---------------------------------------------------------------------------
# 4. Baseline
# ---------------------------------------------------------------------------
$baseline = @{}   # id -> @{ Reason; Parity }
if (Test-Path -LiteralPath $BaselineFile) {
    $lineNo = 0
    foreach ($raw in (Get-Content -LiteralPath $BaselineFile)) {
        $lineNo++
        $line = $raw.Trim()
        if ($line -eq '' -or $line.StartsWith('#')) { continue }
        $entry = $line; $reason = ''
        $hashIdx = $line.IndexOf('#')
        if ($hashIdx -ge 0) {
            $entry = $line.Substring(0, $hashIdx).Trim()
            $reason = $line.Substring($hashIdx + 1).Trim()
        }
        if ($entry -eq '') { continue }
        if ($baseline.ContainsKey($entry)) {
            $failures.Add("baseline: duplicate entry '$entry' (line $lineNo)")
            continue
        }
        $baseline[$entry] = @{ Reason = $reason; Parity = ($reason -match '\bPARITY:') }
    }
}

# ---------------------------------------------------------------------------
# 5. Rules
# ---------------------------------------------------------------------------

# Pair status precomputation
$pairsById = @{}   # member id -> list of pairs containing it
foreach ($pair in $ParityPairs) {
    foreach ($m in $pair.Members) {
        if (-not $pairsById.ContainsKey($m)) { $pairsById[$m] = @() }
        $pairsById[$m] += , $pair
    }
}
$pairAllCovered = @{}
foreach ($pair in $ParityPairs) {
    $exists = $true
    foreach ($m in $pair.Members) {
        if ($m -notin $allIds) {
            $failures.Add("parity pair '$($pair.Name)': member '$m' does not exist in the tree — fix the pair declaration or the test")
            $exists = $false
        }
    }
    $pairAllCovered[$pair.Name] = $exists -and -not ($pair.Members | Where-Object { -not $covered[$_] })
}

# 5a. Uncovered items must be baselined
$uncoveredUnbaselined = @()
foreach ($id in ($allIds | Sort-Object)) {
    if (-not $covered[$id] -and -not $baseline.ContainsKey($id)) {
        $uncoveredUnbaselined += $id
        $failures.Add("UNCOVERED (not in baseline): $id")
    }
}

# 5b. Baseline rot / ratchet
foreach ($id in ($baseline.Keys | Sort-Object)) {
    $b = $baseline[$id]
    if ($id -notin $allIds) {
        $failures.Add("STALE baseline entry (test no longer exists): $id")
        continue
    }
    if ($covered[$id]) {
        $exempt = $false
        if ($b.Parity -and $pairsById.ContainsKey($id)) {
            foreach ($pair in $pairsById[$id]) {
                if (-not $pairAllCovered[$pair.Name]) { $exempt = $true }
            }
        }
        if (-not $exempt) {
            $failures.Add("STALE baseline entry (now covered — remove it, the ratchet must burn down): $id")
        }
    }
    if ($b.Parity -and -not $pairsById.ContainsKey($id)) {
        $failures.Add("baseline: entry '$id' is PARITY-tagged but appears in no declared parity pair")
    }
}

# 5c. Parity pairs: all-covered, or ALL members PARITY-baselined
foreach ($pair in $ParityPairs) {
    $missing = $pair.Members | Where-Object { $_ -notin $allIds }
    if ($missing) { continue }  # already failed above
    if ($pairAllCovered[$pair.Name]) { continue }
    $notParityBaselined = $pair.Members | Where-Object {
        -not ($baseline.ContainsKey($_) -and $baseline[$_].Parity)
    }
    if ($notParityBaselined) {
        $sides = ($pair.Members | ForEach-Object {
            $state = if ($covered[$_]) { 'covered' } elseif ($baseline.ContainsKey($_)) { 'baselined' } else { 'uncovered' }
            "$_ [$state$(if ($baseline.ContainsKey($_) -and $baseline[$_].Parity) { ', PARITY' })]"
        }) -join ' <-> '
        $failures.Add("PARITY pair '$($pair.Name)' one-sided: $sides — both sides must be covered, or BOTH baselined with PARITY: tags")
    }
}

# ---------------------------------------------------------------------------
# 6. Report
# ---------------------------------------------------------------------------
$rustCovered = @($rustItems.Keys | Where-Object { $covered[$_] }).Count
$dotnetCovered = @($dotnetItems.Keys | Where-Object { $covered[$_] }).Count
Write-Host '== CI coverage meta-check =='
Write-Host ("Rust integration-test files: {0} discovered, {1} covered by workflows" -f $rustItems.Count, $rustCovered)
Write-Host (".NET test classes:           {0} discovered, {1} covered by workflows" -f $dotnetItems.Count, $dotnetCovered)
Write-Host ("Baseline entries:            {0} ({1})" -f $baseline.Count, $BaselineFile)
Write-Host ("Parity pairs:                {0} declared, {1} fully covered" -f $ParityPairs.Count, (@($ParityPairs | Where-Object { $pairAllCovered[$_.Name] }).Count))

if ($ShowCovered) {
    Write-Host "`n-- inventory --"
    foreach ($id in ($allIds | Sort-Object)) {
        $state = if ($covered[$id]) { 'COVERED  ' } elseif ($baseline.ContainsKey($id)) { 'BASELINED' } else { 'UNCOVERED' }
        Write-Host ("  {0} {1}" -f $state, $id)
    }
}

if ($warnings.Count -gt 0) {
    Write-Host "`n-- warnings (treated conservatively: no coverage granted) --"
    foreach ($w in ($warnings | Sort-Object -Unique)) { Write-Host "  WARN: $w" }
}

$parityBaselined = @($baseline.Keys | Where-Object { $baseline[$_].Parity })
if ($parityBaselined.Count -gt 0) {
    Write-Host "`n-- PARITY-tagged baseline entries (C2 top priority; each pair leaves the baseline together) --"
    foreach ($id in ($parityBaselined | Sort-Object)) { Write-Host "  $id" }
}

if ($failures.Count -gt 0) {
    Write-Host "`n== FAIL: $($failures.Count) problem(s) =="
    foreach ($f in $failures) { Write-Host "  FAIL: $f" }
    Write-Host "`nEvery test suite must run in some workflow, or carry an explicit entry in"
    Write-Host "$BaselineFile (with a reason). Baseline entries for now-covered or deleted"
    Write-Host 'tests must be removed. Parity pairs must be covered on BOTH sides.'
    exit 1
}

Write-Host "`nOK: no silent coverage gaps."
exit 0
