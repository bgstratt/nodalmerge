#ifndef ACTIVESYNC_HOST_H
#define ACTIVESYNC_HOST_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct as_host_engine as_host_engine;

typedef struct {
    const uint8_t* ptr;
    size_t len;
} as_bytes_view;

typedef struct {
    uint8_t* ptr;
    size_t len;
} as_bytes_owned;

typedef enum {
    AS_OK = 0,
    AS_ERR_INVALID_ARG = 1,
    AS_ERR_NOT_FOUND = 2,
    AS_ERR_AUTH = 3,
    AS_ERR_POLICY = 4,
    AS_ERR_PROTOCOL = 5,
    AS_ERR_INTERNAL = 255
} as_status;

uint32_t as_host_abi_version(void);

as_status as_host_engine_new(as_host_engine** out_engine);
as_status as_host_engine_free(as_host_engine* engine);

as_status as_host_submit_command(
    as_host_engine* engine,
    as_bytes_view command_bin,
    as_bytes_owned* out_events_bin
);

as_status as_host_submit_command_json(
    as_host_engine* engine,
    as_bytes_view command_json,
    as_bytes_owned* out_events_json
);

void as_bytes_owned_free(as_bytes_owned bytes);

#ifdef __cplusplus
}
#endif

#endif
