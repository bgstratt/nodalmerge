//! F7 — `MongoNodeStore`: a `NodePersistence` adapter backed by MongoDB
//! (mongodb 3.x async driver).
//!
//! # Canonical cross-runtime schema (plan S5)
//!
//! This store writes the same `accepted_nodes` document shape as the .NET
//! host's `MongoNodeStoreProvider` (see docs/PERSISTENCE_SCHEMA.md), so both
//! runtimes can share one database as a single source of truth:
//!
//! ```jsonc
//! {
//!   "_id":          "<room_id>:<node_id_hex>",  // deterministic; set on insert only
//!   "room_id":      "<room_id>",
//!   "node_id_hex":  "<64-hex node id>",         // or "pack:<sha256>" for .NET pack records
//!   "payload":      <BinData postcard pack (1..n nodes)>,
//!   "payload_kind": "pack",
//!   "causal_parent_node_ids": ["<hex>", ...],
//!   "frontier_hash_hex": null | "<hex>",
//!   "applied":      true,
//!   "is_tombstone": false,
//!   "accepted_at_utc": <ISODate>,               // hydration sort key (+ node_id_hex tiebreak)
//!   "eligible_for_compaction_at_utc": null | <ISODate>,
//!   "updated_at_utc": <ISODate>
//! }
//! ```
//!
//! Writes are `updateOne(filter: {room_id, node_id_hex}, {$set: ..,
//! $setOnInsert: {_id}}, upsert)` — idempotent, and legacy documents written
//! by earlier versions of either runtime (ObjectId `_id`, or this store's
//! old `bytes`/`seq` shape) are upgraded in place / still readable.

use std::sync::Arc;
use std::time::Instant;
use tracing::info;

fn redact_uri(u: &str) -> String {
    // Replace user:pass@ with ****:****@ or hide until '@'
    if let Some(idx) = u.find('@') {
        if let Some(scheme_end) = u.find("//") {
            // keep scheme (e.g., mongodb+srv://)
            let before = &u[..scheme_end + 2];
            let after = &u[idx + 1..];
            return format!("{}***@{}", before, after);
        }
    }
    u.to_string()
}

use nodalmerge_core::{pack_nodes, unpack_nodes, SyncNode};
use nodalmerge_server::store::NodePersistence;
use mongodb::bson::{self, doc, Binary, DateTime as BsonDateTime, Document};
use mongodb::error::ErrorKind;
use mongodb::options::{ClientOptions, FindOptions, IndexOptions};
use mongodb::{Client, Collection, IndexModel};
use tokio::runtime::Runtime;

// Use a shared tokio runtime owned by MongoNodeStore to bridge sync trait
// methods into async MongoDB operations without creating a runtime per call.
use futures_util::TryStreamExt;

#[derive(Debug, thiserror::Error)]
pub enum MongoStoreError {
    #[error("mongodb: {0}")]
    Mongo(#[from] mongodb::error::Error),
    #[error("config: {0}")]
    Config(String),
}

#[derive(Debug, Clone)]
pub struct MongoNodeStoreConfig {
    pub connection_uri: String,
    pub database: String,
    /// Default `"accepted_nodes"` — the canonical cross-runtime collection
    /// shared with the .NET host (docs/PERSISTENCE_SCHEMA.md).
    pub collection: String,
}

impl MongoNodeStoreConfig {
    pub fn new(connection_uri: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            connection_uri: connection_uri.into(),
            database: database.into(),
            collection: "accepted_nodes".into(),
        }
    }
}

pub struct MongoNodeStore {
    client: Client,
    client_id: u64,
    db_name: String,
    collection_name: String,
    rt: Arc<Runtime>,
}

impl std::fmt::Debug for MongoNodeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MongoNodeStore").finish_non_exhaustive()
    }
}

impl MongoNodeStore {
    /// Connect, ensure indexes. Idempotent; safe to call on every boot.
    pub async fn connect(cfg: MongoNodeStoreConfig) -> Result<Self, MongoStoreError> {
        if cfg.connection_uri.is_empty() {
            return Err(MongoStoreError::Config("connection_uri must not be empty".into()));
        }
        if cfg.database.is_empty() {
            return Err(MongoStoreError::Config("database must not be empty".into()));
        }
        let db_name = cfg.database.clone();
        let collection_name = cfg.collection.clone();
        let collection_name_for_init = collection_name.clone();
        let connection_uri = cfg.connection_uri.clone();
        let database_for_thread = cfg.database.clone();

        // Build the client using the current async runtime rather than
        // spawning an internal runtime from within `connect`.
        let client = {
            // Use the connection URI as provided; rely on the driver to
            // handle `mongodb+srv://` resolution and TXT options. Manual
            // expansion caused complexity and dependency fragility.
            let parsed_uri = connection_uri.clone();

            // Log the final URI we're about to hand to the driver (redacted)
            info!(uri = %redact_uri(&parsed_uri), "Mongo driver will parse connection URI");

            // Parse client options so we can set a sane selection policy
            // and a longer server selection timeout to tolerate transient
            // topology/pool readiness during warmup.
            let mut opts = ClientOptions::parse(&parsed_uri).await?;
            opts.server_selection_timeout = Some(std::time::Duration::from_secs(60));
            opts.retry_writes = Some(true);
            tracing::debug!(hosts = ?opts.hosts, server_selection_timeout = ?opts.server_selection_timeout, "parsed ClientOptions for Mongo");
            let client = Client::with_options(opts)?;
            let db = client.database(&database_for_thread);

            // Quick connectivity/auth check so runtime logs show whether
            // the client can reach and authenticate to the server.
            match db.run_command(doc! { "ping": 1 }).await {
                Ok(_) => info!("Mongo ping succeeded"),
                Err(e) => {
                    tracing::error!(?e, "Mongo ping failed");
                    return Err(MongoStoreError::Mongo(e));
                }
            }

            // Force primary readiness using a lightweight write; clean up
            // afterwards. This gives the client a chance to observe the
            // primary topology and establish the pool.
            let init_coll = client.database(&database_for_thread).collection::<Document>("nodalmerge_init_test");
            match init_coll.insert_one(doc! { "init": true }).await {
                Ok(_) => {
                    tracing::info!("Mongo init write succeeded");
                    let _ = init_coll.delete_many(doc! { "init": true }).await;
                }
                Err(e) => { tracing::warn!(?e, "Mongo init write failed (init test)"); }
            }

            // Ensure the canonical indexes exist — identical names/options to
            // the .NET MongoNodeStoreProvider so both runtimes can boot
            // against the same collection without index conflicts.
            let nodes: Collection<Document> = db.collection(&collection_name_for_init);
            nodes
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "room_id": 1i32, "node_id_hex": 1i32 })
                        .options(
                            IndexOptions::builder()
                                .unique(true)
                                .name("ux_room_node".to_string())
                                .build(),
                        )
                        .build(),
                )
                .await?;
            nodes
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "room_id": 1i32, "eligible_for_compaction_at_utc": 1i32 })
                        .options(
                            IndexOptions::builder()
                                .name("ix_room_compaction_eligibility".to_string())
                                .build(),
                        )
                        .build(),
                )
                .await?;
            client
        };

        let rt = Arc::new(Runtime::new().map_err(|e| MongoStoreError::Config(format!("tokio runtime: {e}")))?);
        use std::sync::atomic::{AtomicU64, Ordering};
        static CLIENT_COUNTER: AtomicU64 = AtomicU64::new(1);
        let cid = CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        tracing::info!(client_id = cid, "mongo client stored in MongoNodeStore");
        Ok(Self { client, client_id: cid, db_name, collection_name, rt })
    }

    /// Run a small test write (insert a transient doc) to verify connectivity
    /// and force the driver to select a primary.
    pub async fn test_write(&self) -> Result<(), MongoStoreError> {
        let client = self.client.clone();
        let db_name = self.db_name.clone();
        let coll = client.database(&db_name).collection::<Document>("nodalmerge_dev_startup_test");
        match coll.insert_one(doc! { "startup": true, "ts": bson::DateTime::now() }).await {
            Ok(_) => {
                let _ = coll.delete_many(doc! { "startup": true }).await;
                Ok(())
            }
            Err(e) => Err(MongoStoreError::Mongo(e)),
        }
    }

    /// Idempotent canonical-schema upsert for one node document.
    /// `$setOnInsert` gives fresh documents the deterministic compound `_id`
    /// while leaving legacy ObjectId documents (written by earlier .NET
    /// versions) untouched — the unique `(room_id, node_id_hex)` index is the
    /// real identity either way.
    async fn upsert_node_doc(
        nodes_coll: &Collection<Document>,
        room_id: &str,
        node_id_hex: &str,
        set_fields: Document,
    ) -> Result<(), mongodb::error::Error> {
        nodes_coll
            .update_one(
                doc! { "room_id": room_id, "node_id_hex": node_id_hex },
                doc! {
                    "$set": set_fields,
                    "$setOnInsert": {
                        "_id": doc_id(room_id, node_id_hex),
                        "accepted_at_utc": BsonDateTime::now(),
                    },
                },
            )
            .upsert(true)
            .await?;
        Ok(())
    }
}

fn doc_id(room_id: &str, node_id_hex: &str) -> String {
    format!("{room_id}:{node_id_hex}")
}

/// Canonical `$set` fields for a single-node record (docs/PERSISTENCE_SCHEMA.md).
/// `accepted_at_utc` is set on insert only (it's the hydration sort key and
/// must not move on idempotent re-writes); `updated_at_utc` always advances.
fn build_set_fields(room_id: &str, node: &SyncNode, payload: Vec<u8>) -> Document {
    let parents: Vec<String> = node.transaction.parents.iter().map(|h| h.to_hex()).collect();
    doc! {
        "room_id": room_id,
        "node_id_hex": node.id.to_hex(),
        "payload": Binary { subtype: bson::spec::BinarySubtype::Generic, bytes: payload },
        "payload_kind": "pack",
        "causal_parent_node_ids": parents,
        "frontier_hash_hex": bson::Bson::Null,
        "applied": true,
        "is_tombstone": false,
        "eligible_for_compaction_at_utc": bson::Bson::Null,
        "updated_at_utc": BsonDateTime::now(),
    }
}

impl NodePersistence for MongoNodeStore {
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode> {
        tracing::debug!(client_id = self.client_id, "mongo load_room_nodes using client");
        let nodes_coll = self.client.database(&self.db_name).collection::<Document>(&self.collection_name);
        let rid = room_id.to_string();

        let rt = self.rt.clone();
        let handle = std::thread::spawn(move || {
            rt.block_on(async move {
                // Canonical hydration order, matching the .NET provider's
                // LoadRoomSnapshotAsync: accepted_at_utc then node_id_hex.
                // Documents without accepted_at_utc (legacy Rust `bytes`/`seq`
                // shape) sort first, which is correct — they predate the
                // canonical schema.
                let opts = FindOptions::builder()
                    .sort(doc! { "accepted_at_utc": 1i32, "node_id_hex": 1i32 })
                    .build();
                let mut attempt = 0u8;
                loop {
                    match nodes_coll.find(doc! { "room_id": &rid }).with_options(opts.clone()).await {
                        Ok(mut cursor) => {
                            let mut out: Vec<Vec<u8>> = Vec::new();
                            while let Some(d) = cursor.try_next().await? {
                                // Canonical field first, legacy fallback for
                                // documents written before S5.
                                if let Ok(b) = d.get_binary_generic("payload") {
                                    out.push(b.clone());
                                } else if let Ok(b) = d.get_binary_generic("bytes") {
                                    out.push(b.clone());
                                }
                            }
                            break Ok(out);
                        }
                        Err(e) if matches!(*e.kind, ErrorKind::ServerSelection { .. }) && attempt < 2 => {
                            attempt += 1;
                            tracing::warn!(error = ?e, attempt = attempt, "read retry after ServerSelection");
                            tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                            continue;
                        }
                        Err(e) => break Err(e),
                    }
                }
            })
        });
        let res: Result<Vec<Vec<u8>>, mongodb::error::Error> = match handle.join() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "mongo thread join failed");
                return Vec::new();
            }
        };

        let raw = match res {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "mongo load_room_nodes failed");
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(raw.len());
        for bytes in raw {
            match unpack_nodes(&bytes) {
                // Canonical payloads are packs of 1..n nodes: this store
                // writes single-node packs, the .NET host writes whole
                // inbound/snapshot packs — accept both.
                Ok(ns) => out.extend(ns),
                Err(e) => tracing::warn!(?e, "unpack persisted node payload failed"),
            }
        }
        out
    }

    fn persist_node(&self, room_id: &str, node: &SyncNode) {
        let t0 = Instant::now();
        tracing::debug!(client_id = self.client_id, "mongo persist_node using client");
        let nodes_coll = self.client.database(&self.db_name).collection::<Document>(&self.collection_name);
        let rid = room_id.to_string();
        let payload = pack_nodes(&[node]);
        let node_id_hex = node.id.to_hex();
        let set_fields = build_set_fields(room_id, node, payload);
        let rt = self.rt.clone();
        let handle = std::thread::spawn(move || {
            rt.block_on(async move {
                let mut attempt = 0u8;
                loop {
                    match Self::upsert_node_doc(&nodes_coll, &rid, &node_id_hex, set_fields.clone()).await {
                        Ok(()) => break Ok(()),
                        Err(e) if matches!(*e.kind, ErrorKind::ServerSelection { .. }) && attempt < 2 => {
                            attempt += 1;
                            tracing::warn!(error = ?e, attempt = attempt, "write retry after ServerSelection");
                            tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                            continue;
                        }
                        Err(e) => break Err(e),
                    }
                }
            })
        });
        let res: Result<(), mongodb::error::Error> = match handle.join() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "mongo thread join failed");
                Err(mongodb::error::Error::from(std::io::Error::new(std::io::ErrorKind::Other, "thread join failed")))
            }
        };

        if let Err(e) = res {
            tracing::warn!(?e, "mongo persist_node failed");
        }
        metrics::histogram!(
            "nodalmerge_persistence_write_seconds",
            "kind" => "node",
            "backend" => "mongo",
        )
        .record(t0.elapsed().as_secs_f64());
    }

    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        if nodes.is_empty() {
            return;
        }
        let t0 = Instant::now();
        tracing::debug!(client_id = self.client_id, "mongo persist_nodes using client");
        let nodes_coll = self.client.database(&self.db_name).collection::<Document>(&self.collection_name);
        let rid = room_id.to_string();

        let owned: Vec<(String, Document)> = nodes
            .iter()
            .map(|n| {
                (
                    n.id.to_hex(),
                    build_set_fields(room_id, n, pack_nodes(&[*n])),
                )
            })
            .collect();

        let rt = self.rt.clone();
        let handle = std::thread::spawn(move || {
            rt.block_on(async move {
                for (node_id_hex, set_fields) in owned {
                    let mut attempt = 0u8;
                    loop {
                        match Self::upsert_node_doc(&nodes_coll, &rid, &node_id_hex, set_fields.clone()).await {
                            Ok(()) => break,
                            Err(e) if matches!(*e.kind, ErrorKind::ServerSelection { .. }) && attempt < 2 => {
                                attempt += 1;
                                tracing::warn!(error = ?e, attempt = attempt, "batch write retry after ServerSelection");
                                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                                continue;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                Ok(())
            })
        });
        let res: Result<(), mongodb::error::Error> = match handle.join() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "mongo thread join failed");
                Err(mongodb::error::Error::from(std::io::Error::new(std::io::ErrorKind::Other, "thread join failed")))
            }
        };

        if let Err(e) = res {
            tracing::warn!(?e, "mongo persist_nodes failed");
        }
        metrics::histogram!(
            "nodalmerge_persistence_write_seconds",
            "kind" => "nodes_batch",
            "backend" => "mongo",
        )
        .record(t0.elapsed().as_secs_f64());
    }

    fn nodes_durable(&self) -> bool {
        true
    }

    /// blob-cas-remediation.md slice 1.1 (finding #1) — real enumeration
    /// via a `distinct("room_id")`, so the global blob GC sweep
    /// (`Rooms::sweep_blobs`) can protect blobs owned by cold/non-resident
    /// rooms instead of the trait's unsafe empty default.
    fn known_room_ids(&self) -> Vec<String> {
        tracing::debug!(client_id = self.client_id, "mongo known_room_ids using client");
        let nodes_coll = self.client.database(&self.db_name).collection::<Document>(&self.collection_name);
        let rt = self.rt.clone();
        let handle = std::thread::spawn(move || {
            rt.block_on(async move {
                let mut attempt = 0u8;
                loop {
                    match nodes_coll.distinct("room_id", doc! {}).await {
                        Ok(values) => {
                            break Ok(values
                                .into_iter()
                                .filter_map(|b| b.as_str().map(|s| s.to_string()))
                                .collect::<Vec<String>>());
                        }
                        Err(e) if matches!(*e.kind, ErrorKind::ServerSelection { .. }) && attempt < 2 => {
                            attempt += 1;
                            tracing::warn!(error = ?e, attempt = attempt, "distinct retry after ServerSelection");
                            tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                            continue;
                        }
                        Err(e) => break Err(e),
                    }
                }
            })
        });
        let res: Result<Vec<String>, mongodb::error::Error> = match handle.join() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "mongo thread join failed");
                return Vec::new();
            }
        };
        match res {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(?e, "mongo known_room_ids failed");
                Vec::new()
            }
        }
    }

    /// See [`Self::known_room_ids`] — this backend genuinely enumerates.
    fn can_enumerate_rooms(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_uri_rejected() {
        let cfg = MongoNodeStoreConfig::new("", "db");
        assert!(matches!(
            MongoNodeStore::connect(cfg).await,
            Err(MongoStoreError::Config(_))
        ));
    }

    #[tokio::test]
    async fn empty_db_rejected() {
        let cfg = MongoNodeStoreConfig::new("mongodb://x", "");
        assert!(matches!(
            MongoNodeStore::connect(cfg).await,
            Err(MongoStoreError::Config(_))
        ));
    }

    #[test]
    fn config_defaults() {
        let cfg = MongoNodeStoreConfig::new("mongodb://x", "db");
        assert_eq!(cfg.collection, "accepted_nodes");
    }

    #[test]
    fn doc_id_format() {
        assert_eq!(doc_id("r1", "abcd"), "r1:abcd");
    }
}
