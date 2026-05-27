export interface NodalMergeSdkOptions {
  wsUrl: string;
  roomId: string;
  authorKey?: Uint8Array;
  wasmModule?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module;
  token?: unknown;
  tickIntervalMs?: number;
  maxOpsPerTick?: number;
  reconnect?: {
    enabled?: boolean;
    initialDelayMs?: number;
    maxDelayMs?: number;
    factor?: number;
    maxAttempts?: number;
  };
  offline?: {
    persistenceKey?: string;
  };
  transport?: {
    mode?: "ws-only" | "auto";
  };
}

export interface TopologySnapshot {
  connected: boolean;
  outboxDepth: number;
  nodeCount: number;
  frontier: string[];
  pubkey: string;
  transportPolicy: "ws-only" | "auto";
  activeTransport: "ws-only" | "ws+webrtc";
}

export type NodalMergeRuntimeMessageType =
  | "welcome"
  | "pack"
  | "blob-pack"
  | "blob-redirect"
  | "presence"
  | "presence-snapshot"
  | "presence-leave"
  | "peer-joined"
  | "peer-left"
  | "peer-signal"
  | "webrtc-offer"
  | "webrtc-answer"
  | "webrtc-ice"
  | "error"
  | "noop-ack"
  | "session-opened"
  | "session-closed"
  | "query.registered"
  | "query.register.rejected"
  | "projection.build.completed"
  | "projection.build.rejected"
  | "projection.read.result"
  | "projection.read.rejected"
  | "projection.invalidated"
  | "projection.invalidate.rejected"
  | "projection.list.result"
  | "projection.list.rejected";

export interface NodalMergeRuntimeMessage {
  type: NodalMergeRuntimeMessageType;
  [key: string]: unknown;
}

export type ProjectionCheckpointSelector =
  | { selector?: "latest" }
  | { selector: "seq"; canonical_seq: number }
  | { selector: "hash"; canonical_hash: string }
  | { selector: "frontier"; frontier: string[] };

export type NodalMergeSdkEvent =
  | "message"
  | "error"
  | "state"
  | "connected"
  | "disconnected"
  | "reconnect"
  | "presence"
  | "signal"
  | "runtime-message"
  | "transport";

export type TextInsertAnchor =
  | { kind: "offset"; pos: number }
  | { kind: "start" }
  | { kind: "end" }
  | { kind: "after"; lamport: number | bigint; author: string };

export type TextDeleteAnchor =
  | { kind: "offset"; pos: number }
  | { kind: "start" }
  | { kind: "after"; lamport: number | bigint; author: string };

export declare class NodalMergeSdk {
  constructor(options: NodalMergeSdkOptions);
  initialize(): Promise<void>;

  room: {
    connect: () => Promise<void>;
    disconnect: () => void;
  };

  sync: {
    set: (key: string, value: string) => void;
    get: (key: string) => string | null;
    getText: (key: string) => string;
    getTextCanonical: (key: string) => string;
    del: (key: string) => void;
    insertTextAt: (key: string, pos: number, text: string) => void;
    deleteTextAt: (key: string, pos: number, len: number) => void;
    insertTextRange: (key: string, anchor: TextInsertAnchor, text: string) => void;
    deleteTextRange: (key: string, anchor: TextDeleteAnchor, len: number) => void;
    push: () => void;
    pull: () => void;
  };

  replay: {
    state: () => Record<string, string>;
    canonicalHash: () => string;
    replayPack: (packB64: string) => unknown;
  };

  query: {
    registerSpec: (args: {
      querySpecId: string;
      version: string;
      descriptor: unknown;
      options?: unknown;
      timeoutMs?: number;
    }) => Promise<NodalMergeRuntimeMessage>;
    buildProjection: (args: {
      projectionId: string;
      querySpecId: string;
      targetCheckpoint?: ProjectionCheckpointSelector;
      timeoutMs?: number;
    }) => Promise<NodalMergeRuntimeMessage>;
    readProjection: (args: {
      projectionId: string;
      limit: number;
      pageToken?: string;
      timeoutMs?: number;
    }) => Promise<NodalMergeRuntimeMessage>;
    invalidateProjection: (args: {
      projectionId: string;
      reason?: string;
      timeoutMs?: number;
    }) => Promise<NodalMergeRuntimeMessage>;
    listProjections: (args?: {
      querySpecId?: string;
      stateFilter?: string;
      cursor?: string;
      timeoutMs?: number;
    }) => Promise<NodalMergeRuntimeMessage>;
  };

  offline: {
    outboxDepth: () => number;
    flush: () => void;
    clearPersisted: () => void;
  };

  presence: {
    set: (data: unknown, options?: { sessionId?: string; ttlMs?: number; nowUnixMs?: number }) => void;
    getAll: () => void;
    sweep: (nowUnixMs: number) => void;
  };

  signaling: {
    relay: (type: string, to: string, payload?: Record<string, unknown>) => void;
    offer: (to: string, sdp: string) => void;
    answer: (to: string, sdp: string) => void;
    ice: (to: string, candidate: string, sdpMid?: string, sdpMLineIndex?: number) => void;
  };

  cas: {
    setBlob: (key: string, bytes: Uint8Array) => string;
    getBlob: (hashHex: string) => Uint8Array;
    requestMissingBlobs: () => void;
  };

  topology: {
    snapshot: () => TopologySnapshot;
  };

  on(event: NodalMergeSdkEvent, handler: (payload: unknown) => void): () => void;
}

export declare function parseRuntimeMessage(data: string): NodalMergeRuntimeMessage | null;

export declare function createNodalMergeSdk(options: NodalMergeSdkOptions): Promise<NodalMergeSdk>;
