-- F7: PostgresNodeStore schema.
CREATE TABLE IF NOT EXISTS nodalmerge_nodes (
    room_id    TEXT        NOT NULL,
    node_id    BYTEA       NOT NULL,
    seq        BIGSERIAL   PRIMARY KEY,
    bytes      BYTEA       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (room_id, node_id)
);

CREATE INDEX IF NOT EXISTS idx_nodalmerge_nodes_room_seq
    ON nodalmerge_nodes (room_id, seq);
