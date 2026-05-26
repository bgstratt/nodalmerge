//! F7 — `MongoNodeStore`: a `NodePersistence` adapter backed by MongoDB
//! (mongodb 3.x async driver).
//!
//! Document shape (collection `activesync_nodes`):
//!
//! ```jsonc
//! {
//!   "_id":     "<room_id>:<node_id_hex>",   // compound key, idempotent re-insert
//!   "room_id": "<room_id>",                 // indexed
//!   "node_id": <BinData 32>,
//!   "seq":     <i64 from per-room counter>, // ordered hydration
//!   "bytes":   <BinData postcard SyncNode>,
//!   "created_at": <ISODate>
//! }
//! ```
//!
//! Per-batch sequence allocation uses a counter collection (`activesync_seq`)
//! with `findOneAndUpdate $inc` — one round trip per `persist_nodes` call,
//! not per node.

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
use mongodb::options::{ClientOptions, FindOneAndUpdateOptions, FindOptions, IndexOptions, ReturnDocument};
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
    /// Default `"activesync_nodes"`.
    pub collection: String,
    /// Default `"activesync_seq"`.
    pub seq_collection: String,
}

impl MongoNodeStoreConfig {
    pub fn new(connection_uri: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            connection_uri: connection_uri.into(),
            database: database.into(),
            collection: "activesync_nodes".into(),
            seq_collection: "activesync_seq".into(),
        }
    }
}

pub struct MongoNodeStore {
    client: Client,
    client_id: u64,
    db_name: String,
    collection_name: String,
    seq_collection_name: String,
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
        let seq_collection_name = cfg.seq_collection.clone();
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
            let init_coll = client.database(&database_for_thread).collection::<Document>("activesync_init_test");
            match init_coll.insert_one(doc! { "init": true }).await {
                Ok(_) => {
                    tracing::info!("Mongo init write succeeded");
                    let _ = init_coll.delete_many(doc! { "init": true }).await;
                }
                Err(e) => { tracing::warn!(?e, "Mongo init write failed (init test)"); }
            }

            // Ensure the nodestore index exists.
            let nodes: Collection<Document> = db.collection(&collection_name_for_init);
            nodes
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "room_id": 1i32, "seq": 1i32 })
                        .options(IndexOptions::builder().name("room_seq".to_string()).build())
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
        Ok(Self { client, client_id: cid, db_name, collection_name, seq_collection_name, rt })
    }

    /// Run a small test write (insert a transient doc) to verify connectivity
    /// and force the driver to select a primary.
    pub async fn test_write(&self) -> Result<(), MongoStoreError> {
        let client = self.client.clone();
        let db_name = self.db_name.clone();
        let coll = client.database(&db_name).collection::<Document>("activesync_dev_startup_test");
        match coll.insert_one(doc! { "startup": true, "ts": bson::DateTime::now() }).await {
            Ok(_) => {
                let _ = coll.delete_many(doc! { "startup": true }).await;
                Ok(())
            }
            Err(e) => Err(MongoStoreError::Mongo(e)),
        }
    }

    /// Reserve `count` consecutive sequence numbers for `room_id`. Returns
    /// the first allocated `seq`; nodes use `first_seq + i`.
    async fn reserve_seq(
        seq_coll: &Collection<Document>,
        room_id: &str,
        count: u64,
    ) -> Result<i64, mongodb::error::Error> {
        let opts = FindOneAndUpdateOptions::builder()
            .upsert(true)
            .return_document(ReturnDocument::After)
            .build();
        let updated = seq_coll
            .find_one_and_update(
                doc! { "_id": room_id },
                doc! { "$inc": { "next": count as i64 } },
            )
            .with_options(opts)
            .await?;
        let after_next = updated
            .and_then(|d| d.get_i64("next").ok())
            .unwrap_or(count as i64);
        Ok(after_next - count as i64)
    }
}

fn doc_id(room_id: &str, node_id_hex: &str) -> String {
    format!("{room_id}:{node_id_hex}")
}

fn build_doc(
    room_id: &str,
    node: &SyncNode,
    seq: i64,
    bytes: Vec<u8>,
) -> Document {
    let node_id_bytes = node.id.as_bytes().to_vec();
    let node_id_hex = node.id.to_hex();
    doc! {
        "_id":     doc_id(room_id, &node_id_hex),
        "room_id": room_id,
        "node_id": Binary { subtype: bson::spec::BinarySubtype::Generic, bytes: node_id_bytes },
        "seq":     seq,
        "bytes":   Binary { subtype: bson::spec::BinarySubtype::Generic, bytes },
        "created_at": BsonDateTime::now(),
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
                let opts = FindOptions::builder().sort(doc! { "seq": 1i32 }).build();
                let mut attempt = 0u8;
                loop {
                    match nodes_coll.find(doc! { "room_id": &rid }).with_options(opts.clone()).await {
                        Ok(mut cursor) => {
                            let mut out: Vec<Vec<u8>> = Vec::new();
                            while let Some(d) = cursor.try_next().await? {
                                if let Ok(b) = d.get_binary_generic("bytes") {
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
                Ok(mut ns) if ns.len() == 1 => out.push(ns.pop().unwrap()),
                Ok(_) => tracing::warn!("mongo doc held != 1 nodes, skipping"),
                Err(e) => tracing::warn!(?e, "unpack persisted node failed"),
            }
        }
        out
    }

    fn persist_node(&self, room_id: &str, node: &SyncNode) {
        let t0 = Instant::now();
        tracing::debug!(client_id = self.client_id, "mongo persist_node using client");
        let nodes_coll = self.client.database(&self.db_name).collection::<Document>(&self.collection_name);
        let seq_coll = self.client.database(&self.db_name).collection::<Document>(&self.seq_collection_name);
        let rid = room_id.to_string();
        let bytes = pack_nodes(&[node]);
        let node_clone = node.clone();
        let rt = self.rt.clone();
        let handle = std::thread::spawn(move || {
            rt.block_on(async move {
                let seq = Self::reserve_seq(&seq_coll, &rid, 1).await?;
                let d = build_doc(&rid, &node_clone, seq, bytes);
                let mut attempt = 0u8;
                loop {
                    match nodes_coll.insert_one(d.clone()).await {
                        Ok(_) => break Ok(()),
                        Err(e) if matches!(*e.kind, ErrorKind::ServerSelection { .. }) && attempt < 2 => {
                            attempt += 1;
                            tracing::warn!(error = ?e, attempt = attempt, "write retry after ServerSelection");
                            tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                            continue;
                        }
                        Err(e) => {
                            let s = e.to_string();
                            if s.contains("E11000") || s.contains("duplicate key") {
                                break Ok(());
                            }
                            break Err(e);
                        }
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
            "activesync_persistence_write_seconds",
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
        let seq_coll = self.client.database(&self.db_name).collection::<Document>(&self.seq_collection_name);
        let rid = room_id.to_string();
        let n = nodes.len() as u64;

        let owned: Vec<(SyncNode, Vec<u8>)> = nodes
            .iter()
            .map(|n| ((*n).clone(), pack_nodes(&[*n])))
            .collect();

        let rt = self.rt.clone();
        let handle = std::thread::spawn(move || {
            rt.block_on(async move {
                let first_seq = Self::reserve_seq(&seq_coll, &rid, n).await?;
                let docs: Vec<Document> = owned
                    .into_iter()
                    .enumerate()
                    .map(|(i, (node, bytes))| build_doc(&rid, &node, first_seq + i as i64, bytes))
                    .collect();

                let mut attempt = 0u8;
                loop {
                    match nodes_coll.insert_many(docs.clone()).ordered(false).await {
                        Ok(_) => break Ok(()),
                        Err(e) if matches!(*e.kind, ErrorKind::ServerSelection { .. }) && attempt < 2 => {
                            attempt += 1;
                            tracing::warn!(error = ?e, attempt = attempt, "batch write retry after ServerSelection");
                            tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                            continue;
                        }
                        Err(e) => {
                            let s = e.to_string();
                            if s.contains("E11000") || s.contains("duplicate key") {
                                break Ok(());
                            }
                            break Err(e);
                        }
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
            tracing::warn!(?e, "mongo persist_nodes failed");
        }
        metrics::histogram!(
            "activesync_persistence_write_seconds",
            "kind" => "nodes_batch",
            "backend" => "mongo",
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
        assert_eq!(cfg.collection, "activesync_nodes");
        assert_eq!(cfg.seq_collection, "activesync_seq");
    }

    #[test]
    fn doc_id_format() {
        assert_eq!(doc_id("r1", "abcd"), "r1:abcd");
    }
}
