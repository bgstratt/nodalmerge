// sdk.d.ts — TypeScript types for the NodalMerge SDK.

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [k: string]: JsonValue };

export type ChangeSource = 'local' | 'remote';

export interface ChangeEvent {
  source: ChangeSource;
  /** 'map' | 'text' for fine-grained local events; 'pack' for bulk remote merges. */
  type: 'map' | 'text' | 'list' | 'pack' | 'bulk' | 'blob' | 'undo' | 'redo';
  /** 'webrtc' when the event arrived via a peer data channel, 'ws' (or undefined) via the server. */
  transport?: 'ws' | 'webrtc';
  from?: string;
  hash?: string;
  path?: string;
  namespace?: string;
  key?: string;
  op?: 'insert' | 'delete';
  pos?: number;
  len?: number;
  blob?: string;
  deleted?: boolean;
  id?: string;
  index?: number;
  reason?: string;
  origin?: string;
  dragged?: string;
  target?: string;
  before?: string | null;
  after?: string | null;
  a?: string;
  b?: string;
  fallback?: string;
}

export type Unsubscribe = () => void;

export interface NamespaceCapabilitySpec {
  read?: string[];
  write?: string[];
  derive?: string[];
}

export type TokenCapsOption = string[] | NamespaceCapabilitySpec;

export interface MapHandle {
  set(key: string, value: JsonValue): void;
  setBlob(key: string, bytes: Uint8Array, options?: { contentType?: string }): string;
  get(key: string): JsonValue | undefined;
  getBlob(hashOrKey: string): Uint8Array | undefined;
  delete(key: string): void;
  /** Returns a shallow object of every key currently resolved under this namespace. */
  all(): Record<string, JsonValue>;
  onChange(cb: (ev: ChangeEvent) => void): Unsubscribe;
}

export interface TextHandle {
  insert(pos: number, str: string): void;
  delete(pos: number, len?: number): void;
  toString(): string;
  onChange(cb: (ev: ChangeEvent) => void): Unsubscribe;
}

export interface ListEntry<T = JsonValue> {
  /** 32-char hex item id (16 raw bytes), stable across moves and re-orderings. */
  id: string;
  /** JSON-decoded item content from the sidecar Map, or undefined if the
   *  content node hasn't arrived yet (ordering can briefly outrun content). */
  content: T | undefined;
}

export interface ListHandle<T = JsonValue> {
  /** Number of visible (non-tombstoned) items. */
  readonly length: number;
  /** Item ids in order — cheap; doesn't decode content. */
  ids(): string[];
  /** Decoded item content for `id`, or undefined if missing. */
  get(id: string): T | undefined;
  /** Visible items in order, with decoded content. */
  toArray(): ListEntry<T>[];
  /** Append `content` to the end. Returns the new item id. */
  push(content: T): string;
  /** Insert at index (clamped to [0, length]). Returns the new item id. */
  insert(index: number, content: T): string;
  /** Insert immediately after `anchorId`. Falls back to append if the anchor
   *  is missing (e.g. concurrently deleted). Returns the new item id. */
  insertAfter(anchorId: string, content: T): string;
  /** Insert immediately before `anchorId`. Falls back to append if missing. */
  insertBefore(anchorId: string, content: T): string;
  /** Move an existing item to `index`, interpreted in the list with the
   *  moved item removed (so `move(id, length-1)` always lands at the end). */
  move(id: string, index: number): void;
  /** Tombstone the item; idempotent. */
  delete(id: string): void;
  /** Replace the item content; ordering is unchanged. */
  update(id: string, content: T): void;
  /** Fires on any change — ordering or content — for this list. */
  onChange(cb: (ev: ChangeEvent) => void): Unsubscribe;
  /** Fires only when ordering changes (insert / move / delete / remote pack). */
  onReorder(cb: (ev: ChangeEvent) => void): Unsubscribe;
  /** Drag-and-drop gestures composed from primitive ops, designed so
   *  concurrent gestures from different peers converge to sensible UX
   *  (no dragged item silently vanishes). Apps that prefer different
   *  semantics can call the primitives directly. */
  gestures: ListGestures;
}

export interface ListGestures {
  /** Drop dragged onto target. If target is present, tombstones it and
   *  moves dragged into its slot (replace). If target was already
   *  concurrently deleted, appends dragged to the end. Concurrent
   *  dropOnto against the same target preserves both dragged items. */
  dropOnto(draggedId: string, targetId: string): void;
  /** Drop dragged into the gap between two anchors. Either anchor may be
   *  null for list-start / list-end. Stale anchors fall through. */
  dropBetween(draggedId: string, beforeId: string | null, afterId: string | null): void;
  /** Swap two items' positions. No-op if either id is missing. */
  swap(aId: string, bId: string): void;
  /** Move dragged to land immediately before target (append on stale target). */
  dropBefore(draggedId: string, targetId: string): void;
  /** Move dragged to land immediately after target (append on stale target). */
  dropAfter(draggedId: string, targetId: string): void;
}

export interface PresencePeer {
  pubkey: string;
  state: Record<string, JsonValue>;
  lastSeen: number;
}

export interface PresenceJoinEvent   { pubkey: string; state: Record<string, JsonValue>; }
export interface PresenceUpdateEvent { pubkey: string; state: Record<string, JsonValue>; }
export interface PresenceLeaveEvent  { pubkey: string; reason: 'disconnect' | 'stale' | 'cleared'; }

export interface PresenceHandle {
  /** Merge-update our local presence state and broadcast. Returns the new merged state. */
  set(patch: Record<string, JsonValue>): Record<string, JsonValue>;
  /** Stop broadcasting and signal clear to other peers. */
  clear(): void;
  /** Our current local state, or null if `set` was never called. */
  me(): Record<string, JsonValue> | null;
  /** All known remote peers with fresh presence. */
  others(): PresencePeer[];
  /** Lookup a specific peer's state. */
  get(pubkey: string): PresencePeer | undefined;
  onJoin(cb: (ev: PresenceJoinEvent) => void): Unsubscribe;
  onLeave(cb: (ev: PresenceLeaveEvent) => void): Unsubscribe;
  onUpdate(cb: (ev: PresenceUpdateEvent) => void): Unsubscribe;
}

export interface MeshPeer {
  pubkey: string;
  /** Active outbound transport for this peer: 'webrtc' when a data channel is open, 'ws' otherwise. */
  transport: 'ws' | 'webrtc';
  syncReady: boolean;
  blobsReady: boolean;
  connectionState: RTCPeerConnectionState | 'unknown';
}

export interface ConflictEvent {
  at: number;
  kind: string;
  key: string;
  localOp: unknown;
  winningOp: unknown;
  byYou: boolean;
  raw: unknown;
}

export interface RejectionEvent {
  at: number;
  source: 'server';
  reasonClass: string | null;
  command: string | null;
  requiredCapability: string | null;
  message: string;
  status: number | null;
  raw: unknown;
}

export interface NodalMergeRejectionError extends Error {
  rejection?: RejectionEvent;
  serverEnvelope?: Record<string, unknown>;
}

export interface MapHistoryItem {
  lamport: number;
  wallMs: number;
  /** Author pubkey (64-char hex). */
  author: string;
  /** DAG node id (64-char hex). */
  nodeId: string;
  op: 'set' | 'delete';
  key: string;
  /** Decoded like `map().get()`; undefined for deletes. */
  value: unknown;
}

export interface UndoManager {
  undo(): boolean;
  redo(): boolean;
  clear(): void;
  commit(): void;
  readonly undoDepth: number;
  readonly redoDepth: number;
  destroy(): void;
}

export interface Doc {
  readonly pubkeyHex: string;
  readonly authorSeed: Uint8Array;
  readonly isConnected: boolean;
  /** Escape hatch: the low-level `SyncStore` handle. Use for advanced APIs
   *  (speculative vs canonical reads, frontier inspection, etc). */
  readonly store: unknown;

  map(namespace: string): MapHandle;
  text(key: string): TextHandle;
  /** Ordered list with fractional-index CRDT semantics (F8). Item ordering
   *  lives in `Op::List` for `key`; item content lives in the sidecar Map
   *  at `${key}/items/<itemId>`. The SDK composes the two transparently. */
  list<T = JsonValue>(key: string): ListHandle<T>;
  /** Ephemeral side-channel for cursors, selections, and other
   *  not-stored-in-the-DAG state. See PresenceHandle. */
  readonly presence: PresenceHandle;

  /** Active subscription glob patterns (F3a). Reads and change events are
   *  filtered to these patterns; writes are not. */
  readonly subscription: string[];
  isSubscribed(path: string): boolean;
  subscribe(patterns: string[]): void;
  onSubscriptionChange(cb: (ev: { patterns: string[] }) => void): Unsubscribe;

  onChange(cb: (ev: ChangeEvent) => void): Unsubscribe;
  onConnect(cb: () => void): Unsubscribe;
  onDisconnect(cb: () => void): Unsubscribe;
  onError(cb: (err: NodalMergeRejectionError) => void): Unsubscribe;
  /** Typed server-side rejection stream parsed from error envelopes and reject prefixes. */
  onRejection(cb: (ev: RejectionEvent) => void): Unsubscribe;
  /** Return buffered rejections newer than `sinceMs` (default 5 minutes). */
  recentRejections(sinceMs?: number): RejectionEvent[];
  /** G9 conflict surfacing hook. */
  onConflict(cb: (ev: ConflictEvent) => void): Unsubscribe;
  /** Return buffered conflicts newer than `sinceMs` (default 5 minutes). */
  recentConflicts(sinceMs?: number): ConflictEvent[];

  /**
   * Local DAG map-op history for keys under `prefix`, ordered by
   * (lamport, node id) — deterministic causal order, identical on every
   * peer. Late-join catch-up delivers real op nodes, so this includes
   * history from before this peer joined; it reaches back to the last
   * snapshot rebuild (compact-room), not before it.
   */
  history(opts: {
    prefix: string;
    fromLamport?: number;
    limit?: number;
    cursor?: string | null;
  }): { items: MapHistoryItem[]; nextCursor: string | null };

  /** E3 app-layer undo/redo manager using compensating ops. */
  undoManager(opts?: {
    scope?: string[];
    captureTimeout?: number;
    maxItems?: number;
  }): UndoManager;

  connect(): void;
  disconnect(): void;
  /** List known peers and their active transport (D2). */
  peers(): MeshPeer[];
  /** Escape hatch — send a raw wire-protocol message (e.g. `set-room-key`,
   *  WebRTC signaling). Most apps do not need this. */
  send(msg: Record<string, unknown>): void;
  close(): void;
}

export interface CreateDocOptions {
  serverUrl: string;
  room: string;
  authorSeed?: Uint8Array;
  roomSeed?: Uint8Array;
  /**
   * Capability grants for room token signing.
   *
   * String-array form uses full capability strings:
   *   ["read:world/**", "write:intent/**"]
   *
   * Object form is namespace-focused ergonomics and expands to canonical
   * capability strings:
   *   { read: ["world"], write: ["intent"] }
   */
  tokenCaps?: TokenCapsOption;
  tokenExpirySecs?: number;
  /**
   * Server-mint hook (downstream Phase 5a). When provided, the SDK calls
   * this on every (re)connect to obtain a RoomToken signed by a trusted
   * bridge. Takes precedence over `roomSeed` / `tokenCaps`. The provider
   * receives `{room, pubkeyHex}` and must return a token shaped like the
   * upstream JWT-bridge `/mint` response.
   */
  tokenProvider?: (ctx: { room: string; pubkeyHex: string }) => Promise<{
    peer_pubkey_hex: string;
    expiry_secs: number;
    capabilities: string[];
    sig_hex: string;
    continuity?: {
      predecessor_peer_pubkey: string;
      overlap_not_after: number;
      revoked_predecessors?: string[];
    };
  }>;
  /** Typed server-side rejection callback (same payload as `doc.onRejection`). */
  onRejection?: (ev: RejectionEvent) => void;
  autoConnect?: boolean;
  /** F3a: glob patterns for client-side materialization. Default `["**"]`. */
  subscribe?: string[];
  /** 'auto' (default) opens WebRTC data channels to other peers and sends
   *  packs/blobs over them in parallel with WS. 'ws-only' disables WebRTC
   *  entirely — useful for corporate networks or debugging. */
  transport?: 'auto' | 'ws-only';
  /** Override STUN/TURN servers for WebRTC peers. Default: public Google STUN. */
  iceServers?: RTCIceServer[];
  /** Interval (ms) between presence heartbeats. 0 disables. Default 15000. */
  presenceHeartbeatMs?: number;
  /** Time (ms) after which a silent peer is treated as gone. Default 45000. */
  presenceStaleMs?: number;
  logger?: (level: 'log' | 'info' | 'warn' | 'error', ...args: unknown[]) => void;
  /** G8 metric hook. No-op overhead when unset. */
  onMetric?: (ev: {
    kind: string;
    value: number;
    labels: Record<string, unknown>;
    timestamp: number;
  }) => void;
  /** F6: callback invoked after a direct presigned PUT upload completes successfully. */
  onDirectUpload?: (args: { hash: string; length: number }) => void | Promise<void>;
  persistence?: {
    enabled?: boolean;
    dbName?: string;
    dbVersion?: number;
    debounceMs?: number;
    migrateLegacyDemo?: boolean;
  };
  /** Bench/dev only: skip Ed25519 signing of locally-authored nodes. Unsigned
   *  nodes are rejected by servers, so only use with local-only docs
   *  (`autoConnect: false`). Lets benchmarks isolate engine cost from
   *  signature cost. Default: false (sign everything). */
  unsignedNodes?: boolean;
  /** wasm-bindgen InitInput (URL string, Request, Module, or bytes) for bundlers
   *  that relocate the bridge .wasm (e.g. Vite `?url` imports). Default: fetch
   *  relative to the bridge module. First init wins; later inputs are ignored. */
  wasmModule?: string | URL | Request | WebAssembly.Module | BufferSource | Promise<unknown>;
}

export function createDoc(opts: CreateDocOptions): Promise<Doc>;
export function ready(wasmInput?: CreateDocOptions['wasmModule']): Promise<unknown>;

/** Build a single capability string like `read:world/**`. */
export function capability(
  scope: 'read' | 'write' | 'derive',
  pathPattern: string,
): string;

/** Build canonical namespace capability strings from a namespace spec. */
export function namespaceCapabilities(spec?: NamespaceCapabilitySpec): string[];

export interface AttachServerOptions {
  mode?: 'immediate' | 'wait-for-welcome' | 'manual';
  onProgress?: (
    stage: 'start' | 'manual' | 'done' | 'error',
    payload: Record<string, unknown>,
  ) => void;
}

/** Lightweight transport-attach helper used by demo/harness flows. */
export function attachServer(
  doc: Doc,
  serverUrl: string,
  options?: AttachServerOptions,
): Promise<void>;

// Low-level re-exports.
export { SyncStore, sign_room_token, room_pubkey_hex } from './pkg/nodalmerge_bridge.js';
