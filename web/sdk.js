// sdk.js — ActiveSync high-level SDK (F1)
//
// Firebase/Replicache-style document API on top of the low-level `SyncStore`.
// The underlying bridge is unchanged; this file is a pure JS wrapper.
//
// Usage:
//   import { createDoc } from './sdk.js';
//   const doc = await createDoc({ serverUrl, room });
//   const world = doc.map('world');
//   world.set('player1', { x: 10, y: 20 });
//   world.onChange(ev => render(world.all()));
//   const notes = doc.text('notes/welcome');
//   notes.insert(0, 'Hello');
//
// Status:
//   v0 — LWW Map + RGA Text + IBF/MST handshake + blob sync.
//   Deferred: WebRTC P2P (D2), presence (F2), client-side subscribe (F3a),
//             speculative/canonical split (E2 is in the bridge but not yet
//             surfaced here — apps wanting it can drop to `doc.store`).

import init, {
  SyncStore,
  sign_room_token,
  room_pubkey_hex,
} from './pkg/activesync_bridge.js';

// -----------------------------------------------------------------------------
// Module init — idempotent. Callers can also await `createDoc` directly; this
// is exposed so apps that load the SDK early can fire WASM fetch in parallel.
// -----------------------------------------------------------------------------
let _initPromise = null;
export function ready() {
  if (!_initPromise) _initPromise = init();
  return _initPromise;
}

// -----------------------------------------------------------------------------
// Small utilities
// -----------------------------------------------------------------------------
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function jsonToBytes(v) { return textEncoder.encode(JSON.stringify(v)); }
function bytesToJson(b) {
  if (!b || b.length === 0) return undefined;
  try { return JSON.parse(textDecoder.decode(b)); } catch { return undefined; }
}
function b64decode(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function makeEmitter() {
  const listeners = new Set();
  return {
    emit(arg) { for (const l of listeners) { try { l(arg); } catch (e) { console.error('[sdk] listener error', e); } } },
    on(cb) { listeners.add(cb); return () => listeners.delete(cb); },
    size() { return listeners.size; },
  };
}

// G8 — build a metrics dispatcher. Returns `{ emit }` where `emit` is a
// no-op when no `onMetric` hook is configured (zero cost on the hot
// path). When a hook is provided, wraps it so an app-side throw never
// corrupts the SDK state. See docs/sdk.md for the event kind catalogue.
function makeMetrics(onMetric) {
  if (typeof onMetric !== 'function') {
    return { emit() {}, enabled: false };
  }
  return {
    emit(kind, value, labels) {
      try {
        onMetric({
          kind,
          value,
          labels: labels || {},
          timestamp: Date.now(),
        });
      } catch (e) {
        console.error('[sdk] onMetric hook threw', e);
      }
    },
    enabled: true,
  };
}

function randomSeed32() {
  const s = new Uint8Array(32);
  (globalThis.crypto ?? crypto).getRandomValues(s);
  return s;
}

// -----------------------------------------------------------------------------
// Transport — WebSocket wrapper that speaks the ActiveSync wire protocol.
// Reuses the protocol shipped in demo.js (hello/welcome/pack/request/mst/blob).
// -----------------------------------------------------------------------------
function makeTransport({ serverUrl, room, store, getToken, getPubkey, getSubscription, onRemotePack, onConnect, onDisconnect, onError, onPresence, onPeerJoined, onPeerLeft, onWelcomePeers, onSignal, log, metrics }) {
  metrics = metrics || { emit() {}, enabled: false };
  let ws = null;
  let reconnectTimer = null;
  let reconnectDelay = 1000;
  const maxReconnectDelay = 30_000;
  let closed = false;
  let connected = false;
  // G8: counts every `openSocket()` call; resets to 0 on a successful
  // onopen so `attempt` in `ws_reconnect` events reflects the current
  // streak of failed attempts, not the lifetime total.
  let reconnectAttempt = 0;

  // MST descent state.
  let mstDescentRoot = null;
  // Track what we've already sent the server so deltas are minimal.
  const sentToServer = new Set();
  // Track in-flight blob requests to avoid re-asking for the same hash.
  const pendingBlobFetches = new Set();

  // ── F6: direct blob I/O state ─────────────────────────────────────────
  // Latest negotiated capabilities (assigned at welcome). When
  // `supports_direct_blob_io` is true we may bifurcate large blob I/O off
  // the WebSocket onto presigned URLs.
  let negotiatedCaps = {};
  // Pending request-upload calls awaiting a server response. Keyed by
  // hash hex; value is a resolver function `(reply) => void` that takes
  // either an `upload-granted` or `upload-denied` envelope.
  const pendingUploadGrants = new Map();
  // GET URL cache: hash → { url, expiresAtMs }. Refreshed by
  // `blob-redirect` messages and by re-asking the server when expiry
  // approaches or a fetch returns 403.
  const blobUrlCache = new Map();
  // Threshold below which we don't even ask for a presigned PUT — the WS
  // round-trip is cheaper. Server may also enforce its own threshold;
  // this is just the client-side optimization.
  const DIRECT_UPLOAD_THRESHOLD = 1 * 1024 * 1024;
  // Refresh GET URLs this many ms before they expire so an in-flight
  // fetch doesn't 403.
  const URL_REFRESH_LEAD_MS = 60_000;

  function refreshSentToServer() {
    sentToServer.clear();
    for (const id of JSON.parse(store.all_node_ids_json())) sentToServer.add(id);
  }

  function send(obj) {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
  }

  function sendLocalDelta() {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    try {
      const delta = store.export_nodes_missing_from(JSON.stringify([...sentToServer]));
      send({ type: 'pack', nodes: delta });
      refreshSentToServer();
    } catch (e) { log('warn', '[sdk] sendLocalDelta', e); }
  }

  // Upload specific blob hashes to the server so other peers can fetch them.
  // Called by `doc.map(...).setBlob(...)` after a local blob is stored.
  //
  // F6: when `supports_direct_blob_io` is negotiated and the blob is
  // ≥ DIRECT_UPLOAD_THRESHOLD, ask the server for a presigned PUT URL,
  // upload directly to S3, then send `blob-uploaded`. Falls back to the
  // existing `blob-upload` (bytes-over-WS) path on any failure.
  function sendBlobs(hashes) {
    if (!hashes || hashes.length === 0) return;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    try {
      const direct = [];
      const wsBytes = [];
      for (const h of hashes) {
        let bytes;
        try { bytes = store.get_blob_bytes(h); } catch (_) { continue; }
        if (!bytes) continue;
        if (negotiatedCaps.supports_direct_blob_io && bytes.length >= DIRECT_UPLOAD_THRESHOLD) {
          direct.push({ hash: h, bytes });
        } else {
          wsBytes.push(h);
        }
      }
      if (wsBytes.length > 0) {
        const blobs = JSON.parse(store.export_blobs_json(JSON.stringify(wsBytes)));
        if (blobs.length > 0) {
          send({ type: 'blob-upload', blobs });
          let total = 0;
          for (const b of blobs) {
            // b.data is base64; estimate bytes as 0.75 * len (upper bound).
            total += Math.ceil((b?.data?.length ?? 0) * 0.75);
          }
          metrics.emit('blob_upload', total, {
            transport: 'ws',
            result: 'ok',
            count: blobs.length,
          });
        }
      }
      for (const { hash, bytes } of direct) {
        directUpload(hash, bytes).catch(err => {
          log('warn', '[sdk] direct upload failed; falling back to ws', err);
          metrics.emit('direct_upload_fallback', bytes.length, {
            reason: err?.message ?? 'unknown',
          });
          // Fallback: ship the bytes over WS.
          try {
            const blobs = JSON.parse(store.export_blobs_json(JSON.stringify([hash])));
            if (blobs.length > 0) {
              send({ type: 'blob-upload', blobs });
              metrics.emit('blob_upload', bytes.length, { transport: 'ws', result: 'ok' });
            }
          } catch (_) {}
        });
      }
    } catch (e) { log('warn', '[sdk] sendBlobs', e); }
  }

  // F6: ask the server for a presigned PUT, upload, then notify.
  async function directUpload(hash, bytes) {
    const reply = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pendingUploadGrants.delete(hash);
        reject(new Error('request-upload timeout'));
      }, 30_000);
      pendingUploadGrants.set(hash, (envelope) => {
        clearTimeout(timer);
        pendingUploadGrants.delete(hash);
        resolve(envelope);
      });
      send({ type: 'request-upload', hash, size: bytes.length });
    });
    if (reply.type === 'upload-denied') {
      throw new Error('upload-denied: ' + (reply.reason ?? 'unknown'));
    }
    if (reply.type !== 'upload-granted') {
      throw new Error('unexpected reply: ' + reply.type);
    }
    const resp = await fetch(reply.url, {
      method: 'PUT',
      body: bytes,
      // Avoid the browser appending `Content-Type: text/plain;...` which
      // would mismatch the presign signature on some providers.
      headers: { 'Content-Type': 'application/octet-stream' },
    });
    if (!resp.ok) {
      throw new Error('PUT failed: ' + resp.status);
    }
    // Tell the server the object exists; server HEAD-verifies and
    // broadcasts blob-available so other peers can pull.
    send({ type: 'blob-uploaded', hash });
    metrics.emit('blob_upload', bytes.length, { transport: 'direct', result: 'ok' });
  }

  // F6: download a blob via presigned URL and stash it in the store.
  // On 403 (URL expired or revoked) re-issues a `blob-request` so the
  // server can mint a fresh URL. On any other error, throws so the
  // caller can fall back to the WS path.
  async function fetchBlobViaUrl(hash, url) {
    const resp = await fetch(url, { method: 'GET', cache: 'no-store' });
    if (resp.status === 403) {
      blobUrlCache.delete(hash);
      metrics.emit('blob_download', 0, { transport: 'redirect', result: 'expired' });
      throw new Error('presigned URL rejected (403)');
    }
    if (!resp.ok) {
      metrics.emit('blob_download', 0, { transport: 'redirect', result: 'err', status: resp.status });
      throw new Error('GET failed: ' + resp.status);
    }
    const bytes = new Uint8Array(await resp.arrayBuffer());
    try {
      store.store_blob_bytes(hash, bytes);
    } finally {
      pendingBlobFetches.delete(hash);
    }
    metrics.emit('blob_download', bytes.length, { transport: 'redirect', result: 'ok' });
    onRemotePack({ from: 'direct', kind: 'blobs' });
  }

  function requestMissingBlobs() {
    try {
      const missing = JSON.parse(store.missing_blob_hashes_json())
        .filter(h => !pendingBlobFetches.has(h));
      if (missing.length > 0) {
        missing.forEach(h => pendingBlobFetches.add(h));
        send({ type: 'blob-request', hashes: missing });
      }
    } catch (_) {}
  }

  function handleWelcome(msg) {
    sentToServer.clear();

    // Server told us what it's missing — ship a targeted pack.
    const missing = msg.missing ?? [];
    if (missing.length > 0) {
      const delta = store.export_nodes_missing_from(JSON.stringify(missing));
      send({ type: 'pack', nodes: delta });
    } else if (msg.root && msg.root !== store.merkle_root_hex()) {
      // Roots differ but server didn't tell us what it wants (IBF decode
      // failure on a large diff, or a freshly-restarted server). Push what
      // we have that the server's frontier doesn't already cover — server
      // dedupes cheaply. Without this, new client mutations would arrive
      // at the server with unknown parents and be silently dropped.
      const serverFrontier = Array.isArray(msg.frontier) ? msg.frontier : [];
      try {
        const delta = store.export_nodes_missing_from(JSON.stringify(serverFrontier));
        if (delta && delta.length > 0) send({ type: 'pack', nodes: delta });
      } catch (e) { log('warn', '[sdk] welcome push', e); }
    }
    refreshSentToServer();

    // D2: hand the peer list to the mesh so it can open data channels.
    if (Array.isArray(msg.peers)) onWelcomePeers?.(msg.peers);

    // MST descent when supported + roots differ; else request full catchup.
    const caps = msg.caps ?? {};
    negotiatedCaps = caps; // F6: stash for sendBlobs / blob-redirect.
    if (caps.supports_mst && msg.mst_root && msg.mst_root !== store.mst_root_hex()) {
      mstDescentRoot = msg.mst_root;
      send({ type: 'mst-request', paths: [''] });
    } else if (msg.root && msg.root !== store.merkle_root_hex()) {
      send({ type: 'request', known: JSON.parse(store.all_node_ids_json()) });
    }

    requestMissingBlobs();
  }

  function handleMstResponse(msg) {
    const serverNodes = msg.nodes ?? [];
    const myRoot = store.mst_root_hex();
    const result = JSON.parse(
      store.mst_process_response_json(JSON.stringify(serverNodes), myRoot, mstDescentRoot)
    );
    if (result.next_paths.length > 0) {
      send({ type: 'mst-request', paths: result.next_paths });
      return;
    }
    if (result.only_theirs.length > 0) send({ type: 'mst-done', ids: result.only_theirs });
    if (result.only_mine.length > 0) {
      try {
        const delta = store.export_nodes_missing_from(JSON.stringify(result.only_theirs));
        send({ type: 'pack', nodes: delta });
      } catch (_) {}
    }
    mstDescentRoot = null;
  }

  function handleMessage(e) {
    let msg;
    try { msg = JSON.parse(e.data); } catch (err) { onError(new Error('Bad server message: ' + err.message)); return; }

    switch (msg.type) {
      case 'welcome':
        handleWelcome(msg);
        break;

      case 'mst-response':
        handleMstResponse(msg);
        break;

      case 'pack': {
        const nodeCount = Array.isArray(msg.nodes) ? msg.nodes.length : 0;
        try { store.import_pack(msg.nodes); } catch (err) { onError(err); break; }
        refreshSentToServer();
        requestMissingBlobs();
        metrics.emit('pack_applied', nodeCount, {
          from: msg.from === undefined || msg.from === 'server' ? 'live' : String(msg.from),
        });
        onRemotePack({ from: msg.from ?? 'server' });
        break;
      }

      case 'blob-available': {
        const available = msg.hashes ?? [];
        try {
          const myMissing = new Set(JSON.parse(store.missing_blob_hashes_json()));
          const want = available.filter(h => myMissing.has(h));
          if (want.length > 0) {
            want.forEach(h => pendingBlobFetches.add(h));
            send({ type: 'blob-request', hashes: want });
          }
        } catch (_) {}
        break;
      }

      case 'blob-pack': {
        (msg.requested ?? []).forEach(h => pendingBlobFetches.delete(h));
        let touched = false;
        let totalBytes = 0;
        let count = 0;
        for (const { hash, data } of (msg.blobs ?? [])) {
          try {
            const raw = b64decode(data);
            store.store_blob_bytes(hash, raw);
            totalBytes += raw.length;
            count += 1;
            touched = true;
          } catch (_) {}
        }
        if (touched) {
          metrics.emit('blob_download', totalBytes, {
            transport: 'ws',
            result: 'ok',
            count,
          });
          onRemotePack({ from: msg.from ?? 'server', kind: 'blobs' });
        }
        break;
      }

      // F6: server told us to fetch some blobs from a presigned URL
      // instead of bytes-over-WS. Cache the URLs and kick off downloads.
      case 'blob-redirect': {
        const redirects = msg.redirects ?? [];
        for (const r of redirects) {
          if (!r.hash || !r.url) continue;
          const expiresAtMs = (r.expires_at_unix ?? 0) * 1000;
          blobUrlCache.set(r.hash, { url: r.url, expiresAtMs });
          fetchBlobViaUrl(r.hash, r.url).catch(err => {
            log('warn', '[sdk] direct blob fetch failed; retrying via ws', err);
            pendingBlobFetches.delete(r.hash);
            blobUrlCache.delete(r.hash);
            // Fall back to the bytes-over-WS path.
            send({ type: 'blob-request', hashes: [r.hash] });
            pendingBlobFetches.add(r.hash);
          });
        }
        break;
      }

      // F6: response to our `request-upload`. Hand it to the waiting promise.
      case 'upload-granted':
      case 'upload-denied':
      case 'upload-rejected': {
        const cb = msg.hash ? pendingUploadGrants.get(msg.hash) : null;
        if (cb) cb(msg);
        break;
      }

      case 'error':
        onError(new Error('server: ' + msg.msg));
        break;

      // Presence, WebRTC signaling, room-lock acks: mostly SDK-handled (F2/D2).
      case 'presence':
        onPresence?.(msg);
        break;
      case 'peer-joined':
        onPeerJoined?.(msg);
        break;
      case 'peer-left':
        onPeerLeft?.(msg);
        break;
      case 'webrtc-offer':
      case 'webrtc-answer':
      case 'webrtc-ice':
        onSignal?.(msg);
        break;
      default:
        break;
    }
  }

  function openSocket() {
    if (closed) return;
    const url = `${serverUrl.replace(/\/$/, '')}/ws/${encodeURIComponent(room)}`;
    ws = new WebSocket(url);

    ws.onopen = () => {
      const successfulAttempt = reconnectAttempt;
      reconnectDelay = 1000;
      reconnectAttempt = 0;
      connected = true;
      pendingBlobFetches.clear();
      if (successfulAttempt > 0) {
        metrics.emit('ws_reconnect', successfulAttempt, { outcome: 'ok' });
      }
      try {
        const hello = {
          type: 'hello',
          room,
          pubkey: getPubkey(),
          root: store.merkle_root_hex(),
          frontier: JSON.parse(store.frontier_hex_json()),
          ibf: store.compute_ibf_b64(),
          mst_root: store.mst_root_hex(),
          caps: JSON.parse(store.our_capabilities_json()),
        };
        const tok = getToken?.();
        if (tok) hello.token = tok;
        // F3b: tell the server which paths we care about so it can filter relay.
        try {
          const subPatterns = getSubscription?.();
          if (Array.isArray(subPatterns)) hello.subscribe = subPatterns;
        } catch (_) {}
        send(hello);

        // Re-upload our blobs: server store is in-memory today (F4 pending).
        try {
          const localHashes = JSON.parse(store.local_blob_hashes_json());
          if (localHashes.length > 0) {
            const blobs = JSON.parse(store.export_blobs_json(JSON.stringify(localHashes)));
            if (blobs.length > 0) send({ type: 'blob-upload', blobs });
          }
        } catch (_) {}

        onConnect();
      } catch (err) { onError(err); }
    };

    ws.onmessage = handleMessage;

    ws.onerror = () => { /* onclose will handle reconnect */ };

    ws.onclose = () => {
      const wasConnected = connected;
      connected = false;
      ws = null;
      if (wasConnected) onDisconnect();
      if (!closed) {
        reconnectAttempt += 1;
        metrics.emit('ws_reconnect', reconnectAttempt, {
          outcome: 'scheduled',
          delay_ms: reconnectDelay,
          was_connected: wasConnected,
        });
        clearTimeout(reconnectTimer);
        reconnectTimer = setTimeout(openSocket, reconnectDelay);
        reconnectDelay = Math.min(reconnectDelay * 2, maxReconnectDelay);
      }
    };
  }

  return {
    connect() { closed = false; openSocket(); },
    disconnect() {
      closed = true;
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
      if (ws) { try { ws.close(); } catch (_) {} ws = null; }
      connected = false;
    },
    sendLocalDelta,
    sendBlobs,
    send,
    get connected() { return connected; },
  };
}

// -----------------------------------------------------------------------------
// Path helpers
// -----------------------------------------------------------------------------
function joinPath(ns, key) {
  if (!ns) return key;
  if (ns.endsWith('/')) return ns + key;
  return ns + '/' + key;
}
function matchesPrefix(path, ns) {
  if (!ns) return true;
  const p = ns.endsWith('/') ? ns : ns + '/';
  return path === ns || path.startsWith(p);
}

// Glob compile: `/**` trailing = optional subtree, `**` = any chars incl. `/`,
// `*` = any chars excl. `/`, else literal.
// Examples:
//   "world/**"          → matches  world, world/a, world/a/b
//   "world/*"           → matches  world/a    but not world/a/b
//   "notes/welcome"     → matches  notes/welcome only
function compileGlob(pattern) {
  if (pattern === '**') return () => true;
  let re = '';
  for (let i = 0; i < pattern.length; i++) {
    const c = pattern[i];
    if (c === '/' && pattern[i + 1] === '*' && pattern[i + 2] === '*') {
      re += '(?:/.*)?'; i += 2;
    } else if (c === '*' && pattern[i + 1] === '*') {
      re += '.*'; i++;
    } else if (c === '*') {
      re += '[^/]*';
    } else if ('.+?^$()[]{}|\\'.includes(c)) {
      re += '\\' + c;
    } else {
      re += c;
    }
  }
  const rx = new RegExp('^' + re + '$');
  return (path) => rx.test(path);
}

function compileSubscription(patterns) {
  const list = (patterns && patterns.length > 0) ? patterns : ['**'];
  const fns = list.map(compileGlob);
  const matches = (path) => { for (const f of fns) if (f(path)) return true; return false; };
  return { patterns: list.slice(), matches };
}

// -----------------------------------------------------------------------------
// PeerMesh (D2 / F1b) — WebRTC data channels between peers in the same room.
//
// Transport policy: WS is always the authoritative path (server persists +
// relays to peers without a direct channel). When a data channel to a peer is
// open, the SDK sends the SAME pack/blob over that channel too. `import_pack`
// dedupes on node id so the duplicate is free; RTC wins on latency and on
// LAN bandwidth. If RTC negotiation fails or the channel closes, WS continues
// to carry everything — zero-config fallback.
//
// Initiation rule: the peer with the lexicographically-smaller pubkey hex
// creates the offer. Symmetric. Prevents both sides dialing at once.
// -----------------------------------------------------------------------------
function hexToBytes(hex) {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}
function bytesToHex(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i++) s += bytes[i].toString(16).padStart(2, '0');
  return s;
}
const DEFAULT_ICE_SERVERS = [
  { urls: 'stun:stun.l.google.com:19302' },
  { urls: 'stun:stun1.l.google.com:19302' },
];

function makePeerMesh({
  store,
  getPubkey,
  sendSignal,
  onRemotePack,
  onRemoteBlob,
  iceServers,
  log,
}) {
  // Runtime gate: environments without RTCPeerConnection (Node, some tests)
  // silently degrade to WS-only.
  const RTC = (typeof globalThis !== 'undefined' && globalThis.RTCPeerConnection) ? globalThis.RTCPeerConnection : null;
  const enabled = !!RTC;

  /** @type {Map<string, RtcPeerState>} */
  const peers = new Map();

  class RtcPeerState {
    constructor(remotePubkey, isInitiator) {
      this.remote = remotePubkey;
      this.isInitiator = isInitiator;
      this.pc = new RTCPeerConnection({ iceServers: iceServers || DEFAULT_ICE_SERVERS });
      /** @type {RTCDataChannel|null} */ this.sync = null;
      /** @type {RTCDataChannel|null} */ this.blobs = null;
      this._ibfSent = false;
      this._haveSent = false;
      this._pendingIce = [];

      this.pc.onicecandidate = ({ candidate }) => {
        if (candidate) sendSignal({ type: 'webrtc-ice', to: remotePubkey, candidate });
      };
      this.pc.onconnectionstatechange = () => {
        const s = this.pc.connectionState;
        log('log', `[mesh] ${remotePubkey.slice(0,8)} ${s}`);
        if (s === 'failed' || s === 'closed' || s === 'disconnected') {
          this.close();
          peers.delete(remotePubkey);
        }
      };

      if (isInitiator) {
        this.sync  = this.pc.createDataChannel('sync',  { ordered: true });
        this.blobs = this.pc.createDataChannel('blobs', { ordered: true });
        this._setupSync(this.sync);
        this._setupBlobs(this.blobs);
      } else {
        this.pc.ondatachannel = ({ channel }) => {
          if (channel.label === 'sync')  { this.sync  = channel; this._setupSync(channel); }
          if (channel.label === 'blobs') { this.blobs = channel; this._setupBlobs(channel); }
        };
      }
    }

    get syncReady()  { return this.sync  && this.sync.readyState  === 'open'; }
    get blobsReady() { return this.blobs && this.blobs.readyState === 'open'; }

    _setupSync(ch) {
      ch.onopen = () => {
        // Kick off reconciliation by exchanging IBFs.
        try {
          ch.send(JSON.stringify({ type: 'ibf', ibf: store.compute_ibf_b64() }));
          this._ibfSent = true;
        } catch (e) { log('warn', '[mesh] ibf send', e); }
      };
      ch.onmessage = ({ data }) => {
        let msg; try { msg = JSON.parse(data); } catch { return; }
        this._onSync(msg, ch);
      };
      ch.onerror = (e) => log('warn', '[mesh] sync error', e?.message ?? e);
    }

    _onSync(msg, ch) {
      switch (msg.type) {
        case 'ibf': {
          let diff;
          try { diff = JSON.parse(store.diff_with_ibf_b64(msg.ibf)); }
          catch {
            // Overflow — fall back to pushing whatever they're missing from
            // an empty "known" set, i.e. our full graph. import_pack dedupes.
            try {
              const pack = store.export_nodes_missing_from(JSON.stringify([]));
              ch.send(JSON.stringify({ type: 'pack', nodes: pack }));
            } catch (_) {}
            if (!this._ibfSent) {
              try { ch.send(JSON.stringify({ type: 'ibf', ibf: store.compute_ibf_b64() })); this._ibfSent = true; } catch (_) {}
            }
            break;
          }
          if (diff.only_mine && diff.only_mine.length > 0) {
            try {
              const pack = store.export_nodes_missing_from(JSON.stringify(diff.only_theirs || []));
              ch.send(JSON.stringify({ type: 'pack', nodes: pack }));
            } catch (_) {}
          }
          if (diff.only_theirs && diff.only_theirs.length > 0) {
            ch.send(JSON.stringify({ type: 'want', ids: diff.only_theirs }));
          }
          if (!this._ibfSent) {
            try { ch.send(JSON.stringify({ type: 'ibf', ibf: store.compute_ibf_b64() })); this._ibfSent = true; } catch (_) {}
          }
          break;
        }
        case 'pack': {
          try { store.import_pack(msg.nodes); onRemotePack({ from: this.remote, transport: 'webrtc' }); }
          catch (e) { log('warn', '[mesh] import_pack', e); }
          break;
        }
        case 'want': {
          try {
            const pack = store.export_nodes_missing_from(JSON.stringify(msg.ids || []));
            ch.send(JSON.stringify({ type: 'pack', nodes: pack }));
          } catch (_) {}
          break;
        }
        default: break;
      }
    }

    _setupBlobs(ch) {
      ch.binaryType = 'arraybuffer';
      ch.onopen = () => {
        try {
          const hashes = JSON.parse(store.local_blob_hashes_json());
          ch.send(JSON.stringify({ type: 'blob-have', hashes }));
          this._haveSent = true;
        } catch (e) { log('warn', '[mesh] blob-have', e); }
      };
      ch.onmessage = ({ data }) => {
        if (typeof data === 'string') {
          let msg; try { msg = JSON.parse(data); } catch { return; }
          if (msg.type === 'blob-have') this._onBlobHave(msg, ch);
        } else {
          this._onBlobFrame(data);
        }
      };
      ch.onerror = (e) => log('warn', '[mesh] blobs error', e?.message ?? e);
    }

    _onBlobHave(msg, ch) {
      const theirs = new Set(msg.hashes || []);
      let mine = [];
      try { mine = JSON.parse(store.local_blob_hashes_json()); } catch (_) {}
      const toSend = mine.filter(h => !theirs.has(h));
      for (const hashHex of toSend) {
        try {
          const blobsJson = store.export_blobs_json(JSON.stringify([hashHex]));
          const blobs = JSON.parse(blobsJson);
          if (!blobs.length) continue;
          const raw = b64decode(blobs[0].data);
          // Frame: [32-byte hash][blob bytes]
          const frame = new Uint8Array(32 + raw.length);
          frame.set(hexToBytes(hashHex), 0);
          frame.set(raw, 32);
          ch.send(frame.buffer);
        } catch (_) {}
      }
      if (!this._haveSent) {
        try {
          const hashes = JSON.parse(store.local_blob_hashes_json());
          ch.send(JSON.stringify({ type: 'blob-have', hashes }));
          this._haveSent = true;
        } catch (_) {}
      }
    }

    _onBlobFrame(ab) {
      if (ab.byteLength < 32) return;
      const hashBytes = new Uint8Array(ab, 0, 32);
      const blobBytes = new Uint8Array(ab, 32);
      const hashHex = bytesToHex(hashBytes);
      try {
        store.store_blob_bytes(hashHex, blobBytes);
        onRemoteBlob({ from: this.remote, hash: hashHex, bytes: blobBytes.length, transport: 'webrtc' });
      } catch (e) { log('warn', '[mesh] store_blob_bytes', e); }
    }

    async negotiate() {
      try {
        const offer = await this.pc.createOffer();
        await this.pc.setLocalDescription(offer);
        sendSignal({ type: 'webrtc-offer', to: this.remote, sdp: this.pc.localDescription });
      } catch (e) { log('warn', '[mesh] negotiate', e); }
    }
    async handleOffer(sdp) {
      try {
        await this.pc.setRemoteDescription(new RTCSessionDescription(sdp));
        for (const c of this._pendingIce) { try { await this.pc.addIceCandidate(new RTCIceCandidate(c)); } catch (_) {} }
        this._pendingIce = [];
        const answer = await this.pc.createAnswer();
        await this.pc.setLocalDescription(answer);
        sendSignal({ type: 'webrtc-answer', to: this.remote, sdp: this.pc.localDescription });
      } catch (e) { log('warn', '[mesh] handleOffer', e); }
    }
    async handleAnswer(sdp) {
      try {
        await this.pc.setRemoteDescription(new RTCSessionDescription(sdp));
        for (const c of this._pendingIce) { try { await this.pc.addIceCandidate(new RTCIceCandidate(c)); } catch (_) {} }
        this._pendingIce = [];
      } catch (e) { log('warn', '[mesh] handleAnswer', e); }
    }
    async addIce(candidate) {
      if (!candidate) return;
      if (!this.pc.remoteDescription) { this._pendingIce.push(candidate); return; }
      try { await this.pc.addIceCandidate(new RTCIceCandidate(candidate)); }
      catch (e) { log('warn', '[mesh] addIce', e); }
    }

    broadcastPack(nodesB64) {
      if (!this.syncReady) return false;
      try { this.sync.send(JSON.stringify({ type: 'pack', nodes: nodesB64 })); return true; }
      catch { return false; }
    }
    broadcastBlobs(hashes) {
      if (!this.blobsReady || !hashes || hashes.length === 0) return false;
      let sent = false;
      for (const hashHex of hashes) {
        try {
          const blobs = JSON.parse(store.export_blobs_json(JSON.stringify([hashHex])));
          if (!blobs.length) continue;
          const raw = b64decode(blobs[0].data);
          const frame = new Uint8Array(32 + raw.length);
          frame.set(hexToBytes(hashHex), 0);
          frame.set(raw, 32);
          this.blobs.send(frame.buffer);
          sent = true;
        } catch (_) {}
      }
      return sent;
    }

    close() {
      try { this.sync?.close(); } catch (_) {}
      try { this.blobs?.close(); } catch (_) {}
      try { this.pc.close(); } catch (_) {}
    }
  }

  function ensurePeer(remotePubkey) {
    if (!enabled) return null;
    if (peers.has(remotePubkey)) return peers.get(remotePubkey);
    const me = getPubkey();
    const isInitiator = me < remotePubkey;
    const p = new RtcPeerState(remotePubkey, isInitiator);
    peers.set(remotePubkey, p);
    if (isInitiator) p.negotiate();
    return p;
  }

  return {
    enabled,
    onWelcomePeers(list) {
      if (!enabled) return;
      for (const pk of (list || [])) if (pk && pk !== getPubkey()) ensurePeer(pk);
    },
    onPeerJoined(remotePubkey) {
      if (!enabled || !remotePubkey || remotePubkey === getPubkey()) return;
      ensurePeer(remotePubkey);
    },
    onPeerLeft(remotePubkey) {
      const p = peers.get(remotePubkey);
      if (p) { p.close(); peers.delete(remotePubkey); }
    },
    onSignal(msg) {
      if (!enabled) return;
      const from = msg.from;
      if (!from || from === getPubkey()) return;
      if (msg.to && msg.to !== getPubkey()) return;
      const p = ensurePeer(from);
      if (!p) return;
      if (msg.type === 'webrtc-offer')  p.handleOffer(msg.sdp);
      else if (msg.type === 'webrtc-answer') p.handleAnswer(msg.sdp);
      else if (msg.type === 'webrtc-ice')    p.addIce(msg.candidate);
    },
    broadcastPack(nodesB64) {
      let count = 0;
      if (!enabled) return 0;
      for (const p of peers.values()) if (p.broadcastPack(nodesB64)) count++;
      return count;
    },
    broadcastBlobs(hashes) {
      let count = 0;
      if (!enabled) return 0;
      for (const p of peers.values()) if (p.broadcastBlobs(hashes)) count++;
      return count;
    },
    peers() {
      const out = [];
      for (const [pk, p] of peers) {
        out.push({
          pubkey: pk,
          transport: p.syncReady ? 'webrtc' : 'ws',
          syncReady: p.syncReady,
          blobsReady: p.blobsReady,
          connectionState: p.pc?.connectionState ?? 'unknown',
        });
      }
      return out;
    },
    shutdown() {
      for (const p of peers.values()) p.close();
      peers.clear();
    },
  };
}

// -----------------------------------------------------------------------------
// createDoc — main entry point.
// -----------------------------------------------------------------------------
/**
 * @param {{
 *   serverUrl: string,
 *   room: string,
 *   authorSeed?: Uint8Array,
 *   roomSeed?: Uint8Array,            // if set, sign a capability token on connect
 *   tokenCaps?: string[],              // e.g. ["write:world/**"]; empty = full
 *   tokenExpirySecs?: number,          // default 24h
 *   autoConnect?: boolean,             // default true
 *   subscribe?: string[],              // F3a: glob patterns to materialize; default ["**"]
 *   transport?: 'auto' | 'ws-only',    // default 'auto' (WS + WebRTC when available)
 *   iceServers?: RTCIceServer[],       // override STUN/TURN for WebRTC peers
 *   logger?: (level, ...args) => void, // default console
 *   onMetric?: (ev) => void,           // G8: metric event hook. See docs/sdk.md.
 * }} opts
 */
export async function createDoc(opts) {
  await ready();

  const {
    serverUrl,
    room,
    authorSeed = randomSeed32(),
    roomSeed = null,
    tokenCaps = [],
    tokenExpirySecs = 60 * 60 * 24,
    autoConnect = true,
    subscribe: subscribePatterns = ['**'],
    transport: transportMode = 'auto',
    iceServers = null,
    presenceHeartbeatMs = 15_000,
    presenceStaleMs = 45_000,
    logger = (level, ...a) => console[level === 'warn' ? 'warn' : 'log']('[activesync]', ...a),
    onMetric = null,
  } = opts;

  if (!serverUrl) throw new Error('createDoc: serverUrl is required');
  if (!room) throw new Error('createDoc: room is required');

  // G8 — metrics dispatcher. `metrics.emit(kind, value, labels)` is a
  // cheap no-op when `onMetric` is unset; otherwise it delivers a
  // structured `{kind, value, labels, timestamp}` event.
  const metrics = makeMetrics(onMetric);

  const store = new SyncStore(authorSeed);
  const pubkeyHex = store.pubkey_hex();

  // ---- subscription (F3a) ----
  // Client-side materialization filter. The wire still carries the full room
  // (F3b will add server-side enforcement); this just gates what the SDK
  // exposes to callers. Writes are NOT filtered — authors can write outside
  // their subscription, and F3b's token caps are the write-side authority.
  let subscription = compileSubscription(subscribePatterns);
  const subscriptionChangeE = makeEmitter();

  // ---- event emitters ----
  const anyChange = makeEmitter();
  const connectE = makeEmitter();
  const disconnectE = makeEmitter();
  const errorE = makeEmitter();
  // G9 — conflict surfacing. `conflictE` is the live fan-out; the SDK
  // polls `store.take_conflicts_json()` after every pack-apply. A
  // bounded ring buffer backs `doc.recentConflicts(sinceMs)` for apps
  // that want a history panel instead of a live toast.
  const conflictE = makeEmitter();
  const CONFLICT_BUFFER_MAX = 256;
  /** @type {{ at:number, kind:string, key:string, localOp:any, winningOp:any, byYou:boolean, raw:any }[]} */
  const conflictBuffer = [];

  function decodeOpValue(op) {
    if (!op || typeof op !== 'object') return op;
    if (op.kind === 'set' && Array.isArray(op.value)) {
      // serde serializes Vec<u8> as an array of numbers. Leave as array;
      // apps know to decode JSON themselves if that's what they stored.
      return op;
    }
    return op;
  }

  function recordConflict(raw) {
    const byYou = raw.winner_author === pubkeyHex;
    const ev = {
      at:        Date.now(),
      kind:      raw.kind,
      key:       raw.key,
      localOp:   decodeOpValue(raw.loser_op),
      winningOp: {
        ...decodeOpValue(raw.winner_op),
        author:  raw.winner_author,
        lamport: raw.winner_lamport,
      },
      byYou,
      raw,
    };
    conflictBuffer.push(ev);
    while (conflictBuffer.length > CONFLICT_BUFFER_MAX) conflictBuffer.shift();
    try { conflictE.emit(ev); } catch (_) {}
    metrics.emit('conflict', 1, { kind: raw.kind, by_you: byYou });
  }

  function pollConflicts() {
    try {
      const raw = JSON.parse(store.take_conflicts_json());
      for (const r of raw) recordConflict(r);
    } catch (e) {
      // Older bridges without take_conflicts_json — silently skip.
    }
  }

  function emitChange(ev) {
    anyChange.emit(ev);
    // Any pack/bulk/local apply may have surfaced new conflicts.
    if (ev.type === 'pack' || ev.type === 'bulk' || ev.source === 'local') {
      pollConflicts();
    }
  }

  // ---- token factory (C3) ----
  function getToken() {
    if (!roomSeed) return null;
    const expiry = Math.floor(Date.now() / 1000) + tokenExpirySecs;
    const caps = tokenCaps.length > 0 ? tokenCaps : null;
    return JSON.parse(sign_room_token(room, pubkeyHex, BigInt(expiry), roomSeed, caps));
  }

  // ---- presence (F2) ----
  // Ephemeral side-channel. Not stored in the DAG, not replayed on reconnect
  // by the server. We rebroadcast our own state on connect + on peer-join so
  // fresh joiners learn about us, and we heartbeat so peers that missed a
  // message still converge. A peer is considered gone when either peer-left
  // fires or no heartbeat arrives within presenceStaleMs.
  const presence = (() => {
    const joinE   = makeEmitter();
    const leaveE  = makeEmitter();
    const updateE = makeEmitter();
    /** @type {Map<string,{state:object,lastSeen:number}>} */
    const others = new Map();
    let myState = null;
    let heartbeatTimer = null;
    let sweepTimer = null;

    function broadcast() {
      if (myState == null) return;
      if (!transport.connected) return;
      transport.send({ type: 'presence', data: myState });
    }

    function startHeartbeat() {
      clearInterval(heartbeatTimer);
      if (myState == null || presenceHeartbeatMs <= 0) return;
      heartbeatTimer = setInterval(broadcast, presenceHeartbeatMs);
    }

    function startSweep() {
      clearInterval(sweepTimer);
      if (presenceStaleMs <= 0) return;
      sweepTimer = setInterval(() => {
        const cutoff = Date.now() - presenceStaleMs;
        for (const [pk, entry] of others) {
          if (entry.lastSeen < cutoff) {
            others.delete(pk);
            leaveE.emit({ pubkey: pk, reason: 'stale' });
          }
        }
      }, Math.max(1000, Math.floor(presenceStaleMs / 3)));
    }

    startSweep();

    return {
      set(patch) {
        myState = { ...(myState ?? {}), ...patch };
        broadcast();
        startHeartbeat();
        return myState;
      },
      clear() {
        myState = null;
        clearInterval(heartbeatTimer);
        heartbeatTimer = null;
        // Best-effort: tell others we're gone. Server doesn't have a
        // "presence-clear" verb, so we send an empty object as a signal.
        if (transport.connected) transport.send({ type: 'presence', data: null });
      },
      me() { return myState; },
      others() {
        return [...others.entries()].map(([pubkey, { state, lastSeen }]) => ({ pubkey, state, lastSeen }));
      },
      get(pubkey) {
        const e = others.get(pubkey);
        return e ? { pubkey, state: e.state, lastSeen: e.lastSeen } : undefined;
      },
      onJoin(cb)   { return joinE.on(cb); },
      onLeave(cb)  { return leaveE.on(cb); },
      onUpdate(cb) { return updateE.on(cb); },
      _onMessage(msg) {
        const pk = msg.from;
        if (!pk || pk === pubkeyHex) return;
        // `data: null` is our "clear" signal.
        if (msg.data == null) {
          if (others.delete(pk)) leaveE.emit({ pubkey: pk, reason: 'cleared' });
          return;
        }
        const prev = others.get(pk);
        const entry = { state: msg.data, lastSeen: Date.now() };
        others.set(pk, entry);
        if (!prev) joinE.emit({ pubkey: pk, state: entry.state });
        else updateE.emit({ pubkey: pk, state: entry.state });
      },
      _onPeerJoined(msg) {
        // Someone new — rebroadcast our state so they see us immediately.
        if (msg.pubkey && msg.pubkey !== pubkeyHex) broadcast();
      },
      _onPeerLeft(msg) {
        const pk = msg.from;
        if (pk && others.delete(pk)) leaveE.emit({ pubkey: pk, reason: 'disconnect' });
      },
      _onConnect() { broadcast(); startHeartbeat(); },
      _onDisconnect() {
        clearInterval(heartbeatTimer);
        heartbeatTimer = null;
        // Don't clear `others` immediately: the socket may reconnect and the
        // presence sweep / peer-left events will clean up stragglers.
      },
      _shutdown() {
        clearInterval(heartbeatTimer);
        clearInterval(sweepTimer);
        others.clear();
      },
    };
  })();

  // ---- transport ----
  // Forward declaration so the mesh can call transport.send for signaling.
  let transport;

  // ---- peer mesh (D2) ----
  // When transportMode==='ws-only' we install a no-op mesh so every broadcast
  // call collapses to nothing and WS remains the sole path.
  const mesh = (transportMode === 'ws-only')
    ? {
        enabled: false,
        onWelcomePeers() {}, onPeerJoined() {}, onPeerLeft() {}, onSignal() {},
        broadcastPack() { return 0; }, broadcastBlobs() { return 0; },
        peers() { return []; }, shutdown() {},
      }
    : makePeerMesh({
        store,
        getPubkey: () => pubkeyHex,
        sendSignal: (msg) => { try { transport?.send(msg); } catch (_) {} },
        onRemotePack: (ev) => { emitChange({ source: 'remote', type: 'pack', transport: 'webrtc', from: ev.from }); },
        onRemoteBlob: (ev) => { emitChange({ source: 'remote', type: 'blob', transport: 'webrtc', from: ev.from, hash: ev.hash }); },
        iceServers,
        log: logger,
      });

  // ---- transport ----
  transport = makeTransport({
    serverUrl,
    room,
    store,
    getToken,
    getPubkey: () => pubkeyHex,
    getSubscription: () => subscription.patterns.slice(),
    onRemotePack: () => emitChange({ source: 'remote', type: 'pack' }),
    onConnect: () => { connectE.emit(); presence._onConnect(); },
    onDisconnect: () => { disconnectE.emit(); presence._onDisconnect(); },
    onError: (err) => errorE.emit(err),
    onPresence: (msg) => presence._onMessage(msg),
    onPeerJoined: (msg) => { mesh.onPeerJoined(msg.pubkey || msg.from); presence._onPeerJoined(msg); },
    onPeerLeft: (msg) => { mesh.onPeerLeft(msg.from); presence._onPeerLeft(msg); },
    onWelcomePeers: (peers) => mesh.onWelcomePeers(peers),
    onSignal: (msg) => mesh.onSignal(msg),
    log: logger,
    metrics,
  });

  // After any local mutation, broadcast the delta + emit change.
  function afterLocalMutation(ev) {
    const t0 = (globalThis.performance?.now?.() ?? Date.now());
    transport.sendLocalDelta();
    // Also push over any open data channels. Remote peers dedupe by node id.
    try {
      const nodesB64 = store.export_nodes_missing_from(JSON.stringify([]));
      mesh.broadcastPack(nodesB64);
    } catch (_) {}
    emitChange(ev);
    // G8 — op_apply_latency is the local-apply -> local-broadcast round
    // trip. Cheap (<1ms typically); useful for spotting regressions.
    if (metrics.enabled) {
      const t1 = (globalThis.performance?.now?.() ?? Date.now());
      metrics.emit('op_apply_latency', t1 - t0, {
        type: ev.type ?? 'unknown',
        source: ev.source ?? 'local',
      });
    }
  }

  // ---- Map handle ----
  function makeMap(namespace) {
    const changeE = makeEmitter();
    const unsubRoot = anyChange.on(ev => {
      if (ev.type === 'map' && ev.namespace === namespace) {
        if (subscription.matches(ev.path)) changeE.emit(ev);
        return;
      }
      if (ev.type === 'pack' || ev.type === 'bulk') changeE.emit(ev);
    });
    return {
      set(key, value) {
        const fullKey = joinPath(namespace, key);
        store.set(fullKey, jsonToBytes(value));
        afterLocalMutation({ source: 'local', type: 'map', namespace, key, path: fullKey });
      },
      setBlob(key, bytes) {
        const fullKey = joinPath(namespace, key);
        const hash = store.set_blob(fullKey, bytes);
        afterLocalMutation({ source: 'local', type: 'map', namespace, key, path: fullKey, blob: hash });
        // Push the blob bytes to the server so other peers can fetch them.
        // (The metadata node is propagated by sendLocalDelta; the raw bytes
        // are a separate side-channel.)
        transport.sendBlobs([hash]);
        mesh.broadcastBlobs([hash]);
        return hash;
      },
      get(key) {
        const fullKey = joinPath(namespace, key);
        if (!subscription.matches(fullKey)) return undefined;
        const all = JSON.parse(store.resolve_json());
        return all[fullKey];
      },
      getBlob(hashOrKey) {
        // Accepts either a blob hash hex string or a map key; prefers hash.
        try { return store.get_blob_bytes(hashOrKey); } catch (_) {}
        const v = this.get(hashOrKey);
        if (v && typeof v === 'string') { try { return store.get_blob_bytes(v); } catch (_) {} }
        return undefined;
      },
      delete(key) {
        const fullKey = joinPath(namespace, key);
        store.delete(fullKey);
        afterLocalMutation({ source: 'local', type: 'map', namespace, key, path: fullKey, deleted: true });
      },
      all() {
        const flat = JSON.parse(store.resolve_json());
        const prefix = namespace ? (namespace.endsWith('/') ? namespace : namespace + '/') : '';
        const out = {};
        for (const [k, v] of Object.entries(flat)) {
          if (prefix && !k.startsWith(prefix)) continue;
          if (!subscription.matches(k)) continue;
          out[prefix ? k.slice(prefix.length) : k] = v;
        }
        return out;
      },
      onChange(cb) { return changeE.on(cb); },
    };
  }

  // ---- Text handle (RGA, C1) ----
  function makeText(key) {
    if (!subscription.matches(key)) {
      throw new Error(`doc.text(${JSON.stringify(key)}): path is outside the current subscription ${JSON.stringify(subscription.patterns)}`);
    }
    const changeE = makeEmitter();
    const unsubRoot = anyChange.on(ev => {
      if (ev.type === 'text' && ev.path === key) { changeE.emit(ev); return; }
      if (ev.type === 'pack' || ev.type === 'bulk') changeE.emit(ev);
    });
    return {
      insert(pos, str) {
        if (typeof str !== 'string') throw new TypeError('text.insert: str must be a string');
        for (let i = 0; i < str.length; i++) store.insert_text(key, pos + i, str[i]);
        afterLocalMutation({ source: 'local', type: 'text', path: key, op: 'insert', pos, len: str.length });
      },
      delete(pos, len = 1) {
        for (let i = 0; i < len; i++) store.delete_text(key, pos);
        afterLocalMutation({ source: 'local', type: 'text', path: key, op: 'delete', pos, len });
      },
      toString() { return store.resolve_text(key); },
      onChange(cb) { return changeE.on(cb); },
    };
  }

  // ---- List handle (fractional-index CRDT, F8) ----
  //
  // The ordering lives in Op::List on `key`; item content lives in the Map
  // sidecar under `${key}/items/<itemIdHex>`. The SDK composes the two so
  // callers see a flat ordered array of `{id, content}` entries. Subscription
  // patterns covering `key` automatically cover the sidecar (they share a
  // prefix), so no extra subscription work is needed.
  function makeList(key) {
    if (!subscription.matches(key)) {
      throw new Error(`doc.list(${JSON.stringify(key)}): path is outside the current subscription ${JSON.stringify(subscription.patterns)}`);
    }
    const itemsPrefix = key + '/items/';
    const changeE   = makeEmitter();
    const reorderE  = makeEmitter();
    const unsubRoot = anyChange.on(ev => {
      // Local list mutations: ordering events come through type='list',
      // content updates ride on type='map' under itemsPrefix.
      if (ev.type === 'list' && ev.path === key) {
        reorderE.emit(ev);
        changeE.emit(ev);
        return;
      }
      if (ev.type === 'map' && typeof ev.path === 'string' && ev.path.startsWith(itemsPrefix)) {
        changeE.emit(ev);
        return;
      }
      // Remote ops arrive as 'pack' / 'bulk' — we can't tell ordering from
      // content cheaply here, so emit on both channels and let consumers
      // re-resolve. Cheap because re-resolve is HashMap-pass over local nodes.
      if (ev.type === 'pack' || ev.type === 'bulk') {
        reorderE.emit(ev);
        changeE.emit(ev);
      }
    });

    function sidecarKey(idHex) { return itemsPrefix + idHex; }

    function readContent(idHex, allMap) {
      const fullKey = sidecarKey(idHex);
      const b64 = allMap[fullKey];
      if (b64 === undefined) return undefined;
      return bytesToJson(b64decode(b64));
    }

    function newItemId() {
      const a = new Uint8Array(16);
      crypto.getRandomValues(a);
      let out = '';
      for (let i = 0; i < 16; i++) out += a[i].toString(16).padStart(2, '0');
      return out;
    }

    function writeContent(idHex, content) {
      // Sidecar lives under the same subscription prefix as the list key,
      // so it round-trips with the ordering.
      store.set(sidecarKey(idHex), jsonToBytes(content));
    }

    return {
      get length() { return store.list_length(key); },

      ids() {
        return JSON.parse(store.list_ids_json(key));
      },

      get(idHex) {
        const all = JSON.parse(store.resolve_json());
        return readContent(idHex, all);
      },

      toArray() {
        const ids = JSON.parse(store.list_ids_json(key));
        const all = JSON.parse(store.resolve_json());
        const out = new Array(ids.length);
        for (let i = 0; i < ids.length; i++) {
          out[i] = { id: ids[i], content: readContent(ids[i], all) };
        }
        return out;
      },

      push(content) {
        const id = newItemId();
        writeContent(id, content);
        store.list_insert_at(key, this.length, id);
        afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'insert', id, index: this.length - 1 });
        return id;
      },

      insert(index, content) {
        const id = newItemId();
        writeContent(id, content);
        const i = Math.max(0, Math.min(index | 0, this.length));
        store.list_insert_at(key, i, id);
        afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'insert', id, index: i });
        return id;
      },

      insertAfter(anchorIdHex, content) {
        const ids = JSON.parse(store.list_ids_json(key));
        const at  = ids.indexOf(anchorIdHex);
        // Anchor missing → fall back to append. Avoids throwing on a stale
        // id the caller may be holding from before a delete arrived.
        const i = at < 0 ? ids.length : at + 1;
        return this.insert(i, content);
      },

      insertBefore(anchorIdHex, content) {
        const ids = JSON.parse(store.list_ids_json(key));
        const at  = ids.indexOf(anchorIdHex);
        const i = at < 0 ? ids.length : at;
        return this.insert(i, content);
      },

      move(idHex, index) {
        const len = this.length;
        if (len === 0) return; // nothing to move
        const i = Math.max(0, Math.min(index | 0, len - 1));
        store.list_move_to(key, idHex, i);
        afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'move', id: idHex, index: i });
      },

      delete(idHex) {
        store.list_delete(key, idHex);
        // Tombstone the sidecar too — keeps resolve_json() free of orphan
        // content bytes for items no peer can ever reference again.
        try { store.delete(sidecarKey(idHex)); } catch (_) {}
        afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'delete', id: idHex });
      },

      update(idHex, content) {
        writeContent(idHex, content);
        // No list op — pure content edit. Map-level event suffices.
        afterLocalMutation({ source: 'local', type: 'map', namespace: '', key: sidecarKey(idHex), path: sidecarKey(idHex) });
      },

      onChange(cb)  { return changeE.on(cb); },
      onReorder(cb) { return reorderE.on(cb); },

      // ---- Gesture helpers (F8) ----
      //
      // Each helper is a fixed sequence of primitive ops (insert/move/delete)
      // chosen so concurrent gestures from different peers converge to a
      // sensible UX, *not* "loser silently vanishes". Apps that prefer
      // different semantics can call the primitives directly.
      gestures: {
        /**
         * Drop `draggedId` onto `targetId`. If target is still present:
         * tombstone target and move dragged into target's slot (replace).
         * If target was concurrently deleted: append dragged to the end
         * (graceful fallback rather than throwing on a stale anchor).
         *
         * Concurrent dropOnto from two peers against the same target:
         * both deletes are idempotent; both moves operate on different
         * dragged ids so neither overrides the other — both dragged
         * items survive in the converged list.
         */
        dropOnto: (draggedId, targetId) => {
          const ids = JSON.parse(store.list_ids_json(key));
          const at  = ids.indexOf(targetId);
          if (at < 0) {
            // Target already gone. Just position dragged at the end.
            const len = store.list_length(key);
            store.list_move_to(key, draggedId, Math.max(0, len - 1));
            afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'gesture-dropOnto', dragged: draggedId, target: targetId, fallback: 'append' });
            return;
          }
          // Tombstone target first so the subsequent move targets the now-
          // vacant slot. Both ops commit independently and idempotently.
          store.list_delete(key, targetId);
          try { store.delete(sidecarKey(targetId)); } catch (_) {}
          // After delete, the slot at `at` shifted: items that were after
          // target moved up by one. Index `at` now points to where target
          // used to be (or to end if target was last). list_move_to clamps.
          store.list_move_to(key, draggedId, at);
          afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'gesture-dropOnto', dragged: draggedId, target: targetId });
        },

        /**
         * Drop `draggedId` between `beforeId` and `afterId`. Either anchor
         * may be `null` to mean "list start" or "list end" respectively.
         * Stale anchors fall through gracefully (use the surviving anchor;
         * if both are stale, append).
         */
        dropBetween: (draggedId, beforeId, afterId) => {
          const ids = JSON.parse(store.list_ids_json(key));
          let index;
          if (afterId != null) {
            const at = ids.indexOf(afterId);
            if (at >= 0)      index = at;       // land just before `after`
            else if (beforeId != null) {
              const ab = ids.indexOf(beforeId);
              index = ab >= 0 ? ab + 1 : ids.length;
            } else            index = 0;
          } else if (beforeId != null) {
            const ab = ids.indexOf(beforeId);
            index = ab >= 0 ? ab + 1 : ids.length;
          } else              index = ids.length;
          // The bridge clamps & filters dragged, so concurrent
          // dropBetween operations using the same anchors converge by LWW
          // on the dragged item id (last writer wins for that single item).
          store.list_move_to(key, draggedId, index);
          afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'gesture-dropBetween', dragged: draggedId, before: beforeId, after: afterId, index });
        },

        /**
         * Swap two items' positions. Concurrent swaps of overlapping pairs
         * resolve via LWW per moved item — identity is preserved (no item
         * is duplicated or lost).
         */
        swap: (aId, bId) => {
          if (aId === bId) return;
          const ids = JSON.parse(store.list_ids_json(key));
          const ia = ids.indexOf(aId);
          const ib = ids.indexOf(bId);
          if (ia < 0 || ib < 0) return; // either side stale → no-op
          // Move a → b's slot first, then b → a's original slot. Each
          // list_move_to interprets the index in the list with the moved
          // id filtered out, so consecutive swaps don't interfere.
          store.list_move_to(key, aId, ib);
          store.list_move_to(key, bId, ia);
          afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'gesture-swap', a: aId, b: bId });
        },

        /** Move `draggedId` to land immediately before `targetId`. */
        dropBefore: (draggedId, targetId) => {
          const ids = JSON.parse(store.list_ids_json(key));
          const at  = ids.indexOf(targetId);
          const index = at < 0 ? ids.length : at;
          store.list_move_to(key, draggedId, index);
          afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'gesture-dropBefore', dragged: draggedId, target: targetId });
        },

        /** Move `draggedId` to land immediately after `targetId`. */
        dropAfter: (draggedId, targetId) => {
          const ids = JSON.parse(store.list_ids_json(key));
          const at  = ids.indexOf(targetId);
          const index = at < 0 ? ids.length : at + 1;
          store.list_move_to(key, draggedId, index);
          afterLocalMutation({ source: 'local', type: 'list', path: key, op: 'gesture-dropAfter', dragged: draggedId, target: targetId });
        },
      },
    };
  }


  // ---- Doc ----
  const doc = {
    get pubkeyHex() { return pubkeyHex; },
    get authorSeed() { return authorSeed; },
    get isConnected() { return transport.connected; },
    get store() { return store; }, // escape hatch for advanced APIs

    map: makeMap,
    text: makeText,
    list: makeList,
    presence,

    // F3a subscription API -------------------------------------------------
    get subscription() { return subscription.patterns.slice(); },
    isSubscribed(path) { return subscription.matches(path); },
    subscribe(patterns) {
      subscription = compileSubscription(patterns);
      // F3b: push the new filter to the server so it stops relaying
      // non-matching nodes (client still filters its own reads belt-and-braces).
      try { transport.send({ type: 'subscribe', patterns: subscription.patterns.slice() }); } catch (_) {}
      subscriptionChangeE.emit({ patterns: subscription.patterns.slice() });
      emitChange({ source: 'local', type: 'bulk', reason: 'subscription-changed' });
    },
    onSubscriptionChange(cb) { return subscriptionChangeE.on(cb); },

    onChange(cb)     { return anyChange.on(cb); },
    onConnect(cb)    { return connectE.on(cb); },
    onDisconnect(cb) { return disconnectE.on(cb); },
    onError(cb)      { return errorE.on(cb); },
    // G9 — conflict surfacing. `cb` is invoked with
    // `{ at, kind, key, localOp, winningOp:{...,author,lamport}, byYou, raw }`.
    // Fired once per conflict per SDK lifetime (the bridge dedups by
    // fingerprint; the SDK buffer caps at 256 events).
    onConflict(cb)   { return conflictE.on(cb); },
    /** Return buffered conflict events no older than `sinceMs`. */
    recentConflicts(sinceMs = 5 * 60 * 1000) {
      const cutoff = Date.now() - sinceMs;
      return conflictBuffer.filter(ev => ev.at >= cutoff).slice();
    },

    connect()    { transport.connect(); },
    disconnect() { transport.disconnect(); },
    // D2 observability: list known peers and their active transport.
    peers() { return mesh.peers(); },
    // Escape hatch: send a raw wire-protocol message to the server. Used by
    // apps that need verbs the SDK doesn't surface directly (e.g. set-room-key,
    // WebRTC signaling). The SDK handles blob-upload / pack / mst-* / presence
    // automatically — callers generally do not need this.
    send(msg) { transport.send(msg); },
    close() {
      transport.disconnect();
      mesh.shutdown();
      presence._shutdown();
      try { store.free(); } catch (_) {}
    },
  };

  if (autoConnect) transport.connect();

  return doc;
}

// Re-export low-level primitives for apps that want to drop down.
export { SyncStore, sign_room_token, room_pubkey_hex };
