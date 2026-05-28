//! Durable lineage metadata store (topology Wave 3 follow-up).
//!
//! With `--store`, lineage metadata survives restart in
//! `{store_root}/topology-lineage.db`. In-memory servers keep a volatile map.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use nodalmerge_core::RoomLineage;
use rusqlite::{params, Connection};
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
pub struct LineageStoreHandle {
    inner: Arc<LineageStoreInner>,
}

enum LineageStoreInner {
    Volatile(RwLock<HashMap<String, RoomLineage>>),
    Durable(Mutex<Connection>),
}

impl LineageStoreHandle {
    /// `store_root` is `Some` when [`crate::store::DirPersistence`] backs the server.
    pub fn open(store_root: Option<PathBuf>) -> Self {
        let inner = match store_root {
            Some(root) => {
                std::fs::create_dir_all(&root).ok();
                let db_path = root.join("topology-lineage.db");
                let conn = Connection::open(&db_path)
                    .unwrap_or_else(|e| panic!("open topology-lineage.db: {e}"));
                conn.pragma_update(None, "journal_mode", "WAL").ok();
                conn.pragma_update(None, "synchronous", "NORMAL").ok();
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS room_lineage (
                       room_id TEXT PRIMARY KEY,
                       parent_room_id TEXT NOT NULL,
                       lineage_json TEXT NOT NULL
                     );
                     CREATE INDEX IF NOT EXISTS idx_room_lineage_parent ON room_lineage(parent_room_id);",
                )
                .unwrap_or_else(|e| panic!("init room_lineage schema: {e}"));
                LineageStoreInner::Durable(Mutex::new(conn))
            }
            None => LineageStoreInner::Volatile(RwLock::new(HashMap::new())),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn is_durable(&self) -> bool {
        matches!(&*self.inner, LineageStoreInner::Durable(_))
    }

    pub async fn get_lineage(&self, room_id: &str) -> Option<RoomLineage> {
        match &*self.inner {
            LineageStoreInner::Volatile(map) => map.read().await.get(room_id).cloned(),
            LineageStoreInner::Durable(conn) => {
                let conn = conn.lock().await;
                let mut stmt = conn
                    .prepare("SELECT lineage_json FROM room_lineage WHERE room_id = ?1")
                    .ok()?;
                let json: String = stmt.query_row(params![room_id], |row| row.get(0)).ok()?;
                serde_json::from_str(&json).ok()
            }
        }
    }

    pub async fn list_children(&self, parent_room_id: &str) -> Vec<String> {
        match &*self.inner {
            LineageStoreInner::Volatile(map) => {
                let map = map.read().await;
                map.iter()
                    .filter_map(|(room_id, lineage)| {
                        if lineage.parent_room_id == parent_room_id {
                            Some(room_id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            LineageStoreInner::Durable(conn) => {
                let conn = conn.lock().await;
                let mut stmt = match conn
                    .prepare("SELECT room_id FROM room_lineage WHERE parent_room_id = ?1")
                {
                    Ok(stmt) => stmt,
                    Err(_) => return Vec::new(),
                };
                let rows = match stmt.query_map(params![parent_room_id], |row| row.get::<_, String>(0))
                {
                    Ok(rows) => rows,
                    Err(_) => return Vec::new(),
                };
                rows.flatten().collect()
            }
        }
    }

    pub async fn upsert_lineage(&self, room_id: &str, lineage: RoomLineage) {
        match &*self.inner {
            LineageStoreInner::Volatile(map) => {
                map.write().await.insert(room_id.to_string(), lineage);
            }
            LineageStoreInner::Durable(conn) => {
                let conn = conn.lock().await;
                let json = serde_json::to_string(&lineage).expect("RoomLineage serializes");
                if let Err(e) = conn.execute(
                    "INSERT OR REPLACE INTO room_lineage (room_id, parent_room_id, lineage_json) VALUES (?1, ?2, ?3)",
                    params![room_id, lineage.parent_room_id, json],
                ) {
                    tracing::warn!(?e, room_id, "lineage_store save failed");
                }
            }
        }
    }
}
