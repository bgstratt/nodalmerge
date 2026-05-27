import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { initSync, room_pubkey_hex } from "../web/pkg/nodalmerge_bridge.js";
import { createDoc } from "../web/sdk.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const wasmPath = path.resolve(__dirname, "../web/pkg/nodalmerge_bridge_bg.wasm");
const wasmBytes = fs.readFileSync(wasmPath);
initSync(wasmBytes);

const defaultTargets = [
  { name: "rust-combined-server", serverUrl: "ws://127.0.0.1:7878" },
  { name: "rust-integrated-hosted-server", serverUrl: "ws://127.0.0.1:7979" },
  { name: "dotnet-host-runtime-alias", serverUrl: "ws://127.0.0.1:8787" },
];

const args = parseArgs(process.argv.slice(2));
const peerCounts = parseIntList(args.peers, [2, 10, 20]);
const iterations = parseIntSafe(args.iterations, 3);
const mapOps = parseIntSafe(args.mapOps, 60);
const listOps = parseIntSafe(args.listOps, 60);
const blobOps = parseIntSafe(args.blobOps, 20);
const blobSizeBytes = parseIntSafe(args.blobSizeBytes, 4096);
const warmupOps = parseIntSafe(args.warmupOps, 8);
const timeoutMs = parseIntSafe(args.timeoutMs, 30000);
const opDelayMs = Number.parseInt(String(args.opDelayMs ?? "0"), 10);
const transportMode = String(args.transport ?? "ws-only");
const authMode = String(args.authMode ?? "none");
const tokenExpirySecs = parseIntSafe(args.tokenExpirySecs, 60 * 60);
const authTokenCaps = parseStringList(args.authTokenCaps, ["read:bench/**", "write:bench/**"]);
const authAdminCaps = parseStringList(args.authAdminCaps, ["read:bench/**", "write:bench/**", "room.admin"]);
const outputJsonPath = args.outputJsonPath || "";
const targetFilter = args.targets ? new Set(args.targets.split(",").map((x) => x.trim()).filter(Boolean)) : null;

const selectedTargets = targetFilter
  ? defaultTargets.filter((t) => targetFilter.has(t.name))
  : defaultTargets;

if (selectedTargets.length === 0) {
  console.error("No targets selected.");
  process.exit(1);
}

console.log("SDK scenario benchmark config:");
console.log(
  JSON.stringify(
    {
      peerCounts,
      iterations,
      mapOps,
      listOps,
      blobOps,
      blobSizeBytes,
      warmupOps,
      timeoutMs,
      opDelayMs,
      transport: transportMode,
      authMode,
      tokenExpirySecs,
      authTokenCaps,
      targets: selectedTargets.map((t) => t.name),
    },
    null,
    2,
  ),
);

const runStarted = Date.now();
const results = [];

for (const target of selectedTargets) {
  console.log(`\nTarget: ${target.name} (${target.serverUrl})`);
  for (const peerCount of peerCounts) {
    const scenarioRuns = [];
    let skipped = false;

    for (let iteration = 0; iteration < iterations; iteration++) {
      const room = `bench-${target.name}-${peerCount}p-${Date.now()}-${iteration}-${Math.random().toString(16).slice(2, 8)}`;
      let docs = [];
      let authSetup = null;

      try {
        authSetup = await setupAuthMode({
          authMode,
          serverUrl: target.serverUrl,
          room,
          transportMode,
          timeoutMs,
          tokenExpirySecs,
          authTokenCaps,
          authAdminCaps,
        });

        docs = await createPeers(target.serverUrl, room, peerCount, transportMode, authSetup);
        await waitForAllConnected(docs, timeoutMs);

        const warmupStarted = nowMs();
        await warmupPeers(docs, warmupOps, timeoutMs);
        const warmupMs = nowMs() - warmupStarted;

        const scenario = {
          map: await tryScenario(() => runMapScenario(docs, mapOps, timeoutMs, opDelayMs)),
          list: await tryScenario(() => runListScenario(docs, listOps, timeoutMs, opDelayMs)),
          blob: await tryScenario(() => runBlobScenario(docs, blobOps, blobSizeBytes, timeoutMs, opDelayMs)),
        };

        scenarioRuns.push({
          iteration,
          warmup_ms: round3(warmupMs),
          map_ms: scenario.map.ok ? round3(scenario.map.ms) : null,
          map_error: scenario.map.ok ? null : scenario.map.error,
          list_ms: scenario.list.ok ? round3(scenario.list.ms) : null,
          list_error: scenario.list.ok ? null : scenario.list.error,
          blob_ms: scenario.blob.ok ? round3(scenario.blob.ms) : null,
          blob_error: scenario.blob.ok ? null : scenario.blob.error,
        });

        const mapLabel = scenario.map.ok ? `${round3(scenario.map.ms)}ms` : `ERR(${scenario.map.error})`;
        const listLabel = scenario.list.ok ? `${round3(scenario.list.ms)}ms` : `ERR(${scenario.list.error})`;
        const blobLabel = scenario.blob.ok ? `${round3(scenario.blob.ms)}ms` : `ERR(${scenario.blob.error})`;
        console.log(`  peers=${peerCount} iter=${iteration} warmup=${round3(warmupMs)}ms map=${mapLabel} list=${listLabel} blob=${blobLabel}`);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        if (iteration === 0 && (msg.includes("connect-timeout") || msg.includes("ECONNREFUSED") || msg.includes("ERR_CONNECTION_REFUSED"))) {
          console.log(`  peers=${peerCount} status: SKIPPED (${msg})`);
          skipped = true;
          break;
        }

        console.log(`  peers=${peerCount} iter=${iteration} status: ERROR (${msg})`);
        scenarioRuns.push({ iteration, status: "error", error: msg });
      } finally {
        closePeers(docs);
        if (authSetup?.cleanup) {
          await authSetup.cleanup();
        }
      }
    }

    if (skipped) {
      results.push({
        target: target.name,
        serverUrl: target.serverUrl,
        peerCount,
        status: "skipped-unreachable",
      });
      continue;
    }

    const mapValues = scenarioRuns.filter((r) => typeof r.map_ms === "number").map((r) => r.map_ms);
    const listValues = scenarioRuns.filter((r) => typeof r.list_ms === "number").map((r) => r.list_ms);
    const blobValues = scenarioRuns.filter((r) => typeof r.blob_ms === "number").map((r) => r.blob_ms);
    const warmupValues = scenarioRuns.filter((r) => typeof r.warmup_ms === "number").map((r) => r.warmup_ms);

    if (mapValues.length === 0 && listValues.length === 0 && blobValues.length === 0) {
      results.push({
        target: target.name,
        serverUrl: target.serverUrl,
        peerCount,
        status: "error-no-samples",
        runs: scenarioRuns,
      });
      continue;
    }

    const summary = {
      target: target.name,
      serverUrl: target.serverUrl,
      peerCount,
      status: "partial",
      warmup_samples: warmupValues.length,
      warmup_avg_ms: warmupValues.length > 0 ? round3(avg(warmupValues)) : null,
      warmup_p95_ms: warmupValues.length > 0 ? round3(pct(warmupValues, 95)) : null,
      map_samples: mapValues.length,
      map_avg_ms: mapValues.length > 0 ? round3(avg(mapValues)) : null,
      map_p95_ms: mapValues.length > 0 ? round3(pct(mapValues, 95)) : null,
      list_samples: listValues.length,
      list_avg_ms: listValues.length > 0 ? round3(avg(listValues)) : null,
      list_p95_ms: listValues.length > 0 ? round3(pct(listValues, 95)) : null,
      blob_samples: blobValues.length,
      blob_avg_ms: blobValues.length > 0 ? round3(avg(blobValues)) : null,
      blob_p95_ms: blobValues.length > 0 ? round3(pct(blobValues, 95)) : null,
      runs: scenarioRuns,
    };

    if (summary.map_samples > 0 && summary.list_samples > 0 && summary.blob_samples > 0) {
      summary.status = "ok";
    }

    results.push(summary);

    console.log(`  peers=${peerCount} warmup_avg=${summary.warmup_avg_ms ?? "n/a"}ms map_samples=${summary.map_samples} map_avg=${summary.map_avg_ms ?? "n/a"}ms list_samples=${summary.list_samples} list_avg=${summary.list_avg_ms ?? "n/a"}ms blob_samples=${summary.blob_samples} blob_avg=${summary.blob_avg_ms ?? "n/a"}ms`);
  }
}

const output = {
  started_at_unix_ms: runStarted,
  completed_at_unix_ms: Date.now(),
  config: {
    peerCounts,
    iterations,
    mapOps,
    listOps,
    blobOps,
    blobSizeBytes,
    warmupOps,
    timeoutMs,
    opDelayMs,
    transport: transportMode,
    authMode,
    tokenExpirySecs,
    authTokenCaps,
  },
  results,
};

if (outputJsonPath) {
  const fullPath = path.resolve(process.cwd(), outputJsonPath);
  fs.mkdirSync(path.dirname(fullPath), { recursive: true });
  fs.writeFileSync(fullPath, JSON.stringify(output, null, 2), "utf8");
  console.log(`\nWrote results: ${fullPath}`);
}

console.log("\nDone.");

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const key = a.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      out[key] = "true";
      continue;
    }
    out[key] = next;
    i++;
  }
  return out;
}

function parseIntSafe(value, fallback) {
  const n = Number.parseInt(String(value ?? ""), 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

function parseIntList(value, fallback) {
  if (!value) return fallback;
  const list = String(value)
    .split(",")
    .map((x) => Number.parseInt(x.trim(), 10))
    .filter((n) => Number.isFinite(n) && n > 0);
  return list.length > 0 ? list : fallback;
}

function parseStringList(value, fallback) {
  if (!value) return fallback;
  const list = String(value)
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
  return list.length > 0 ? [...new Set(list)] : fallback;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function nowMs() {
  return performance.now();
}

function round3(n) {
  return Math.round(n * 1000) / 1000;
}

function avg(values) {
  return values.reduce((a, b) => a + b, 0) / values.length;
}

function pct(values, percentile) {
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.max(0, Math.min(sorted.length - 1, Math.ceil((percentile / 100) * sorted.length) - 1));
  return sorted[index];
}

function randomSeed32() {
  const seed = new Uint8Array(32);
  crypto.getRandomValues(seed);
  return seed;
}

function makeBlobBytes(sizeBytes, salt) {
  const bytes = new Uint8Array(sizeBytes);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = (salt + i) % 251;
  }
  return bytes;
}

async function createPeers(serverUrl, room, peerCount, transportMode, authSetup) {
  const docs = [];
  for (let i = 0; i < peerCount; i++) {
    const createDocOptions = {
      serverUrl,
      room,
      authorSeed: randomSeed32(),
      autoConnect: true,
      transport: transportMode,
    };

    if (authSetup?.mode === "room-lock-tokened") {
      createDocOptions.roomSeed = authSetup.roomSeed;
      createDocOptions.tokenCaps = authSetup.tokenCaps;
      createDocOptions.tokenExpirySecs = authSetup.tokenExpirySecs;
    }

    const doc = await createDoc(createDocOptions);
    docs.push(doc);
  }
  return docs;
}

async function setupAuthMode({
  authMode,
  serverUrl,
  room,
  transportMode,
  timeoutMs,
  tokenExpirySecs,
  authTokenCaps,
  authAdminCaps,
}) {
  if (authMode !== "room-lock-tokened") {
    return null;
  }

  const roomSeed = randomSeed32();
  const adminDoc = await createDoc({
    serverUrl,
    room,
    authorSeed: randomSeed32(),
    roomSeed,
    tokenCaps: authAdminCaps,
    tokenExpirySecs,
    autoConnect: true,
    transport: transportMode,
  });

  await waitForAllConnected([adminDoc], timeoutMs);
  const roomPubkeyHex = room_pubkey_hex(roomSeed);
  adminDoc.send({ type: "set-room-key", pubkey: roomPubkeyHex });
  await sleep(250);
  adminDoc.close();

  return {
    mode: "room-lock-tokened",
    roomSeed,
    tokenCaps: authTokenCaps,
    tokenExpirySecs,
    cleanup: async () => {},
  };
}

function closePeers(docs) {
  for (const doc of docs) {
    try {
      doc.close();
    } catch {
      // ignore cleanup failures
    }
  }
}

async function waitForAllConnected(docs, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (docs.every((d) => d.isConnected)) {
      await sleep(150);
      return;
    }
    await sleep(50);
  }
  throw new Error(`connect-timeout (${timeoutMs}ms)`);
}

async function waitUntil(predicate, timeoutMs, label, getDiagnostics) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (predicate()) return;
    await sleep(40);
  }

  let details = "";
  if (typeof getDiagnostics === "function") {
    try {
      const diag = getDiagnostics();
      if (diag !== undefined && diag !== null && diag !== "") {
        if (typeof diag === "string") {
          details = diag;
        } else {
          details = JSON.stringify(diag);
        }
      }
    } catch {
      // best-effort diagnostics only
    }
  }

  if (details) {
    throw new Error(`${label}-timeout (${timeoutMs}ms) details=${details}`);
  }

  throw new Error(`${label}-timeout (${timeoutMs}ms)`);
}

async function tryScenario(fn) {
  try {
    const ms = await fn();
    return { ok: true, ms };
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    return { ok: false, error: msg };
  }
}

async function warmupPeers(docs, warmupOps, timeoutMs) {
  const map = docs[0].map("bench/warmup");
  for (let i = 0; i < warmupOps; i++) {
    map.set(`k${i}`, { i, warmup: true });
  }

  await waitUntil(() => {
    return docs.every((doc) => {
      const all = doc.map("bench/warmup").all();
      return Object.keys(all).length >= warmupOps;
    });
  }, timeoutMs, "warmup-convergence", () => {
    return {
      perPeerWarmupKeys: docs.map((doc, idx) => ({
        peer: idx,
        keys: Object.keys(doc.map("bench/warmup").all()).length,
      })),
      expectedKeys: warmupOps,
    };
  });
}

async function runMapScenario(docs, opCount, timeoutMs, opDelayMs) {
  const started = nowMs();
  for (let i = 0; i < opCount; i++) {
    const writer = docs[i % docs.length];
    writer.map("bench/map").set(`k${i}`, { i, peer: i % docs.length, t: Date.now() });
    if (opDelayMs > 0) {
      await sleep(opDelayMs);
    }
  }

  await waitUntil(() => {
    return docs.every((doc) => Object.keys(doc.map("bench/map").all()).length >= opCount);
  }, timeoutMs, "map-convergence", () => {
    return {
      perPeerMapKeys: docs.map((doc, idx) => ({
        peer: idx,
        keys: Object.keys(doc.map("bench/map").all()).length,
      })),
      expectedKeys: opCount,
    };
  });

  return nowMs() - started;
}

async function runListScenario(docs, opCount, timeoutMs, opDelayMs) {
  const started = nowMs();
  for (let i = 0; i < opCount; i++) {
    const writer = docs[i % docs.length];
    writer.list("bench/list").push({ i, peer: i % docs.length });
    if (opDelayMs > 0) {
      await sleep(opDelayMs);
    }
  }

  await waitUntil(() => {
    return docs.every((doc) => doc.list("bench/list").length >= opCount);
  }, timeoutMs, "list-convergence", () => {
    return {
      perPeerListLength: docs.map((doc, idx) => ({
        peer: idx,
        length: doc.list("bench/list").length,
      })),
      expectedLength: opCount,
    };
  });

  return nowMs() - started;
}

async function runBlobScenario(docs, opCount, blobSizeBytes, timeoutMs, opDelayMs) {
  const hashes = [];
  const reader = docs[docs.length - 1];
  const started = nowMs();

  for (let i = 0; i < opCount; i++) {
    const writer = docs[i % docs.length];
    const blobKey = `asset-${i}`;
    const bytes = makeBlobBytes(blobSizeBytes, i + 17);
    const hash = writer.map("bench/blob").setBlob(blobKey, bytes, { contentType: "application/octet-stream" });
    hashes.push(hash);
    if (opDelayMs > 0) {
      await sleep(opDelayMs);
    }
  }

  await waitUntil(() => {
    const view = reader.map("bench/blob").all();
    if (Object.keys(view).length < opCount) {
      return false;
    }

    for (const hash of hashes) {
      try {
        const data = reader.map("bench/blob").getBlob(hash);
        if (!data || data.length !== blobSizeBytes) {
          return false;
        }
      } catch {
        return false;
      }
    }

    return true;
  }, timeoutMs, "blob-convergence", () => {
    const view = reader.map("bench/blob").all();
    let readable = 0;
    for (const hash of hashes) {
      try {
        const data = reader.map("bench/blob").getBlob(hash);
        if (data && data.length === blobSizeBytes) {
          readable++;
        }
      } catch {
        // ignore; counted as unreadable
      }
    }

    return {
      readerPeer: docs.length - 1,
      expectedMetadataKeys: opCount,
      seenMetadataKeys: Object.keys(view).length,
      expectedReadableBlobs: opCount,
      readableBlobs: readable,
    };
  });

  return nowMs() - started;
}
