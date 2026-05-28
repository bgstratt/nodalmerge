import test from "node:test";
import assert from "node:assert/strict";
import {
  createPeerLocalIndexedDbPersistence
} from "./peer-local-indexeddb.js";

function createMockIndexedDb() {
  const databases = new Map();

  class MockRequest {
    constructor() {
      this.result = null;
      this.error = null;
      this.onsuccess = null;
      this.onerror = null;
      this.onupgradeneeded = null;
    }

    _finish() {
      if (this.error && this.onerror) {
        this.onerror({ target: this });
      } else if (this.onsuccess) {
        this.onsuccess({ target: this });
      }
    }
  }

  class MockCursor {
    constructor(entries, index = 0) {
      this._entries = entries;
      this._index = index;
      this.key = entries[index]?.[0] ?? null;
      this.value = entries[index]?.[1] ?? null;
    }

    continue() {
      const next = this._index + 1;
      if (next >= this._entries.length) {
        this.key = null;
        this.value = null;
        return;
      }
      Object.assign(this, new MockCursor(this._entries, next));
    }

    delete() {
      const [key] = this._entries[this._index];
      const store = this._store;
      store._data.delete(key);
    }
  }

  class MockObjectStore {
    constructor(dbName, storeName) {
      this._dbName = dbName;
      this._storeName = storeName;
      if (!databases.has(dbName)) {
        databases.set(dbName, new Map());
      }
      const db = databases.get(dbName);
      if (!db.has(storeName)) {
        db.set(storeName, new Map());
      }
      this._data = db.get(storeName);
    }

    put(value, key) {
      const req = new MockRequest();
      if (value === undefined) {
        this._data.delete(key);
      } else {
        this._data.set(key, value);
      }
      queueMicrotask(() => req._finish());
      return req;
    }

    get(key) {
      const req = new MockRequest();
      req.result = this._data.has(key) ? this._data.get(key) : undefined;
      queueMicrotask(() => req._finish());
      return req;
    }

    openCursor() {
      const req = new MockRequest();
      const entries = [...this._data.entries()];
      req.result = entries.length > 0 ? Object.assign(new MockCursor(entries), { _store: this }) : null;
      queueMicrotask(() => req._finish());
      return req;
    }
  }

  class MockTransaction {
    constructor(dbName, storeName, mode) {
      this._store = new MockObjectStore(dbName, storeName);
      this.oncomplete = null;
      this.onerror = null;
      this.error = null;
      queueMicrotask(() => {
        if (this.oncomplete) {
          this.oncomplete();
        }
      });
    }

    objectStore() {
      return this._store;
    }
  }

  class MockDatabase {
    constructor(name) {
      this.name = name;
      this.objectStoreNames = {
        contains: () => true
      };
    }

    transaction(storeName, mode) {
      return new MockTransaction(this.name, storeName, mode);
    }

    close() {}
  }

  return {
    open(name, version) {
      const req = new MockRequest();
      const db = new MockDatabase(name);
      req.result = db;
      queueMicrotask(() => {
        if (req.onupgradeneeded) {
          req.onupgradeneeded({ target: { result: db } });
        }
        req._finish();
      });
      return req;
    }
  };
}

function createMockStore() {
  let nodesPack = "";
  const blobs = new Map();

  return {
    export_all_nodes: () => nodesPack || "pack-v1",
    import_pack: (pack) => {
      nodesPack = pack;
    },
    local_blob_hashes_json: () => JSON.stringify([...blobs.keys()]),
    get_blob_bytes: (hash) => blobs.get(hash),
    store_blob_bytes: (hash, bytes) => {
      blobs.set(hash, bytes);
    },
    resolved_state_hash_hex: () => "abc123"
  };
}

test("indexeddb adapter persists and hydrates nodes + blobs", async () => {
  const adapter = createPeerLocalIndexedDbPersistence({
    dbName: "test-db",
    indexedDB: createMockIndexedDb(),
    debounceMs: 5
  });

  assert.equal(adapter.isAvailable(), true);
  await adapter.open();

  const store = createMockStore();
  store.store_blob_bytes("aa".repeat(32), new Uint8Array([1, 2, 3]));

  await adapter.flush(store, "room-a");
  const empty = createMockStore();
  const report = await adapter.hydrate(empty, "room-a");

  assert.equal(report.nodesPack, "pack-v1");
  assert.equal(report.blobsRestored, 1);
  assert.equal(empty.get_blob_bytes("aa".repeat(32)).length, 3);
});

test("resolvePeerLocalPersistence returns null when disabled", async () => {
  const { resolvePeerLocalPersistence } = await import("./peer-local-indexeddb.js");
  assert.equal(resolvePeerLocalPersistence({}), null);
  assert.equal(resolvePeerLocalPersistence({ persistence: { enabled: false } }), null);
});
