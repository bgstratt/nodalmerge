// gen-doc-module.mjs — stage web/sdk.js into this package as `doc.js`.
//
// `web/sdk.js` (the `createDoc` high-level API) is the source of truth and
// keeps repo-relative imports so the no-bundler `web/` demo works unchanged.
// This script copies it (and `web/sdk.d.ts`) into sdk-js/ at pack time,
// rewriting only the two module specifiers to package-resolvable paths:
//
//   ./pkg/nodalmerge_bridge.js                    -> nodalmerge-bridge
//   ../sdk-js/persistence/peer-local-indexeddb.js -> ./persistence/peer-local-indexeddb.js
//
// Runs via the npm `prepack` lifecycle (see package.json), so `npm pack`
// and pack-local-artifacts.ps1 always emit a fresh copy. Output files are
// generated — do not edit doc.js / doc.d.ts by hand.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const sdkJsDir = dirname(dirname(fileURLToPath(import.meta.url)));
const webDir = join(sdkJsDir, "..", "web");

const rewrites = [
  {
    from: "./pkg/nodalmerge_bridge.js",
    to: "nodalmerge-bridge"
  },
  {
    from: "../sdk-js/persistence/peer-local-indexeddb.js",
    to: "./persistence/peer-local-indexeddb.js"
  }
];

function stage(sourceName, targetName, expectedRewrites) {
  const sourcePath = join(webDir, sourceName);
  let content = readFileSync(sourcePath, "utf8");

  let applied = 0;
  for (const { from, to } of rewrites) {
    const needle = `'${from}'`;
    if (content.includes(needle)) {
      content = content.replaceAll(needle, `'${to}'`);
      applied += 1;
    }
  }
  if (applied !== expectedRewrites) {
    throw new Error(
      `gen-doc-module: expected ${expectedRewrites} import rewrite(s) in ${sourceName}, applied ${applied}. ` +
        `Did the imports in web/${sourceName} change?`
    );
  }

  const banner =
    `// GENERATED FILE — do not edit. Staged from web/${sourceName} by scripts/gen-doc-module.mjs\n`;
  writeFileSync(join(sdkJsDir, targetName), banner + content);
  console.log(`gen-doc-module: wrote ${targetName} from web/${sourceName} (${applied} rewrite(s))`);
}

stage("sdk.js", "doc.js", 2);
// sdk.d.ts re-exports SyncStore/sign_room_token/room_pubkey_hex from the bridge.
stage("sdk.d.ts", "doc.d.ts", 1);
