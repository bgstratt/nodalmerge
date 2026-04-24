// sdk.d.ts — TypeScript types for the ActiveSync SDK.

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
  type: 'map' | 'text' | 'pack' | 'bulk' | 'blob';
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
}

export type Unsubscribe = () => void;

export interface MapHandle {
  set(key: string, value: JsonValue): void;
  setBlob(key: string, bytes: Uint8Array): string;
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

export interface Doc {
  readonly pubkeyHex: string;
  readonly authorSeed: Uint8Array;
  readonly isConnected: boolean;
  /** Escape hatch: the low-level `SyncStore` handle. Use for advanced APIs
   *  (speculative vs canonical reads, frontier inspection, etc). */
  readonly store: unknown;

  map(namespace: string): MapHandle;
  text(key: string): TextHandle;
  /** Not implemented yet — the List CRDT (fractional indexing + move op) is
   *  tracked as post-F in PLAN.md. Throws on call. */
  list(key: string): never;
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
  onError(cb: (err: Error) => void): Unsubscribe;

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
  tokenCaps?: string[];
  tokenExpirySecs?: number;
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
  logger?: (level: 'info' | 'warn' | 'error', ...args: unknown[]) => void;
}

export function createDoc(opts: CreateDocOptions): Promise<Doc>;
export function ready(): Promise<unknown>;

// Low-level re-exports.
export { SyncStore, sign_room_token, room_pubkey_hex } from './pkg/activesync_bridge.js';
