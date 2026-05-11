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

import { createDoc, room_pubkey_hex } from './sdk.js';

// Catch any unhandled promise rejections from the module itself.
window.addEventListener('unhandledrejection', ev => {
  console.error('[activesync] unhandledRejection:', ev.reason);
  const s = document.getElementById('status');
  if (s) s.textContent = 'Module error: ' + ev.reason;
});

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------
const ROOM_ID    = 'default';
const DEFAULT_SERVER_URL = 'ws://127.0.0.1:7878';
const COLORS     = ['#60a5fa','#4ade80','#f472b6','#fb923c','#a78bfa','#34d399','#fbbf24','#f87171'];
const IDB_NAME    = 'activesync-v6';
const IDB_VERSION = 1;
const COLLAB_KEY  = 'collab/doc';
const LIST_KEY    = 'demo/list';

function resolveServerUrl() {
  const storageKey = 'activesync-server-url';
  const params = new URLSearchParams(window.location.search);
  const fromQuery = params.get('server') || params.get('serverUrl') || params.get('ws');
  const fromSession = sessionStorage.getItem(storageKey);
  const candidate = (fromQuery || fromSession || DEFAULT_SERVER_URL).trim();

  try {
    const parsed = new URL(candidate);
    if (parsed.protocol !== 'ws:' && parsed.protocol !== 'wss:') {
      throw new Error(`unsupported protocol: ${parsed.protocol}`);
    }
    const normalized = parsed.toString().replace(/\/$/, '');
    sessionStorage.setItem(storageKey, normalized);
    return normalized;
  } catch (e) {
    console.warn('[boot] invalid server url override, falling back to default:', candidate, e);
    sessionStorage.setItem(storageKey, DEFAULT_SERVER_URL);
    return DEFAULT_SERVER_URL;
  }
}

const SERVER_URL = resolveServerUrl();

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
// Room-auth seed (C3) — sessionStorage; null = open room
// ---------------------------------------------------------------------------
function loadRoomSeed() {
  const stored = sessionStorage.getItem('activesync-room-seed');
  if (!stored) return null;
  try { return new Uint8Array(JSON.parse(stored)); } catch (_) { return null; }
}

function saveRoomSeed(seed32) {
  if (seed32) sessionStorage.setItem('activesync-room-seed', JSON.stringify(Array.from(seed32)));
  else sessionStorage.removeItem('activesync-room-seed');
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
    root:  doc.store.merkle_root_hex(),
    nodes: doc.store.node_count(),
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
  saveState();
  renderState();
  renderCollabText();
  renderList();
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
    renderBlobStats();
  } catch (_) {}
  // Keep presence fresh with our current root + node count so peers
  // can tell when we're in sync.
  if (doc.isConnected) {
    doc.presence.set({
      label: myId,
      color: myColor,
      root:  doc.store.merkle_root_hex(),
      nodes: doc.store.node_count(),
    });
  }
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
const kv = doc.map('');

window.doSet = function () {
  const key = document.getElementById('key-in').value.trim();
  const val = document.getElementById('val-in').value;
  if (!key) return;
  kv.set(key, val);
  logEvent('set', `set "${key}" = ${JSON.stringify(val)}`);
  saveState();
  renderState();
};

window.doDelete = function () {
  const key = document.getElementById('key-in').value.trim();
  if (!key) return;
  kv.delete(key);
  logEvent('del', `delete "${key}"`);
  saveState();
  renderState();
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
  renderState();
  renderBlobStats();
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

function renderCollabText() {
  const ta = document.getElementById('collab-textarea');
  if (!ta) return;
  const resolved = collabText.toString();
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
  if (stats) stats.textContent =
    `${resolved.length} char${resolved.length !== 1 ? 's' : ''} · ${doc.store.node_count()} ops`;
}

(function wireCollabTextarea() {
  const ta = document.getElementById('collab-textarea');
  if (!ta) return;

  ta.addEventListener('beforeinput', (e) => {
    e.preventDefault();
    const start = ta.selectionStart;
    const end   = ta.selectionEnd;

    function deleteRange(from, to) {
      // Delete back-to-front so indices stay stable.
      if (to > from) collabText.delete(from, to - from);
    }

    if (e.inputType === 'insertText' && e.data) {
      if (start !== end) deleteRange(start, end);
      collabText.insert(start, e.data);
    } else if (e.inputType === 'deleteContentBackward') {
      if (start !== end) deleteRange(start, end);
      else if (start > 0) collabText.delete(start - 1, 1);
    } else if (e.inputType === 'deleteContentForward') {
      if (start !== end) deleteRange(start, end);
      else {
        const cur = collabText.toString();
        if (start < cur.length) collabText.delete(start, 1);
      }
    } else if (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph') {
      if (start !== end) deleteRange(start, end);
      collabText.insert(start, '\n');
    }

    saveState();
    renderCollabText();

    const resolved = collabText.toString();
    const delta = e.inputType.startsWith('delete')
      ? 0
      : (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph'
          ? 1
          : (e.data ? [...e.data].length : 0));
    const newCursor = Math.min(
      e.inputType.startsWith('delete') ? start : start + delta,
      resolved.length,
    );
    ta.setSelectionRange(newCursor, newCursor);
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

// drag state — tracked here because HTML5 DnD's dataTransfer is awkward
// for intra-page IDs across subtle browser differences.
let listDragId = null;

function renderList() {
  const ul = document.getElementById('list-items');
  if (!ul) return;
  const items = listHandle.toArray();
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
      renderList();
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
      renderList();
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
    renderList();
  };
  addBtn.addEventListener('click', add);
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') add(); });
})();

listHandle.onChange(() => { renderList(); });

// Initial list render.
renderList();

// ---------------------------------------------------------------------------
// Render — resolved state + peer list
// ---------------------------------------------------------------------------
function renderState() {
  const state = JSON.parse(doc.store.resolve_json_with_meta());
  const dec   = new TextDecoder();
  const box   = document.getElementById('my-state');
  const keys  = Object.keys(state).sort();

  document.getElementById('my-root').textContent = doc.store.merkle_root_hex();
  document.getElementById('my-badge').textContent =
    `${doc.store.node_count()} node${doc.store.node_count() !== 1 ? 's' : ''}`;

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

function renderPeers() {
  const myRoot = doc.store.merkle_root_hex();
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
  const inp = document.getElementById('room-seed-in');
  const status = document.getElementById('room-auth-status');
  if (!inp || !status) return;
  if (roomSeed) {
    try {
      const pubkey = room_pubkey_hex(roomSeed);
      inp.value = Array.from(roomSeed).map(b => b.toString(16).padStart(2, '0')).join('');
      status.textContent = `Room public key: ${pubkey.slice(0, 16)}… (reload after changing)`;
      status.style.color = '#4ade80';
    } catch (_) {}
  } else {
    status.textContent = 'No room seed — room is open';
    status.style.color = '#94a3b8';
  }
}

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
