/**
 * demo.js — ActiveSync Production Demo
 *
 * Transport   : WebSocket → ws://localhost:7878/ws/<room>
 * Identity    : Ed25519 keypair (seed stored in sessionStorage)
 * Auth        : every SyncNode carries an Ed25519 signature (verified by server + peers)
 * Presence    : ephemeral side-channel (not stored in DAG)
 *
 * Server wire protocol:
 *   Client→Server  hello     { type, room, root, frontier:[hex], ibf:"b64", mst_root:"hex", caps:{...}, pubkey }
 *   Server→Client  welcome   { type, root, frontier:[hex], missing:[hex], caps:{...} }
 *   Client→Server  pack      { type, nodes: "<json string>" }
 *   Server→All     pack      { type, from, nodes:"<json string>", root }
 *   Client→Server  request   { type, known:[hex] }
 *   Client→Server  blob-upload { type, blobs:[{hash,data}] }
 *   Client→Server  blob-request { type, hashes:[hex] }
 *   Server→Client  blob-pack { type, blobs:[{hash,data}] }
 *   Client→Server  mst-request { type, paths:["hex",...] }
 *   Server→Client  mst-response { type, nodes:[MstNodeWire,...] }
 *   Client→Server  mst-done    { type, ids:["hex",...] }
 *   Server→All     presence  { type, from, data:{...} }
 */

import init, { SyncStore, sign_room_token, room_pubkey_hex } from './pkg/activesync_bridge.js';

// Catch any unhandled promise rejections from the module itself.
window.addEventListener('unhandledrejection', ev => {
  console.error('[activesync] unhandledRejection:', ev.reason);
  const s = document.getElementById('status');
  if (s) s.textContent = 'Module error: ' + ev.reason;
});

console.log('[boot] starting — loading WASM…');
await init();
console.log('[boot] WASM init done');

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------
const ROOM_ID   = 'default';
const WS_URL    = `ws://127.0.0.1:7878/ws/${ROOM_ID}`;
const COLORS    = ['#60a5fa','#4ade80','#f472b6','#fb923c','#a78bfa','#34d399','#fbbf24','#f87171'];
const IDB_NAME    = 'activesync-v5';
const IDB_VERSION = 1;

// ---------------------------------------------------------------------------
// Room auth (C3) — Ed25519 room keypair seed, kept in sessionStorage.
// null = open room. Set via the Room Auth UI panel.
// ---------------------------------------------------------------------------
let roomSeed = (() => {
  const stored = sessionStorage.getItem('activesync-room-seed');
  if (!stored) return null;
  try { return new Uint8Array(JSON.parse(stored)); } catch (_) { return null; }
})();

function setRoomSeed(seed32) {
  roomSeed = seed32;
  if (seed32) {
    sessionStorage.setItem('activesync-room-seed', JSON.stringify(Array.from(seed32)));
  } else {
    sessionStorage.removeItem('activesync-room-seed');
  }
}

function buildToken() {
  if (!roomSeed) return null;
  // Token valid for 1 hour from now.
  const expiry = Math.floor(Date.now() / 1000) + 3600;
  try {
    // Pass null for caps = full access (E1 will let callers pass capability arrays).
    return JSON.parse(sign_room_token(ROOM_ID, myPubkey, BigInt(expiry), roomSeed, null));
  } catch (_) { return null; }
}

// ---------------------------------------------------------------------------
// Identity — Ed25519 seed in sessionStorage (survives refresh, not close)
// ---------------------------------------------------------------------------
function getOrCreateIdentity() {
  const stored = sessionStorage.getItem('activesync-identity-v3');
  if (stored) return JSON.parse(stored);
  const seed  = crypto.getRandomValues(new Uint8Array(32));
  const color = COLORS[Math.floor(Math.random() * COLORS.length)];
  const id    = Array.from(seed.slice(0, 4)).map(b => b.toString(16).padStart(2,'0')).join('');
  const identity = { seed: Array.from(seed), id, color };
  sessionStorage.setItem('activesync-identity-v3', JSON.stringify(identity));
  return identity;
}

const { seed: seedArr, id: myId, color: myColor } = getOrCreateIdentity();
const authorSeed = new Uint8Array(seedArr);

// ---------------------------------------------------------------------------
// WASM store
// ---------------------------------------------------------------------------
const store = new SyncStore(authorSeed);
const myPubkey = store.pubkey_hex();

// ---------------------------------------------------------------------------
// IndexedDB persistence (C2)
// Nodes: one key 'all' → base64 postcard pack of the full node graph.
// Blobs: key = hash_hex (64 chars) → Uint8Array of raw blob bytes.
// ---------------------------------------------------------------------------
let db = null;

function openDB() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(IDB_NAME, IDB_VERSION);
    req.onupgradeneeded = (e) => {
      const idb = e.target.result;
      if (!idb.objectStoreNames.contains('nodes')) idb.createObjectStore('nodes');
      if (!idb.objectStoreNames.contains('blobs')) idb.createObjectStore('blobs');
    };
    req.onsuccess = (e) => resolve(e.target.result);
    req.onerror   = (e) => reject(e.target.error);
  });
}

function idbPut(storeName, key, value) {
  if (!db) return Promise.resolve();
  return new Promise((resolve) => {
    const tx = db.transaction(storeName, 'readwrite');
    tx.objectStore(storeName).put(value, key);
    tx.oncomplete = resolve;
    tx.onerror    = resolve; // silent failure
  });
}

function idbGet(storeName, key) {
  if (!db) return Promise.resolve(undefined);
  return new Promise((resolve) => {
    const tx  = db.transaction(storeName, 'readonly');
    const req = tx.objectStore(storeName).get(key);
    req.onsuccess = (e) => resolve(e.target.result);
    req.onerror   = ()  => resolve(undefined);
  });
}

function idbCursor(storeName, cb) {
  if (!db) return Promise.resolve();
  return new Promise((resolve) => {
    const tx = db.transaction(storeName, 'readonly');
    tx.objectStore(storeName).openCursor().onsuccess = (e) => {
      const cursor = e.target.result;
      if (cursor) { cb(cursor.key, cursor.value); cursor.continue(); }
      else resolve();
    };
    tx.onerror = resolve;
  });
}

// Save entire node graph — called fire-and-forget after every mutation.
async function saveNodes() {
  try { await idbPut('nodes', 'all', store.export_all_nodes()); } catch (_) {}
}

// Load node graph from IDB; falls back to legacy localStorage on first run.
async function loadNodes() {
  let data = await idbGet('nodes', 'all');
  if (!data) {
    // One-time migration from localStorage (activesync-v4 → v5).
    const legacy = localStorage.getItem('activesync-v4-nodes');
    if (legacy) { data = legacy; localStorage.removeItem('activesync-v4-nodes'); }
  }
  if (data) { try { store.import_pack(data); } catch (_) {} }
}

// Persist a single blob by hash (Uint8Array). Blake3 integrity re-checked on load.
function saveBlob(hashHex, bytes) {
  idbPut('blobs', hashHex, bytes); // fire-and-forget
}

// Reload all blobs from IDB into the WASM BlobStore.
async function loadBlobs() {
  await idbCursor('blobs', (hashHex, bytes) => {
    try { store.store_blob_bytes(hashHex, bytes); } catch (_) {}
  });
}

// saveState: drop-in replacement for old localStorage version — call sites unchanged.
function saveState() {
  // Debounce full-graph IDB writes — exporting + writing every keystroke is
  // the main source of typing latency. Trailing 250 ms is imperceptible while
  // still persisting promptly on pause.
  if (saveState._t) return;
  saveState._t = setTimeout(() => { saveState._t = null; saveNodes(); }, 250);
}

// ---------------------------------------------------------------------------
// Peer registry (includes presence data)
// ---------------------------------------------------------------------------
const peers = new Map(); // pubkey → { color, root, lastSeen, label, cursor? }

// ---------------------------------------------------------------------------
// Blob tracking
// ---------------------------------------------------------------------------
const blobStats = { sent: 0, received: 0, cacheHits: 0 };

// Hashes already requested from server but not yet received.
// Prevents spamming duplicate blob-request messages on every incoming delta.
const pendingBlobFetches = new Set();

function fmtBytes(n) {
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
  return (n / (1024 * 1024)).toFixed(2) + ' MB';
}
function renderBlobStats() {
  const el = document.getElementById('blob-stats');
  if (el) el.textContent =
    `Sent: ${fmtBytes(blobStats.sent)} | Received: ${fmtBytes(blobStats.received)} | Cache hits: ${blobStats.cacheHits}`;
}

// B2: MST descent state — which server MST root we are currently descending.
let mstDescentRoot = null;

/// Begin or continue an MST descent toward `serverMstRoot`.
function mstDescend(serverMstRoot, paths) {
  mstDescentRoot = serverMstRoot;
  wsSend({ type: 'mst-request', paths });
}

// ---------------------------------------------------------------------------
// WebSocket connection
// ---------------------------------------------------------------------------
let ws = null;
let offline = false;
let reconnectTimer = null;

// Delta-broadcast tracking: set of node-id hex strings the server is known to
// have (either we sent them, or server sent them to us). We only broadcast
// the local nodes NOT in this set — keeps per-keystroke traffic tiny.
const sentToServer = new Set();
function refreshSentToServer() {
  try {
    const ids = JSON.parse(store.all_node_ids_json());
    for (const id of ids) sentToServer.add(id);
  } catch (_) {}
}
function broadcastDelta() {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  try {
    const delta = store.export_nodes_missing_from(JSON.stringify([...sentToServer]));
    ws.send(JSON.stringify({ type: 'pack', nodes: delta }));
    refreshSentToServer();
  } catch (e) { console.error('[broadcastDelta]', e); }
}

function wsConnect() {
  if (offline) return;
  clearTimeout(reconnectTimer);
  reconnectTimer = null;

  // Detach handlers from any previous WebSocket so stale close/error events
  // don't schedule extra reconnect timers while the new connection is opening.
  if (ws) {
    ws.onopen    = null;
    ws.onclose   = null;
    ws.onerror   = null;
    ws.onmessage = null;
    if (ws.readyState !== WebSocket.CLOSED) ws.close();
  }

  setStatus('Connecting…');
  ws = new WebSocket(WS_URL);

  ws.onopen = () => {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
    pendingBlobFetches.clear();
    setStatus('Connected');
    updateServerBadge(true);
    logEvent('sync', `✓ WS open → ${WS_URL}`);
    console.log('[ws] onopen — building hello…');
    try {
      // Handshake: hello — send frontier (A6), capabilities (A7), IBF (B1), MST root (B2)
      // C3: include capability token if the room has an auth key configured.
      console.log('[ws] step 1: merkle_root_hex');
      const root = store.merkle_root_hex();
      console.log('[ws] step 2: frontier_hex_json');
      const frontier = JSON.parse(store.frontier_hex_json());
      console.log('[ws] step 3: compute_ibf_b64');
      const ibf = store.compute_ibf_b64();
      console.log('[ws] step 4: mst_root_hex');
      const mst_root = store.mst_root_hex();
      console.log('[ws] step 5: our_capabilities_json');
      const caps = JSON.parse(store.our_capabilities_json());
      console.log('[ws] hello fields ready — caps:', caps, ' frontier len:', frontier.length);
      const helloMsg = { type: 'hello', room: ROOM_ID, root, frontier, ibf, mst_root, caps, pubkey: myPubkey };
      const tok = buildToken();
      if (tok) helloMsg.token = tok;
      wsSend(helloMsg);
      console.log('[ws] hello sent ✓ (' + JSON.stringify(helloMsg).length + ' bytes)');
      // Re-upload all locally-stored blobs so the server is repopulated after
      // a restart (server BlobStore is in-memory; clients are authoritative).
      const localHashes = JSON.parse(store.local_blob_hashes_json());
      if (localHashes.length > 0) {
        try {
          const blobsJson = store.export_blobs_json(JSON.stringify(localHashes));
          const blobs = JSON.parse(blobsJson);
          if (blobs.length > 0) {
            wsSend({ type: 'blob-upload', blobs });
            logEvent('sync', `↑ re-uploaded ${blobs.length} blob(s) to server`);
          }
        } catch (e2) { logEvent('del', `blob re-upload error: ${e2}`); }
      }
      // Start presence heartbeat
      sendPresence();
    } catch (err) {
      console.error('[ws] onopen WASM error:', err);
      logEvent('del', `onopen error: ${err}`);
    }
  };

  ws.onmessage = (e) => {
    try { onServerMessage(JSON.parse(e.data)); } catch (err) {
      console.error('onServerMessage error:', err);
      logEvent('del', `msg error: ${err}`);
    }
  };

  ws.onclose = (ev) => {
    updateServerBadge(false);
    if (!offline) {
      setStatus('Disconnected — reconnecting in 3s…');
      logEvent('del', `WS closed (code ${ev.code}) — reconnecting…`);
      clearTimeout(reconnectTimer);
      reconnectTimer = setTimeout(wsConnect, 3000);
    }
  };

  ws.onerror = (e) => {
    console.error('[ws] WebSocket error (cannot reach server):', e);
    logEvent('del', `WS error — cannot reach server at ${WS_URL}`);
    ws.close();
  };
}

function wsSend(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(obj));
  }
}

// ---------------------------------------------------------------------------
// Server message handler
// ---------------------------------------------------------------------------
function onServerMessage(msg) {
  switch (msg.type) {

    case 'welcome': {
      // New connection: server state unknown, clear tracker and rebuild.
      sentToServer.clear();
      // Server told us what it's missing — send those nodes
      const missing = msg.missing ?? [];
      if (missing.length > 0) {
        const delta = store.export_nodes_missing_from(JSON.stringify(missing));
        wsSend({ type: 'pack', nodes: delta });
      }
      // After handshake completes (server now has everything we just told it
      // about PLUS whatever was already in msg.root), mark our current node
      // set as sent so subsequent keystrokes only broadcast the deltas.
      refreshSentToServer();

      // B2: MST-based descent to discover what WE are missing from the server.
      // Only runs when both peers support MST and roots differ.
      const caps = msg.caps ?? {};
      if (caps.supports_mst && msg.mst_root && msg.mst_root !== store.mst_root_hex()) {
        logEvent('sync', `⇕ MST roots differ — starting descent`);
        mstDescend(msg.mst_root, ['']);
      } else if (msg.root !== store.merkle_root_hex()) {
        // Fallback: legacy request of all missing
        wsSend({ type: 'request', known: JSON.parse(store.all_node_ids_json()) });
      }
      logEvent('sync', `↔ handshake: server root ${msg.root.slice(0,10)}…`);

      // Always check for missing blobs immediately after handshake.
      const missingBlobs = JSON.parse(store.missing_blob_hashes_json())
        .filter(h => !pendingBlobFetches.has(h));
      if (missingBlobs.length > 0) {
        missingBlobs.forEach(h => pendingBlobFetches.add(h));
        wsSend({ type: 'blob-request', hashes: missingBlobs });
        logEvent('sync', `↓ blob-fetch: ${missingBlobs.length} hash(es) on connect`);
      }

      // D2: initiate WebRTC to all peers already in the room.
      for (const peerPubkey of (msg.peers ?? [])) {
        if (peerPubkey !== myPubkey) rtcEnsurePeer(peerPubkey);
      }
      break;
    }

    case 'mst-response': {
      // B2: process server's wire nodes, advance the descent state.
      const serverNodes = msg.nodes ?? [];
      const myRoot  = store.mst_root_hex();
      const result  = JSON.parse(
        store.mst_process_response_json(JSON.stringify(serverNodes), myRoot, mstDescentRoot)
      );

      if (result.next_paths.length > 0) {
        // More levels to traverse — send next batch.
        wsSend({ type: 'mst-request', paths: result.next_paths });
      } else {
        // Descent complete.
        if (result.only_theirs.length > 0) {
          // Ask server to send us the nodes we are missing.
          wsSend({ type: 'mst-done', ids: result.only_theirs });
        }
        if (result.only_mine.length > 0) {
          // Upload nodes the server is missing.
          try {
            const pack = store.export_nodes_missing_from(JSON.stringify([...Object.keys(Object.fromEntries(result.only_theirs.map(k=>[k,1])))]));
            const delta = store.export_nodes_missing_from(JSON.stringify(result.only_theirs));
            wsSend({ type: 'pack', nodes: delta });
          } catch (_) {}
        }
        mstDescentRoot = null;
        logEvent('sync', `✓ MST descent complete: +${result.only_theirs.length} from server, sent ${result.only_mine.length} to server`);
      }
      break;
    }

    case 'pack': {
      mergePackAndRequestBlobs(msg.nodes, msg.from ?? 'server');
      break;
    }

    case 'blob-pack': {
      const blobs = msg.blobs ?? [];
      // Clear pending for everything the server responded about (delivered or not).
      // This allows blob-available to re-trigger a request if the server later
      // receives a blob it previously couldn't serve.
      (msg.requested ?? []).forEach(h => pendingBlobFetches.delete(h));
      let rx = 0;
      for (const { hash, data } of blobs) {
        const bytes = base64Decode(data);
        try { store.store_blob_bytes(hash, bytes); saveBlob(hash, bytes); } catch (_) {}
        rx += bytes.length;
      }
      blobStats.received += rx;
      if (rx > 0) {
        renderState();
        renderBlobStats();
        logEvent('merge', `← blob(s) received: ${blobs.length} (${fmtBytes(rx)})`);
      }
      break;
    }

    case 'blob-available': {
      // Server signals a blob is now stored and fetchable (e.g. a peer just
      // uploaded it). Always re-request if we need it — bypass pendingBlobFetches
      // because a previous request may have gone unanswered (server didn't have
      // it yet). The blob is definitively available now.
      const available = msg.hashes ?? [];
      const myMissing = new Set(JSON.parse(store.missing_blob_hashes_json()));
      const want = available.filter(h => myMissing.has(h));
      if (want.length > 0) {
        want.forEach(h => pendingBlobFetches.add(h));
        wsSend({ type: 'blob-request', hashes: want });
        logEvent('sync', `↓ blob-fetch: ${want.length} hash(es) now available`);
      }
      break;
    }

    case 'presence': {
      const p = peers.get(msg.from) ?? { color: randomColor(msg.from), root: '?' };
      peers.set(msg.from, { ...p, ...msg.data, lastSeen: Date.now() });
      renderPeers();
      break;
    }

    case 'error': {
      logEvent('del', `⚠ server: ${msg.msg}`);
      break;
    }

    // D2: new peer joined the room — initiate WebRTC if we have smaller pubkey.
    case 'peer-joined': {
      if (msg.pubkey && msg.pubkey !== myPubkey) {
        rtcEnsurePeer(msg.pubkey);
        logEvent('sync', `peer joined: ${msg.pubkey.slice(0,8)}…`);
      }
      break;
    }

    // D2: peer left — close and clean up the WebRTC connection.
    case 'peer-left': {
      const peer = rtcPeers.get(msg.from);
      if (peer) { peer.close(); rtcPeers.delete(msg.from); }
      logEvent('sync', `peer left: ${String(msg.from).slice(0,8)}…`);
      break;
    }

    // D2: WebRTC signaling messages relayed by the server.
    case 'webrtc-offer':
    case 'webrtc-answer':
    case 'webrtc-ice': {
      rtcHandleSignal(msg);
      break;
    }

    // C3: server confirms the room is now locked.
    case 'room-locked': {
      logEvent('sync', `🔒 room locked with key ${msg.pubkey.slice(0, 10)}…`);
      renderRoomAuth();
      break;
    }
  }
}

function mergePackAndRequestBlobs(nodesJson, from) {
  try { store.import_pack(nodesJson); } catch (_) {}
  // Anything the server just sent, the server obviously has — skip re-uploading.
  refreshSentToServer();
  saveState();
  renderState();
  renderCollabText();
  renderPeers();
  logEvent('merge', `← delta from ${String(from).slice(0, 10)}…`);

  const missing = JSON.parse(store.missing_blob_hashes_json())
    .filter(h => !pendingBlobFetches.has(h));
  if (missing.length > 0) {
    missing.forEach(h => pendingBlobFetches.add(h));
    wsSend({ type: 'blob-request', hashes: missing });
    logEvent('sync', `↓ blob-fetch: ${missing.length} hash(es) needed`);
  }
}

// ---------------------------------------------------------------------------
// Offline / Lag toggle

// ---------------------------------------------------------------------------
// Offline / Lag toggle
// ---------------------------------------------------------------------------
window.toggleOffline = function () {
  offline = !offline;
  const btn    = document.getElementById('offline-btn');
  const banner = document.getElementById('offline-banner');

  if (offline) {
    btn.textContent      = 'Come Online';
    btn.style.background = '#16a34a';
    banner.hidden        = false;
    ws?.close();
    peers.clear();
    renderPeers();
    logEvent('sync', 'Gone offline — changes are local only');
  } else {
    btn.textContent      = 'Go Offline';
    btn.style.background = '#dc2626';
    banner.hidden        = true;
    logEvent('sync', 'Back online — reconnecting…');
    wsConnect();
  }
};

// ---------------------------------------------------------------------------
// Local mutations
// ---------------------------------------------------------------------------
window.doSet = function () {
  const key = document.getElementById('key-in').value.trim();
  const val = document.getElementById('val-in').value.trim();
  if (!key) return;
  store.set(key, new TextEncoder().encode(val));
  logEvent('set', `set "${key}" = "${val}"`);
  saveState();
  renderState();
  broadcastDelta();
};

window.doDelete = function () {
  const key = document.getElementById('key-in').value.trim();
  if (!key) return;
  store.delete(key);
  logEvent('del', `delete "${key}"`);
  saveState();
  renderState();
  broadcastDelta();
};

// ---------------------------------------------------------------------------
// Blob Lab (Phase 3)
// ---------------------------------------------------------------------------
window.uploadBlob = function () {
  const key  = document.getElementById('blob-key-in').value.trim();
  const val  = document.getElementById('blob-val-in').value;
  if (!key || !val) return;
  const data = new TextEncoder().encode(val);

  if (store.has_blob(/* we need the hash — compute then check */ '')) {
    blobStats.cacheHits++;
  }
  const hashHex = store.set_blob(key, data);
  saveBlob(hashHex, data);
  logEvent('set', `set_blob "${key}" → ${hashHex.slice(0,8)}… (${fmtBytes(data.length)})`);
  saveState();
  renderState();
  renderBlobStats();

  // Upload metadata node
  broadcastDelta();
  // Upload blob bytes to server so other peers can fetch
  const encoded = base64Encode(data);
  blobStats.sent += data.length;
  renderBlobStats();
  wsSend({ type: 'blob-upload', blobs: [{ hash: hashHex, data: encoded }] });
};

window.generateBlobValue = function () {
  const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,\n';
  let s = '';
  for (let i = 0; i < 50 * 1024; i++) s += chars[Math.floor(Math.random() * chars.length)];
  document.getElementById('blob-val-in').value = s;
};

// ---------------------------------------------------------------------------
// Conflict Lab (Phase 4)
// ---------------------------------------------------------------------------
window.conflictPreset = function (key, value) {
  document.getElementById('key-in').value = key;
  document.getElementById('val-in').value = value;
  doSet();
};

// ---------------------------------------------------------------------------
// Presence (ephemeral — not stored in DAG)
// ---------------------------------------------------------------------------
function sendPresence() {
  if (offline || !ws || ws.readyState !== WebSocket.OPEN) return;
  wsSend({
    type: 'presence',
    data: {
      label:  myId,
      color:  myColor,
      root:   store.merkle_root_hex(),
      nodes:  store.node_count(),
    },
  });
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------
function renderState() {
  const state = JSON.parse(store.resolve_json_with_meta());
  const dec   = new TextDecoder();
  const box   = document.getElementById('my-state');
  const keys  = Object.keys(state).sort();

  document.getElementById('my-root').textContent = store.merkle_root_hex();
  document.getElementById('my-badge').textContent =
    `${store.node_count()} node${store.node_count() !== 1 ? 's' : ''}`;

  if (keys.length === 0) {
    box.innerHTML = '<span class="empty">empty</span>';
    return;
  }
  box.innerHTML = keys.map(k => {
    const { value, lamport, author, blob_hash, blob_size } = state[k];
    const authorShort = author.slice(0, 8);
    let displayVal;
    if (blob_hash != null) {
      if (value != null) {
        const preview = esc(dec.decode(base64Decode(value)).slice(0, 40));
        displayVal = `<span style="color:#a78bfa">[blob ✓ ${fmtBytes(blob_size)}]</span> `
          + `<span style="color:#94a3b8">${preview}…</span>`;
      } else {
        displayVal = `<span style="color:#f87171">[blob ⏳ ${blob_hash.slice(0,8)}… pending]</span>`;
      }
    } else {
      displayVal = `<span style="color:#e2e8f0">${esc(dec.decode(base64Decode(value)))}</span>`;
    }
    return `<div class="entry">${esc(k)}: ${displayVal}`
      + `<span class="meta" title="HLC lamport=${lamport}, author=${author}">hlc:${lamport} @${authorShort}</span></div>`;
  }).join('');
}

// ---------------------------------------------------------------------------
// C1: Collaborative Text (RGA)
// ---------------------------------------------------------------------------
const COLLAB_KEY = 'collab/doc';

function renderCollabText() {
  const ta = document.getElementById('collab-textarea');
  if (!ta) return;
  const resolved = store.resolve_text(COLLAB_KEY);
  if (ta.value !== resolved) {
    // Preserve cursor if we're not the active element (remote update).
    const active = document.activeElement === ta;
    const sel    = ta.selectionStart;
    ta.value = resolved;
    if (active) {
      const clamped = Math.min(sel, resolved.length);
      ta.setSelectionRange(clamped, clamped);
    }
  }
  const stats = document.getElementById('collab-stats');
  if (stats) stats.textContent = `${resolved.length} char${resolved.length !== 1 ? 's' : ''} · ${store.node_count()} ops`;
}

// Wire up the collaborative textarea using `beforeinput` so we intercept
// every edit before the browser applies it, call the RGA bridge methods,
// then re-render from resolved state.
(function wireCollabTextarea() {
  const ta = document.getElementById('collab-textarea');
  if (!ta) return;

  ta.addEventListener('beforeinput', (e) => {
    e.preventDefault();
    const start = ta.selectionStart;
    const end   = ta.selectionEnd;

    if (e.inputType === 'insertText' && e.data) {
      // Insert each character at the current cursor position.
      let pos = start;
      // Delete the selection first, if any (back to front to keep indices stable).
      if (start !== end) {
        for (let i = end - 1; i >= start; i--) {
          try { store.delete_text(COLLAB_KEY, i); } catch (_) {}
        }
        pos = start;
      }
      for (const ch of e.data) {
        try { store.insert_text(COLLAB_KEY, pos, ch); pos++; } catch (_) {}
      }
    } else if (e.inputType === 'deleteContentBackward') {
      if (start !== end) {
        for (let i = end - 1; i >= start; i--) {
          try { store.delete_text(COLLAB_KEY, i); } catch (_) {}
        }
      } else if (start > 0) {
        try { store.delete_text(COLLAB_KEY, start - 1); } catch (_) {}
      }
    } else if (e.inputType === 'deleteContentForward') {
      if (start !== end) {
        for (let i = end - 1; i >= start; i--) {
          try { store.delete_text(COLLAB_KEY, i); } catch (_) {}
        }
      } else {
        const cur = store.resolve_text(COLLAB_KEY);
        if (start < cur.length) try { store.delete_text(COLLAB_KEY, start); } catch (_) {}
      }
    } else if (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph') {
      let pos = start;
      if (start !== end) {
        for (let i = end - 1; i >= start; i--) {
          try { store.delete_text(COLLAB_KEY, i); } catch (_) {}
        }
        pos = start;
      }
      try { store.insert_text(COLLAB_KEY, pos, '\n'); } catch (_) {}
    }

    saveState();
    renderCollabText();

    // Advance cursor to reflect the edit.
    const resolved = store.resolve_text(COLLAB_KEY);
    const newCursor = Math.min(
      e.inputType.startsWith('delete') ? start : start + (e.data ? [...e.data].length : 1),
      resolved.length,
    );
    ta.setSelectionRange(newCursor, newCursor);

    // Broadcast the new node(s) to all peers via WebSocket.
    broadcastDelta();
  });
})();

function renderPeers() {
  const myRoot = store.merkle_root_hex();
  const ul     = document.getElementById('peer-list');
  document.getElementById('peer-count').textContent =
    `${peers.size} peer${peers.size !== 1 ? 's' : ''}`;

  if (peers.size === 0) {
    ul.innerHTML = '<li style="color:#475569;font-size:0.8rem;font-style:italic;border:none;background:none">'
      + 'Open another tab to see peers appear here.</li>';
    return;
  }
  ul.innerHTML = [...peers.entries()].map(([pubkey, p]) => {
    const synced   = (p.root ?? '') === myRoot;
    const syncMark = synced
      ? '<span style="color:#4ade80" title="In sync">✓</span>'
      : '<span style="color:#fbbf24" title="Diverged">~</span>';
    const label = p.label ?? pubkey.slice(0, 8);
    return `<li>
      <span class="peer-id" style="color:${esc(p.color ?? '#94a3b8')}">● ${esc(label)}</span>
      <span class="peer-root">${syncMark} ${(p.root ?? '—').slice(0,14)}…</span>
    </li>`;
  }).join('');
}

// ---------------------------------------------------------------------------
// Event log
// ---------------------------------------------------------------------------
function logEvent(type, msg) {
  const ul  = document.getElementById('event-log');
  const li  = document.createElement('li');
  const ts  = new Date().toLocaleTimeString([], { hour12: false });
  const cls = { set:'ev-set', del:'ev-del', sync:'ev-sync', merge:'ev-merge' }[type] ?? '';
  li.innerHTML = `<span class="ts">${ts}</span><span class="${cls}">${esc(msg)}</span>`;
  ul.prepend(li);
  while (ul.children.length > 80) ul.removeChild(ul.lastChild);
}

function setStatus(text) {
  const el = document.getElementById('status');
  if (el) el.textContent = text;
}

function updateServerBadge(connected) {
  const el = document.getElementById('server-badge');
  if (!el) return;
  el.textContent  = connected ? '● server' : '○ server';
  el.style.color  = connected ? '#4ade80' : '#f87171';
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------
function esc(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function randomColor(seed) {
  let h = 0;
  for (const c of seed) h = (h * 31 + c.charCodeAt(0)) & 0xffffffff;
  return COLORS[Math.abs(h) % COLORS.length];
}

function base64Decode(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function base64Encode(data) {
  let bin = '';
  for (const b of data) bin += String.fromCharCode(b);
  return btoa(bin);
}

function hexToBytes(hex) {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

// ---------------------------------------------------------------------------
// D2: WebRTC Peer-to-Peer
//
// Transport architecture:
//   WebSocket  — used for ICE signaling (offer/answer/candidate relay) only.
//   `sync`     — RTCDataChannel, ordered, text frames (JSON).
//                IBF-based node reconciliation; same postcard+base64 packs as WS.
//   `blobs`    — RTCDataChannel, ordered, binary frames.
//                Frame: [32-byte Blake3 hash][raw blob bytes].
//                Blobs up to ~256 KB can be sent in one frame (browser limit).
//
// Initiation rule: the peer with the lexicographically-smaller pubkey hex
// creates the offer. This prevents both peers offering at the same time.
// ---------------------------------------------------------------------------

const STUN_SERVERS = [
  { urls: 'stun:stun.l.google.com:19302' },
  { urls: 'stun:stun1.l.google.com:19302' },
];

// Per-remote-peer WebRTC state.
const rtcPeers = new Map(); // remotePubkey → RtcPeer

class RtcPeer {
  constructor(remotePubkey, isInitiator) {
    this.remotePubkey    = remotePubkey;
    this.isInitiator     = isInitiator;
    this.syncChannel     = null;
    this.blobChannel     = null;
    this._syncReady      = false;
    this._blobReady      = false;
    this._ibfSent        = false;
    this._blobHavesSent  = false;

    this.pc = new RTCPeerConnection({ iceServers: STUN_SERVERS });

    // Relay ICE candidates through the WebSocket signaling path.
    this.pc.onicecandidate = ({ candidate }) => {
      if (candidate) {
        wsSend({ type: 'webrtc-ice', to: remotePubkey, candidate });
      }
    };

    this.pc.onconnectionstatechange = () => {
      const s = this.pc.connectionState;
      logEvent('sync', `RTC ↔ ${remotePubkey.slice(0,8)}: ${s}`);
      if (s === 'failed' || s === 'closed') {
        rtcPeers.delete(remotePubkey);
      }
    };

    if (isInitiator) {
      // Initiator creates both channels; responder receives them via ondatachannel.
      this.syncChannel = this.pc.createDataChannel('sync', { ordered: true });
      this.blobChannel = this.pc.createDataChannel('blobs', { ordered: true });
      this._setupSyncChannel(this.syncChannel);
      this._setupBlobChannel(this.blobChannel);
    } else {
      this.pc.ondatachannel = ({ channel }) => {
        if (channel.label === 'sync')  { this.syncChannel = channel; this._setupSyncChannel(channel); }
        if (channel.label === 'blobs') { this.blobChannel = channel; this._setupBlobChannel(channel); }
      };
    }
  }

  // ── sync channel (text/JSON) ─────────────────────────────────────────────

  _setupSyncChannel(ch) {
    ch.onopen = () => {
      this._syncReady = true;
      logEvent('sync', `RTC sync ↔ ${this.remotePubkey.slice(0,8)} open`);
      // Send our IBF so the remote peer can compute what we're missing.
      ch.send(JSON.stringify({ type: 'ibf', ibf: store.compute_ibf_b64() }));
      this._ibfSent = true;
    };
    ch.onmessage = ({ data }) => {
      try { this._onSyncMsg(JSON.parse(data), ch); } catch (_) {}
    };
    ch.onerror = (e) => logEvent('del', `RTC sync error: ${e.message ?? e}`);
  }

  _onSyncMsg(msg, ch) {
    switch (msg.type) {
      case 'ibf': {
        // Compute the symmetric diff. On overflow (diff > ~26 nodes) fall back
        // to sending our full pack and letting import_pack deduplicate.
        let diff;
        try {
          diff = JSON.parse(store.diff_with_ibf_b64(msg.ibf));
        } catch (_) {
          // ibf-overflow or error — fall back to full pack
          ch.send(JSON.stringify({ type: 'pack', nodes: store.export_all_nodes() }));
          if (!this._ibfSent) {
            ch.send(JSON.stringify({ type: 'ibf', ibf: store.compute_ibf_b64() }));
            this._ibfSent = true;
          }
          break;
        }
        // Send nodes they are missing.
        if (diff.only_mine.length > 0) {
          try {
            const pack = store.export_nodes_missing_from(JSON.stringify(diff.only_theirs));
            ch.send(JSON.stringify({ type: 'pack', nodes: pack }));
          } catch (_) {}
        }
        // Request nodes we are missing (ask the peer to send them to us).
        if (diff.only_theirs.length > 0) {
          ch.send(JSON.stringify({ type: 'want', ids: diff.only_theirs }));
        }
        // Send our own IBF if we haven't yet (responder path).
        if (!this._ibfSent) {
          ch.send(JSON.stringify({ type: 'ibf', ibf: store.compute_ibf_b64() }));
          this._ibfSent = true;
        }
        break;
      }
      case 'pack': {
        try {
          store.import_pack(msg.nodes);
          saveState();
          renderState();
          renderCollabText();
          logEvent('merge', `← RTC pack ↔ ${this.remotePubkey.slice(0,8)}`);
        } catch (_) {}
        break;
      }
      case 'want': {
        // Remote peer wants specific nodes we might have.
        try {
          const pack = store.export_nodes_missing_from(JSON.stringify(msg.ids));
          ch.send(JSON.stringify({ type: 'pack', nodes: pack }));
        } catch (_) {}
        break;
      }
    }
  }

  // ── blob channel (binary) ────────────────────────────────────────────────

  _setupBlobChannel(ch) {
    ch.binaryType = 'arraybuffer';
    ch.onopen = () => {
      this._blobReady = true;
      logEvent('sync', `RTC blobs ↔ ${this.remotePubkey.slice(0,8)} open`);
      // Advertise what we have so the peer can compute what to send us.
      const hashes = JSON.parse(store.local_blob_hashes_json());
      ch.send(JSON.stringify({ type: 'blob-have', hashes }));
      this._blobHavesSent = true;
    };
    ch.onmessage = ({ data }) => {
      if (typeof data === 'string') {
        try { this._onBlobCtrl(JSON.parse(data), ch); } catch (_) {}
      } else {
        this._onBlobFrame(data);
      }
    };
    ch.onerror = (e) => logEvent('del', `RTC blobs error: ${e.message ?? e}`);
  }

  _onBlobCtrl(msg, ch) {
    if (msg.type !== 'blob-have') return;
    const theirSet = new Set(msg.hashes);
    const myHashes = JSON.parse(store.local_blob_hashes_json());
    const toSend   = myHashes.filter(h => !theirSet.has(h));

    for (const hashHex of toSend) {
      try {
        const blobsJson = store.export_blobs_json(JSON.stringify([hashHex]));
        const blobs     = JSON.parse(blobsJson);
        if (!blobs.length) continue;
        const rawBytes  = base64Decode(blobs[0].data);
        // Frame: [32-byte hash][blob bytes]
        const frame = new Uint8Array(32 + rawBytes.length);
        frame.set(hexToBytes(hashHex), 0);
        frame.set(rawBytes, 32);
        ch.send(frame.buffer);
        blobStats.sent += rawBytes.length;
        logEvent('sync', `→ RTC blob ${hashHex.slice(0,8)}: ${fmtBytes(rawBytes.length)}`);
      } catch (_) {}
    }
    renderBlobStats();

    // Send our own blob-have if we haven't (responder path).
    if (!this._blobHavesSent) {
      ch.send(JSON.stringify({ type: 'blob-have', hashes: JSON.parse(store.local_blob_hashes_json()) }));
      this._blobHavesSent = true;
    }
  }

  _onBlobFrame(ab) {
    if (ab.byteLength < 32) return;
    const hashBytes = new Uint8Array(ab, 0, 32);
    const blobBytes = new Uint8Array(ab, 32);
    const hashHex   = Array.from(hashBytes).map(b => b.toString(16).padStart(2,'0')).join('');
    try {
      store.store_blob_bytes(hashHex, blobBytes);
      saveBlob(hashHex, blobBytes);
      blobStats.received += blobBytes.length;
      pendingBlobFetches.delete(hashHex);
      renderState();
      renderBlobStats();
      logEvent('merge', `← RTC blob ${hashHex.slice(0,8)}: ${fmtBytes(blobBytes.length)}`);
    } catch (_) {}
  }

  // ── signaling helpers ────────────────────────────────────────────────────

  async negotiate() {
    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    wsSend({ type: 'webrtc-offer', to: this.remotePubkey, sdp: this.pc.localDescription });
  }

  async handleOffer(sdp) {
    await this.pc.setRemoteDescription(new RTCSessionDescription(sdp));
    const answer = await this.pc.createAnswer();
    await this.pc.setLocalDescription(answer);
    wsSend({ type: 'webrtc-answer', to: this.remotePubkey, sdp: this.pc.localDescription });
  }

  async handleAnswer(sdp) {
    await this.pc.setRemoteDescription(new RTCSessionDescription(sdp));
  }

  async addIce(candidate) {
    if (candidate) await this.pc.addIceCandidate(new RTCIceCandidate(candidate));
  }

  close() { this.pc.close(); }
}

// Create (or return existing) peer connection. Initiation determined by
// lexicographic pubkey ordering — consistent across both peers.
function rtcEnsurePeer(remotePubkey) {
  if (rtcPeers.has(remotePubkey)) return rtcPeers.get(remotePubkey);
  const isInitiator = myPubkey < remotePubkey;
  const peer = new RtcPeer(remotePubkey, isInitiator);
  rtcPeers.set(remotePubkey, peer);
  if (isInitiator) {
    peer.negotiate().catch(e => logEvent('del', `RTC offer failed: ${e}`));
  }
  return peer;
}

// Handle server-relayed WebRTC signaling messages.
function rtcHandleSignal(msg) {
  if (msg.to !== myPubkey) return; // not addressed to us
  switch (msg.type) {
    case 'webrtc-offer': {
      // Always create a fresh responder peer on offer (handles re-negotiation).
      let peer = rtcPeers.get(msg.from);
      if (!peer) { peer = new RtcPeer(msg.from, false); rtcPeers.set(msg.from, peer); }
      peer.handleOffer(msg.sdp).catch(e => logEvent('del', `RTC answer failed: ${e}`));
      break;
    }
    case 'webrtc-answer': {
      const peer = rtcPeers.get(msg.from);
      if (peer) peer.handleAnswer(msg.sdp).catch(() => {});
      break;
    }
    case 'webrtc-ice': {
      const peer = rtcPeers.get(msg.from);
      if (peer) peer.addIce(msg.candidate).catch(() => {});
      break;
    }
  }
}

// ---------------------------------------------------------------------------
// Heartbeats and cleanup

// Retry any still-missing blobs every 10 seconds.
// Handles the race where the initial blob-request beat the uploader to the
// server, or where the server restarted and blobs haven't been re-uploaded yet.
setInterval(() => {
  if (offline || !ws || ws.readyState !== WebSocket.OPEN) return;
  const missing = JSON.parse(store.missing_blob_hashes_json());
  if (missing.length > 0) {
    // Re-request unconditionally (bypass pendingBlobFetches — retry is intentional)
    missing.forEach(h => pendingBlobFetches.add(h));
    wsSend({ type: 'blob-request', hashes: missing });
  }
}, 10_000);

setInterval(() => {
  const cutoff = Date.now() - 30_000;
  let changed = false;
  for (const [id, p] of peers) {
    if ((p.lastSeen ?? 0) < cutoff) { peers.delete(id); changed = true; }
  }
  if (changed) renderPeers();
}, 10_000);

// ---------------------------------------------------------------------------
// Boot (C2: async IDB hydration before first render and connect)
// ---------------------------------------------------------------------------
console.log('[boot] opening IDB…');
db = await openDB().catch((e) => { console.warn('[boot] IDB unavailable:', e); return null; });
console.log('[boot] IDB ready:', db ? 'ok' : 'unavailable');
console.log('[boot] loading nodes…');
await loadNodes();
console.log('[boot] loading blobs…');
await loadBlobs();
console.log('[boot] hydration done — calling wsConnect()');

document.getElementById('my-label').textContent = `You — ${myId}`;
document.getElementById('my-dot').style.background = myColor;
document.getElementById('my-pubkey').textContent = myPubkey.slice(0, 16) + '…';
try { renderState(); } catch (err) { console.error('renderState error:', err); }
try { renderPeers(); } catch (err) { console.error('renderPeers error:', err); }
try { renderRoomAuth(); } catch (err) { console.error('renderRoomAuth error:', err); }
wsConnect();
console.log('[boot] wsConnect() ran');

// ---------------------------------------------------------------------------
// Room Auth (C3)
// ---------------------------------------------------------------------------

function renderRoomAuth() {
  const inp = document.getElementById('room-seed-in');
  const status = document.getElementById('room-auth-status');
  if (!inp || !status) return;
  if (roomSeed) {
    try {
      const pubkey = room_pubkey_hex(roomSeed);
      inp.value = Array.from(roomSeed).map(b => b.toString(16).padStart(2, '0')).join('');
      status.textContent = `Room public key: ${pubkey.slice(0, 16)}…`;
      status.style.color = '#4ade80';
    } catch (_) {}
  } else {
    status.textContent = 'No room seed — room is open';
    status.style.color = '#94a3b8';
  }
}

window.generateRoomSeed = function () {
  const seed = crypto.getRandomValues(new Uint8Array(32));
  setRoomSeed(seed);
  renderRoomAuth();
  logEvent('sync', '🔑 new room seed generated');
};

window.clearRoomSeed = function () {
  setRoomSeed(null);
  renderRoomAuth();
  logEvent('sync', 'room seed cleared — room is open');
};

window.lockRoom = function () {
  if (!roomSeed) { logEvent('del', '⚠ generate a room seed first'); return; }
  try {
    const pubkey = room_pubkey_hex(roomSeed);
    wsSend({ type: 'set-room-key', pubkey });
    logEvent('sync', `🔒 sent set-room-key: ${pubkey.slice(0, 10)}…`);
  } catch (e) {
    logEvent('del', `⚠ lock failed: ${e}`);
  }
};
