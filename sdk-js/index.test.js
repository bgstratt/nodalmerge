import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

async function loadSdkModuleWithBridgeStub() {
  const path = new URL("./index.js", import.meta.url);
  let source = await readFile(path, "utf8");

  source = source.replace(
    'import initBridge, { SyncStore } from "activesync-bridge";',
    [
      "const initBridge = async () => {};",
      "class SyncStore {}"
    ].join("\n")
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
  const { ActiveSyncSdk } = await loadSdkModuleWithBridgeStub();
  const sdk = new ActiveSyncSdk({ wsUrl: "ws://localhost", roomId: "room" });
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
