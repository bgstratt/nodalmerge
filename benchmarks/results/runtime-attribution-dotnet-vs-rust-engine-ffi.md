# Runtime Attribution: DotNet Hosted Native vs Rust Engine/FFI

This breaks end-to-end host runtime cost into layers for identical command shapes.

## Scope
- Machine/runtime: Windows 11, Ryzen 9 5900X, .NET 10.0.4, release builds
- Rust direct engine benchmarks: `engine_direct_*` from `host-ffi/benches/host_runtime_vs_ffi.rs`
- Rust FFI benchmarks: `ffi_submit_json_*` from `host-ffi/benches/host_runtime_vs_ffi.rs`
- DotNet hosted native benchmarks: `HostFfiBenchmarks.*` from `dotnet-host/bench/NodalMerge.DotNetHost.Benchmarks`

## Combined Summary
| Workload | Engine microbench winner | Realtime winner (30 ops) | 30-op delta |
|---|---|---|---:|
| Map | Rust (direct engine) | DotNet Mongo | DotNet faster by 102.142 ms (22.9%) |
| List | Rust (direct engine) | DotNet Mongo | DotNet faster by 81.417 ms (16.1%) |
| Blob | Rust (direct engine) | DotNet Mongo | DotNet faster by 107.084 ms (22.1%) |

Notes:
- "Engine microbench winner" is based on direct in-process engine timing.
- "Realtime winner" is from sequential hosted ws+persistence run (`peers=6`, `ops=30`).

## Raw Means
| Operation | Rust direct engine | Rust FFI submit_json | DotNet HostFfiClient submit_json |
|---|---:|---:|---:|
| Noop | 80.502 ns | 302.77 ns | 3.483 us |
| MapSet | 580.70 ns | 1.3252 us | 15.174 us |
| RequestServerPack1k | 474.06 ns | 1.2066 us | 14.800 us |
| BlobSet50Kb | 1.8673 us | 15.307 us | 360.914 us |

## Layer Multipliers
| Operation | FFI/Direct | DotNet/FFI | DotNet/Direct |
|---|---:|---:|---:|
| Noop | 3.76x | 11.50x | 43.27x |
| MapSet | 2.28x | 11.45x | 26.13x |
| RequestServerPack1k | 2.54x | 12.27x | 31.22x |
| BlobSet50Kb | 8.20x | 23.58x | 193.28x |

## Additive Cost Attribution (approx)
Using:
- FFI boundary cost approx = `RustFFI - RustDirect`
- DotNet hosted/runtime cost approx = `DotNet - RustFFI`

| Operation | FFI boundary (us) | DotNet hosted/runtime (us) | Total above direct (us) |
|---|---:|---:|---:|
| Noop | 0.222 | 3.180 | 3.403 |
| MapSet | 0.745 | 13.849 | 14.593 |
| RequestServerPack1k | 0.733 | 13.593 | 14.326 |
| BlobSet50Kb | 13.440 | 345.607 | 359.047 |

## Interpretation
- Core engine work is still in ns to low-us territory.
- FFI adds a measurable but smaller layer for tiny ops, larger for big payload conversion paths.
- DotNet hosted path dominates for these command shapes, especially 50KB blob JSON/base64 workloads.
- This does not contradict earlier ws/storage benchmarks where system-level behavior (transport, persistence, scheduling, batching, tails) can be comparable or favor DotNet.

## Repro Commands
Rust direct/FFI microbench:
```powershell
cargo bench -p nodalmerge-host-ffi --bench host_runtime_vs_ffi engine_direct_noop engine_direct_map_set engine_direct_request_server_pack_1k engine_direct_blob_set_50kb ffi_submit_json_noop ffi_submit_json_map_set ffi_submit_json_request_server_pack_1k ffi_submit_json_blob_set_50kb -- --sample-size 20
```

DotNet hosted native microbench:
```powershell
dotnet run --project dotnet-host/bench/NodalMerge.DotNetHost.Benchmarks/NodalMerge.DotNetHost.Benchmarks.csproj -c Release
```

## Other Core Benches (Fresh Run)
These are from `cargo bench -p nodalmerge-core` run by bench target.

| Benchmark | Time |
|---|---:|
| blob_verify_50kb | 10.002 us |
| resolve_1k | 353.88 us |
| sync_handshake_pack_1k_missing | 386.44 us |
| sync_handshake_ibf_encode_1k | 277.47 us |
| sync_handshake_ibf_decode_1k_diff | 883.29 ns |
| sync_handshake_mst_build_1k | 1.2964 ms |
| sync_handshake_mst_build_2k | 3.2109 ms |
| sync_handshake_mst_simulate_1k_diff | 311.35 us |
| merge_10k | 363.06 ms |
| merge_10k_batch | 35.139 ms |
| tx_serialize_json | 877.44 ns |
| tx_serialize_postcard | 464.20 ns |
| tx_hash_json_plus_blake3 | 1.2119 us |
| tx_hash_postcard_plus_blake3 | 618.16 ns |

## Comparability Notes
- Directly comparable across all three layers in this report: `noop`, `map_set`, `request_server_pack_1k`, `blob_set_50kb`.
- Not yet directly comparable to DotNet hosted runtime from this report: `resolve_1k`, handshake internals (`ibf_*`, `mst_*`), `merge_10k`, and tx hash/serialize microbenches.
- To compare those to DotNet hosted native path, we need matching DotNet benchmark commands that drive equivalent host-core operations through the same command payload model.

## Realtime Hosted Benchmarks (Sequential4)
These are the realtime ws + host + persistence comparison runs (peers=6) from:
`benchmarks/results/ops-sweep-peers6-integrated-vs-dotnet-mongo-sequential4.md`.

### Map
| Ops | Rust Integrated avg ms | DotNet Mongo avg ms | Faster |
|---|---:|---:|---|
| 2 | 75.483 | 60.965 | DotNet |
| 6 | 116.902 | 81.705 | DotNet |
| 12 | 213.418 | 162.338 | DotNet |
| 30 | 445.703 | 343.561 | DotNet |

### List
| Ops | Rust Integrated avg ms | DotNet Mongo avg ms | Faster |
|---|---:|---:|---|
| 2 | 76.352 | 61.730 | DotNet |
| 6 | 103.051 | 91.514 | DotNet |
| 12 | 222.031 | 187.806 | DotNet |
| 30 | 506.009 | 424.592 | DotNet |

### Blob
| Ops | Rust Integrated avg ms | DotNet Mongo avg ms | Faster |
|---|---:|---:|---|
| 2 | 60.841 | 69.430 | Rust |
| 6 | 113.608 | 87.589 | DotNet |
| 12 | 202.370 | 152.065 | DotNet |
| 30 | 485.513 | 378.429 | DotNet |

### Realtime Takeaway
- Realtime hosted path is mostly DotNet-favored in this sequential apples-to-apples run.
- This coexists with Rust winning core-engine microbenchmarks because realtime results include transport, host runtime, storage, scheduling, and batching behavior.
