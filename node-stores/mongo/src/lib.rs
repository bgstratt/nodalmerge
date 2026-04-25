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

use activesync_core::{pack_nodes, unpack_nodes, SyncNode};
use activesync_server::store::NodePersistence;
use mongodb::bson::{self, doc, Binary, DateTime as BsonDateTime, Document};
use mongodb::options::{ClientOptions, FindOneAndUpdateOptions, FindOptions, IndexOptions, ReturnDocument};
use mongodb::{Client, Collection, IndexModel};
use tokio::runtime::Runtime;
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
    nodes: Collection<Document>,
    seq: Collection<Document>,
    rt: Arc<Runtime>,
}

impl std::fmt::Debug for MongoNodeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MongoNodeStore").finish_non_exhaustive()
    }
}

impl MongoNodeStore {
    /// Connect, ensure indexes. Idempotent; safe to call on every boot.
    pub fn connect(cfg: MongoNodeStoreConfig) -> Result<Self, MongoStoreError> {
        if cfg.connection_uri.is_empty() {
            return Err(MongoStoreError::Config("connection_uri must not be empty".into()));
        }
        if cfg.database.is_empty() {
            return Err(MongoStoreError::Config("database must not be empty".into()));
        }
        let rt = Arc::new(
            Runtime::new()
                .map_err(|e| MongoStoreError::Config(format!("tokio runtime: {e}")))?,
        );
        let cfg2 = cfg.clone();
        let (nodes, seq) = rt.block_on(async move {
            let opts = ClientOptions::parse(&cfg2.connection_uri).await?;
            let client = Client::with_options(opts)?;
            let db = client.database(&cfg2.database);
            let nodes: Collection<Document> = db.collection(&cfg2.collection);
            let seq: Collection<Document> = db.collection(&cfg2.seq_collection);

            // Index: ordered hydration by (room_id, seq).
            nodes
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "room_id": 1i32, "seq": 1i32 })
                        .options(IndexOptions::builder().name("room_seq".to_string()).build())
                        .build(),
                )
                .await?;

            Ok::<_, mongodb::error::Error>((nodes, seq))
        })?;

        Ok(Self { nodes, seq, rt })
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
        let nodes_coll = self.nodes.clone();
        let rid = room_id.to_string();
        let res: Result<Vec<Vec<u8>>, mongodb::error::Error> = self.rt.block_on(async move {
            let opts = FindOptions::builder().sort(doc! { "seq": 1i32 }).build();
            let mut cursor = nodes_coll
                .find(doc! { "room_id": &rid })
                .with_options(opts)
                .await?;
            let mut out: Vec<Vec<u8>> = Vec::new();
            while let Some(d) = cursor.try_next().await? {
                if let Ok(b) = d.get_binary_generic("bytes") {
                    out.push(b.clone());
                }
            }
            Ok(out)
        });
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
        let nodes_coll = self.nodes.clone();
        let seq_coll = self.seq.clone();
        let rid = room_id.to_string();
        let bytes = pack_nodes(&[node]);
        let node_clone = node.clone();
        let res: Result<(), mongodb::error::Error> = self.rt.block_on(async move {
            let seq = Self::reserve_seq(&seq_coll, &rid, 1).await?;
            let d = build_doc(&rid, &node_clone, seq, bytes);
            match nodes_coll.insert_one(d).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    // Duplicate key (idempotent re-insert) is success.
                    let s = e.to_string();
                    if s.contains("E11000") || s.contains("duplicate key") {
                        Ok(())
                    } else {
                        Err(e)
                    }
                }
            }
        });
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
        let nodes_coll = self.nodes.clone();
        let seq_coll = self.seq.clone();
        let rid = room_id.to_string();
        let n = nodes.len() as u64;

        // Pre-pack and clone owned data so the async block is 'static.
        let owned: Vec<(SyncNode, Vec<u8>)> = nodes
            .iter()
            .map(|n| ((*n).clone(), pack_nodes(&[*n])))
            .collect();

        let res: Result<(), mongodb::error::Error> = self.rt.block_on(async move {
            let first_seq = Self::reserve_seq(&seq_coll, &rid, n).await?;
            let docs: Vec<Document> = owned
                .into_iter()
                .enumerate()
                .map(|(i, (node, bytes))| build_doc(&rid, &node, first_seq + i as i64, bytes))
                .collect();
            // ordered=false so a duplicate key inside the batch doesn't
            // poison the rest. The driver returns BulkWriteError; we treat
            // duplicate-key entries as success.
            match nodes_coll
                .insert_many(docs)
                .ordered(false)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    let s = e.to_string();
                    if s.contains("E11000") || s.contains("duplicate key") {
                        Ok(())
                    } else {
                        Err(e)
                    }
                }
            }
        });
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

    #[test]
    fn empty_uri_rejected() {
        let cfg = MongoNodeStoreConfig::new("", "db");
        assert!(matches!(
            MongoNodeStore::connect(cfg),
            Err(MongoStoreError::Config(_))
        ));
    }

    #[test]
    fn empty_db_rejected() {
        let cfg = MongoNodeStoreConfig::new("mongodb://x", "");
        assert!(matches!(
            MongoNodeStore::connect(cfg),
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
