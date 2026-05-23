# Benchmark Reality Check (Actionable)

Date: 2026-05-22
Purpose: fast way to decide whether auth/guardrails are acceptable overhead or a regression.

## What this benchmark actually exercises

1. Targets: `rust-combined-server`, `dotnet-host-runtime-alias`
2. Peer counts: `2`, `10`, `20`
3. Iterations: `3` per peer count
4. Command mix per iteration: `mapOps=60`, `listOps=60`, `blobOps=20`, `blobSizeBytes=4096`
5. Transport: `ws-only`

Per row, that is 18 scenario cells (2 targets x 3 peer counts x 3 iterations).

## Alternating sample used for this note

Sequence used: baseline/auth/baseline/auth (A-B-A-B)

Artifacts:

1. `benchmarks/results/benchmark-matrix-baseline-a-20260522-135151.json`
2. `benchmarks/results/benchmark-matrix-auth-b-20260522-135151.json`
3. `benchmarks/results/benchmark-matrix-baseline-c-20260522-135151.json`
4. `benchmarks/results/benchmark-matrix-auth-d-20260522-135151.json`

## Drift-aware summary (average of two baseline vs two auth runs)

Interpretation: positive percent means auth-enforced is slower.

### Aggregate (cross-peer combined)

1. DotNet: map `677.84 -> 710.23` (`+4.78%`), list `937.19 -> 983.58` (`+4.95%`), blob `348.93 -> 376.63` (`+7.94%`)
2. Rust: map `654.51 -> 666.95` (`+1.90%`), list `889.60 -> 907.50` (`+2.01%`), blob `362.72 -> 385.57` (`+6.30%`)

### Per-peer deltas (auth vs baseline)

1. DotNet 2 peers: map `+3.68%`, list `+5.51%`, blob `+7.31%`
2. DotNet 10 peers: map `+2.96%`, list `+3.59%`, blob `+1.60%`
3. DotNet 20 peers: map `+7.61%`, list `+5.69%`, blob `+13.36%`
4. Rust 2 peers: map `+0.52%`, list `+1.63%`, blob `+15.25%`
5. Rust 10 peers: map `+2.14%`, list `+1.74%`, blob `+0.75%`
6. Rust 20 peers: map `+2.82%`, list `+2.49%`, blob `+6.51%`

## So, rocketship or push cart?

Current evidence says not a push cart.

1. Map/list overhead is modest (roughly `+2%` to `+5%` overall, depending on host).
2. Blob overhead is the most sensitive axis (roughly `+6%` to `+8%` overall; occasional higher per-peer spikes).
3. No catastrophic cliff observed under 20-peer load in this sample.

## How to run this again (same method)

```powershell
$ts = Get-Date -Format "yyyyMMdd-HHmmss"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds baseline-default-auth -OutputPath ".\benchmarks\results\benchmark-matrix-baseline-a-$ts.json"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds auth-enforced-room-lock-tokened -OutputPath ".\benchmarks\results\benchmark-matrix-auth-b-$ts.json"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds baseline-default-auth -OutputPath ".\benchmarks\results\benchmark-matrix-baseline-c-$ts.json"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds auth-enforced-room-lock-tokened -OutputPath ".\benchmarks\results\benchmark-matrix-auth-d-$ts.json"
```

## Action thresholds (quick decision guide)

Use these until we replace them with formal gates:

1. Green: map/list delta <= `+8%` and blob delta <= `+12%`
2. Yellow: map/list delta `+8..+12%` or blob delta `+12..+18%` (investigate)
3. Red: map/list delta > `+12%` or blob delta > `+18%`

Based on this run set: currently Green/Yellow, not Red.

## Important caveat

The timed map/list/blob scenario blocks measure steady-state session behavior. They do not currently break out separate timing fields for auth setup (room-key bootstrap and token provisioning). Add explicit setup timing fields if you want a full cold-path auth tax number in the same report.

## Rust trace microbench (260k ops) - signed/unsigned, batch/non-batch

Date: 2026-05-22
Source trace: `docs/rustcode.json`
Runner: `core/src/bin/text_trace_rustcode_oneshot.rs`

Command shape used for each row (with env var toggles):

```powershell
& "C:\Users\bgstr\.cargo\bin\cargo.exe" run --release --manifest-path ".\core\Cargo.toml" --bin text_trace_rustcode_oneshot
```

All rows below use `ACTIVESYNC_TEXT_TRACE_MAX_OPS=260000`.

| Variant | signed | use_batch | batch_size | applied_ops | apply_ms | ops_per_sec |
|---|---:|---:|---:|---:|---:|---:|
| unsigned non-batch | 0 | 0 | n/a | 260000 | 335 | 774955 |
| unsigned batch | 0 | 1 | default | 260000 | 382 | 679002 |
| signed non-batch | 1 | 0 | n/a | 260000 | 8500 | 30587 |
| signed batch | 1 | 1 | default | 260000 | 680 | 382220 |
| signed batch (50k) | 1 | 1 | 50000 | 260000 | 722 | 360029 |

Interpretation notes:

1. `apply_ms` and `ops_per_sec` are apply-phase only (pre-translated nodes), not JSON parse/adapter translation time.
2. Signed mode includes Ed25519 verification cost during apply; batch mode uses batched verify in `apply_remote_batch`.
3. For this run, batching helps significantly in signed mode, while unsigned mode is slightly faster without batch.

## Full ecosystem benchmarks retained (.NET host vs Rust host)

This microbench section is additive and does not replace host-level apples-to-apples measurements.

Existing ecosystem benchmark coverage remains in place:

1. `benchmarks/Run-SdkScenarioBenchmarks.mjs` (semantic map/list/blob scenarios across targets)
2. `benchmarks/Run-BenchmarkMatrix.ps1` with `benchmarks/benchmark-matrix.v1.json` (auth/security/guardrail profile rows)
3. target comparison includes `rust-combined-server` and `dotnet-host-runtime-alias` as documented above in this file
