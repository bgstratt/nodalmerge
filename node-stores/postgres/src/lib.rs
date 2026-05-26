//! F7 — `PostgresNodeStore`: a `NodePersistence` adapter backed by Postgres
//! (sqlx 0.8, runtime-tokio-rustls).
//!
//! Schema lives in `migrations/`; call [`PostgresNodeStore::migrate`] once at
//! startup to apply. Hydration order is by `seq ASC`.
//!
//! Pair with any `BlobPersistence` via
//! `Composite::new(PostgresNodeStore::connect(...).await?, S3BlobStore::new(...)?)`.

use std::sync::Arc;
use std::time::Instant;

use nodalmerge_core::{pack_nodes, unpack_nodes, SyncNode};
use nodalmerge_server::store::NodePersistence;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use tokio::runtime::Runtime;

/// Embedded migrations applied by [`PostgresNodeStore::migrate`].
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum PostgresStoreError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("config: {0}")]
    Config(String),
}

#[derive(Debug, Clone)]
pub struct PostgresNodeStoreConfig {
    pub connection_uri: String,
    /// Default 20.
    pub max_connections: u32,
}

impl PostgresNodeStoreConfig {
    pub fn new(connection_uri: impl Into<String>) -> Self {
        Self { connection_uri: connection_uri.into(), max_connections: 20 }
    }
}

/// Sync `NodePersistence` impl wrapping an async sqlx pool.
///
/// Owns a private single-thread tokio `Runtime` so the sync trait methods
/// can drive async sqlx without blocking on the server's main runtime.
pub struct PostgresNodeStore {
    pool: PgPool,
    rt: Arc<Runtime>,
}

impl std::fmt::Debug for PostgresNodeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresNodeStore").finish_non_exhaustive()
    }
}

impl PostgresNodeStore {
    /// Connect using the given config. Does **not** run migrations — call
    /// [`Self::migrate`] explicitly so callers control schema lifecycle.
    pub fn connect(cfg: PostgresNodeStoreConfig) -> Result<Self, PostgresStoreError> {
        if cfg.connection_uri.is_empty() {
            return Err(PostgresStoreError::Config("connection_uri must not be empty".into()));
        }
        let rt = Runtime::new()
            .map_err(|e| PostgresStoreError::Config(format!("tokio runtime: {e}")))?;
        let pool = rt.block_on(async {
            PgPoolOptions::new()
                .max_connections(cfg.max_connections)
                .connect(&cfg.connection_uri)
                .await
        })?;
        Ok(Self { pool, rt: Arc::new(rt) })
    }

    /// Apply all embedded migrations. Idempotent; safe to call on every boot.
    pub fn migrate(&self) -> Result<(), PostgresStoreError> {
        let pool = self.pool.clone();
        self.rt.block_on(async move { MIGRATOR.run(&pool).await })?;
        Ok(())
    }

    /// Convenience: connect + migrate in one call. Mirrors `DirPersistence::open`.
    pub fn connect_and_migrate(
        cfg: PostgresNodeStoreConfig,
    ) -> Result<Self, PostgresStoreError> {
        let s = Self::connect(cfg)?;
        s.migrate()?;
        Ok(s)
    }
}

impl NodePersistence for PostgresNodeStore {
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode> {
        let pool = self.pool.clone();
        let rid = room_id.to_string();
        let rows: Result<Vec<Vec<u8>>, sqlx::Error> = self.rt.block_on(async move {
            let rows = sqlx::query(
                "SELECT bytes FROM nodalmerge_nodes \
                 WHERE room_id = $1 ORDER BY seq ASC",
            )
            .bind(&rid)
            .fetch_all(&pool)
            .await?;
            Ok(rows.into_iter().map(|r| r.get::<Vec<u8>, _>("bytes")).collect())
        });
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "postgres load_room_nodes failed");
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for bytes in rows {
            match unpack_nodes(&bytes) {
                Ok(mut ns) if ns.len() == 1 => out.push(ns.pop().unwrap()),
                Ok(_) => tracing::warn!("postgres row held != 1 nodes, skipping"),
                Err(e) => tracing::warn!(?e, "unpack persisted node failed"),
            }
        }
        out
    }

    fn persist_node(&self, room_id: &str, node: &SyncNode) {
        let t0 = Instant::now();
        let pool = self.pool.clone();
        let rid = room_id.to_string();
        let node_id = node.id.as_bytes().to_vec();
        let bytes = pack_nodes(&[node]);
        let res: Result<(), sqlx::Error> = self.rt.block_on(async move {
            sqlx::query(
                "INSERT INTO nodalmerge_nodes (room_id, node_id, bytes) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (room_id, node_id) DO NOTHING",
            )
            .bind(&rid)
            .bind(&node_id)
            .bind(&bytes)
            .execute(&pool)
            .await
            .map(|_| ())
        });
        if let Err(e) = res {
            tracing::warn!(?e, "postgres persist_node failed");
        }
        metrics::histogram!(
            "nodalmerge_persistence_write_seconds",
            "kind" => "node",
            "backend" => "postgres",
        )
        .record(t0.elapsed().as_secs_f64());
    }

    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        if nodes.is_empty() {
            return;
        }
        let t0 = Instant::now();
        // Pre-encode outside the runtime to keep the async window tight.
        let rid = room_id.to_string();
        let mut node_ids: Vec<Vec<u8>> = Vec::with_capacity(nodes.len());
        let mut bytes_col: Vec<Vec<u8>> = Vec::with_capacity(nodes.len());
        for n in nodes {
            node_ids.push(n.id.as_bytes().to_vec());
            bytes_col.push(pack_nodes(&[*n]));
        }
        let pool = self.pool.clone();
        let res: Result<(), sqlx::Error> = self.rt.block_on(async move {
            // UNNEST-based batched insert. Single round trip, single
            // transaction, ON CONFLICT preserves at-least-once safety.
            let room_ids: Vec<String> = std::iter::repeat(rid).take(node_ids.len()).collect();
            sqlx::query(
                "INSERT INTO nodalmerge_nodes (room_id, node_id, bytes) \
                 SELECT * FROM UNNEST($1::text[], $2::bytea[], $3::bytea[]) \
                 ON CONFLICT (room_id, node_id) DO NOTHING",
            )
            .bind(&room_ids)
            .bind(&node_ids)
            .bind(&bytes_col)
            .execute(&pool)
            .await
            .map(|_| ())
        });
        if let Err(e) = res {
            tracing::warn!(?e, "postgres persist_nodes failed");
        }
        metrics::histogram!(
            "nodalmerge_persistence_write_seconds",
            "kind" => "nodes_batch",
            "backend" => "postgres",
        )
        .record(t0.elapsed().as_secs_f64());
    }

    fn nodes_durable(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_uri_rejected() {
        let cfg = PostgresNodeStoreConfig::new("");
        assert!(matches!(
            PostgresNodeStore::connect(cfg),
            Err(PostgresStoreError::Config(_))
        ));
    }

    #[test]
    fn config_defaults() {
        let cfg = PostgresNodeStoreConfig::new("postgres://x");
        assert_eq!(cfg.max_connections, 20);
    }
}
