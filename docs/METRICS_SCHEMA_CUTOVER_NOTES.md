# Metrics and Schema Hard Cutover Notes

## Scope

This change removes legacy `activesync_*` compatibility aliases in runtime telemetry and persistence schema naming.

## Metrics Rename

All server and storage metrics now emit **only** `nodalmerge_*` names.
Examples:

- `activesync_rooms_total` -> `nodalmerge_rooms_total`
- `activesync_merge_batch_seconds` -> `nodalmerge_merge_batch_seconds`
- `activesync_persistence_write_seconds` -> `nodalmerge_persistence_write_seconds`
- `activesync_filtered_nodes_total` -> `nodalmerge_filtered_nodes_total`

## Data Schema Rename

### PostgreSQL

- table: `activesync_nodes` -> `nodalmerge_nodes`
- index: `idx_activesync_nodes_room_seq` -> `idx_nodalmerge_nodes_room_seq`
- test DB defaults: `activesync_test` -> `nodalmerge_test`

### MongoDB

Default collection names changed:

- `activesync_nodes` -> `nodalmerge_nodes`
- `activesync_seq` -> `nodalmerge_seq`

Dev/test helper collections similarly moved to `nodalmerge_*` names.

### SQLite (DirPersistence)

- file: `activesync.db` -> `nodalmerge.db`

## Migration Guidance

Existing deployments must migrate historical data before rolling out this cutover.

1. PostgreSQL: rename table/index (or copy-forward) and validate row counts.
2. MongoDB: copy/rename node + sequence collections; preserve `_id` and `seq` ordering semantics.
3. SQLite: rename `activesync.db` to `nodalmerge.db` or export/import content.
4. Monitoring: update dashboards and alerts from `activesync_*` to `nodalmerge_*`.

No runtime dual-read/dual-write compatibility path remains after this change.