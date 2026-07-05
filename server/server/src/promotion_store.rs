//! Durable promotion proposal store (topology Wave 3).
//!
//! When the server runs with `--store`, proposals survive process restart in
//! `{store_root}/topology-promotions.db`. In-memory-only servers use a volatile
//! map (same semantics as pre-Wave-3, lost on restart).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{params, Connection};
use tokio::sync::{Mutex, RwLock};

use crate::promotion::PromotionRecord;

#[derive(Clone)]
pub struct PromotionStoreHandle {
    inner: Arc<PromotionStoreInner>,
}

enum PromotionStoreInner {
    Volatile(RwLock<HashMap<String, PromotionRecord>>),
    Durable(Mutex<Connection>),
}

impl PromotionStoreHandle {
    /// `store_root` is `Some` when [`crate::store::DirPersistence`] backs the server.
    pub fn open(store_root: Option<PathBuf>) -> Self {
        let inner = match store_root {
            Some(root) => {
                std::fs::create_dir_all(&root).ok();
                let db_path = root.join("topology-promotions.db");
                let conn = Connection::open(&db_path)
                    .unwrap_or_else(|e| panic!("open topology-promotions.db: {e}"));
                conn.pragma_update(None, "journal_mode", "WAL").ok();
                conn.pragma_update(None, "synchronous", "NORMAL").ok();
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS promotion_proposals (
                       proposal_id TEXT PRIMARY KEY,
                       record_json TEXT NOT NULL
                     );",
                )
                .unwrap_or_else(|e| panic!("init promotion_proposals schema: {e}"));
                PromotionStoreInner::Durable(Mutex::new(conn))
            }
            None => PromotionStoreInner::Volatile(RwLock::new(HashMap::new())),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn is_durable(&self) -> bool {
        matches!(&*self.inner, PromotionStoreInner::Durable(_))
    }

    pub async fn get(&self, proposal_id: &str) -> Option<PromotionRecord> {
        match &*self.inner {
            PromotionStoreInner::Volatile(map) => map.read().await.get(proposal_id).cloned(),
            PromotionStoreInner::Durable(conn) => {
                let conn = conn.lock().await;
                load_sqlite(&conn, proposal_id)
            }
        }
    }

    pub async fn insert(&self, proposal_id: &str, record: PromotionRecord) {
        match &*self.inner {
            PromotionStoreInner::Volatile(map) => {
                map.write().await.insert(proposal_id.to_string(), record);
            }
            PromotionStoreInner::Durable(conn) => {
                let conn = conn.lock().await;
                let json = serde_json::to_string(&record).expect("PromotionRecord serializes");
                save_sqlite(&conn, proposal_id, &json);
            }
        }
    }

    pub async fn update(&self, proposal_id: &str, record: PromotionRecord) {
        self.insert(proposal_id, record).await;
    }
}

fn load_sqlite(conn: &Connection, proposal_id: &str) -> Option<PromotionRecord> {
    let mut stmt = conn
        .prepare("SELECT record_json FROM promotion_proposals WHERE proposal_id = ?1")
        .ok()?;
    let json: String = stmt
        .query_row(params![proposal_id], |row| row.get(0))
        .ok()?;
    serde_json::from_str(&json).ok()
}

fn save_sqlite(conn: &Connection, proposal_id: &str, json: &str) {
    if let Err(e) = conn.execute(
        "INSERT OR REPLACE INTO promotion_proposals (proposal_id, record_json) VALUES (?1, ?2)",
        params![proposal_id, json],
    ) {
        tracing::warn!(?e, proposal_id, "promotion_store save failed");
    }
}
