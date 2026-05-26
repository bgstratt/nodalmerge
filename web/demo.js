/**
 * demo.js — ActiveSync Production Demo (F1 SDK port)
 *
 * Rewritten to consume the high-level `createDoc` SDK from `./sdk.js`.
 * The SDK owns the wire protocol, MST/IBF handshake, blob fan-out, presence
 * heartbeat+sweep, and token minting. This file is UI + local persistence.
 *
 * Features retained from the pre-SDK demo:
 *   • Identity bootstrap (Ed25519 seed in sessionStorage).
 *   • Room-auth seed management + `set-room-key` (via `doc.send` escape hatch).
 *   • IndexedDB persistence of nodes + blobs (SDK doesn't opine on storage).
 *   • Key/Value lab, Blob Lab, Conflict Lab, Collaborative Text (RGA).
 *   • Peer list, event log, server badge, offline toggle.
 *
 * Dropped from this port:
 *   • Direct WebRTC P2P (D2). The SDK does not yet surface raw signaling
 *     messages (`peer-joined`, `webrtc-offer/answer/ice`); WebSocket sync
 *     covers the same ground for the demo. Re-adding P2P is tracked as a
 *     post-F1 SDK extension.
 *
 * See `docs/sdk.md` for the full SDK surface.
 */

import { createDoc, room_pubkey_hex } from './sdk.js?v=2';

// Catch any unhandled promise rejections from the module itself.
window.addEventListener('unhandledrejection', ev => {
  console.error('[activesync] unhandledRejection:', ev.reason);
  const s = document.getElementById('status');
  if (s) s.textContent = 'Module error: ' + ev.reason;
});

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------
const DEFAULT_ROOM_ID = 'default';
const DEFAULT_SERVER_URL = 'ws://127.0.0.1:5271';
const COLORS     = ['#60a5fa','#4ade80','#f472b6','#fb923c','#a78bfa','#34d399','#fbbf24','#f87171'];
const IDB_NAME    = 'activesync-v7';
const IDB_VERSION = 1;
const COLLAB_KEY  = 'collab/doc';
const LIST_KEY    = 'demo/list';
const STORAGE_KEYS = {
  serverUrl: 'nodalmerge-server-url',
  serverUrlLegacy: 'activesync-server-url',
  roomId: 'nodalmerge-room-id',
  roomIdLegacy: 'activesync-room-id',
  identity: 'nodalmerge-identity-v3',
  identityLegacy: 'activesync-identity-v3',
  roomSeed: 'nodalmerge-room-seed',
  roomSeedLegacy: 'activesync-room-seed'
};
let uiRefreshTimer = null;
const ENV_SERVER_URL = (globalThis.ACTIVESYNC_CONFIG && globalThis.ACTIVESYNC_CONFIG.serverUrl)
  ? String(globalThis.ACTIVESYNC_CONFIG.serverUrl).trim()
  : '';

function scheduleUiRefresh() {
  if (uiRefreshTimer) return;
  uiRefreshTimer = setTimeout(() => {
    uiRefreshTimer = null;
    renderState();
    renderCollabText();
    renderList();
    renderPeers();
    renderBlobStats();
  }, 50);
}

function resolveServerUrl() {
  const params = new URLSearchParams(window.location.search);
  const fromQuery = params.get('server') || params.get('serverUrl') || params.get('ws');
  const fromSession = sessionStorage.getItem(STORAGE_KEYS.serverUrl)
    || sessionStorage.getItem(STORAGE_KEYS.serverUrlLegacy);
  const candidate = (fromQuery || fromSession || ENV_SERVER_URL || DEFAULT_SERVER_URL).trim();

  try {
    const parsed = new URL(candidate);
    if (parsed.protocol === 'http:') parsed.protocol = 'ws:';
    if (parsed.protocol === 'https:') parsed.protocol = 'wss:';
    if (parsed.protocol !== 'ws:' && parsed.protocol !== 'wss:') {
      throw new Error(`unsupported protocol: ${parsed.protocol}`);
    }
    const normalized = parsed.toString().replace(/\/$/, '');
    sessionStorage.setItem(STORAGE_KEYS.serverUrl, normalized);
    return normalized;
  } catch (e) {
    console.warn('[boot] invalid server url override, falling back to default:', candidate, e);
    sessionStorage.setItem(STORAGE_KEYS.serverUrl, DEFAULT_SERVER_URL);
    return DEFAULT_SERVER_URL;
  }
}

function normalizeRoomId(raw) {
  const value = String(raw || '').trim();
  return value
    .replace(/[^a-zA-Z0-9._-]/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '') || DEFAULT_ROOM_ID;
}

function resolveRoomId() {
  const params = new URLSearchParams(window.location.search);
  const fromQuery = params.get('room');
  const fromSession = sessionStorage.getItem(STORAGE_KEYS.roomId)
    || sessionStorage.getItem(STORAGE_KEYS.roomIdLegacy);
  const normalized = normalizeRoomId(fromQuery || fromSession || DEFAULT_ROOM_ID);
  sessionStorage.setItem(STORAGE_KEYS.roomId, normalized);
  return normalized;
}

const SERVER_URL = resolveServerUrl();
const ROOM_ID = resolveRoomId();

// ---------------------------------------------------------------------------
// Identity — Ed25519 seed in sessionStorage (survives refresh, not close)
// ---------------------------------------------------------------------------
function getOrCreateIdentity() {
  const stored = sessionStorage.getItem(STORAGE_KEYS.identity)
    || sessionStorage.getItem(STORAGE_KEYS.identityLegacy);
  if (stored) {
    sessionStorage.setItem(STORAGE_KEYS.identity, stored);
    return JSON.parse(stored);
  }
  const seed  = crypto.getRandomValues(new Uint8Array(32));
  const color = COLORS[Math.floor(Math.random() * COLORS.length)];
  const id    = Array.from(seed.slice(0, 4)).map(b => b.toString(16).padStart(2,'0')).join('');
  const identity = { seed: Array.from(seed), id, color };
  sessionStorage.setItem(STORAGE_KEYS.identity, JSON.stringify(identity));
  return identity;
}

const { seed: seedArr, id: myId, color: myColor } = getOrCreateIdentity();
const authorSeed = new Uint8Array(seedArr);

// ---------------------------------------------------------------------------
// Room-auth seed (C3) — sessionStorage; null = open room
// ---------------------------------------------------------------------------
function loadRoomSeed() {
  const stored = sessionStorage.getItem(STORAGE_KEYS.roomSeed)
    || sessionStorage.getItem(STORAGE_KEYS.roomSeedLegacy);
  if (!stored) return null;
  try {
    sessionStorage.setItem(STORAGE_KEYS.roomSeed, stored);
    return new Uint8Array(JSON.parse(stored));
  } catch (_) {
    return null;
  }
}

function saveRoomSeed(seed32) {
  if (seed32) sessionStorage.setItem(STORAGE_KEYS.roomSeed, JSON.stringify(Array.from(seed32)));
  else sessionStorage.removeItem(STORAGE_KEYS.roomSeed);
}

let roomSeed = loadRoomSeed();

// ---------------------------------------------------------------------------
// IndexedDB persistence (C2). Kept client-side — the SDK doesn't opine on
// local storage. Nodes: one key 'all' → pack string. Blobs: hash → bytes.
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
    tx.onerror    = resolve;
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

// Debounced full-graph save so typing doesn't thrash IDB.
function saveState() {
  if (saveState._t) return;
  saveState._t = setTimeout(async () => {
    saveState._t = null;
    try { await idbPut('nodes', 'all', doc.store.export_all_nodes()); } catch (_) {}
  }, 250);
}

function saveBlob(hashHex, bytes) { idbPut('blobs', hashHex, bytes); }

// ---------------------------------------------------------------------------
// Stats / local state
// ---------------------------------------------------------------------------
const blobStats = { sent: 0, received: 0, cacheHits: 0 };
let lastKnownBlobHashes = new Set(); // local blob hashes at last render
let offline = false;
let stateCache = {};
let collabTextCache = '';
let listCache = [];
let lastKnownRoot = '—';
let lastKnownNodeCount = 0;
let collabTextBroken = true;

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

// ---------------------------------------------------------------------------
// Logging / status
// ---------------------------------------------------------------------------
function esc(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function logEvent(type, msg) {
  const ul = document.getElementById('event-log');
  if (!ul) return;
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
  el.textContent = connected ? '● server' : '○ server';
  el.style.color = connected ? '#4ade80' : '#f87171';
}

function base64Decode(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function formatResolvedValueForDisplay(raw) {
  if (typeof raw !== 'string') return String(raw);

  try {
    const bytes = base64Decode(raw);
    const text = new TextDecoder().decode(bytes);
    const parsed = JSON.parse(text);
    if (typeof parsed === 'string') return parsed;
    if (parsed === null || typeof parsed === 'number' || typeof parsed === 'boolean') {
      return String(parsed);
    }
    return JSON.stringify(parsed);
  } catch (_) {
    return raw;
  }
}

function formatResolvedKeyForDisplay(key) {
  if (key.startsWith('demo/kv/')) {
    return {
      label: key.slice('demo/kv/'.length),
      meta: 'map key',
    };
  }
  return { label: key, meta: null };
}

// ---------------------------------------------------------------------------
// Boot: hydrate from IDB, then createDoc with authorSeed + roomSeed.
// ---------------------------------------------------------------------------
console.log('[boot] opening IDB…');
db = await openDB().catch((e) => { console.warn('[boot] IDB unavailable:', e); return null; });
console.log('[boot] IDB ready:', db ? 'ok' : 'unavailable');

// Construct the doc first (autoConnect:false) so we can hydrate into its
// store before the first wire handshake.
setStatus('Loading WASM…');
const doc = await createDoc({
  serverUrl: SERVER_URL,
  room: ROOM_ID,
  authorSeed,
  roomSeed,
  transport: 'ws-only',
  autoConnect: false,
  // G8 — surface metric events on a debug channel so the demo exercises
  // the hook end-to-end. Real apps would forward to OpenTelemetry / a
  // counter/histogram backend instead of console.
  onMetric: (ev) => {
    console.debug('[metric]', ev.kind, ev.value, ev.labels);
  },
});
const myPubkey = doc.pubkeyHex;

// Dev convenience: expose the doc on window so `doc.peers()` etc. work from
// the browser console. Harmless in demos; strip in production embeds.
if (typeof window !== 'undefined') window.doc = doc;

// Hydrate nodes + blobs from IDB into the SDK's underlying SyncStore.
console.log('[boot] loading nodes…');
const storedNodes = await idbGet('nodes', 'all');
if (storedNodes) { try { doc.store.import_pack(storedNodes); } catch (_) {} }
console.log('[boot] loading blobs…');
await idbCursor('blobs', (hashHex, bytes) => {
  try { doc.store.store_blob_bytes(hashHex, bytes); } catch (_) {}
});
for (const h of JSON.parse(doc.store.local_blob_hashes_json())) lastKnownBlobHashes.add(h);
try {
  stateCache = JSON.parse(doc.store.resolve_json());
} catch (_) {
  stateCache = {};
}
try { lastKnownRoot = doc.store.merkle_root_hex(); } catch (_) {}
try { lastKnownNodeCount = doc.store.node_count(); } catch (_) {}
console.log('[boot] hydration done — connecting…');

// Wire header fields now that we have pubkey.
document.getElementById('my-label').textContent = `You — ${myId}`;
document.getElementById('my-dot').style.background = myColor;
document.getElementById('my-pubkey').textContent = myPubkey.slice(0, 16) + '…';

renderState();
renderPeers();
renderRoomAuth();
renderBlobStats();

// ---------------------------------------------------------------------------
// SDK hooks → UI
// ---------------------------------------------------------------------------
doc.onConnect(() => {
  setStatus('Connected');
  updateServerBadge(true);
  logEvent('sync', `✓ connected → ${SERVER_URL}/ws/${ROOM_ID}`);
  // Seed our presence so other peers discover us immediately. The SDK
  // rebroadcasts on peer-joined + heartbeats on a timer.
  doc.presence.set({
    label: myId,
    color: myColor,
    root:  lastKnownRoot,
    nodes: lastKnownNodeCount,
  });
});

doc.onDisconnect(() => {
  updateServerBadge(false);
  if (!offline) {
    setStatus('Disconnected — reconnecting…');
    logEvent('del', 'WS closed — SDK will auto-reconnect');
  }
});

doc.onError((err) => {
  logEvent('del', `⚠ ${err.message ?? err}`);
});

doc.onChange((ev) => {
  if (ev && ev.source === 'remote') {
    const via = ev.transport ? ` via ${ev.transport}` : '';
    const from = ev.from ? ` from ${String(ev.from).slice(0, 8)}…` : '';
    const kind = ev.type === 'blob' ? `blob ${String(ev.hash || '').slice(0, 8)}…` : (ev.type || 'delta');
    logEvent('sync', `← received ${kind}${via}${from}`);
  }
  queueMicrotask(() => {
    saveState();
    // Persist any newly-arrived blobs to IDB.
    try {
      const hashes = JSON.parse(doc.store.local_blob_hashes_json());
      for (const h of hashes) {
        if (!lastKnownBlobHashes.has(h)) {
          lastKnownBlobHashes.add(h);
          try {
            const bytes = doc.store.get_blob_bytes(h);
            if (bytes) {
              saveBlob(h, bytes);
              blobStats.received += bytes.length;
            }
          } catch (_) {}
        }
      }
    } catch (_) {}
    // Keep presence fresh with our current root + node count so peers
    // can tell when we're in sync.
    if (doc.isConnected) {
      doc.presence.set({
        label: myId,
        color: myColor,
        root:  lastKnownRoot,
        nodes: lastKnownNodeCount,
      });
    }

    // Keep local UI caches aligned with canonical merged state so
    // cross-peer updates are reflected without requiring local edits.
    try {
      stateCache = JSON.parse(doc.store.resolve_json());
    } catch (_) {}
    try { lastKnownRoot = doc.store.merkle_root_hex(); } catch (_) {}
    try { lastKnownNodeCount = doc.store.node_count(); } catch (_) {}
    if (!collabTextBroken) {
      try { collabTextCache = collabText.toString(); } catch (_) {}
    } else {
      try {
        const next = collabFallbackMap.get('text');
        collabTextCache = typeof next === 'string' ? next : '';
      } catch (_) {}
    }

    scheduleUiRefresh();
  });
});

doc.presence.onJoin(()   => renderPeers());
doc.presence.onUpdate(() => renderPeers());
doc.presence.onLeave(()  => renderPeers());

// Kick off the connection.
setStatus('Connecting…');
doc.connect();

// ---------------------------------------------------------------------------
// Key/Value lab (empty namespace → keys are used as-is). The SDK's `set`
// JSON-encodes its value (so `set('color','red')` stores 5 bytes `"red"`);
// the state box below renders those bytes verbatim, so scalar values appear
// quoted. The conflict-lab presets are still illustrative — what matters for
// the demo is that both tabs converge on the same winner.
// ---------------------------------------------------------------------------
const kv = doc.map('demo/kv');

window.doSet = function () {
  const key = document.getElementById('key-in').value.trim();
  const val = document.getElementById('val-in').value;
  if (!key) return;
  stateCache = { ...stateCache, [key]: val };
  lastKnownNodeCount = Object.keys(stateCache).length;
  kv.set(key, val);
  logEvent('set', `set "${key}" = ${JSON.stringify(val)}`);
  saveState();
  scheduleUiRefresh();
};

window.doDelete = function () {
  const key = document.getElementById('key-in').value.trim();
  if (!key) return;
  const next = { ...stateCache };
  delete next[key];
  stateCache = next;
  lastKnownNodeCount = Object.keys(stateCache).length;
  kv.delete(key);
  logEvent('del', `delete "${key}"`);
  saveState();
  scheduleUiRefresh();
};

// ---------------------------------------------------------------------------
// Blob Lab
// ---------------------------------------------------------------------------
window.uploadBlob = function () {
  const key = document.getElementById('blob-key-in').value.trim();
  const val = document.getElementById('blob-val-in').value;
  if (!key || !val) return;
  const data = new TextEncoder().encode(val);
  // Cache-hit heuristic: if we already have a blob with this exact hash.
  // (We compute it implicitly: set_blob returns the Blake3 hash.)
  const hashHex = kv.setBlob(key, data);
  if (lastKnownBlobHashes.has(hashHex)) blobStats.cacheHits++;
  else lastKnownBlobHashes.add(hashHex);
  saveBlob(hashHex, data);
  blobStats.sent += data.length;
  logEvent('set', `set_blob "${key}" → ${hashHex.slice(0,8)}… (${fmtBytes(data.length)})`);
  saveState();
  scheduleUiRefresh();
};

window.generateBlobValue = function () {
  const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,\n';
  let s = '';
  for (let i = 0; i < 50 * 1024; i++) s += chars[Math.floor(Math.random() * chars.length)];
  document.getElementById('blob-val-in').value = s;
};

// ---------------------------------------------------------------------------
// Conflict Lab
// ---------------------------------------------------------------------------
window.conflictPreset = function (key, value) {
  document.getElementById('key-in').value = key;
  document.getElementById('val-in').value = value;
  doSet();
};

// ---------------------------------------------------------------------------
// Offline toggle
// ---------------------------------------------------------------------------
window.toggleOffline = function () {
  offline = !offline;
  const btn    = document.getElementById('offline-btn');
  const banner = document.getElementById('offline-banner');
  if (offline) {
    btn.textContent = 'Come Online';
    btn.style.background = '#16a34a';
    banner.hidden = false;
    doc.disconnect();
    renderPeers();
    logEvent('sync', 'Gone offline — changes are local only');
  } else {
    btn.textContent = 'Go Offline';
    btn.style.background = '#dc2626';
    banner.hidden = true;
    logEvent('sync', 'Back online — reconnecting…');
    doc.connect();
  }
};

// ---------------------------------------------------------------------------
// Collaborative text (RGA, C1) — wire the textarea through `doc.text`.
// ---------------------------------------------------------------------------
const collabText = doc.text(COLLAB_KEY);
const collabFallbackMap = doc.map('demo/collab-fallback');
if (collabTextBroken) {
  try {
    const next = collabFallbackMap.get('text');
    collabTextCache = typeof next === 'string' ? next : '';
  } catch (_) {
    collabTextCache = '';
  }
} else {
  try { collabTextCache = collabText.toString(); } catch (_) { collabTextCache = ''; }
}

function publishFallbackText(next) {
  queueMicrotask(() => {
    try {
      collabFallbackMap.set('text', next);
    } catch (err) {
      logEvent('del', `⚠ ${err.message ?? err}`);
    }
  });
}

collabFallbackMap.onChange(() => {
  try {
    const next = collabFallbackMap.get('text');
    if (typeof next === 'string') {
      collabTextCache = next;
      // If any peer has switched to fallback writes, converge all peers on
      // that path until text-engine stability is restored.
      collabTextBroken = true;
    }
  } catch (_) {
    if (collabTextBroken) collabTextCache = '';
  }
  scheduleUiRefresh();
});

function renderCollabText() {
  const ta = document.getElementById('collab-textarea');
  if (!ta) return;
  const resolved = collabTextCache;
  if (ta.value !== resolved) {
    const active = document.activeElement === ta;
    const sel    = ta.selectionStart;
    ta.value = resolved;
    if (active) {
      const clamped = Math.min(sel, resolved.length);
      ta.setSelectionRange(clamped, clamped);
    }
  }
  const stats = document.getElementById('collab-stats');
  if (stats) {
    const mode = collabTextBroken ? 'LWW fallback' : 'RGA';
    stats.textContent = `${resolved.length} char${resolved.length !== 1 ? 's' : ''} · ${mode}`;
  }
}

(function wireCollabTextarea() {
  const ta = document.getElementById('collab-textarea');
  if (!ta) return;

  ta.addEventListener('input', () => {
    if (!collabTextBroken) return;
    collabTextCache = ta.value;
    publishFallbackText(collabTextCache);
    saveState();
    scheduleUiRefresh();
  });

  ta.addEventListener('beforeinput', (e) => {
    if (collabTextBroken) {
      return;
    }

    e.preventDefault();

    const start = ta.selectionStart;
    const end   = ta.selectionEnd;

    function deleteRange(from, to) {
      // Delete back-to-front so indices stay stable.
      if (to > from) collabText.delete(from, to - from);
    }

    if (e.inputType === 'insertText' && e.data) {
      if (start !== end) deleteRange(start, end);
      try {
        collabText.insert(start, e.data);
      } catch (err) {
        collabTextBroken = true;
        logEvent('del', `⚠ text engine fallback enabled: ${err.message ?? err}`);
        const next = collabTextCache.slice(0, start) + e.data + collabTextCache.slice(end);
        collabTextCache = next;
        publishFallbackText(next);
        saveState();
        scheduleUiRefresh();
        return;
      }
      collabTextCache = collabTextCache.slice(0, start) + e.data + collabTextCache.slice(start);
    } else if (e.inputType === 'deleteContentBackward') {
      if (start !== end) deleteRange(start, end);
      else if (start > 0) {
        collabText.delete(start - 1, 1);
        collabTextCache = collabTextCache.slice(0, start - 1) + collabTextCache.slice(start);
      }
    } else if (e.inputType === 'deleteContentForward') {
      if (start !== end) deleteRange(start, end);
      else {
        if (start < ta.value.length) {
          collabText.delete(start, 1);
          collabTextCache = collabTextCache.slice(0, start) + collabTextCache.slice(start + 1);
        }
      }
    } else if (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph') {
      if (start !== end) deleteRange(start, end);
      collabText.insert(start, '\n');
      collabTextCache = collabTextCache.slice(0, start) + '\n' + collabTextCache.slice(start);
    }

    saveState();
    scheduleUiRefresh();

    const delta = e.inputType.startsWith('delete')
      ? 0
      : (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph'
          ? 1
          : (e.data ? [...e.data].length : 0));
    setTimeout(() => {
      try {
        const newCursor = e.inputType.startsWith('delete') ? Math.max(0, start - 1) : start + delta;
        ta.setSelectionRange(newCursor, newCursor);
      } catch (_) {}
    }, 0);
  });
})();

// ---------------------------------------------------------------------------
// Ordered List (F8) — drag-to-reorder + drop-onto-to-replace.
//
// Uses `doc.list(LIST_KEY)` whose ordering lives in Op::List. Item content
// (just a string label here) lives in the sidecar Map at
// `${LIST_KEY}/items/<itemId>`. The SDK composes the two automatically.
// ---------------------------------------------------------------------------
const listHandle = doc.list(LIST_KEY);
function refreshListCache() {
  try { listCache = listHandle.toArray(); } catch (_) { listCache = []; }
}
refreshListCache();

// drag state — tracked here because HTML5 DnD's dataTransfer is awkward
// for intra-page IDs across subtle browser differences.
let listDragId = null;

function renderList() {
  const ul = document.getElementById('list-items');
  if (!ul) return;
  refreshListCache();
  const items = listCache;
  const stats = document.getElementById('list-stats');
  if (stats) stats.textContent = `${items.length} item${items.length !== 1 ? 's' : ''}`;

  if (items.length === 0) {
    ul.innerHTML = '<li style="color:#475569;font-style:italic;border:none;background:none;cursor:default">'
      + 'Empty — add an item above, or open another tab and drag.</li>';
    return;
  }

  ul.innerHTML = items.map(({ id, content }) => {
    const label = content && typeof content.label === 'string' ? content.label : '(no label)';
    return `<li draggable="true" data-id="${esc(id)}">
      <span style="color:#64748b;font-family:monospace;font-size:0.7rem">${esc(id.slice(0, 6))}</span>
      <span>${esc(label)}</span>
      <button data-del="${esc(id)}" title="Delete">✕</button>
    </li>`;
  }).join('');

  // Wire delete buttons.
  ul.querySelectorAll('button[data-del]').forEach(btn => {
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      listHandle.delete(btn.dataset.del);
      saveState();
      scheduleUiRefresh();
    });
  });

  // Wire DnD on each row.
  ul.querySelectorAll('li[draggable="true"]').forEach(li => {
    li.addEventListener('dragstart', (e) => {
      listDragId = li.dataset.id;
      li.classList.add('dragging');
      try { e.dataTransfer.effectAllowed = 'move'; } catch (_) {}
      try { e.dataTransfer.setData('text/plain', listDragId); } catch (_) {}
    });
    li.addEventListener('dragend', () => {
      li.classList.remove('dragging');
      clearDropHints(ul);
      listDragId = null;
    });
    li.addEventListener('dragover', (e) => {
      if (!listDragId || li.dataset.id === listDragId) return;
      e.preventDefault();
      try { e.dataTransfer.dropEffect = 'move'; } catch (_) {}
      clearDropHints(ul);
      const region = dropRegion(e, li); // 'before' | 'onto' | 'after'
      li.classList.add(region === 'onto' ? 'drop-onto'
                      : region === 'before' ? 'drop-before' : 'drop-after');
    });
    li.addEventListener('dragleave', () => li.classList.remove('drop-before', 'drop-after', 'drop-onto'));
    li.addEventListener('drop', (e) => {
      e.preventDefault();
      clearDropHints(ul);
      const dragged = listDragId;
      const target  = li.dataset.id;
      listDragId = null;
      if (!dragged || dragged === target) return;
      const region = dropRegion(e, li);
      // Gesture dispatch — convergence is the SDK's job.
      if (region === 'onto')        listHandle.gestures.dropOnto(dragged, target);
      else if (region === 'before') listHandle.gestures.dropBefore(dragged, target);
      else                          listHandle.gestures.dropAfter(dragged, target);
      saveState();
      scheduleUiRefresh();
    });
  });
}

function clearDropHints(ul) {
  ul.querySelectorAll('li').forEach(x => x.classList.remove('drop-before', 'drop-after', 'drop-onto'));
}

// Split each row into thirds vertically: top→drop-before, middle→drop-onto,
// bottom→drop-after. Mirrors VS Code / Finder drag affordances.
function dropRegion(e, li) {
  const r = li.getBoundingClientRect();
  const y = e.clientY - r.top;
  if (y < r.height / 3)      return 'before';
  if (y > (2 * r.height) / 3) return 'after';
  return 'onto';
}

(function wireListControls() {
  const input = document.getElementById('list-input');
  const addBtn = document.getElementById('list-add');
  if (!input || !addBtn) return;
  const add = () => {
    const label = input.value.trim();
    if (!label) return;
    listHandle.push({ label });
    input.value = '';
    saveState();
    scheduleUiRefresh();
  };
  addBtn.addEventListener('click', add);
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') add(); });
})();

listHandle.onChange(() => {
  refreshListCache();
  scheduleUiRefresh();
});

// Initial list render.
renderList();

// ---------------------------------------------------------------------------
// Render — resolved state + peer list
// ---------------------------------------------------------------------------
function renderState() {
  const box   = document.getElementById('my-state');
  const keys  = Object.keys(stateCache).sort();

  document.getElementById('my-root').textContent = lastKnownRoot;
  document.getElementById('my-badge').textContent =
    `${lastKnownNodeCount} node${lastKnownNodeCount !== 1 ? 's' : ''}`;

  if (keys.length === 0) {
    box.innerHTML = '<span class="empty">empty</span>';
    return;
  }
  box.innerHTML = keys.map(k => {
    const { label, meta } = formatResolvedKeyForDisplay(k);
    const displayVal = esc(formatResolvedValueForDisplay(stateCache[k]));
    const metaHtml = meta ? ` <span class="meta">(${esc(meta)})</span>` : '';
    return `<div class="entry" title="${esc(k)}">${esc(label)}${metaHtml}: <span style="color:#e2e8f0">${displayVal}</span></div>`;
  }).join('');
}

function renderPeers() {
  const myRoot = lastKnownRoot;
  const ul     = document.getElementById('peer-list');
  const others = doc.presence.others();
  document.getElementById('peer-count').textContent =
    `${others.length} peer${others.length !== 1 ? 's' : ''}`;

  if (others.length === 0) {
    ul.innerHTML = '<li style="color:#475569;font-size:0.8rem;font-style:italic;border:none;background:none">'
      + 'Open another tab to see peers appear here.</li>';
    return;
  }
  ul.innerHTML = others.map(({ pubkey, state: p }) => {
    const root = (p && p.root) || '—';
    const synced = root === myRoot;
    const syncMark = synced
      ? '<span style="color:#4ade80" title="In sync">✓</span>'
      : '<span style="color:#fbbf24" title="Diverged">~</span>';
    const label = (p && p.label) || pubkey.slice(0, 8);
    const color = (p && p.color) || '#94a3b8';
    return `<li>
      <span class="peer-id" style="color:${esc(color)}">● ${esc(label)}</span>
      <span class="peer-root">${syncMark} ${String(root).slice(0,14)}…</span>
    </li>`;
  }).join('');
}

// ---------------------------------------------------------------------------
// Room Auth (C3) — room-seed UI. Changing the seed requires a reload because
// the SDK reads it once at `createDoc`.
// ---------------------------------------------------------------------------
function renderRoomAuth() {
  const roomInp = document.getElementById('room-id-in');
  const inp = document.getElementById('room-seed-in');
  const status = document.getElementById('room-auth-status');
  if (!inp || !status) return;
  if (roomInp) roomInp.value = ROOM_ID;
  if (roomSeed) {
    try {
      const pubkey = room_pubkey_hex(roomSeed);
      inp.value = Array.from(roomSeed).map(b => b.toString(16).padStart(2, '0')).join('');
      status.textContent = `Room: ${ROOM_ID} | Room public key: ${pubkey.slice(0, 16)}… (reload after changing)`;
      status.style.color = '#4ade80';
    } catch (_) {}
  } else {
    status.textContent = `Room: ${ROOM_ID} | No room seed — room is open`;
    status.style.color = '#94a3b8';
  }
}

window.setRoomId = function () {
  const roomInp = document.getElementById('room-id-in');
  if (!roomInp) return;
  const room = normalizeRoomId(roomInp.value);
  sessionStorage.setItem(STORAGE_KEYS.roomId, room);
  logEvent('sync', `room set to "${room}" — reloading…`);
  setTimeout(() => window.location.reload(), 50);
};

window.clearRoomId = function () {
  sessionStorage.removeItem(STORAGE_KEYS.roomId);
  logEvent('sync', `room reset to "${DEFAULT_ROOM_ID}" — reloading…`);
  setTimeout(() => window.location.reload(), 50);
};

window.generateRoomSeed = function () {
  const seed = crypto.getRandomValues(new Uint8Array(32));
  saveRoomSeed(seed);
  roomSeed = seed;
  renderRoomAuth();
  logEvent('sync', '🔑 new room seed generated — reload to sign tokens with it');
};

window.clearRoomSeed = function () {
  saveRoomSeed(null);
  roomSeed = null;
  renderRoomAuth();
  logEvent('sync', 'room seed cleared — reload to connect as open');
};

window.lockRoom = function () {
  if (!roomSeed) { logEvent('del', '⚠ generate a room seed first'); return; }
  try {
    const pubkey = room_pubkey_hex(roomSeed);
    doc.send({ type: 'set-room-key', pubkey });
    logEvent('sync', `🔒 sent set-room-key: ${pubkey.slice(0, 10)}…`);
  } catch (e) {
    logEvent('del', `⚠ lock failed: ${e}`);
  }
};

console.log('[boot] demo ready.');
