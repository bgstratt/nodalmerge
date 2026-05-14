# Hosted Service Benchmarks

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
