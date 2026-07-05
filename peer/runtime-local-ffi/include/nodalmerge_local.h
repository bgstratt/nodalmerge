#ifndef NODALMERGE_LOCAL_H
#define NODALMERGE_LOCAL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct nm_local_store nm_local_store;

typedef struct {
    const uint8_t* ptr;
    size_t len;
} nm_bytes_view;

typedef struct {
    uint8_t* ptr;
    size_t len;
} nm_bytes_owned;

typedef enum {
    NM_LOCAL_OK = 0,
    NM_LOCAL_ERR_INVALID_ARG = 1,
    NM_LOCAL_ERR_NOT_FOUND = 2,
    NM_LOCAL_ERR_UNAVAILABLE = 3,
    NM_LOCAL_ERR_CORRUPTION = 4,
    NM_LOCAL_ERR_TAIL_CONFLICT = 5,
    NM_LOCAL_ERR_QUOTA = 6,
    NM_LOCAL_ERR_READONLY = 7,
    NM_LOCAL_ERR_INTERNAL = 255
} nm_local_status;

uint32_t nm_local_abi_version(void);

nm_local_status nm_local_store_open(
    nm_bytes_view backend,
    nm_bytes_view data_dir,
    nm_local_store** out_store
);

nm_local_status nm_local_store_free(nm_local_store* store);

nm_local_status nm_local_store_is_durable(nm_local_store* store, uint8_t* out_durable);

nm_local_status nm_local_store_hydrate_json(
    nm_local_store* store,
    nm_bytes_view room_id,
    nm_bytes_owned* out_json
);

nm_local_status nm_local_store_recover_json(
    nm_local_store* store,
    nm_bytes_view room_id,
    nm_bytes_owned* out_json
);

nm_local_status nm_local_store_flush_json(
    nm_local_store* store,
    nm_bytes_view room_id,
    nm_bytes_owned* out_json
);

nm_local_status nm_local_store_append_nodes_json(
    nm_local_store* store,
    nm_bytes_view room_id,
    nm_bytes_view nodes_json,
    nm_bytes_owned* out_json
);

/** Decode a WS `pack` nodes base64 blob and append unpacked sync nodes. */
nm_local_status nm_local_store_append_pack_b64(
    nm_local_store* store,
    nm_bytes_view room_id,
    nm_bytes_view pack_nodes_b64,
    nm_bytes_owned* out_json
);

nm_local_status nm_local_store_canonical_hash_hex(
    nm_local_store* store,
    nm_bytes_view room_id,
    nm_bytes_owned* out_hex
);

void nm_bytes_owned_free(nm_bytes_owned bytes);

#ifdef __cplusplus
}
#endif

#endif
