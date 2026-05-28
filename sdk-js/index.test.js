import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

async function loadSdkModuleWithBridgeStub() {
  const path = new URL("./index.js", import.meta.url);
  let source = await readFile(path, "utf8");

  source = source.replace(
    'import initBridge, { SyncStore } from "nodalmerge-bridge";',
    [
      "const initBridge = async () => {};",
      "class SyncStore {}"
    ].join("\n")
  );

  source = source.replace(
    'import { resolvePeerLocalPersistence } from "./persistence/peer-local-indexeddb.js";',
    "function resolvePeerLocalPersistence() { return null; }"
  );

  source = source.replace(
    /export\s*\{\s*createPeerLocalIndexedDbPersistence,\s*resolvePeerLocalPersistence\s*\}\s*from\s*"\.\/persistence\/peer-local-indexeddb\.js";\s*/,
    ""
  );

  const dataUrl = `data:text/javascript;charset=utf-8,${encodeURIComponent(source)}`;
  return import(dataUrl);
}

function createMockStore() {
  const calls = [];
  return {
    calls,
    resolve_text: (...args) => {
      calls.push(["resolve_text", ...args]);
      return "spec-text";
    },
    resolve_text_canonical: (...args) => {
      calls.push(["resolve_text_canonical", ...args]);
      return "canon-text";
    },
    insert_text_range: (...args) => calls.push(["insert_text_range", ...args]),
    delete_text_range: (...args) => calls.push(["delete_text_range", ...args]),
    insert_text_range_start: (...args) => calls.push(["insert_text_range_start", ...args]),
    insert_text_range_end: (...args) => calls.push(["insert_text_range_end", ...args]),
    insert_text_range_after: (...args) => calls.push(["insert_text_range_after", ...args]),
    delete_text_range_start: (...args) => calls.push(["delete_text_range_start", ...args]),
    delete_text_range_after: (...args) => calls.push(["delete_text_range_after", ...args]),
  };
}

test("sync.getText and sync.getTextCanonical route to bridge text resolvers", async () => {
  const { sdk, mockStore } = await createSdkWithMockStore();

  const speculativeText = sdk.sync.getText("doc");
  const canonicalText = sdk.sync.getTextCanonical("doc");

  assert.equal(speculativeText, "spec-text");
  assert.equal(canonicalText, "canon-text");
  assert.deepEqual(mockStore.calls, [
    ["resolve_text", "doc"],
    ["resolve_text_canonical", "doc"],
  ]);
});

async function createSdkWithMockStore() {
  const { NodalMergeSdk } = await loadSdkModuleWithBridgeStub();
  const sdk = new NodalMergeSdk({ wsUrl: "ws://localhost", roomId: "room" });
  const mockStore = createMockStore();
  sdk.store = mockStore;
  return { sdk, mockStore };
}

test("sync.insertTextRange routes offset/start/end anchors", async () => {
  const { sdk, mockStore } = await createSdkWithMockStore();

  sdk.sync.insertTextRange("doc", { kind: "offset", pos: 2 }, "xy");
  sdk.sync.insertTextRange("doc", { kind: "start" }, "a");
  sdk.sync.insertTextRange("doc", { kind: "end" }, "z");

  assert.deepEqual(mockStore.calls, [
    ["insert_text_range", "doc", 2, "xy"],
    ["insert_text_range_start", "doc", "a"],
    ["insert_text_range_end", "doc", "z"],
  ]);
});

test("sync.insertTextRange routes after anchor and normalizes inputs", async () => {
  const { sdk, mockStore } = await createSdkWithMockStore();

  sdk.sync.insertTextRange(
    "doc",
    {
      kind: "after",
      lamport: 42,
      author: "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"
    },
    "!"
  );

  assert.equal(mockStore.calls.length, 1);
  assert.equal(mockStore.calls[0][0], "insert_text_range_after");
  assert.equal(mockStore.calls[0][1], "doc");
  assert.equal(mockStore.calls[0][2], 42n);
  assert.equal(
    mockStore.calls[0][3],
    "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
  );
  assert.equal(mockStore.calls[0][4], "!");
});

test("sync.deleteTextRange routes offset/start/after anchors", async () => {
  const { sdk, mockStore } = await createSdkWithMockStore();

  sdk.sync.deleteTextRange("doc", { kind: "offset", pos: 1 }, 2);
  sdk.sync.deleteTextRange("doc", { kind: "start" }, 3);
  sdk.sync.deleteTextRange(
    "doc",
    {
      kind: "after",
      lamport: 9n,
      author: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    },
    1
  );

  assert.deepEqual(mockStore.calls, [
    ["delete_text_range", "doc", 1, 2],
    ["delete_text_range_start", "doc", 3],
    ["delete_text_range_after", "doc", 9n, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", 1],
  ]);
});

test("sync range helpers validate anchor constraints", async () => {
  const { sdk } = await createSdkWithMockStore();

  assert.throws(
    () => sdk.sync.deleteTextRange("doc", { kind: "end" }, 1),
    /delete does not support end anchor/
  );
  assert.throws(
    () => sdk.sync.insertTextRange("doc", { kind: "offset", pos: -1 }, "a"),
    /non-negative integer pos/
  );
  assert.throws(
    () => sdk.sync.insertTextRange("doc", { kind: "after", lamport: 1, author: "bad" }, "a"),
    /64-char hex author/
  );
});

test("sync.insertTextAt and deleteTextAt dispatch to offset range methods", async () => {
  const { sdk, mockStore } = await createSdkWithMockStore();

  sdk.sync.insertTextAt("doc", 0, "abc");
  sdk.sync.deleteTextAt("doc", 1, 1);

  assert.deepEqual(mockStore.calls, [
    ["insert_text_range", "doc", 0, "abc"],
    ["delete_text_range", "doc", 1, 1],
  ]);
});

test("query.buildProjection sends canonical checkpoint payload and resolves completed event", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.buildProjection({
    projectionId: "p.rooms",
    querySpecId: "q.rooms",
    targetCheckpoint: {
      selector: "hash",
      canonical_hash: "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"
    },
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "projection.build.completed",
    projection_id: "p.rooms",
    checkpoint: { selector: "hash", canonical_hash: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789" },
    digest: "dig-1"
  });

  const result = await pending;
  assert.equal(result.type, "projection.build.completed");
  assert.deepEqual(sent, [
    {
      type: "projection.build",
      projection_id: "p.rooms",
      query_spec_id: "q.rooms",
      target_checkpoint: {
        selector: "hash",
        canonical_hash: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
      }
    }
  ]);
});

test("query.buildProjection validates selector payload shape before send", async () => {
  const { sdk } = await createSdkWithMockStore();
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = () => {
    throw new Error("send should not be called");
  };

  await assert.rejects(
    () =>
      sdk.query.buildProjection({
        projectionId: "p.rooms",
        querySpecId: "q.rooms",
        targetCheckpoint: {
          selector: "seq",
          canonical_seq: 1,
          frontier: ["seq:1"]
        },
        timeoutMs: 50
      }),
    /selector seq only allows canonical_seq/
  );
});

test("query.readProjection sends limit and page token and resolves read result", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.readProjection({
    projectionId: "p.rooms",
    limit: 2,
    pageToken: "offset:1",
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "projection.read.result",
    projection_id: "p.rooms",
    rows: [{ k: "world/a", v: "1" }],
    checkpoint: { selector: "seq", canonical_seq: 1 },
    digest: "dig-2",
    next_page_token: "offset:2"
  });

  const result = await pending;
  assert.equal(result.type, "projection.read.result");
  assert.deepEqual(sent, [
    {
      type: "projection.read",
      projection_id: "p.rooms",
      limit: 2,
      page_token: "offset:1"
    }
  ]);
});

test("query helpers require open runtime websocket connection", async () => {
  const { sdk } = await createSdkWithMockStore();
  sdk.connected = false;
  sdk.ws = null;

  await assert.rejects(
    () => sdk.query.readProjection({ projectionId: "p.rooms", limit: 1, timeoutMs: 20 }),
    /requires an open runtime websocket connection/
  );
});

test("query.registerSpec resolves deterministic rejected response", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.registerSpec({
    querySpecId: "q.rooms",
    version: "v2",
    descriptor: { source: "rooms" },
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "query.register.rejected",
    query_spec_id: "q.rooms",
    version: "v2",
    reason_class: "reject.query_unsupported_version",
    reason_message: "unsupported"
  });

  const result = await pending;
  assert.equal(result.type, "query.register.rejected");
  assert.equal(result.reason_class, "reject.query_unsupported_version");
  assert.equal(result.reason_message, "unsupported");
  assert.deepEqual(sent, [
    {
      type: "query.register",
      query_spec_id: "q.rooms",
      version: "v2",
      descriptor: { source: "rooms" },
      options: undefined
    }
  ]);
});

test("query.buildProjection resolves rejected response with bounded reason taxonomy fields", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.buildProjection({
    projectionId: "p.rooms",
    querySpecId: "q.rooms",
    targetCheckpoint: { selector: "frontier", frontier: ["seq:1"] },
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "projection.build.rejected",
    projection_id: "p.rooms",
    reason_class: "reject.checkpoint_selector_invalid",
    reason_message: "selector frontier requires tokens in seq:<u64> format"
  });

  const result = await pending;
  assert.equal(result.type, "projection.build.rejected");
  assert.equal(result.reason_class, "reject.checkpoint_selector_invalid");
  assert.match(String(result.reason_message), /selector frontier/);
  assert.deepEqual(sent, [
    {
      type: "projection.build",
      projection_id: "p.rooms",
      query_spec_id: "q.rooms",
      target_checkpoint: { selector: "frontier", frontier: ["seq:1"] }
    }
  ]);
});

test("query.readProjection resolves rejected response with deterministic reason taxonomy fields", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.readProjection({
    projectionId: "p.rooms",
    limit: 2,
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "projection.read.rejected",
    projection_id: "p.rooms",
    reason_class: "reject.projection_not_found",
    reason_message: "projection is not registered"
  });

  const result = await pending;
  assert.equal(result.type, "projection.read.rejected");
  assert.equal(result.reason_class, "reject.projection_not_found");
  assert.equal(result.reason_message, "projection is not registered");
  assert.deepEqual(sent, [
    {
      type: "projection.read",
      projection_id: "p.rooms",
      limit: 2,
      page_token: undefined
    }
  ]);
});

test("query.invalidateProjection resolves rejected response with deterministic reason taxonomy fields", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.invalidateProjection({
    projectionId: "p.rooms",
    reason: "manual",
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "projection.invalidate.rejected",
    projection_id: "p.rooms",
    reason_class: "reject.projection_immutable",
    reason_message: "projection cannot be invalidated in current state"
  });

  const result = await pending;
  assert.equal(result.type, "projection.invalidate.rejected");
  assert.equal(result.reason_class, "reject.projection_immutable");
  assert.equal(result.reason_message, "projection cannot be invalidated in current state");
  assert.deepEqual(sent, [
    {
      type: "projection.invalidate",
      projection_id: "p.rooms",
      reason: "manual"
    }
  ]);
});

test("query.listProjections resolves rejected response scoped to query_spec_id", async () => {
  const { sdk } = await createSdkWithMockStore();
  const sent = [];
  sdk.connected = true;
  sdk.ws = { readyState: 1 };
  sdk.sendOrQueue = (msg) => sent.push(msg);

  const pending = sdk.query.listProjections({
    querySpecId: "q.rooms",
    stateFilter: "active",
    timeoutMs: 200
  });

  sdk.emit("runtime-message", {
    type: "projection.list.rejected",
    query_spec_id: "q.other",
    reason_class: "reject.query_spec_not_found",
    reason_message: "wrong query spec"
  });

  sdk.emit("runtime-message", {
    type: "projection.list.rejected",
    query_spec_id: "q.rooms",
    reason_class: "reject.query_spec_not_found",
    reason_message: "query spec is not registered"
  });

  const result = await pending;
  assert.equal(result.type, "projection.list.rejected");
  assert.equal(result.reason_class, "reject.query_spec_not_found");
  assert.equal(result.reason_message, "query spec is not registered");
  assert.deepEqual(sent, [
    {
      type: "projection.list",
      query_spec_id: "q.rooms",
      state_filter: "active",
      cursor: undefined
    }
  ]);
});
