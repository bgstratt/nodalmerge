import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const thisFile = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(thisFile), "..", "..");
const wsUrl = process.env.NODALMERGE_SMOKE_WS_URL ?? "ws://127.0.0.1:7878";
const webUrl = process.env.NODALMERGE_SMOKE_WEB_URL ?? "http://127.0.0.1:8080";
const pageUrl = `${webUrl}/?server=${encodeURIComponent(wsUrl)}`;

function checkPort(host, port) {
  return new Promise((resolve) => {
    const socket = new net.Socket();
    socket.setTimeout(800);
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
    socket.once("timeout", () => {
      socket.destroy();
      resolve(false);
    });
    socket.connect(port, host);
  });
}

async function waitForPort(host, port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await checkPort(host, port)) return true;
    await delay(300);
  }
  return false;
}

function spawnManaged(command, args, cwd, label) {
  const child = spawn(command, args, {
    cwd,
    shell: true,
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout.on("data", (chunk) => {
    const msg = String(chunk).trim();
    if (msg) process.stdout.write(`[${label}] ${msg}\n`);
  });

  child.stderr.on("data", (chunk) => {
    const msg = String(chunk).trim();
    if (msg) process.stderr.write(`[${label}] ${msg}\n`);
  });

  return child;
}

async function ensureService({ host, port, command, args, cwd, label }) {
  if (await checkPort(host, port)) {
    return { child: null, started: false };
  }

  const child = spawnManaged(command, args, cwd, label);
  const ok = await waitForPort(host, port, 60_000);
  if (!ok) {
    child.kill();
    throw new Error(`${label} did not become ready on ${host}:${port}`);
  }
  return { child, started: true };
}

async function waitForConnected(page) {
  await page.waitForFunction(() => {
    const status = document.querySelector("#status")?.textContent ?? "";
    return status.includes("Connected");
  }, undefined, { timeout: 20_000 });
}

async function waitForMapValue(page, key, expectedValue) {
  await page.waitForFunction(
    ([k, v]) => {
      const doc = window.doc;
      if (!doc) return false;
      const map = doc.map("demo/kv");
      return map.get(k) === v;
    },
    [key, expectedValue],
    { timeout: 20_000 }
  );
}

async function waitForMapAbsent(page, key) {
  await page.waitForFunction(
    (k) => {
      const doc = window.doc;
      if (!doc) return false;
      const map = doc.map("demo/kv");
      return map.get(k) === undefined;
    },
    key,
    { timeout: 20_000 }
  );
}

async function waitForTextMirror(page, expected) {
  await page.waitForFunction(
    (v) => {
      const ta = document.querySelector("#collab-textarea");
      return !!ta && ta.value === v;
    },
    expected,
    { timeout: 20_000 }
  );
}

async function waitForListContains(page, expected) {
  await page.waitForFunction(
    (v) => {
      const doc = window.doc;
      if (!doc) return false;
      const list = doc.list("demo/list").toArray();
      return list.some((item) => item?.content?.label === v || item?.content === v);
    },
    expected,
    { timeout: 20_000 }
  );
}

async function waitForBlobSignal(page, key) {
  await page.waitForFunction(
    (k) => {
      const doc = window.doc;
      if (!doc) return false;
      const map = doc.map("demo/kv");
      return typeof map.get(k) === "string";
    },
    key,
    { timeout: 20_000 }
  );
}

async function run() {
  const managed = [];
  const failures = [];
  let pageA = null;
  let pageB = null;
  const runId = `${Date.now()}-${Math.floor(Math.random() * 1_000_000)}`;
  const mapKey = `smoke-map-${runId}`;
  const blobKey = `smoke-blob-${runId}`;
  const listLabel = `smoke-item-${runId}`;
  const collabValue = `smoke text mirrors ${runId}`;

  async function runStep(name, fn) {
    try {
      await fn();
      console.log(`STEP PASS: ${name}`);
    } catch (err) {
      throw new Error(`${name}: ${err?.message ?? err}`);
    }
  }

  try {
    managed.push(
      await ensureService({
        host: "127.0.0.1",
        port: 7878,
        command: "cargo",
        args: ["run", "--bin", "nodalmerge-server"],
        cwd: repoRoot,
        label: "server",
      })
    );

    managed.push(
      await ensureService({
        host: "127.0.0.1",
        port: 8080,
        command: "python",
        args: ["web/serve.py", "8080"],
        cwd: repoRoot,
        label: "web",
      })
    );

    const browser = await chromium.launch({ headless: true });
    const contextA = await browser.newContext();
    const contextB = await browser.newContext();
    pageA = await contextA.newPage();
    pageB = await contextB.newPage();

    await runStep("open pages", async () => {
      await Promise.all([pageA.goto(pageUrl), pageB.goto(pageUrl)]);
    });

    await runStep("connect both peers", async () => {
      await Promise.all([waitForConnected(pageA), waitForConnected(pageB)]);
    });

    // MAP add/update/delete mirroring.
    await runStep("map set mirrors", async () => {
      await pageA.fill("#key-in", mapKey);
      await pageA.fill("#val-in", "value-1");
      const setRes = await pageA.evaluate(() => {
        try {
          window.doSet();
          return { ok: true };
        } catch (e) {
          return {
            ok: false,
            error: String(e?.message ?? e),
            stack: String(e?.stack ?? ""),
          };
        }
      });
      if (!setRes.ok) {
        throw new Error(`local doSet failed: ${setRes.error}\n${setRes.stack}`.trim());
      }
      await waitForMapValue(pageB, mapKey, "value-1");
    });

    await runStep("map update mirrors", async () => {
      await pageA.fill("#key-in", mapKey);
      await pageA.fill("#val-in", "value-2");
      const setRes = await pageA.evaluate(() => {
        try {
          window.doSet();
          return { ok: true };
        } catch (e) {
          return {
            ok: false,
            error: String(e?.message ?? e),
            stack: String(e?.stack ?? ""),
          };
        }
      });
      if (!setRes.ok) {
        throw new Error(`local doSet failed: ${setRes.error}\n${setRes.stack}`.trim());
      }
      await waitForMapValue(pageB, mapKey, "value-2");
    });

    await runStep("map delete mirrors", async () => {
      await pageA.fill("#key-in", mapKey);
      const delRes = await pageA.evaluate(() => {
        try {
          window.doDelete();
          return { ok: true };
        } catch (e) {
          return {
            ok: false,
            error: String(e?.message ?? e),
            stack: String(e?.stack ?? ""),
          };
        }
      });
      if (!delRes.ok) {
        throw new Error(`local doDelete failed: ${delRes.error}\n${delRes.stack}`.trim());
      }
      await waitForMapAbsent(pageB, mapKey);
    });

    // BLOB metadata mirroring.
    await runStep("blob metadata mirrors", async () => {
      await pageA.fill("#blob-key-in", blobKey);
      await pageA.fill("#blob-val-in", "blob-value-1234567890");
      await pageA.click("button:has-text('Upload Blob')");
      await waitForBlobSignal(pageB, blobKey);
    });

    // COLLAB TEXT mirroring.
    await runStep("collab text mirrors", async () => {
      await pageA.fill("#collab-textarea", collabValue);
      await waitForTextMirror(pageB, collabValue);
    });

    // LIST mirroring.
    await runStep("list item mirrors", async () => {
      await pageA.fill("#list-input", listLabel);
      await pageA.click("#list-add");
      await waitForListContains(pageB, listLabel);
    });

    await browser.close();

    if (failures.length === 0) {
      console.log("SMOKE PASS: map/blob/text/list mirrored across peers.");
      return;
    }
  } catch (err) {
    failures.push(String(err?.message ?? err));
    if (pageA && pageB) {
      const dump = async (page, label) => {
        const snap = await page.evaluate(() => {
          const status = document.querySelector("#status")?.textContent ?? "";
          const state = document.querySelector("#my-state")?.textContent ?? "";
          const log = [...document.querySelectorAll("#event-log li")]
            .slice(0, 8)
            .map((li) => li.textContent ?? "");
          return { status, state, log };
        });
        console.error(`${label} status: ${snap.status}`);
        console.error(`${label} state: ${snap.state}`);
        for (const line of snap.log) {
          console.error(`${label} log: ${line}`);
        }
      };
      try {
        await dump(pageA, "peerA");
        await dump(pageB, "peerB");
      } catch (diagErr) {
        console.error(`diagnostic collection failed: ${diagErr?.message ?? diagErr}`);
      }
    }
  } finally {
    for (const svc of managed) {
      if (svc?.child && !svc.child.killed) {
        svc.child.kill();
      }
    }
  }

  console.error("SMOKE FAIL:");
  for (const f of failures) {
    console.error(` - ${f}`);
  }
  process.exit(1);
}

run();
