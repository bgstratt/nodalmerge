# NodalMerge .NET Host Packages

NodalMerge host packages provide a package-first path for embedding a deterministic sync runtime in .NET hosts.

## Packages

- `NodalMerge.Host.Abstractions` (wrapper): provider contracts package identity during migration.
- `NodalMerge.Host.Composition` (wrapper): composition helpers package identity during migration.
- `NodalMerge.DotNetHost.Native.win-x64` (wrapper): native runtime package identity for Windows x64.
- `NodalMerge.DotNetHost.Native.linux-x64` (wrapper): native runtime package identity for Linux x64.

Compatibility note:

- Legacy `ActiveSync.*` package IDs are still supported during migration window.

## Dead-Simple Runtime Surface

The runtime bridge is intentionally small and centered on these concepts:

1. room
2. sync
3. replay
4. offline
5. CAS
6. topology

For transport, use `GET /ws/runtime` and exchange JSON command/event frames.

## Local Pre-Publish Validation

Use local packages before any remote publish:

```powershell
cd nodalmerge-host
pwsh -File .\pack-local-nuget.ps1 -Version 0.1.0-local
dotnet restore .\NodalMerge.DotNetHost.slnx --configfile .\NuGet.Local.config -p:ActiveSyncUseNuGetPackages=true -p:ActiveSyncPackageVersion=0.1.0-local
pwsh -File .\verify.ps1 -UseNuGetPackages -NodalMergePackageVersion 0.1.0-local
```

## Notes

- Default project build mode uses project references.
- Package mode is opt-in through `ActiveSyncUseNuGetPackages=true`.
- Native runtime can be overridden via `NODALMERGE_HOST_FFI_DLL` (legacy `ACTIVESYNC_HOST_FFI_DLL` fallback remains).
