/**
 * Browser peer-local persistence via IndexedDB (Phase C).
 *
 * Layout (db version 1):
 *   nodes — key: roomId, value: export_all_nodes() pack string
 *   blobs — key: `${roomId}:${hashHex}`, value: Uint8Array
 */

const DEFAULT_DB_NAME = "nodalmerge-peer-local";
const DEFAULT_DB_VERSION = 1;
const DEFAULT_DEBOUNCE_MS = 250;

function parseJsonOrDefault(value, fallback) {
  try {
    return JSON.parse(value);
  } catch {
    return fallback;
  }
}

function blobStoreKey(roomId, hashHex) {
  return `${roomId}:${hashHex}`;
}

export function createPeerLocalIndexedDbPersistence(options = {}) {
  const dbName = options.dbName ?? DEFAULT_DB_NAME;
  const dbVersion = options.dbVersion ?? DEFAULT_DB_VERSION;
  const debounceMs = options.debounceMs ?? DEFAULT_DEBOUNCE_MS;
  const migrateLegacyDemoDefault = options.migrateLegacyDemo === true;
  const indexedDBRef = options.indexedDB ?? (typeof indexedDB !== "undefined" ? indexedDB : null);

  let db = null;
  let persistTimer = null;
  let pendingPersist = null;

  async function openDatabase() {
    if (!indexedDBRef) {
      throw new Error("IndexedDB is not available in this environment");
    }
    if (db) {
      return db;
    }

    db = await new Promise((resolve, reject) => {
      const req = indexedDBRef.open(dbName, dbVersion);
      req.onupgradeneeded = (event) => {
        const idb = event.target.result;
        if (!idb.objectStoreNames.contains("nodes")) {
          idb.createObjectStore("nodes");
        }
        if (!idb.objectStoreNames.contains("blobs")) {
          idb.createObjectStore("blobs");
        }
      };
      req.onsuccess = (event) => resolve(event.target.result);
      req.onerror = (event) => reject(event.target.error ?? new Error("indexedDB.open failed"));
    });
    return db;
  }

  function idbPut(storeName, key, value) {
    return new Promise((resolve, reject) => {
      const tx = db.transaction(storeName, "readwrite");
      const store = tx.objectStore(storeName);
      const req = store.put(value, key);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error ?? new Error(`idb put ${storeName}`));
      tx.onerror = () => reject(tx.error ?? new Error(`idb tx ${storeName}`));
    });
  }

  function idbGet(storeName, key) {
    return new Promise((resolve, reject) => {
      const tx = db.transaction(storeName, "readonly");
      const req = tx.objectStore(storeName).get(key);
      req.onsuccess = (event) => resolve(event.target.result);
      req.onerror = () => reject(req.error ?? new Error(`idb get ${storeName}`));
      tx.onerror = () => reject(tx.error ?? new Error(`idb tx ${storeName}`));
    });
  }

  function idbDelete(storeName, key) {
    return new Promise((resolve, reject) => {
      const tx = db.transaction(storeName, "readwrite");
      const req = tx.objectStore(storeName).delete(key);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error ?? new Error(`idb delete ${storeName}`));
      tx.onerror = () => reject(tx.error ?? new Error(`idb tx ${storeName}`));
    });
  }

  function idbDeletePrefix(storeName, prefix) {
    return new Promise((resolve, reject) => {
      const tx = db.transaction(storeName, "readwrite");
      const store = tx.objectStore(storeName);
      const req = store.openCursor();
      req.onsuccess = (event) => {
        const cursor = event.target.result;
        if (!cursor) {
          return;
        }
        if (typeof cursor.key === "string" && cursor.key.startsWith(prefix)) {
          cursor.delete();
        }
        cursor.continue();
      };
      req.onerror = () => reject(req.error ?? new Error(`idb cursor ${storeName}`));
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error(`idb tx ${storeName}`));
    });
  }

  async function persistGraph(store, roomId) {
    if (!db || !store) {
      return { nodesSaved: false, blobsSaved: 0 };
    }

    const nodesPack = store.export_all_nodes();
    await idbPut("nodes", roomId, nodesPack);

    const hashes = parseJsonOrDefault(store.local_blob_hashes_json(), []);
    let blobsSaved = 0;
    for (const hashHex of hashes) {
      if (typeof hashHex !== "string" || hashHex.length === 0) {
        continue;
      }
      try {
        const bytes = store.get_blob_bytes(hashHex);
        if (bytes && bytes.length > 0) {
          await idbPut("blobs", blobStoreKey(roomId, hashHex), bytes);
          blobsSaved += 1;
        }
      } catch {
        // Skip blobs that are referenced but not yet materialized locally.
      }
    }

    return { nodesSaved: true, blobsSaved };
  }

  return {
    kind: "indexeddb",
    dbName,

    isAvailable() {
      return indexedDBRef != null;
    },

    async open() {
      await openDatabase();
      return { dbName, dbVersion };
    },

    async migrateLegacyDemo(roomId, store) {
      if (!db) {
        await openDatabase();
      }

      const report = { nodesMigrated: false, blobsMigrated: 0 };

      const legacyPack = await idbGet("nodes", "all");
      const roomPack = await idbGet("nodes", roomId);
      if (typeof legacyPack === "string" && legacyPack.length > 0 && roomPack == null) {
        store.import_pack(legacyPack);
        await idbPut("nodes", roomId, legacyPack);
        await idbDelete("nodes", "all");
        report.nodesMigrated = true;
      }

      const legacyBlobs = [];
      await new Promise((resolve, reject) => {
        const tx = db.transaction("blobs", "readonly");
        const storeHandle = tx.objectStore("blobs");
        const req = storeHandle.openCursor();
        req.onsuccess = (event) => {
          const cursor = event.target.result;
          if (!cursor) {
            return;
          }
          const key = cursor.key;
          if (typeof key === "string" && /^[0-9a-fA-F]{64}$/.test(key)) {
            legacyBlobs.push([key, cursor.value]);
          }
          cursor.continue();
        };
        req.onerror = () => reject(req.error ?? new Error("idb legacy blob cursor"));
        tx.oncomplete = () => resolve();
        tx.onerror = () => reject(tx.error ?? new Error("idb legacy blob tx"));
      });

      for (const [hashHex, bytes] of legacyBlobs) {
        try {
          store.store_blob_bytes(hashHex, bytes);
          await idbPut("blobs", blobStoreKey(roomId, hashHex), bytes);
          await idbDelete("blobs", hashHex);
          report.blobsMigrated += 1;
        } catch {
          // Skip corrupt legacy blobs.
        }
      }

      return report;
    },

    async hydrate(store, roomId, hydrateOptions = { migrateLegacyDemo: migrateLegacyDemoDefault }) {
      if (!db) {
        await openDatabase();
      }

      const report = {
        roomId,
        nodesPack: null,
        blobsRestored: 0,
        canonicalHash: null,
        legacyMigration: null
      };

      const nodesPack = await idbGet("nodes", roomId);
      if (typeof nodesPack === "string" && nodesPack.length > 0) {
        store.import_pack(nodesPack);
        report.nodesPack = nodesPack;
      } else if (hydrateOptions.migrateLegacyDemo) {
        report.legacyMigration = await this.migrateLegacyDemo(roomId, store);
        const afterLegacy = await idbGet("nodes", roomId);
        if (typeof afterLegacy === "string" && afterLegacy.length > 0) {
          report.nodesPack = afterLegacy;
        }
      }

      const prefix = `${roomId}:`;
      await new Promise((resolve, reject) => {
        const tx = db.transaction("blobs", "readonly");
        const storeHandle = tx.objectStore("blobs");
        const req = storeHandle.openCursor();
        req.onsuccess = (event) => {
          const cursor = event.target.result;
          if (!cursor) {
            return;
          }
          const key = cursor.key;
          if (typeof key === "string" && key.startsWith(prefix)) {
            const hashHex = key.slice(prefix.length);
            const bytes = cursor.value;
            try {
              store.store_blob_bytes(hashHex, bytes);
              report.blobsRestored += 1;
            } catch {
              // Ignore corrupt or mismatched blobs during hydrate.
            }
          }
          cursor.continue();
        };
        req.onerror = () => reject(req.error ?? new Error("idb hydrate cursor"));
        tx.oncomplete = () => resolve();
        tx.onerror = () => reject(tx.error ?? new Error("idb hydrate tx"));
      });

      try {
        report.canonicalHash = store.resolved_state_hash_hex();
      } catch {
        report.canonicalHash = null;
      }

      return report;
    },

    schedulePersist(store, roomId) {
      if (!db) {
        return;
      }
      pendingPersist = { store, roomId };
      if (persistTimer != null) {
        return;
      }
      persistTimer = setTimeout(() => {
        persistTimer = null;
        const job = pendingPersist;
        pendingPersist = null;
        if (!job) {
          return;
        }
        void persistGraph(job.store, job.roomId).catch(() => {
          // Best-effort peer-local durability.
        });
      }, debounceMs);
    },

    async flush(store, roomId) {
      if (persistTimer != null) {
        clearTimeout(persistTimer);
        persistTimer = null;
      }
      pendingPersist = null;
      if (!store) {
        return { flushed: false };
      }
      if (!db) {
        await openDatabase();
      }
      const result = await persistGraph(store, roomId);
      return { flushed: true, ...result };
    },

    async recover(store, roomId) {
      return this.hydrate(store, roomId);
    },

    async clearRoom(roomId) {
      if (!db) {
        await openDatabase();
      }
      await idbDelete("nodes", roomId);
      await idbDeletePrefix("blobs", `${roomId}:`);
      return { cleared: true, roomId };
    },

    async close() {
      if (persistTimer != null) {
        clearTimeout(persistTimer);
        persistTimer = null;
      }
      pendingPersist = null;
      if (db) {
        db.close();
        db = null;
      }
    }
  };
}

/**
 * Resolve SDK `persistence` options into an adapter or null (disabled).
 */
export function resolvePeerLocalPersistence(options) {
  const cfg = options?.persistence;
  if (!cfg || cfg.enabled === false) {
    return null;
  }

  if (cfg.adapter && typeof cfg.adapter === "object") {
    return cfg.adapter;
  }

  const kind = cfg.adapter ?? "indexeddb";
  if (kind !== "indexeddb") {
    throw new Error(`unsupported persistence.adapter: ${kind}`);
  }

  const adapter = createPeerLocalIndexedDbPersistence({
    dbName: cfg.dbName,
    dbVersion: cfg.dbVersion,
    debounceMs: cfg.debounceMs,
    indexedDB: cfg.indexedDB,
    migrateLegacyDemo: cfg.migrateLegacyDemo === true
  });

  if (!adapter.isAvailable()) {
    throw new Error("persistence.adapter indexeddb requires IndexedDB");
  }

  return adapter;
}
