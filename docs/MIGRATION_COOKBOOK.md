# Migration Cookbook (FSE-06 Baseline)

Canonical templates for schema/version rollouts across NodalMerge surfaces.
Use this with:

1. `docs/migration.md` (phase-by-phase upgrade deltas)
2. `docs/sdk.md` (client behavior and rejection handling)
3. `docs/operator.md` (operational rollout and rollback choreography)
4. `docs/MIGRATION_ANTI_PATTERNS_CHECKLIST.md` (CI/PR gate checklist)

---

## Compatibility-window conventions

Use these terms consistently in docs, runbooks, and tickets:

1. **Introduce window**: deploy readers that support both old and new shapes.
2. **Write-cutover window**: switch writers to new shape while dual-read remains enabled.
3. **Convergence window**: monitor fleet/version convergence and reject rates.
4. **Retire window**: remove old-shape writers/readers only after convergence gate is met.

Explicitly track in each rollout:

1. `min_supported_version`
2. `max_supported_version`
3. `target_write_version`
4. `retire_not_before` (date or release gate)

---

## Template A: Forward-only additive change

Use when introducing optional fields or additive message variants.

Steps:

1. ship readers that accept old + new payloads
2. switch writers to include the new optional field
3. verify old clients still function (ignoring unknown field)
4. lock in alerts for `reject.*unsupported*` drift before retire

Success gate:

1. no elevated rejection/error rates after write-cutover
2. mixed-client sessions remain healthy

Rollback:

1. stop writing new optional field
2. keep dual-read active

---

## Template B: Dual-read + staged write switch

Use when value semantics changed but old key/value must coexist temporarily.

Steps:

1. deploy dual-read parser: prefer V2, fallback to V1
2. start writing both V1+V2 (or write V2 + read-repair V1)
3. monitor convergence metrics (`v1_seen`, `v2_seen`, reject classes)
4. switch to V2-only write once convergence SLO holds
5. retire V1 read path after retire window

Success gate:

1. fallback-hit rate trends toward zero through convergence window
2. no unresolved `unsupported-window` incidents during canary and broad rollout

Rollback:

1. restore dual-write or V1-only write
2. keep dual-read path enabled

---

## Template C: Rollback-safe key rename (copy-then-tombstone)

Use when renaming paths/keys.

Steps:

1. readers accept old key and new key (prefer new key)
2. writers copy value to new key
3. after copy is observed, tombstone old key in separate write
4. keep old-key read fallback until retire gate

Success gate:

1. no data loss in replay validation
2. deterministic state parity between old/new reader builds

Rollback:

1. continue reading both keys
2. stop tombstoning until rollout is healthy

---

## Unsupported-window handling pattern

When server/client versions fall outside compatibility window:

1. emit stable reject classes (for example `reject.query_unsupported_version`)
2. treat as **non-retry-until-input-or-version-changes**
3. surface actionable operator/client logs:
   - requested version
   - supported range
   - command and room
4. retry only after selecting supported version or completing upgrade

SDK guidance:

1. convert unsupported-window rejects into typed user-visible events
2. backoff alone is insufficient; require version/profile adjustment

Operator guidance:

1. freeze write-cutover
2. widen dual-read/dual-write window or roll back writer cohort
3. resume rollout only after reject rate returns to baseline

---

## Acceptance checklist (copy into rollout issue)

1. compatibility window documented (`min/max/target`)
2. dual-read behavior tested in mixed-version lane
3. unsupported-window reject classes verified in staging
4. canary metrics stable for agreed duration
5. rollback command/flags validated before broad rollout
6. retire gate evidence attached before removing legacy path
