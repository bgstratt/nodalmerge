# ActiveSync Packaging Publish Runbook

Status: Ready for execution
Updated: 2026-05-20

This runbook covers pre-publish checks and publish steps for:

1. NuGet (.NET host + native runtime)
2. npm (bridge + sdk-js wrapper)
3. crates.io (core + host + ffi + bridge)

## 0. One-Command Local Packaging

For local staging of all package surfaces into one output root:

```powershell
pwsh -File ./pack-local-artifacts.ps1 -Version 0.1.0-local
```

Output folders:

1. `artifacts/package-local/npm` (`*.tgz`)
2. `artifacts/package-local/nuget` (`*.nupkg`)
3. `artifacts/package-local/crates` (`*.crate`)

Notes:

1. `wasm-pack` is required for bridge package generation.
2. `cargo package` without `--allow-dirty` requires a clean working tree.
3. For active dev branches, add `-AllowDirtyCrates` while keeping publish flows clean/repeatable.
4. The local bundler uses `cargo package --no-verify` for dependency-ordered workspace staging; keep the publish dry-run checks in Sections 1 and 4 before external release.
5. If dependent crates are not yet published to crates.io, use `-AllowCrateDependencyFailures` for local staging while still producing available `.crate` artifacts.

## 0.1 Two-Machine Local Publish + Consume

Goal: clone this repo on machine A or B, build local artifacts, and consume them from another app without project references.

### Step A: Build local artifacts on each machine

From repo root:

```powershell
pwsh -File ./pack-local-artifacts.ps1 -Version 0.1.0-local -AllowDirtyCrates -AllowCrateDependencyFailures
```

This produces:

1. `artifacts/package-local/nuget` (NuGet packages)
2. `artifacts/package-local/npm` (npm tarballs)
3. `artifacts/package-local/crates` (crate archives)

### Step B: Consume NuGet packages in another .NET app

Create or update a `NuGet.config` in the consuming app:

```xml
<?xml version="1.0" encoding="utf-8"?>
<configuration>
	<packageSources>
		<clear />
		<add key="local-activesync" value="C:\\path\\to\\activesync\\artifacts\\package-local\\nuget" />
		<add key="nuget.org" value="https://api.nuget.org/v3/index.json" />
	</packageSources>
</configuration>
```

Then add package references in the consumer project:

1. `ActiveSync.Host.Abstractions` version `0.1.0-local`
2. `ActiveSync.Host.Composition` version `0.1.0-local`
3. optional native RID packages when needed by deployment mode

### Step C: Consume Rust crates in another app without workspace project refs

1. Expand local `.crate` archives into unpacked directories:

```powershell
pwsh -File ./expand-local-crates.ps1
```

2. In the consuming `Cargo.toml`, reference unpacked package paths:

```toml
[dependencies]
activesync-core = { path = "C:/path/to/activesync/artifacts/package-local/crates/unpacked/activesync-core-0.1.0" }
activesync-host-core = { path = "C:/path/to/activesync/artifacts/package-local/crates/unpacked/activesync-host-core-0.1.0" }
activesync-host-ffi = { path = "C:/path/to/activesync/artifacts/package-local/crates/unpacked/activesync-host-ffi-0.1.0" }
```

This keeps the consumer independent from workspace project references while still using package snapshots generated from this repo.

### Step D: Optional npm local consume

In a consuming JS app:

```powershell
npm install C:\path\to\activesync\artifacts\package-local\npm\activesync-bridge-0.1.0.tgz
npm install C:\path\to\activesync\artifacts\package-local\npm\activesync-sdk-js-0.1.0.tgz
```

## 1. Preflight Gates

Run from repo root unless noted.

1. `cargo test -p activesync-core`
2. `cargo test -p activesync-host-core`
3. `cargo test -p activesync-host-ffi`
4. `dotnet test dotnet-host/ActiveSync.DotNetHost.slnx`
5. `cd dotnet-host; pwsh -File ./pack-local-nuget.ps1 -Version 0.1.0-local`
6. `cd dotnet-host; dotnet restore ./ActiveSync.DotNetHost.slnx --configfile ./NuGet.Local.config -p:ActiveSyncUseNuGetPackages=true -p:ActiveSyncPackageVersion=0.1.0-local`
7. `cd dotnet-host; pwsh -File ./verify.ps1 -UseNuGetPackages -ActiveSyncPackageVersion 0.1.0-local`

Required outcome:

1. All tests and package-mode smoke checks pass.
2. `artifacts/nuget-local` contains managed and native packages.

## 2. NuGet Publish Flow

Primary package IDs:

1. `ActiveSync.Host.Abstractions`
2. `ActiveSync.Host.Composition`
3. `ActiveSync.DotNetHost.Native.win-x64`
4. `ActiveSync.DotNetHost.Native.linux-x64`

Local validation already uses `dotnet-host/NUGET_README.md` as package readme metadata.

Publish options:

1. GitHub Actions workflow: `.github/workflows/nuget-build-push.yml`
2. Manual `dotnet nuget push` from prepared artifacts directory

Manual push shape:

```powershell
dotnet nuget push <path-to-nupkg> --source <source-name-or-url> --api-key <token> --skip-duplicate
```

## 3. npm Publish Flow

Packages:

1. `activesync-bridge` from `bridge/pkg`
2. `activesync-sdk-js` from `sdk-js`

### 3.1 Build bridge package assets

If bridge assets need refresh:

```bash
cd bridge
wasm-pack build --target web
```

### 3.2 Validate package contents

```bash
cd bridge/pkg
npm pack --dry-run

cd ../../sdk-js
npm pack --dry-run
```

Check that expected files are included:

1. Bridge: wasm/js/d.ts/readme
2. SDK: index.js/index.d.ts/readme

### 3.3 Publish

```bash
cd bridge/pkg
npm publish --access public

cd ../../sdk-js
npm publish --access public
```

If publishing to private registry, use registry-specific auth and omit `--access public` as needed.

## 4. crates.io Publish Flow

Publish order (dependency-safe):

1. `activesync-core`
2. `activesync-host-core`
3. `activesync-host-ffi`
4. `activesync-host-axum`
5. `activesync-bridge`

Dry-run first for each crate:

```bash
cargo publish -p activesync-core --dry-run
```

Important staging note:

1. `activesync-host-core`, `activesync-host-ffi`, and `activesync-bridge` depend on crates that must already exist on crates.io.
2. Before the first public release, dry-run for those dependent crates will fail until upstream crates are published.

After publishing `activesync-core`, run:

```bash
cargo publish -p activesync-host-core --dry-run
```

After publishing `activesync-host-core`, run:

```bash
cargo publish -p activesync-host-ffi --dry-run
cargo publish -p activesync-host-axum --dry-run
```

After `activesync-core` is available on crates.io, run:

```bash
cargo publish -p activesync-bridge --dry-run
```

Then publish in the same order:

```bash
cargo publish -p activesync-core
cargo publish -p activesync-host-core
cargo publish -p activesync-host-ffi
cargo publish -p activesync-host-axum
cargo publish -p activesync-bridge
```

## 5. Release Sign-Off Checklist

1. Dead-simple API coverage documented: room/sync/replay/offline/CAS/topology.
2. Package readmes present and embedded where platform supports it.
3. Local package consumption works before remote publish.
4. Publish dry-runs are green for npm and crates.
5. Version numbers are aligned across release artifacts.
