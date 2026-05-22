# Hosted Service Benchmarks

For a drift-aware interpretation playbook with go/no-go thresholds and a concrete baseline/auth alternating sample, see `benchmarks/benchmarks.md`.

This folder provides a simple websocket round-trip benchmark harness for:

1. rust-combined-server (`activesync-server`)
2. rust-integrated-hosted-server (`activesync-dev-server`, optional, Mongo-backed)
3. dotnet-host-runtime (`ActiveSync.DotNetHost`)

## Does the combined integrated Rust server still exist?

Yes. The combined server is still present as `activesync-server`.

Evidence:

1. `server/Cargo.toml` depends on both `activesync-core` and `activesync-host-core`.
2. The binary entrypoint remains `server/src/main.rs`.

## What is benchmarked?

The harness measures websocket connect plus first-response latency using protocol-aware probes:

1. rust targets (`activesync-server`, `activesync-dev-server`): connect, send `hello`, wait for first response (typically `welcome`)
2. dotnet target (`ActiveSync.DotNetHost`): connect, send `hello`, send `noop`, wait for first response (typically `noop-ack`)
3. each iteration uses a unique synthetic pubkey to avoid duplicate-peer churn effects
4. record elapsed milliseconds

Reported stats per target:

1. samples
2. average latency
3. p50/p95/p99 latency
4. min/max latency
5. approximate requests/second (derived from sample count / elapsed)

## Quick start

From repo root:

```powershell
pwsh -File .\benchmarks\Start-BenchmarkTargets.ps1
```

In another shell, run:

```powershell
pwsh -File .\benchmarks\Run-HostedServiceBenchmarks.ps1 -MeasureIterations 100
```

Optional JSON output:

```powershell
pwsh -File .\benchmarks\Run-HostedServiceBenchmarks.ps1 -MeasureIterations 100 -OutputJsonPath .\benchmarks\results\latest.json
```

## Apples-to-apples scenario benchmark (map/list/blob, multi-peer)

Use the SDK-driven scenario runner for semantic parity across targets. It runs:

1. map convergence scenario
2. list convergence scenario
3. blob propagation/retrieval scenario

Each scenario is executed with warm session initialization and peer-count sweeps (default: 2, 10, 20 peers), then reported per target.

Example:

```powershell
node .\benchmarks\Run-SdkScenarioBenchmarks.mjs --iterations 3 --peers 2,10,20 --outputJsonPath .\benchmarks\results\sdk-scenarios-latest.json
```

Useful options:

1. `--targets rust-combined-server,dotnet-host-runtime-alias`
2. `--mapOps 60 --listOps 60 --blobOps 20`
3. `--blobSizeBytes 4096`
4. `--warmupOps 8`
5. `--timeoutMs 30000`
6. `--authMode room-lock-tokened` (locks room via `set-room-key` and benchmarks tokened hello sessions)
7. `--authTokenCaps read:bench/**,write:bench/**`
8. `--authAdminCaps read:bench/**,write:bench/**,room.admin`
9. `--tokenExpirySecs 3600`

## Benchmark matrix runner (auth/security/guardrails)

Use the matrix runner when you want repeatable profile comparisons across optional auth/security/guardrail setup while keeping the same workload semantics on both hosts.

Matrix definition file:

1. `benchmarks/benchmark-matrix.v1.json`

Result schema:

1. `benchmarks/results/benchmark-matrix-result.schema.v1.json`

Run all rows:

```powershell
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1
```

Run selected rows only:

```powershell
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds baseline-default-auth,embedded-auth-guardrails
```

Seed a fast baseline artifact for repeatable reporting:

```powershell
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds baseline-default-auth -OutputPath .\benchmarks\results\benchmark-matrix-baseline.json
```

Current matrix dimensions include:

1. auth mode (default, embedded, and `room-lock-tokened` scenario mode)
2. room scale (`peers` sweep)
3. command mix (`mapOps`, `listOps`, `blobOps`, blob size)
4. reconnect/churn intensity (iteration count)
5. guardrail/security toggles (scope budget strictness, server-peer configured/unset, compaction on/off)
6. capability composition profile-on rows (`benchmarks/profiles/capability-profile.v1.json`)

## Target defaults

1. rust-combined-server: `ws://127.0.0.1:7878/ws/bench-room`
2. rust-integrated-hosted-server: `ws://127.0.0.1:7979/ws/bench-room`
3. dotnet-host-runtime: `ws://127.0.0.1:8787/ws/runtime`

If a target is unreachable, it is skipped and benchmarking continues.

## Notes

1. `rust-integrated-hosted-server` is optional and requires `MONGO_URI` when starting.
2. `.NET host` needs `ACTIVESYNC_HOST_FFI_DLL`; the starter script auto-resolves from local `target/debug` or `target/release` when available.
3. The current Rust websocket path includes a small stabilization delay before welcome send, so absolute values are best used for trend tracking unless probe semantics are fully normalized across targets.
4. This is a practical smoke benchmark harness, not a full load/stress framework. Use it for local comparisons and regression trending.
5. `Run-SdkScenarioBenchmarks.mjs` is the preferred apples-to-apples comparison because all targets are exercised through the same SDK semantics rather than target-specific probe messages.
