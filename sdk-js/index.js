import initBridge, { SyncStore } from "nodalmerge-bridge";

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

const defaultReconnectPolicy = {
  enabled: true,
  initialDelayMs: 500,
  maxDelayMs: 15_000,
  factor: 2,
  maxAttempts: Infinity
};

const runtimeMessageTypes = new Set([
  "welcome",
  "pack",
  "blob-pack",
  "blob-redirect",
  "presence",
  "presence-snapshot",
  "presence-leave",
  "peer-joined",
  "peer-left",
  "peer-signal",
  "webrtc-offer",
  "webrtc-answer",
  "webrtc-ice",
  "error",
  "noop-ack",
  "session-opened",
  "session-closed"
]);

function toBase64(bytes) {
  if (typeof Buffer !== "undefined") {
    return Buffer.from(bytes).toString("base64");
  }

  let binary = "";
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

function fromBase64(b64) {
  if (!b64) {
    return new Uint8Array(0);
  }

  if (typeof Buffer !== "undefined") {
    return new Uint8Array(Buffer.from(b64, "base64"));
  }

  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

function createAuthorKey(seed) {
  if (seed) {
    if (!(seed instanceof Uint8Array) || seed.length !== 32) {
      throw new Error("authorKey must be a 32-byte Uint8Array");
    }
    return seed;
  }

  const key = new Uint8Array(32);
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    crypto.getRandomValues(key);
    return key;
  }

  throw new Error("No crypto.getRandomValues available; provide authorKey explicitly");
}

function parseJsonOrDefault(value, fallback) {
  try {
    return JSON.parse(value);
  } catch {
    return fallback;
  }
}

function hasLocalStorage() {
  return typeof window !== "undefined" && typeof window.localStorage !== "undefined";
}

function buildReconnectPolicy(input) {
  if (!input) {
    return { ...defaultReconnectPolicy };
  }

  return {
    enabled: input.enabled ?? defaultReconnectPolicy.enabled,
    initialDelayMs: input.initialDelayMs ?? defaultReconnectPolicy.initialDelayMs,
    maxDelayMs: input.maxDelayMs ?? defaultReconnectPolicy.maxDelayMs,
    factor: input.factor ?? defaultReconnectPolicy.factor,
    maxAttempts: input.maxAttempts ?? defaultReconnectPolicy.maxAttempts
  };
}

function shouldEmitSignalEvent(msg) {
  const type = typeof msg?.type === "string" ? msg.type : "";
  return (
    type === "peer-joined" ||
    type === "peer-left" ||
    type === "peer-signal" ||
    type === "webrtc-offer" ||
    type === "webrtc-answer" ||
    type === "webrtc-ice"
  );
}

function shouldEmitPresenceEvent(msg) {
  const type = typeof msg?.type === "string" ? msg.type : "";
  return type === "presence" || type === "presence-snapshot" || type === "presence-leave";
}

function cloneMessagePayload(payload) {
  return parseJsonOrDefault(JSON.stringify(payload), payload);
}

function normalizeTransportMode(mode) {
  if (mode === "auto") {
    return "auto";
  }
  return "ws-only";
}

function isNonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

function normalizeRangeAnchor(anchor, op) {
  if (!anchor || typeof anchor !== "object") {
    throw new Error(`${op} anchor must be an object`);
  }

  const kind = typeof anchor.kind === "string" ? anchor.kind : "";
  if (kind === "offset") {
    if (!isNonNegativeInteger(anchor.pos)) {
      throw new Error(`${op} offset anchor requires non-negative integer pos`);
    }
    return { kind, pos: anchor.pos };
  }

  if (kind === "start") {
    return { kind };
  }

  if (kind === "end") {
    if (op === "delete") {
      throw new Error("delete does not support end anchor");
    }
    return { kind };
  }

  if (kind === "after") {
    const { lamport, author } = anchor;
    let lamportBig;
    if (typeof lamport === "bigint") {
      lamportBig = lamport;
    } else {
      const lamportNum = Number(lamport);
      if (!Number.isFinite(lamportNum) || !Number.isInteger(lamportNum)) {
        throw new Error(`${op} after anchor requires integer lamport`);
      }
      lamportBig = BigInt(lamportNum);
    }
    if (lamportBig < 0n) {
      throw new Error(`${op} after anchor requires non-negative lamport`);
    }
    if (typeof author !== "string" || !/^[0-9a-fA-F]{64}$/.test(author)) {
      throw new Error(`${op} after anchor requires 64-char hex author`);
    }
    return { kind, lamport: lamportBig, author: author.toLowerCase() };
  }

  throw new Error(`${op} anchor kind must be one of: offset, start, end, after`);
}

export function parseRuntimeMessage(data) {
  const parsed = parseJsonOrDefault(data, null);
  if (!parsed || typeof parsed !== "object") {
    return null;
  }

  if (typeof parsed.type !== "string") {
    return null;
  }

  if (!runtimeMessageTypes.has(parsed.type)) {
    return null;
  }

  return parsed;
}

export class ActiveSyncSdk {
  constructor(options) {
    this.options = options;
    this.store = null;
    this.ws = null;
    this.outbox = [];
    this.handlers = {
      message: [],
      error: [],
      state: [],
      connected: [],
      disconnected: [],
      reconnect: [],
      presence: [],
      signal: [],
      "runtime-message": [],
      transport: []
    };
    this.connected = false;
    this.manualDisconnect = false;
    this.reconnectAttempt = 0;
    this.reconnectTimer = null;
    this.reconnectPolicy = buildReconnectPolicy(options.reconnect);
    this.offlinePersistenceKey = options.offline?.persistenceKey ?? null;
    this.transportPolicy = normalizeTransportMode(options.transport?.mode);
    this.activeTransportMode = "ws-only";

    this.compat = createSdkMigrationShim(this);
  }

  async initialize() {
    const initInput = this.options.wasmModule;
    if (initInput) {
      await initBridge(initInput);
    } else {
      await initBridge();
    }

    const authorKey = createAuthorKey(this.options.authorKey);
    this.store = new SyncStore(authorKey);

    this.loadPersistedOutbox();

    if (this.options.tickIntervalMs && this.options.maxOpsPerTick) {
      this.store.set_tick_config(BigInt(this.options.tickIntervalMs), this.options.maxOpsPerTick);
    }
  }

  room = {
    connect: async () => {
      if (!this.store) {
        throw new Error("initialize() must be called before connect()");
      }

      this.manualDisconnect = false;
      this.clearReconnectTimer();

      if (this.ws && this.ws.readyState === WebSocket.OPEN) {
        return;
      }

      const ws = new WebSocket(this.options.wsUrl);
      this.ws = ws;

      await new Promise((resolve, reject) => {
        ws.onopen = () => resolve();
        ws.onerror = (err) => reject(err);
      });

      this.connected = true;
      this.reconnectAttempt = 0;
      this.emitState();
      this.emit("connected", this.topology.snapshot());

      ws.onmessage = (evt) => {
        if (typeof evt.data !== "string") {
          return;
        }

        const msg = parseJsonOrDefault(evt.data, null);
        if (!msg || typeof msg !== "object") {
          return;
        }

        const runtimeMessage = parseRuntimeMessage(evt.data);
        if (runtimeMessage) {
          this.emit("runtime-message", runtimeMessage);
        }

        if (msg.type === "pack" && typeof msg.nodes === "string") {
          this.store.import_pack(msg.nodes);
          this.emitState();
        }

        if (msg.type === "blob-pack" && Array.isArray(msg.blobs)) {
          for (const entry of msg.blobs) {
            if (entry && typeof entry.hash === "string" && typeof entry.data === "string") {
              this.store.store_blob_bytes(entry.hash, fromBase64(entry.data));
            }
          }
          this.emitState();
        }

        if (shouldEmitPresenceEvent(msg)) {
          this.emit("presence", cloneMessagePayload(msg));
        }

        if (shouldEmitSignalEvent(msg)) {
          if (this.transportPolicy === "auto") {
            this.setActiveTransportMode("ws+webrtc");
          }
          this.emit("signal", cloneMessagePayload(msg));
        }

        this.emit("message", msg);
      };

      ws.onclose = () => {
        this.connected = false;
        this.emitState();
        this.emit("disconnected", this.topology.snapshot());

        if (!this.manualDisconnect) {
          this.scheduleReconnect();
        }
      };

      const hello = {
        type: "hello",
        room: this.options.roomId,
        pubkey: this.store.pubkey_hex(),
        frontier: parseJsonOrDefault(this.store.frontier_hex_json(), []),
        caps: parseJsonOrDefault(this.store.our_capabilities_json(), {})
      };

      if (this.options.token) {
        hello.token = this.options.token;
      }

      this.sendOrQueue(hello);
      this.flushOutbox();
      this.sync.pull();
    },

    disconnect: () => {
      this.manualDisconnect = true;
      this.clearReconnectTimer();
      if (this.ws) {
        this.ws.close();
      }
      this.connected = false;
      this.emitState();
    }
  };

  sync = {
    set: (key, value) => {
      this.store.set(key, textEncoder.encode(value));
    },

    get: (key) => {
      const b64 = this.store.read_speculative(key);
      if (!b64) {
        return null;
      }
      return textDecoder.decode(fromBase64(b64));
    },

    getText: (key) => {
      return this.store.resolve_text(key);
    },

    getTextCanonical: (key) => {
      return this.store.resolve_text_canonical(key);
    },

    del: (key) => {
      this.store.delete(key);
    },

    insertTextAt: (key, pos, text) => {
      if (typeof text !== "string" || text.length === 0) {
        return;
      }
      if (!isNonNegativeInteger(pos)) {
        throw new Error("insertTextAt pos must be a non-negative integer");
      }
      this.store.insert_text_range(key, pos, text);
    },

    deleteTextAt: (key, pos, len) => {
      if (!isNonNegativeInteger(pos)) {
        throw new Error("deleteTextAt pos must be a non-negative integer");
      }
      if (!isNonNegativeInteger(len)) {
        throw new Error("deleteTextAt len must be a non-negative integer");
      }
      if (len === 0) {
        return;
      }
      this.store.delete_text_range(key, pos, len);
    },

    insertTextRange: (key, anchor, text) => {
      if (typeof text !== "string" || text.length === 0) {
        return;
      }
      const normalized = normalizeRangeAnchor(anchor, "insert");
      switch (normalized.kind) {
        case "offset":
          this.store.insert_text_range(key, normalized.pos, text);
          return;
        case "start":
          this.store.insert_text_range_start(key, text);
          return;
        case "end":
          this.store.insert_text_range_end(key, text);
          return;
        case "after":
          this.store.insert_text_range_after(key, normalized.lamport, normalized.author, text);
          return;
        default:
          throw new Error("Unsupported insert anchor kind");
      }
    },

    deleteTextRange: (key, anchor, len) => {
      if (!isNonNegativeInteger(len)) {
        throw new Error("deleteTextRange len must be a non-negative integer");
      }
      if (len === 0) {
        return;
      }
      const normalized = normalizeRangeAnchor(anchor, "delete");
      switch (normalized.kind) {
        case "offset":
          this.store.delete_text_range(key, normalized.pos, len);
          return;
        case "start":
          this.store.delete_text_range_start(key, len);
          return;
        case "after":
          this.store.delete_text_range_after(key, normalized.lamport, normalized.author, len);
          return;
        default:
          throw new Error("Unsupported delete anchor kind");
      }
    },

    push: () => {
      this.sendOrQueue({
        type: "pack",
        nodes: this.store.export_all_nodes()
      });
    },

    pull: () => {
      this.sendOrQueue({
        type: "request",
        known: parseJsonOrDefault(this.store.all_node_ids_json(), [])
      });
    }
  };

  replay = {
    state: () => parseJsonOrDefault(this.store.resolve_json(), {}),
    canonicalHash: () => this.store.resolved_state_hash_hex(),
    replayPack: (packB64) => parseJsonOrDefault(this.store.replay_nodes_json(packB64), {})
  };

  offline = {
    outboxDepth: () => this.outbox.length,
    flush: () => this.flushOutbox(),
    clearPersisted: () => this.clearPersistedOutbox()
  };

  presence = {
    set: (data, options = {}) => {
      this.sendOrQueue({
        type: "presence-set",
        session_id: options.sessionId,
        data,
        ttl_ms: options.ttlMs,
        now_unix_ms: options.nowUnixMs
      });
    },

    getAll: () => {
      this.sendOrQueue({
        type: "presence-get"
      });
    },

    sweep: (nowUnixMs) => {
      this.sendOrQueue({
        type: "presence-sweep",
        now_unix_ms: nowUnixMs
      });
    }
  };

  signaling = {
    relay: (type, to, payload = {}) => {
      this.sendOrQueue({
        type,
        to,
        ...payload
      });
    },

    offer: (to, sdp) => {
      this.signaling.relay("webrtc-offer", to, { sdp });
    },

    answer: (to, sdp) => {
      this.signaling.relay("webrtc-answer", to, { sdp });
    },

    ice: (to, candidate, sdpMid, sdpMLineIndex) => {
      this.signaling.relay("webrtc-ice", to, { candidate, sdpMid, sdpMLineIndex });
    }
  };

  cas = {
    setBlob: (key, bytes) => {
      const hash = this.store.set_blob(key, bytes);
      this.sync.push();
      return hash;
    },

    getBlob: (hashHex) => this.store.get_blob_bytes(hashHex),

    requestMissingBlobs: () => {
      const hashes = parseJsonOrDefault(this.store.missing_blob_hashes_json(), []);
      if (hashes.length === 0) {
        return;
      }
      this.sendOrQueue({
        type: "blob-request",
        hashes
      });
    }
  };

  topology = {
    snapshot: () => ({
      connected: this.connected,
      outboxDepth: this.outbox.length,
      nodeCount: this.store.node_count(),
      frontier: parseJsonOrDefault(this.store.frontier_hex_json(), []),
      pubkey: this.store.pubkey_hex(),
      transportPolicy: this.transportPolicy,
      activeTransport: this.activeTransportMode
    })
  };

  on(event, handler) {
    const bucket = this.handlers[event];
    if (!bucket) {
      throw new Error(`Unsupported event type: ${event}`);
    }
    bucket.push(handler);

    return () => {
      const idx = bucket.indexOf(handler);
      if (idx >= 0) {
        bucket.splice(idx, 1);
      }
    };
  }

  sendOrQueue(message) {
    const payload = JSON.stringify(message);
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(payload);
      return;
    }
    this.outbox.push(payload);
    this.persistOutbox();
  }

  flushOutbox() {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      return;
    }

    while (this.outbox.length > 0) {
      const msg = this.outbox.shift();
      this.ws.send(msg);
    }

    this.persistOutbox();
    this.emitState();
  }

  scheduleReconnect() {
    if (!this.reconnectPolicy.enabled) {
      return;
    }

    if (this.reconnectAttempt >= this.reconnectPolicy.maxAttempts) {
      return;
    }

    const delay = Math.min(
      this.reconnectPolicy.maxDelayMs,
      this.reconnectPolicy.initialDelayMs * Math.pow(this.reconnectPolicy.factor, this.reconnectAttempt)
    );

    this.reconnectAttempt += 1;
    this.emit("reconnect", { attempt: this.reconnectAttempt, delayMs: delay });
    this.clearReconnectTimer();
    this.reconnectTimer = setTimeout(() => {
      this.room.connect().catch((error) => this.emit("error", error));
    }, delay);
  }

  clearReconnectTimer() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  persistOutbox() {
    if (!this.offlinePersistenceKey || !hasLocalStorage()) {
      return;
    }

    try {
      window.localStorage.setItem(this.offlinePersistenceKey, JSON.stringify(this.outbox));
    } catch {
      // Best effort persistence only.
    }
  }

  loadPersistedOutbox() {
    if (!this.offlinePersistenceKey || !hasLocalStorage()) {
      return;
    }

    try {
      const raw = window.localStorage.getItem(this.offlinePersistenceKey);
      if (!raw) {
        return;
      }

      const parsed = parseJsonOrDefault(raw, []);
      if (Array.isArray(parsed)) {
        this.outbox = parsed.filter((entry) => typeof entry === "string");
      }
    } catch {
      // Ignore malformed persistence data.
    }
  }

  clearPersistedOutbox() {
    this.outbox = [];
    this.persistOutbox();
  }

  setActiveTransportMode(nextMode) {
    if (this.activeTransportMode === nextMode) {
      return;
    }

    this.activeTransportMode = nextMode;
    this.emit("transport", {
      policy: this.transportPolicy,
      active: this.activeTransportMode
    });
    this.emitState();
  }

  emit(event, payload) {
    for (const handler of this.handlers[event]) {
      try {
        handler(payload);
      } catch (err) {
        if (event !== "error") {
          this.emit("error", err);
        }
      }
    }
  }

  emitState() {
    this.emit("state", this.topology.snapshot());
  }
}

export async function createActiveSyncSdk(options) {
  const client = new ActiveSyncSdk(options);
  await client.initialize();
  return client;
}

export function createSdkMigrationShim(sdk) {
  return {
    sendPack: () => sdk.sync.push(),
    requestServerPack: () => sdk.sync.pull(),
    setPresence: (data, options) => sdk.presence.set(data, options),
    sendWebRtcOffer: (to, sdp) => sdk.signaling.offer(to, sdp),
    sendWebRtcAnswer: (to, sdp) => sdk.signaling.answer(to, sdp),
    sendWebRtcIce: (to, candidate, sdpMid, sdpMLineIndex) => sdk.signaling.ice(to, candidate, sdpMid, sdpMLineIndex),
    onRuntimeEvent: (type, handler) => sdk.on("runtime-message", (message) => {
      if (message && message.type === type) {
        handler(message);
      }
    })
  };
}
