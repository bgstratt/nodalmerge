# nodalmerge-runtime-local-ffi

C ABI wrapper for [`nodalmerge-runtime-local`](../runtime-local) peer-local persistence.

## Build

```bash
cargo build -p nodalmerge-runtime-local-ffi --release
```

Windows artifact: `target/release/nodalmerge_runtime_local_ffi.dll`

## .NET embedding

`NodalMerge.DotNetHost.Ffi.LocalPersistFfiClient` loads this library (set `NODALMERGE_LOCAL_FFI_DLL` if needed).

```csharp
using var store = new LocalPersistFfiClient("embedded", dataDir: @"C:\peer-data");
using var report = store.Hydrate("my-room");
var hash = store.CanonicalHashHex("my-room");
```

WS `pack` blobs (base64 `nodes` field) can be appended with `AppendPackB64` / `nm_local_store_append_pack_b64`.

## .NET runtime host

Set in `appsettings`:

```json
"NodalMerge": {
  "Runtime": {
    "PeerLocal": {
      "Enabled": true,
      "Backend": "embedded",
      "DataDir": "data/peer-local"
    }
  }
}
```

Inbound `pack` messages on `/ws/runtime` are mirrored into peer-local storage via `RuntimePeerLocalPersistenceService`.

## C header

See `include/nodalmerge_local.h`.
