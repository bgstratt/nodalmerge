import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const thisFile = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(thisFile), '..', '..');
const sdkJsPath = path.join(repoRoot, 'web', 'sdk.js');
const sdkDtsPath = path.join(repoRoot, 'web', 'sdk.d.ts');
const outPath = path.join(repoRoot, 'docs', 'acceptance', 'authz-conformance-sdk-parity.json');

const checks = [];

function addCheck(name, passed, details) {
  checks.push({ name, passed, details });
}

function fileContains(filePath, needle) {
  const content = fs.readFileSync(filePath, 'utf8');
  return content.includes(needle);
}

function safeRead(filePath) {
  try {
    return fs.readFileSync(filePath, 'utf8');
  } catch (err) {
    return null;
  }
}

const sdkJs = safeRead(sdkJsPath);
const sdkDts = safeRead(sdkDtsPath);

addCheck('sdk_js_exists', sdkJs !== null, sdkJsPath);
addCheck('sdk_dts_exists', sdkDts !== null, sdkDtsPath);

if (sdkJs && sdkDts) {
  addCheck(
    'reject_prefix_parser_present',
    sdkJs.includes('function parseRejectPrefix(message)'),
    'parseRejectPrefix function should exist in web/sdk.js'
  );

  addCheck(
    'rejection_envelope_parser_present',
    sdkJs.includes('function parseRejectionEnvelope(err)'),
    'parseRejectionEnvelope function should exist in web/sdk.js'
  );

  addCheck(
    'reason_class_normalization_present',
    sdkJs.includes("reasonClass && !String(reasonClass).startsWith('reject.')"),
    'reasonClass should normalize to reject.* namespace'
  );

  addCheck(
    'typed_on_rejection_surface_present',
    sdkJs.includes('onRejection(cb)') && sdkJs.includes('recentRejections(sinceMs'),
    'doc API should expose onRejection/recentRejections'
  );

  addCheck(
    'sdk_rejection_error_type_present',
    sdkDts.includes('export interface ActiveSyncRejectionError extends Error'),
    'Type surface should expose ActiveSyncRejectionError'
  );

  addCheck(
    'sdk_rejection_event_type_present',
    sdkDts.includes('onRejection(cb: (ev: RejectionEvent) => void)') &&
      sdkDts.includes('recentRejections(sinceMs?: number): RejectionEvent[]'),
    'Type surface should expose RejectionEvent callbacks/history'
  );
}

const failed = checks.filter((c) => !c.passed);
const status = failed.length === 0 ? 'pass' : 'fail';

const output = {
  run_id: `sdk-parity-${new Date().toISOString()}`,
  utc_timestamp: new Date().toISOString(),
  status,
  parity: {
    status,
    mismatches: failed.map((c) => ({ name: c.name, details: c.details })),
  },
  checks,
  notes: 'SDK rejection-surface parity validation for nightly conformance workflow',
};

fs.writeFileSync(outPath, JSON.stringify(output, null, 2));
console.log(`Wrote SDK parity artifact: ${outPath}`);
console.log(`SDK parity status: ${status}`);

if (status !== 'pass') {
  process.exitCode = 2;
}
