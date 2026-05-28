# Hosted Dashboard Bootstrap (Deferred Productization)

This is a starter checklist for hosted dashboard rollout once product chooses a dashboard stack.

## 1) Baseline metric set

Server:

1. `nodalmerge_topology_promotion_total`
2. `nodalmerge_topology_promotion_seconds`
3. `nodalmerge_topology_promotion_inflight`
4. `nodalmerge_topology_promotion_queue_depth`
5. `nodalmerge_topology_promotion_queued_total`
6. `nodalmerge_topology_promotion_queue_wait_seconds`
7. `nodalmerge_query_build_total`
8. `nodalmerge_query_build_seconds`
9. `nodalmerge_query_build_inflight`
10. `nodalmerge_query_build_queue_depth`
11. `nodalmerge_query_build_queued_total`
12. `nodalmerge_query_build_queue_wait_seconds`

## 2) Suggested dashboard sections

1. Promotion throughput and rejection reasons.
2. Promotion queue pressure (depth, wait time, inflight).
3. Query projection build pressure (depth, wait time, row-limit rejections).
4. Room health (rooms, peers, evictions, memory resident estimate).

## 3) Alert starter thresholds

1. Promotion queue depth > 0 for sustained 5 min.
2. Promotion queue wait p95 > 2 s for 10 min.
3. Query queue depth > 0 sustained + increased reject rates.
4. Stale-parent / invalid-lineage rejection spikes.

## 4) Rollout notes

1. Treat this as post-core productization.
2. Keep local metrics endpoint (`--metrics-addr`) as ground truth while hosted dashboards are deferred.
