# Contract Health Monitoring

`src/health_monitor.rs` keeps an on-chain picture of how the contract is doing:
health checks, uptime, error rates, performance and resource usage. It also
raises an alert event when something is wrong. `frontend/src/views/HealthDashboard.tsx`
renders the report.

The module is crate-private (`mod health_monitor`). Other modules call it
directly, as `achievement_engine` and `difficulty_curve` already do. Callers
are responsible for authorising anything that changes configuration.

## What is tracked

| Signal | Recorded with | Stored under |
| --- | --- | --- |
| Raw metric samples (existing) | `record_contract_health(env, metric, value)` / `record_contract_health_batch` | `Metric(Symbol)`, `HealthRegistry` |
| Uptime | first sample of any kind | `HealthStartedAt` (ledger timestamp) |
| Calls, errors, cost per operation | `record_operation(env, op, success, cost)` | `HealthOperation(Symbol)`, `HealthOpRegistry` |
| Resource usage | `record_resource_usage(env, resource, used, capacity)` | `HealthResource(Symbol)`, `HealthResourceRegistry` |
| Alert thresholds | `set_health_thresholds(env, thresholds)` | `HealthThresholdConfig` |

`cost` is whatever unit the caller measures per call: CPU instructions from a
simulation, fee in stroops, items processed. Keep it consistent per operation.
Soroban does not expose metered resources to the contract at runtime, so the
caller reports them.

At most `MAX_TRACKED_ITEMS` (32) distinct operations and 32 resources are
tracked, which keeps `check_health` bounded. A new name beyond that returns
`HealthError::RegistryFull`. Names already tracked keep recording.

## Health checks

`check_health(env) -> HealthReport` reads every tracked operation and resource
and runs three checks:

| Check | Failing name | Degraded when | Unhealthy when |
| --- | --- | --- | --- |
| Error rate (all operations) | `err_rate` | `errors / calls >= degraded_error_bps` | `>= unhealthy_error_bps` |
| Most utilised resource | `resource` | `used / capacity >= degraded_resource_bps` | `>= unhealthy_resource_bps` |
| Average cost per call | `perf` | `avg_cost > max_avg_cost` (if `max_avg_cost > 0`) | never |

The overall `status` is the worst result. Rates are integer basis points
(10 000 = 100 %).

Default thresholds (`default_thresholds()`):

| Field | Default |
| --- | --- |
| `degraded_error_bps` | 500 (5 %) |
| `unhealthy_error_bps` | 2 000 (20 %) |
| `degraded_resource_bps` | 8 000 (80 %) |
| `unhealthy_resource_bps` | 9 500 (95 %) |
| `max_avg_cost` | 0 (performance check off) |

`set_health_thresholds` rejects degraded > unhealthy, or any bps > 10 000,
with `HealthError::InvalidThresholds`.

### Report fields

`status`, `uptime_secs` (now minus first sample, 0 before any sample),
`total_calls`, `total_errors`, `error_rate_bps`, `avg_cost`, `max_cost`,
`worst_resource` (`none` if no resources), `worst_resource_bps`,
`failed_checks`, `checked_at`, and the full `operations` and `resources` lists
for drill-down.

## Alerts

When the status is not `Healthy`, `check_health` publishes:

```
topics: ("health", "alert")
data:   (status, failed_checks)
```

Off-chain, route this event the same way as other contract events: an
indexer or `monitoring/exporters/contract_exporter.py`, then Alertmanager. To
configure alerting, tune the thresholds on-chain with `set_health_thresholds`
behind an admin-authorised entry point. Then decide in Alertmanager who gets
paged for `Degraded` and who for `Unhealthy`.

Each `record_operation` also emits `("health", "op")` with
`(operation, success, cost)` for indexers that keep their own time series.

## Dashboard

`HealthDashboard` (exported from `@stellar-nebula/frontend-components`) takes
a `HealthReport` (camelCase mirror in `frontend/src/utils/health.ts`) and an
`onRefresh` callback. It shows:

- the status as text (colour is only a secondary cue) in a `role="status"` live region,
- active alerts with readable labels,
- uptime, call count, error rate, average and max cost,
- a per-operation table (calls, errors, error rate, average cost),
- a meter per resource with current, capacity, percentage and peak.

```tsx
<HealthDashboard report={reportFromContract} onRefresh={() => refetch()} />
```

## Complexity

- `record_operation` / `record_resource_usage`: O(R) registry membership check
  (R ≤ 32), O(1) other storage ops.
- `check_health`: O(O + R) storage reads and O(O + R) memory for the report.
