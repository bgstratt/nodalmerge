# Tools Version Pin Template

Use this template in the `nodalmerge-tools` repo release notes and CI summaries.

## 1. Core Pin Block (Required)

- Core version: `X.Y.Z`
- Core git tag: `vX.Y.Z`
- Core git SHA: `<core-commit-sha>`
- Core release manifest: `docs/release/core-release-manifest-template.json` (resolved with real release values)

## 2. Dependency Pins (Release Branches)

### Cargo

Use exact pins for release lanes:

```toml
[dependencies]
nodalmerge-core = "=X.Y.Z"
nodalmerge-runtime-local = "=X.Y.Z"
```

### Native runtime / NuGet interop

- `NodalMerge.DotNetHost.Native.win-x64`: `X.Y.Z`
- `NodalMerge.DotNetHost.Native.linux-x64`: `X.Y.Z`
- `nodalmerge_runtime_local_ffi` runtime artifact version: `X.Y.Z`

## 3. CI Evidence Block (Required)

Include immutable references in workflow summary:

1. Core SHA: `<core-commit-sha>`
2. Acceptance artifact links:
   - `docs/acceptance/repo-boundary-parity-phaseb-tools-artifact-link-check-run01.json`
   - `docs/acceptance/repo-boundary-parity-phaseb-packaging-docs-split-run01.json`
3. CI run IDs:
   - core parity lane run id: `<id>`
   - tools pinned-core lane run id: `<id>`
   - tools canary lane run id (if run): `<id>`

## 4. Release Gate Checklist

- [ ] Pinned-core release lane green
- [ ] Tier 2 SDK/headless parity lane green
- [ ] Integration smoke lane green
- [ ] Acceptance artifact updates committed in core (if any bounded deltas)

## 5. Notes

- `main` development lanes may temporarily use core git revisions.
- Release lanes must use exact version pins and immutable artifact references.
