# Migration Anti-Patterns Checklist (FSE-06)

Use this checklist in rollout PRs and release gates to catch schema/version migration risks early.

Reference docs:

1. `docs/MIGRATION_COOKBOOK.md`
2. `docs/migration.md`

---

## CI/PR gate checklist

Mark each item as pass/fail in rollout PR description.

1. No semantic key reuse
   - existing keys are not repurposed for new meanings
   - renames use copy-then-tombstone, not in-place mutation
2. Compatibility window declared
   - `min_supported_version` and `max_supported_version` documented
   - retire gate/date (`retire_not_before`) documented
3. Dual-read safety proven (when applicable)
   - parser/reader supports old + new shapes during cutover
   - fallback path covered by tests
4. Unsupported-window behavior defined
   - stable `reject.*unsupported*` class is documented
   - caller behavior is non-retry-until-version/profile change
5. Rollback path documented
   - explicit writer rollback procedure exists
   - dual-read path remains available during rollback
6. Observability attached
   - reject/error metrics or logs identified for rollout monitoring
   - convergence signal is defined (for example fallback-hit trend)
7. Legacy removal gated
   - no legacy path deletion before convergence evidence
   - removal criteria and owner are captured

---

## Common anti-patterns (blockers)

1. **In-place semantic reuse** of a key/path.
2. **Single-step hard cutover** without dual-read window.
3. **Retry loops on unsupported-window rejects** instead of version/profile adjustment.
4. **Legacy deletion before convergence evidence**.
5. **Missing rollback playbook** for writer cutover.
6. **No reject taxonomy updates** when introducing new compatibility classes.

---

## Suggested PR snippet

Copy into rollout PR body:

```markdown
## Migration checklist

- [ ] No semantic key reuse (copy-then-tombstone used when renaming)
- [ ] Compatibility window declared (`min/max/target/retire_not_before`)
- [ ] Dual-read fallback tested
- [ ] Unsupported-window reject class and caller behavior documented
- [ ] Rollback path validated
- [ ] Monitoring/convergence signals attached
- [ ] Legacy removal gated on convergence evidence
```
