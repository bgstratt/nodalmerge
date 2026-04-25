-- F7: PostgresNodeStore schema.
CREATE TABLE IF NOT EXISTS activesync_nodes (
    room_id    TEXT        NOT NULL,
    node_id    BYTEA       NOT NULL,
    seq        BIGSERIAL   PRIMARY KEY,
    bytes      BYTEA       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (room_id, node_id)
);

CREATE INDEX IF NOT EXISTS idx_activesync_nodes_room_seq
    ON activesync_nodes (room_id, seq);
