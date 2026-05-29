# Tools Split First Real Rehearsal Runbook

Owner: Platform/runtime + tools maintainers  
Status: Ready for execution  
Updated: 2026-05-27

This runbook executes the first real cross-repo rehearsal for the optional `nodalmerge-tools` split (`headless` + `cli`).

## 0. Inputs and prerequisites

1. Core repo branch with:
   - `docs/release/core-release-manifest-template.json`
   - `docs/release/tools-version-pin-template.md`
   - acceptance artifacts for repo boundary/parity Phase B
2. Tools repo branch prepared to pin core versions.
3. CI access in both repos.

## 1. Prepare core release candidate (RC)

1. Choose rehearsal RC version (example: `0.1.0-rc1`).
2. Build/package candidate artifacts in core:
   - crates
   - npm packages
   - NuGet + native runtime artifacts
3. Record:
   - core git SHA
   - RC tag name
   - artifact manifest location
4. Update a concrete copy of `core-release-manifest-template.json` with real values.
5. Validate the concrete manifest before handoff:

```powershell
pwsh -File ./scripts/release/Validate-CoreReleaseManifest.ps1 -ManifestPath .\docs\release\core-release-manifest-runNN.json
```

## 2. Run core parity gates (Tier 1 + Tier 3)

1. Run required Tier 1 parity lanes:
   - Rust host runtime <-> .NET host runtime parity smoke
   - runtime-local-ffi ABI lane
2. Run Tier 3 compatibility lane (advisory):
   - legacy Rust integrated server compatibility smoke
3. Capture core CI run IDs.

## 3. Pin tools repo to core RC

1. In tools repo, set exact pins to core RC versions.
2. Populate release/CI summary block from `tools-version-pin-template.md`:
   - core version/tag/SHA
   - acceptance artifact links (immutable core SHA links)
3. Commit pin update.

## 4. Run tools gates (Tier 2)

1. Run tools pinned-core lanes:
   - SDK/headless semantic parity lane
   - integration smoke lane (`run`, `topology`, `archive`; query lane if configured)
2. Run tools canary lane against core `main` (advisory).
3. Capture tools CI run IDs.

## 5. Record rehearsal evidence in core

1. Update:
   - `docs/acceptance/repo-boundary-parity-phaseb-tools-artifact-link-check-run01.json`
   - `docs/acceptance/repo-boundary-parity-phaseb-packaging-docs-split-run01.json`
2. Fill:
   - real core SHA/tag
   - real core CI run IDs
   - real tools CI run IDs
   - pass/fail per required gate
3. If any blocker fails, add bounded delta note + owner + expiry date.

## 6. Exit criteria (rehearsal pass)

1. Core Tier 1 required gates pass.
2. Tools Tier 2 required gates pass using pinned core RC.
3. Acceptance artifacts include immutable references and CI run IDs.
4. Any advisory failures are documented with follow-up issue IDs.

## 7. Command skeleton (replace placeholders)

```powershell
# Core repo
pwsh -File ./pack-local-artifacts.ps1 -Version 0.1.0-rc1
pwsh -File ./scripts/release/Validate-CoreReleaseManifest.ps1 -ManifestPath .\docs\release\core-release-manifest-runNN.json

# Run selected parity lanes (examples)
cargo test -p nodalmerge-runtime-local-ffi --test abi
dotnet test nodalmerge-host/NodalMerge.DotNetHost.slnx

# Tools repo (conceptual)
# 1) apply exact core pins (=0.1.0-rc1)
# 2) run tools parity/integration CI workflows
```

