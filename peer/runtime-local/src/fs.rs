//! Durable peer-local persistence on a single-host directory (SQLite node log + blob files).
//!
//! Layout mirrors server `DirPersistence` (SQLite node log + blob dir) but implements
//! the headless **peer-local** contract, not server-side room hydration.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nodalmerge_core::{pack_nodes, unpack_nodes, Hash, SyncNode};
use rusqlite::{params, Connection};

use crate::error::{LocalPersistError, LocalPersistReason, LocalPersistResult};
use crate::util::sanitize_room_id;
use crate::report::{
    AppendReport, CheckpointMeta, CheckpointReport, FlushReport, HydrateReport, NodeLogTail,
    RecoveryReport,
};
use crate::traits::PeerLocalPersistence;

const SCHEMA_VERSION: i64 = 1;

/// On-disk peer-local store rooted at `data_dir`.
///
/// Survives process restart when reopened at the same path (`is_durable() == true`).
#[derive(Debug)]
pub struct FileLocalPersistence {
    data_dir: PathBuf,
    conn: Mutex<Connection>,
    read_only: bool,
}

impl FileLocalPersistence {
    pub fn open(data_dir: impl AsRef<Path>) -> LocalPersistResult<Self> {
        Self::open_with_mode(data_dir, false)
    }

    pub fn open_read_only(data_dir: impl AsRef<Path>) -> LocalPersistResult<Self> {
        Self::open_with_mode(data_dir, true)
    }

    fn open_with_mode(data_dir: impl AsRef<Path>, read_only: bool) -> LocalPersistResult<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        if !read_only {
            std::fs::create_dir_all(&data_dir).map_err(io_unavailable)?;
            std::fs::create_dir_all(data_dir.join("blobs")).map_err(io_unavailable)?;
        }

        let db_path = data_dir.join("local-persist.db");
        let conn = Connection::open(&db_path).map_err(io_unavailable)?;
        if read_only {
            conn.execute_batch("PRAGMA query_only = ON;")
                .map_err(io_corruption)?;
        } else {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(io_corruption)?;
            conn.pragma_update(None, "synchronous", "NORMAL")
                .map_err(io_corruption)?;
            Self::ensure_schema(&conn)?;
        }

        Ok(Self {
            data_dir,
            conn: Mutex::new(conn),
            read_only,
        })
    }

    fn ensure_schema(conn: &Connection) -> LocalPersistResult<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
               version INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS room_state (
               room_id TEXT PRIMARY KEY,
               tail_seq INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS nodes (
               room_id TEXT NOT NULL,
               node_id BLOB NOT NULL,
               bytes   BLOB NOT NULL,
               seq     INTEGER PRIMARY KEY AUTOINCREMENT,
               UNIQUE(room_id, node_id)
             );
             CREATE INDEX IF NOT EXISTS idx_nodes_room ON nodes(room_id, seq);
             CREATE TABLE IF NOT EXISTS checkpoints (
               room_id TEXT NOT NULL,
               cp_seq INTEGER NOT NULL,
               canonical_hash BLOB,
               recorded_at_seq INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_checkpoints_room ON checkpoints(room_id, cp_seq);",
        )
        .map_err(io_corruption)?;

        let version: Option<i64> = conn
            .query_row(
                "SELECT version FROM schema_meta LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok();

        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_meta (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )
                .map_err(io_corruption)?;
            }
            Some(v) if v == SCHEMA_VERSION => {}
            Some(_) => {
                return Err(LocalPersistError::new(
                    LocalPersistReason::VersionSkew,
                    "local-persist.db schema version mismatch",
                ));
            }
        }
        Ok(())
    }

    fn blobs_dir_for(&self, room_id: &str) -> PathBuf {
        self.data_dir.join("blobs").join(sanitize_room_id(room_id))
    }

    fn tail_for(&self, conn: &Connection, room_id: &str) -> LocalPersistResult<u64> {
        let tail: Option<i64> = conn
            .query_row(
                "SELECT tail_seq FROM room_state WHERE room_id = ?1",
                params![room_id],
                |r| r.get(0),
            )
            .ok();
        Ok(tail.unwrap_or(0).max(0) as u64)
    }

    fn load_nodes(conn: &Connection, room_id: &str) -> LocalPersistResult<Vec<SyncNode>> {
        let mut stmt = conn
            .prepare("SELECT bytes FROM nodes WHERE room_id = ?1 ORDER BY seq ASC")
            .map_err(io_corruption)?;
        let rows = stmt
            .query_map(params![room_id], |r| r.get::<_, Vec<u8>>(0))
            .map_err(io_corruption)?;
        let mut out = Vec::new();
        for row in rows.flatten() {
            match unpack_nodes(&row) {
                Ok(mut ns) if ns.len() == 1 => out.push(ns.pop().unwrap()),
                Ok(_) => {
                    return Err(LocalPersistError::new(
                        LocalPersistReason::Corruption,
                        "persisted row held != 1 node",
                    ));
                }
                Err(e) => {
                    return Err(LocalPersistError::new(
                        LocalPersistReason::Corruption,
                        format!("unpack persisted node failed: {e}"),
                    ));
                }
            }
        }
        Ok(out)
    }

    fn load_checkpoints(conn: &Connection, room_id: &str) -> LocalPersistResult<Vec<CheckpointMeta>> {
        let mut stmt = conn
            .prepare(
                "SELECT cp_seq, canonical_hash FROM checkpoints WHERE room_id = ?1 ORDER BY cp_seq ASC",
            )
            .map_err(io_corruption)?;
        let rows = stmt
            .query_map(params![room_id], |r| {
                let seq: i64 = r.get(0)?;
                let hash_bytes: Option<Vec<u8>> = r.get(1)?;
                Ok((seq, hash_bytes))
            })
            .map_err(io_corruption)?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, hash_bytes) = row.map_err(io_corruption)?;
            let canonical_hash = hash_bytes
                .map(|b| {
                    let arr: [u8; 32] = b.try_into().map_err(|_| {
                        LocalPersistError::new(LocalPersistReason::Corruption, "bad hash len")
                    })?;
                    Ok(Hash(arr))
                })
                .transpose()?;
            out.push(CheckpointMeta {
                seq: seq.max(0) as u64,
                canonical_hash,
            });
        }
        Ok(out)
    }
}

impl PeerLocalPersistence for FileLocalPersistence {
    fn is_durable(&self) -> bool {
        true
    }

    fn hydrate(&self, room_id: &str) -> LocalPersistResult<HydrateReport> {
        let conn = self.conn.lock().map_err(lock_unavailable)?;
        let tail = self.tail_for(&conn, room_id)?;
        let nodes = Self::load_nodes(&conn, room_id)?;
        let checkpoints = Self::load_checkpoints(&conn, room_id)?;
        Ok(HydrateReport {
            room_id: room_id.to_string(),
            tail: NodeLogTail { seq: tail },
            nodes,
            checkpoints,
        })
    }

    fn append_nodes(
        &self,
        room_id: &str,
        nodes: &[SyncNode],
        expected_tail: Option<NodeLogTail>,
    ) -> LocalPersistResult<AppendReport> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "filesystem adapter is read-only",
            ));
        }

        let mut conn = self.conn.lock().map_err(lock_unavailable)?;
        let tail = self.tail_for(&conn, room_id)?;
        if let Some(expected) = expected_tail {
            if expected.seq != tail {
                return Err(LocalPersistError::new(
                    LocalPersistReason::TailConflict,
                    format!("expected tail seq {} but room tail is {}", expected.seq, tail),
                ));
            }
        }

        let mut appended = 0usize;
        let mut new_tail = tail;
        let tx = conn.transaction().map_err(io_corruption)?;
        {
            for node in nodes {
                let bytes = pack_nodes(&[node]);
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO nodes (room_id, node_id, bytes) VALUES (?1, ?2, ?3)",
                        params![room_id, &node.id.as_bytes()[..], bytes],
                    )
                    .map_err(io_corruption)?;
                if changed > 0 {
                    new_tail = new_tail.saturating_add(1);
                    appended += 1;
                }
            }
            tx.execute(
                "INSERT INTO room_state (room_id, tail_seq) VALUES (?1, ?2)
                 ON CONFLICT(room_id) DO UPDATE SET tail_seq = excluded.tail_seq",
                params![room_id, new_tail as i64],
            )
            .map_err(io_corruption)?;
        }
        tx.commit().map_err(io_corruption)?;

        Ok(AppendReport {
            room_id: room_id.to_string(),
            appended,
            tail: NodeLogTail { seq: new_tail },
        })
    }

    fn flush(&self, room_id: &str) -> LocalPersistResult<FlushReport> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "filesystem adapter is read-only",
            ));
        }
        let conn = self.conn.lock().map_err(lock_unavailable)?;
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
            .map_err(io_corruption)?;
        let tail = self.tail_for(&conn, room_id)?;
        Ok(FlushReport {
            room_id: room_id.to_string(),
            tail: NodeLogTail { seq: tail },
            durable: true,
        })
    }

    fn checkpoint(
        &self,
        room_id: &str,
        meta: CheckpointMeta,
    ) -> LocalPersistResult<CheckpointReport> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "filesystem adapter is read-only",
            ));
        }
        let conn = self.conn.lock().map_err(lock_unavailable)?;
        let tail = self.tail_for(&conn, room_id)?;
        let hash_blob = meta.canonical_hash.map(|h| h.as_bytes().to_vec());
        conn.execute(
            "INSERT INTO checkpoints (room_id, cp_seq, canonical_hash, recorded_at_seq) VALUES (?1, ?2, ?3, ?4)",
            params![room_id, meta.seq as i64, hash_blob, tail as i64],
        )
        .map_err(io_corruption)?;
        Ok(CheckpointReport {
            room_id: room_id.to_string(),
            checkpoint: meta,
        })
    }

    fn recover(&self, room_id: &str) -> LocalPersistResult<RecoveryReport> {
        self.hydrate(room_id).map(|h| RecoveryReport {
            room_id: h.room_id,
            nodes: h.nodes,
            tail: h.tail,
        })
    }

    fn put_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) -> LocalPersistResult<()> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "filesystem adapter is read-only",
            ));
        }
        let dir = self.blobs_dir_for(room_id);
        std::fs::create_dir_all(&dir).map_err(io_unavailable)?;
        let path = dir.join(hex::encode(hash.as_bytes()));
        std::fs::write(path, bytes).map_err(io_unavailable)
    }

    fn get_blob(&self, room_id: &str, hash: &Hash) -> LocalPersistResult<Option<Vec<u8>>> {
        let path = self.blobs_dir_for(room_id).join(hex::encode(hash.as_bytes()));
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_unavailable(e)),
        }
    }
}

fn io_unavailable(err: impl std::fmt::Display) -> LocalPersistError {
    LocalPersistError::new(LocalPersistReason::Unavailable, err.to_string())
}

fn io_corruption(err: impl std::fmt::Display) -> LocalPersistError {
    LocalPersistError::new(LocalPersistReason::Corruption, err.to_string())
}

fn lock_unavailable(_: impl std::fmt::Display) -> LocalPersistError {
    LocalPersistError::new(LocalPersistReason::Unavailable, "store lock poisoned")
}

mod hex {
    pub fn encode(bytes: &[u8; 32]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(64);
        for b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0xf) as usize] as char);
        }
        s
    }
}
