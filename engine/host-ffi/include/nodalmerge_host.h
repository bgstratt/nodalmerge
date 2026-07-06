#ifndef NODALMERGE_HOST_H
#define NODALMERGE_HOST_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct nm_host_engine nm_host_engine;

typedef struct {
    const uint8_t* ptr;
    size_t len;
} nm_bytes_view;

typedef struct {
    uint8_t* ptr;
    size_t len;
} nm_bytes_owned;

typedef enum {
    NM_HOST_OK = 0,
    NM_HOST_ERR_INVALID_ARG = 1,
    NM_HOST_ERR_NOT_FOUND = 2,
    NM_HOST_ERR_AUTH = 3,
    NM_HOST_ERR_POLICY = 4,
    NM_HOST_ERR_PROTOCOL = 5,
    NM_HOST_ERR_INTERNAL = 255
} nm_host_status;

uint32_t nm_host_abi_version(void);

nm_host_status nm_host_engine_new(nm_host_engine** out_engine);
nm_host_status nm_host_engine_free(nm_host_engine* engine);

nm_host_status nm_host_submit_command(
    nm_host_engine* engine,
    nm_bytes_view command_bin,
    nm_bytes_owned* out_events_bin
);

nm_host_status nm_host_submit_command_json(
    nm_host_engine* engine,
    nm_bytes_view command_json,
    nm_bytes_owned* out_events_json
);

void nm_bytes_owned_free(nm_bytes_owned bytes);

#ifdef __cplusplus
}
#endif

#endif
