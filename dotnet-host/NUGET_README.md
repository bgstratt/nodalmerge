# ActiveSync .NET Host Packages

ActiveSync host packages provide a package-first path for embedding a deterministic sync runtime in .NET hosts.

## Packages

- `ActiveSync.Host.Abstractions`: provider contracts for storage/auth/blob delegation.
- `ActiveSync.Host.Composition`: dependency injection wiring and host composition helpers.
- `ActiveSync.DotNetHost.Native.win-x64`: native runtime (`activesync_host_ffi.dll`) for Windows x64.
- `ActiveSync.DotNetHost.Native.linux-x64`: native runtime (`libactivesync_host_ffi.so`) for Linux x64.

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
cd dotnet-host
pwsh -File .\pack-local-nuget.ps1 -Version 0.1.0-local
dotnet restore .\ActiveSync.DotNetHost.slnx --configfile .\NuGet.Local.config -p:ActiveSyncUseNuGetPackages=true -p:ActiveSyncPackageVersion=0.1.0-local
pwsh -File .\verify.ps1 -UseNuGetPackages -ActiveSyncPackageVersion 0.1.0-local
```

## Notes

- Default project build mode uses project references.
- Package mode is opt-in through `ActiveSyncUseNuGetPackages=true`.
- Native runtime can still be overridden via `ACTIVESYNC_HOST_FFI_DLL` when needed.
